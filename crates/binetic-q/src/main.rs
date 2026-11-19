//! `binetic-q` CLI: compile / check / run / optimize / synth / evolve.
//!
//! Prints one JSON object on stdout on success; errors go to stderr with a
//! non-zero exit code (mirrors the `binetic` CLI contract).
//!
//! The fitness model is a user choice on `optimize`/`evolve`:
//! `--throughput lane` (default; `--tps` at `--ref-lane` lanes) or
//! `--throughput fixed --tps N`, plus `--accuracy` and `--tokens` for the
//! score's accuracy and receipt-normalization terms.

use std::path::PathBuf;

use binetic_evolver::{FixedThroughput, LaneScaledThroughput};
use binetic_q::{
    build_program, check, compile, evolve, optimize, parse_intent, parser::Parser,
    receipt_effect, render_q, run, run_signed, synthesize, EvolveConfig, NodeKey, OptimizeConfig,
    StructureGenome,
};
use serde::Serialize;

#[derive(Debug)]
enum Command {
    Check {
        source: PathBuf,
    },
    Compile {
        source: PathBuf,
    },
    Run {
        source: PathBuf,
    },
    Optimize {
        source: PathBuf,
        generations: usize,
        population: usize,
        seed: u64,
        model: ModelFlags,
    },
    Synth {
        source: PathBuf,
    },
    Evolve {
        source: PathBuf,
        generations: usize,
        population: usize,
        seed: u64,
        max_stages: usize,
        model: ModelFlags,
    },
    Attest {
        source: PathBuf,
        /// Optional node key seed for attribution signing. `None` = unsigned.
        sign_seed: Option<u64>,
    },
}

/// Caller-chosen fitness model inputs for `optimize`/`evolve`.
#[derive(Debug, Clone)]
struct ModelFlags {
    throughput_kind: String,
    tps: f64,
    ref_lane: u16,
    accuracy: f64,
    tokens: u32,
}

impl Default for ModelFlags {
    fn default() -> Self {
        Self {
            throughput_kind: "lane".into(),
            tps: 30.0,
            ref_lane: 8,
            accuracy: 0.95,
            tokens: 32,
        }
    }
}

/// The concrete throughput model chosen on the CLI. The library stays
/// generic over any `ThroughputModel`; the CLI offers these two.
enum Throughput {
    Lane(LaneScaledThroughput),
    Fixed(FixedThroughput),
}

impl ModelFlags {
    /// Materialize the chosen throughput model.
    fn throughput(&self) -> Result<Throughput, String> {
        match self.throughput_kind.as_str() {
            "lane" => Ok(Throughput::Lane(LaneScaledThroughput {
                base_tokens_per_second: self.tps,
                reference_lane_width: self.ref_lane,
            })),
            "fixed" => Ok(Throughput::Fixed(FixedThroughput(self.tps))),
            other => Err(format!(
                "unknown --throughput {other:?}; use lane or fixed"
            )),
        }
    }
}

