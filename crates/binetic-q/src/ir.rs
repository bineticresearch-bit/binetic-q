//! Binetic Q intermediate representation.
//!
//! A Qblock is the smallest unit of composition: a typed kernel with typed
//! ports and a receipt contract. A QGraph wires Qblocks output→input. The IR
//! is the compiler's "assembly" — both the text DSL and the future visual
//! editor lower to it.

use std::collections::HashMap;
use std::fmt;

/// Verification level demanded of a stream or Qblock. Mirrors
/// `binetic_verify::VerificationLevel` but is IR-local so the parser has no
/// dependency on the fabric crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    L0,
    L1,
    L2,
    L3,
}

impl Level {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "L0" => Some(Level::L0),
            "L1" => Some(Level::L1),
            "L2" => Some(Level::L2),
            "L3" => Some(Level::L3),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::L0 => "L0",
            Level::L1 => "L1",
            Level::L2 => "L2",
            Level::L3 => "L3",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Storage dtype on a port. Q0 supports the integer lane substrate; the
/// 23 `binetic-quant` dtypes arrive as annotations in Phase 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DType {
    I64,
    F32,
}

impl DType {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "i64" => Some(DType::I64),
            "f32" => Some(DType::F32),
            _ => None,
        }
    }
}

/// A typed tensor on a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TensorType {
    pub dtype: DType,
    pub shape: Vec<usize>,
}

impl TensorType {
    pub fn new(dtype: DType, shape: Vec<usize>) -> Self {
        Self { dtype, shape }
    }

    pub fn len(&self) -> usize {
        self.shape.iter().product()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Built-in kernel identifiers. Phase 2 adds the full Qblock stdlib; Q0 maps
/// these deterministically onto the fabric's `DotProductKernel`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kernel {
    Dot,
    Fwht,
    Gate,
    Custom(String),
}

impl Kernel {
    pub fn parse(s: &str) -> Self {
        match s {
            "dot" => Kernel::Dot,
            "fwht" => Kernel::Fwht,
            "gate" => Kernel::Gate,
            other => Kernel::Custom(other.to_string()),
        }
    }
}

/// A reference to a port: `(block index, port index)`.
pub type PortRef = (usize, usize);

/// Compile-time contract of a Qblock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockContract {
    pub deterministic: bool,
    pub replays_with: Option<String>,
    pub min_receipt: Level,
}

impl Default for BlockContract {
    fn default() -> Self {
        Self {
            deterministic: true,
            replays_with: None,
            min_receipt: Level::L0,
        }
    }
}

/// One typed, verified transform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Qblock {
    pub name: String,
    pub inputs: Vec<TensorType>,
    pub outputs: Vec<TensorType>,
    pub kernel: Kernel,
    pub state: Option<TensorType>,
    pub contract: BlockContract,
}

impl Qblock {
    pub fn new(name: impl Into<String>, kernel: Kernel) -> Self {
        Self {
            name: name.into(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            kernel,
            state: None,
            contract: BlockContract::default(),
        }
    }
}

/// A wired graph of Qblocks. Lowering maps one output tile to one vport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QGraph {
    pub blocks: Vec<Qblock>,
    pub edges: Vec<(PortRef, PortRef)>,
    pub demand: Level,
}

impl Default for QGraph {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            edges: Vec::new(),
            demand: Level::L0,
        }
    }
}

impl QGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_block(&mut self, block: Qblock) -> usize {
        let idx = self.blocks.len();
        self.blocks.push(block);
        idx
    }

    pub fn connect(&mut self, from: PortRef, to: PortRef) {
        self.edges.push((from, to));
    }
}

/// One source-level pipeline stage: a kernel call with named inputs and a
/// deterministic weight scalar. Pipelines (`h |> dot(5)`) desugar to a stage
/// whose first input is `h`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub name: String,
    pub kernel: Kernel,
    pub inputs: Vec<String>,
    pub weight: i64,
}

/// A parsed `.q` program. Q0 compiles a single top-level qblock into a graph
/// of stages; multi-block programs are Phase 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QProgram {
    pub name: String,
    pub param: String,
    pub param_ty: TensorType,
    pub output_ty: TensorType,
    pub stages: Vec<Stage>,
    pub emit: String,
    pub demand: Level,
    pub config: ProgramConfig,
}

/// Geometry carried from the `config { ... }` block into `PlanConfig`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramConfig {
    pub model_id: u32,
    pub lane_width: u16,
    pub tile_size: u32,
    pub node_count: u32,
    pub seed: u64,
}

impl Default for ProgramConfig {
    fn default() -> Self {
        Self {
            model_id: 11,
            lane_width: 8,
            tile_size: 8,
            node_count: 2,
            seed: 7,
        }
    }
}

/// A source-level name → block index map used by the checker/lowerer.
pub type SymbolTable = HashMap<String, usize>;
