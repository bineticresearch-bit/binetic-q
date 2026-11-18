//! Q3 — intent synthesis: a declarative intent becomes a *verified* `.q`
//! program. This is an honest front end, not magic: the intent is a structured
//! "what I want" (shapes, demanded receipt level, kernel pipeline, geometry
//! policy), and synthesis is constrained so it can only emit a program that
//! passes Q1's effect check and Q2's real-fabric verification at that demand.
//!
//! A free-form natural-language front end is explicitly out of scope: an LLM
//! adapter may *produce* the structured intent below, but the synthesizer is a
//! deterministic, provable compiler. Nothing here claims to understand prose.
//!
//! Grammar (Q3 intent):
//! ```text
//! intent <name> {
//!   input <dtype>[<n>];
//!   output <dtype>[<n>];
//!   demand L<k>;
//!   pipeline [ <kernel>, <kernel>, ... ];
//!   weights [ <int>, <int>, ... ];        // optional
//!   geometry fixed | optimize;            // optional, default fixed
//! }
//! ```

use binetic_evolver::LaneScaledThroughput;

use crate::ir::{receipt_effect, DType, Kernel, Level, ProgramConfig, QProgram, Stage, TensorType};
use crate::lexer::{LexError, Token};
use crate::{check, optimize, run, OptimizeConfig};

/// How execution geometry is chosen during synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Geometry {
    /// Use the explicit/default config.
    Fixed,
    /// Run the Q2 verified-throughput search and emit the winning geometry.
    Optimize,
}

/// A parsed declarative intent — the "what", independent of any Q source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intent {
    pub name: String,
    pub input: TensorType,
    pub output: TensorType,
    pub demand: Level,
    pub kernels: Vec<Kernel>,
    pub weights: Option<Vec<i64>>,
    pub geometry: Geometry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthError {
    Lex(LexError),
    Syntax(String),
    BadShape(String),
    WeightCount { kernels: usize, weights: usize },
    Check(String),
    Unverifiable(String),
}

impl From<LexError> for SynthError {
    fn from(e: LexError) -> Self {
        SynthError::Lex(e)
    }
}

impl std::fmt::Display for SynthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SynthError::Lex(e) => write!(f, "lex error: {e}"),
            SynthError::Syntax(s) => write!(f, "intent syntax: {s}"),
            SynthError::BadShape(s) => write!(f, "intent shape: {s}"),
            SynthError::WeightCount { kernels, weights } => {
                write!(f, "pipeline has {kernels} stages but {weights} weights")
            }
            SynthError::Check(s) => write!(f, "synthesized program rejected: {s}"),
            SynthError::Unverifiable(s) => write!(f, "synthesis refused (unverifiable): {s}"),
        }
    }
}

impl std::error::Error for SynthError {}

struct Cursor {
    tokens: Vec<Token>,
    pos: usize,
}

impl Cursor {
    fn new(src: &str) -> Result<Self, SynthError> {
        Ok(Self {
            tokens: crate::lexer::Lexer::new(src).tokenize()?,
            pos: 0,
        })
    }

    fn cur(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.cur().clone();
        self.pos += 1;
        t
    }

    fn ident(&mut self, what: &str) -> Result<String, SynthError> {
        match self.bump() {
            Token::Ident(s) => Ok(s),
            other => Err(SynthError::Syntax(format!(
                "expected {what}, found {other:?}"
            ))),
        }
    }

    fn uint(&mut self, what: &str) -> Result<u64, SynthError> {
        match self.bump() {
            Token::Int(n) if n >= 0 => Ok(n as u64),
            other => Err(SynthError::Syntax(format!(
                "expected {what}, found {other:?}"
            ))),
        }
    }

    fn expect(&mut self, want: Token, label: &str) -> Result<(), SynthError> {
        let got = self.bump();
        if got == want {
            Ok(())
        } else {
            Err(SynthError::Syntax(format!(
                "expected {label}, found {got:?}"
            )))
        }
    }