fn parse_args() -> Result<Command, String> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let source = args.get(2).ok_or_else(|| {
        "usage: binetic-q <check|compile|run|optimize|synth|evolve|attest> <file> [flags]".to_string()
    })?;
    let source = PathBuf::from(source);
    let flag = |name: &str, default: &str| -> Result<String, String> {
        match args.iter().position(|a| a == name) {
            Some(i) => args
                .get(i + 1)
                .cloned()
                .ok_or_else(|| format!("{name} needs a value")),
            None => Ok(default.to_string()),
        }
    };
    let model = || -> Result<ModelFlags, String> {
        Ok(ModelFlags {
            throughput_kind: flag("--throughput", "lane")?,
            tps: flag("--tps", "30")?.parse().map_err(|_| "bad --tps".to_string())?,
            ref_lane: flag("--ref-lane", "8")?
                .parse()
                .map_err(|_| "bad --ref-lane".to_string())?,
            accuracy: flag("--accuracy", "0.95")?
                .parse()
                .map_err(|_| "bad --accuracy".to_string())?,
            tokens: flag("--tokens", "32")?
                .parse()
                .map_err(|_| "bad --tokens".to_string())?,
        })
    };
    match cmd {
        "check" => Ok(Command::Check { source }),
        "compile" => Ok(Command::Compile { source }),
        "run" => Ok(Command::Run { source }),
        "optimize" => Ok(Command::Optimize {
            source,
            generations: flag("--gen", "8")?
                .parse()
                .map_err(|_| "bad --gen".to_string())?,
            population: flag("--pop", "8")?
                .parse()
                .map_err(|_| "bad --pop".to_string())?,
            seed: flag("--seed", "5150")?
                .parse()
                .map_err(|_| "bad --seed".to_string())?,
            model: model()?,
        }),
        "synth" => Ok(Command::Synth { source }),
        "attest" => Ok(Command::Attest {
            source,
            sign_seed: match args.iter().position(|a| a == "--sign-key") {
                Some(i) => Some(
                    args.get(i + 1)
                        .ok_or_else(|| "--sign-key needs a seed value".to_string())?
                        .parse::<u64>()
                        .map_err(|_| "bad --sign-key (expected u64 seed)".to_string())?,
                ),
                None => None,
            },
        }),
        "evolve" => Ok(Command::Evolve {
            source,
            generations: flag("--gen", "8")?
                .parse()
                .map_err(|_| "bad --gen".to_string())?,
            population: flag("--pop", "8")?
                .parse()
                .map_err(|_| "bad --pop".to_string())?,
            seed: flag("--seed", "5150")?
                .parse()
                .map_err(|_| "bad --seed".to_string())?,
            max_stages: flag("--max-stages", "4")?
                .parse()
                .map_err(|_| "bad --max-stages".to_string())?,
            model: model()?,
        }),
        other => Err(format!(
            "unknown command {other:?}; use check|compile|run|optimize|synth|evolve|attest"
        )),
    }
}

#[derive(Serialize)]
struct CheckJson {
    program: String,
    stages: usize,
    demand: String,
    effect: String,
    ok: bool,
}

#[derive(Serialize)]
struct CompileJson {
    program: String,
    plan_hash: String,
    lanes: u16,
    tile_size: u32,
}

#[derive(Serialize)]
struct RunJson {
    program: String,
    plan_hash: String,
    chain_root: String,
    packets: usize,
    level0: bool,
    level1: bool,
    level2: bool,
    level3: bool,
    all_valid: bool,
}

#[derive(Serialize)]
struct OptimizeJson {
    program: String,
    generations: usize,
    population: usize,
    seed: u64,
    best_lane_width: u16,
    best_tile_size: u32,
    best_node_count: u32,
    best_genome_seed: u64,
    baseline_score: f64,
    best_score: f64,
    improved: bool,
    plan_hash: String,
    chain_root: String,
    receipt_bytes: u64,
    all_levels_valid: bool,
}

#[derive(Serialize)]
struct SynthJson {
    intent: String,
    geometry: String,
    plan_hash: String,
    chain_root: String,
    packets: usize,
    receipt_bytes: u64,
    level0: bool,
    level1: bool,
    level2: bool,
    level3: bool,
    verified: bool,
    source: String,
}

#[derive(Serialize)]
struct EvolveJson {
    intent: String,
    demand: String,
    stages: usize,
    kernels: Vec<String>,
    baseline_score: f64,
    best_score: f64,
    improved: bool,
    generations: usize,
    population: usize,
    plan_hash: String,
    chain_root: String,
    packets: usize,
    receipt_bytes: u64,
    level0: bool,
    level1: bool,
    level2: bool,
    level3: bool,
    all_valid: bool,
    source: String,
}

#[derive(Serialize)]
struct StageJson {
    name: String,
    kernel: String,
    replayed: bool,
}

/// The certificate: the entire claim in one deterministic artifact. Re-running
/// `attest` on the same source must reproduce the plan hash and chain root
/// byte for byte — determinism is the verification.
#[derive(Serialize)]
struct AttestJson {
    program: String,
    demand: String,
    effect: String,
    stages: Vec<StageJson>,
    plan_hash: String,
    chain_root: String,
    packets: usize,
    receipt_bytes: u64,
    level0: bool,
    level1: bool,
    level2: bool,
    level3: bool,
    all_valid: bool,
    /// `null` = unsigned run; hex string = attribution tag over chain_root.
    /// Verify with `verify_attribution(key, chain_root, tag)`.
    attribution_tag: Option<String>,
    source: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_source(path: &PathBuf) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))
}