    fn eat_semi(&mut self) -> Result<(), SynthError> {
        self.expect(Token::Semi, "';'")
    }

    fn ty(&mut self) -> Result<TensorType, SynthError> {
        let name = self.ident("dtype")?;
        let dtype = DType::parse(&name)
            .ok_or_else(|| SynthError::Syntax(format!("unknown dtype {name:?}")))?;
        self.expect(Token::LBracket, "'['")?;
        let n = self.uint("tensor length")? as usize;
        self.expect(Token::RBracket, "']'")?;
        self.eat_semi()?;
        Ok(TensorType::new(dtype, vec![n]))
    }
}

/// Parse a declarative intent source into an [`Intent`].
pub fn parse_intent(src: &str) -> Result<Intent, SynthError> {
    let mut c = Cursor::new(src)?;
    c.expect(Token::Ident("intent".into()), "'intent'")?;
    let name = c.ident("intent name")?;
    c.expect(Token::LBrace, "'{'")?;

    let mut input = None;
    let mut output = None;
    let mut demand = Level::L3;
    let mut kernels: Vec<Kernel> = Vec::new();
    let mut weights: Option<Vec<i64>> = None;
    let mut geometry = Geometry::Fixed;

    while *c.cur() != Token::RBrace {
        let field = c.ident("field name")?;
        match field.as_str() {
            "input" => input = Some(c.ty()?),
            "output" => output = Some(c.ty()?),
            "demand" => {
                let lvl = c.ident("receipt level")?;
                demand = Level::parse(&lvl)
                    .ok_or_else(|| SynthError::Syntax(format!("unknown level {lvl:?}")))?;
                c.eat_semi()?;
            }
            "pipeline" => {
                c.expect(Token::LBracket, "'['")?;
                if *c.cur() != Token::RBracket {
                    loop {
                        let k = c.ident("kernel name")?;
                        kernels.push(Kernel::parse(&k));
                        if *c.cur() == Token::Comma {
                            c.bump();
                            continue;
                        }
                        break;
                    }
                }
                c.expect(Token::RBracket, "']'")?;
                c.eat_semi()?;
            }
            "weights" => {
                c.expect(Token::LBracket, "'['")?;
                let mut ws = Vec::new();
                if *c.cur() != Token::RBracket {
                    loop {
                        ws.push(c.uint("weight")? as i64);
                        if *c.cur() == Token::Comma {
                            c.bump();
                            continue;
                        }
                        break;
                    }
                }
                c.expect(Token::RBracket, "']'")?;
                c.eat_semi()?;
                weights = Some(ws);
            }
            "geometry" => {
                let g = c.ident("geometry policy")?;
                geometry = match g.as_str() {
                    "fixed" => Geometry::Fixed,
                    "optimize" => Geometry::Optimize,
                    other => return Err(SynthError::Syntax(format!("unknown geometry {other:?}"))),
                };
                c.eat_semi()?;
            }
            other => {
                return Err(SynthError::Syntax(format!("unknown field {other:?}")));
            }
        }
    }
    c.expect(Token::RBrace, "'}'")?;
    c.expect(Token::Eof, "end of file")?;

    let input = input.ok_or_else(|| SynthError::Syntax("missing input".into()))?;
    let output = output.unwrap_or_else(|| input.clone());
    if kernels.is_empty() {
        return Err(SynthError::Syntax("empty pipeline".into()));
    }
    if input.len() != output.len() {
        return Err(SynthError::BadShape(format!(
            "input {} != output {}",
            input.len(),
            output.len()
        )));
    }
    if let Some(ws) = &weights {
        if ws.len() != kernels.len() {
            return Err(SynthError::WeightCount {
                kernels: kernels.len(),
                weights: ws.len(),
            });
        }
    }

    Ok(Intent {
        name,
        input,
        output,
        demand,
        kernels,
        weights,
        geometry,
    })
}

fn default_weight(seed: u64, idx: usize) -> i64 {
    // Non-negative (the Q lexer has no negative integer literal) and deterministic.
    (seed.wrapping_add(idx as u64 * 0x9E37_79B9) % 997) as i64
}

/// Expand an intent into a `QProgram` with the given geometry.
pub fn build_program(intent: &Intent, config: ProgramConfig) -> QProgram {
    let mut stages = Vec::new();
    let mut prev = intent_name(&intent.input);
    for (i, kernel) in intent.kernels.iter().enumerate() {
        let name = format!("s{i}");
        let weight = intent
            .weights
            .as_ref()
            .map(|w| w[i])
            .unwrap_or_else(|| default_weight(config.seed, i));
        stages.push(Stage {
            name: name.clone(),
            kernel: kernel.clone(),
            inputs: vec![prev],
            weight,
        });
        prev = name;
    }
    QProgram {
        name: intent.name.clone(),
        param: "x".into(),
        param_ty: intent.input.clone(),
        output_ty: intent.output.clone(),
        stages,
        emit: prev,
        demand: intent.demand,
        config,
    }
}

fn intent_name(_: &TensorType) -> String {
    "x".into()
}

/// The deterministic result of synthesis: a verified program plus its source
/// and receipts. Synthesize never returns a program that fails verification.
#[derive(Debug, Clone)]
pub struct Synthesis {
    pub intent: String,
    pub source: String,
    pub program: QProgram,
    pub geometry: Geometry,
    pub plan_hash: [u8; 32],
    pub chain_root: [u8; 32],
    pub receipt_bytes: u64,
    pub packet_count: usize,
    pub verified: [bool; 4],
}

fn default_throughput() -> LaneScaledThroughput {
    LaneScaledThroughput {
        base_tokens_per_second: 30.0,
        reference_lane_width: 8,
    }
}

/// Synthesize a verified `.q` program from intent source.
pub fn synthesize(src: &str) -> Result<Synthesis, SynthError> {
    let intent = parse_intent(src)?;
    let mut config = ProgramConfig::default();

    // Effect-aware refusal happens before any search: an impossible demand
    // (e.g. L3 over a Custom kernel, capped at L1) is rejected here.
    let probe = build_program(&intent, config.clone());
    check(&probe).map_err(|e| SynthError::Check(e.to_string()))?;

    let geometry = intent.geometry;
    if geometry == Geometry::Optimize {
        let cfg = OptimizeConfig {
            generations: 8,
            population: 8,
            seed: 5150,
            ..OptimizeConfig::default()
        };
        let result = optimize(&probe, default_throughput(), cfg);
        config.lane_width = result.best_genome.lane_width;
        config.tile_size = result.best_genome.tile_size;
        config.node_count = result.best_genome.node_count;
        config.seed = result.best_genome.seed;
    }

    let program = build_program(&intent, config);
    check(&program).map_err(|e| SynthError::Check(e.to_string()))?;
    let effect = receipt_effect(&program);
    if intent.demand > effect {
        return Err(SynthError::Unverifiable(format!(
            "demand {} exceeds program effect {}",
            intent.demand, effect
        )));
    }

    let r = run(&program).map_err(|e| SynthError::Unverifiable(e.to_string()))?;
    let verified = [
        r.report.level0,
        r.report.level1,
        r.report.level2,
        r.report.level3,
    ];
    let demand_ok = match intent.demand {
        Level::L0 => verified[0],
        Level::L1 => verified[1],
        Level::L2 => verified[2],
        Level::L3 => verified[3],
    };
    if !r.report.all_valid || !demand_ok {
        return Err(SynthError::Unverifiable(format!(
            "receipts did not verify at {}",
            intent.demand
        )));
    }

    Ok(Synthesis {
        intent: intent.name.clone(),
        source: render_q(&program),
        program,
        geometry,
        plan_hash: r.plan_hash,
        chain_root: r.chain_root,
        receipt_bytes: r.receipt_bytes,
        packet_count: r.packet_count,
        verified,
    })
}