fn main() {
    let result = real_main();
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<(), String> {
    let cmd = parse_args()?;
    match cmd {
        Command::Check { source } => {
            let src = read_source(&source)?;
            let p = Parser::parse(&src).map_err(|e| format!("parse: {e}"))?;
            check(&p).map_err(|e| format!("check: {e}"))?;
            println!(
                "{}",
                serde_json::to_string(&CheckJson {
                    program: p.name.clone(),
                    stages: p.stages.len(),
                    demand: p.demand.as_str().to_string(),
                    effect: receipt_effect(&p).as_str().to_string(),
                    ok: true,
                })
                .unwrap()
            );
        }
        Command::Compile { source } => {
            let src = read_source(&source)?;
            let p = Parser::parse(&src).map_err(|e| format!("parse: {e}"))?;
            let (h, cfg) = compile(&p).map_err(|e| format!("compile: {e}"))?;
            println!(
                "{}",
                serde_json::to_string(&CompileJson {
                    program: p.name.clone(),
                    plan_hash: hex(&h),
                    lanes: cfg.lane_width,
                    tile_size: cfg.tile_size,
                })
                .unwrap()
            );
        }
        Command::Run { source } => {
            let src = read_source(&source)?;
            let p = Parser::parse(&src).map_err(|e| format!("parse: {e}"))?;
            let r = run(&p).map_err(|e| format!("run: {e}"))?;
            println!(
                "{}",
                serde_json::to_string(&RunJson {
                    program: p.name,
                    plan_hash: hex(&r.plan_hash),
                    chain_root: hex(&r.chain_root),
                    packets: r.packet_count,
                    level0: r.report.level0,
                    level1: r.report.level1,
                    level2: r.report.level2,
                    level3: r.report.level3,
                    all_valid: r.report.all_valid,
                })
                .unwrap()
            );
        }
        Command::Optimize {
            source,
            generations,
            population,
            seed,
            model,
        } => {
            let src = read_source(&source)?;
            let p = Parser::parse(&src).map_err(|e| format!("parse: {e}"))?;
            let cfg = OptimizeConfig {
                seed,
                population,
                generations,
                model_accuracy: model.accuracy,
                tokens: model.tokens,
                ..OptimizeConfig::default()
            };
            // `optimize` is generic over the throughput model; the CLI picks
            // a concrete one from the flags.
            let model = model.throughput()?;
            let r = match model {
                Throughput::Lane(l) => optimize(&p, l, cfg),
                Throughput::Fixed(f) => optimize(&p, f, cfg),
            };
            println!(
                "{}",
                serde_json::to_string(&OptimizeJson {
                    program: p.name,
                    generations: r.generations,
                    population: r.population_size,
                    seed,
                    best_lane_width: r.best_genome.lane_width,
                    best_tile_size: r.best_genome.tile_size,
                    best_node_count: r.best_genome.node_count,
                    best_genome_seed: r.best_genome.seed,
                    baseline_score: r.baseline_fitness.score,
                    best_score: r.best_fitness.score,
                    improved: r.improved(),
                    plan_hash: hex(&r.plan_hash),
                    chain_root: hex(&r.chain_root),
                    receipt_bytes: r.receipt_bytes,
                    all_levels_valid: r.verified_levels.iter().all(|&b| b),
                })
                .unwrap()
            );
        }
        Command::Synth { source } => {
            let src = read_source(&source)?;
            let s = synthesize(&src).map_err(|e| format!("synth: {e}"))?;
            println!(
                "{}",
                serde_json::to_string(&SynthJson {
                    intent: s.intent_name,
                    geometry: match s.geometry {
                        binetic_q::Geometry::Fixed => "fixed".into(),
                        binetic_q::Geometry::Optimize => "optimize".into(),
                    },
                    plan_hash: hex(&s.plan_hash),
                    chain_root: hex(&s.chain_root),
                    packets: s.packet_count,
                    receipt_bytes: s.receipt_bytes,
                    level0: s.verified[0],
                    level1: s.verified[1],
                    level2: s.verified[2],
                    level3: s.verified[3],
                    verified: s.verified.iter().all(|&v| v),
                    source: s.source,
                })
                .unwrap()
            );
        }
        Command::Evolve {
            source,
            generations,
            population,
            seed,
            max_stages,
            model,
        } => {
            let src = read_source(&source)?;
            let intent = parse_intent(&src).map_err(|e| format!("intent: {e}"))?;
            // The author's pipeline is the baseline: same default-weight
            // derivation as Q3 synthesis, so evolution starts where the
            // author left off.
            let starting = build_program(&intent, Default::default());
            let initial = StructureGenome {
                kernels: starting.stages.iter().map(|s| s.kernel.clone()).collect(),
                weights: starting.stages.iter().map(|s| s.weight).collect(),
                skip: vec![None; starting.stages.len()],
                lane_width: starting.config.lane_width,
                tile_size: starting.config.tile_size,
                node_count: starting.config.node_count,
                seed: starting.config.seed,
            };
            let cfg = EvolveConfig {
                seed,
                population,
                generations,
                max_stages,
                model_accuracy: model.accuracy,
                tokens: model.tokens,
                ..EvolveConfig::default()
            };
            let model = model.throughput()?;
            let r = match model {
                Throughput::Lane(l) => evolve(
                    &intent.name,
                    intent.input.clone(),
                    intent.output.clone(),
                    intent.demand,
                    &initial,
                    l,
                    cfg,
                ),
                Throughput::Fixed(f) => evolve(
                    &intent.name,
                    intent.input.clone(),
                    intent.output.clone(),
                    intent.demand,
                    &initial,
                    f,
                    cfg,
                ),
            };
            println!(
                "{}",
                serde_json::to_string(&EvolveJson {
                    intent: intent.name,
                    demand: intent.demand.as_str().to_string(),
                    stages: r.best.kernels.len(),
                    kernels: r.best.kernels.iter().map(|k| k.to_string()).collect(),
                    baseline_score: r.baseline_fitness.score,
                    best_score: r.best_fitness.score,
                    improved: r.improved(),
                    generations: r.generations,
                    population: r.population_size,
                    plan_hash: hex(&r.plan_hash),
                    chain_root: hex(&r.chain_root),
                    packets: r.packet_count,
                    receipt_bytes: r.receipt_bytes,
                    level0: r.verified_levels[0],
                    level1: r.verified_levels[1],
                    level2: r.verified_levels[2],
                    level3: r.verified_levels[3],
                    all_valid: r.verified_levels.iter().all(|&b| b),
                    source: render_q(&r.program),
                })
                .unwrap()
            );
        }
        Command::Attest { source, sign_seed } => {
            let src = read_source(&source)?;
            let p = Parser::parse(&src).map_err(|e| format!("parse: {e}"))?;
            let effect = receipt_effect(&p).as_str().to_string();
            let key = sign_seed.map(NodeKey::from_seed).unwrap_or_else(NodeKey::zero);
            let r = run_signed(&p, &key).map_err(|e| format!("run: {e}"))?;
            println!(
                "{}",
                serde_json::to_string(&AttestJson {
                    program: p.name,
                    demand: p.demand.as_str().to_string(),
                    effect,
                    stages: r
                        .stages
                        .iter()
                        .map(|s| StageJson {
                            name: s.name.clone(),
                            kernel: s.kernel.clone(),
                            replayed: s.replayed,
                        })
                        .collect(),
                    plan_hash: hex(&r.plan_hash),
                    chain_root: hex(&r.chain_root),
                    packets: r.packet_count,
                    receipt_bytes: r.receipt_bytes,
                    level0: r.report.level0,
                    level1: r.report.level1,
                    level2: r.report.level2,
                    level3: r.report.level3,
                    all_valid: r.report.all_valid,
                    attribution_tag: r.attribution_tag.map(|t| hex(&t)),
                    source: src,
                })
                .unwrap()
            );
        }
    }
    Ok(())
}