fn dtype_str(d: DType) -> &'static str {
    match d {
        DType::I64 => "i64",
        DType::F32 => "f32",
    }
}

fn kernel_str(k: &Kernel) -> String {
    match k {
        Kernel::Dot => "dot".into(),
        Kernel::Fwht => "fwht".into(),
        Kernel::Gate => "gate".into(),
        Kernel::Custom(s) => s.clone(),
    }
}

/// Render a `QProgram` back to canonical `.q` source. The output re-parses to
/// an equivalent program (see tests/q3.rs round-trip).
pub fn render_q(p: &QProgram) -> String {
    let mut s = String::new();
    s.push_str(&format!("@receipt {}\n", p.demand));
    s.push_str(&format!(
        "qblock {}({}: {}[{}]) -> {}[{}] {{\n",
        p.name,
        p.param,
        dtype_str(p.param_ty.dtype),
        p.param_ty.len(),
        dtype_str(p.output_ty.dtype),
        p.output_ty.len()
    ));
    s.push_str("  config {\n");
    s.push_str(&format!("    model_id = {};\n", p.config.model_id));
    s.push_str(&format!("    lane_width = {};\n", p.config.lane_width));
    s.push_str(&format!("    tile_size = {};\n", p.config.tile_size));
    s.push_str(&format!("    node_count = {};\n", p.config.node_count));
    s.push_str(&format!("    seed = {};\n", p.config.seed));
    s.push_str("  }\n");
    for stage in &p.stages {
        let input = stage
            .inputs
            .first()
            .cloned()
            .unwrap_or_else(|| p.param.clone());
        s.push_str(&format!(
            "  let {} = {}({}, {});\n",
            stage.name,
            kernel_str(&stage.kernel),
            input,
            stage.weight
        ));
    }
    s.push_str(&format!("  emit {};\n", p.emit));
    s.push_str("}\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTENT: &str = r#"
intent pipeline {
  input i64[8];
  output i64[8];
  demand L3;
  pipeline [dot, dot, gate];
  weights [3, 5, 9];
  geometry fixed;
}
"#;

    #[test]
    fn parses_intent() {
        let i = parse_intent(INTENT).unwrap();
        assert_eq!(i.name, "pipeline");
        assert_eq!(i.demand, Level::L3);
        assert_eq!(i.kernels.len(), 3);
        assert_eq!(i.weights, Some(vec![3, 5, 9]));
        assert_eq!(i.geometry, Geometry::Fixed);
    }

    #[test]
    fn render_round_trips_through_parser() {
        let i = parse_intent(INTENT).unwrap();
        let p = build_program(&i, ProgramConfig::default());
        let source = render_q(&p);
        let reparsed = crate::parser::Parser::parse(&source).unwrap();
        assert_eq!(reparsed, p);
    }

    #[test]
    fn synthesis_is_deterministic_and_verified() {
        let a = synthesize(INTENT).unwrap();
        let b = synthesize(INTENT).unwrap();
        assert_eq!(a.program, b.program);
        assert_eq!(a.source, b.source);
        assert_eq!(a.plan_hash, b.plan_hash);
        assert_eq!(a.chain_root, b.chain_root);
        assert_eq!(a.packet_count, 3);
        assert!(a.verified.iter().all(|&v| v));
    }

    #[test]
    fn refuses_impossible_effect() {
        let bad = INTENT
            .replace("demand L3;", "demand L3;")
            .replace("gate];", "mystery];");
        let err = synthesize(&bad).unwrap_err();
        match err {
            SynthError::Check(_) | SynthError::Unverifiable(_) => {}
            other => panic!("expected effect refusal, got {other:?}"),
        }
    }

    #[test]
    fn rejects_weight_count_mismatch() {
        let bad = INTENT.replace("weights [3, 5, 9];", "weights [3, 5];");
        assert!(matches!(
            parse_intent(&bad),
            Err(SynthError::WeightCount { .. })
        ));
    }
}
