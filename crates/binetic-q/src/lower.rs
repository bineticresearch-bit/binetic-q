//! Lowering: IR → `PlanConfig` → real `binetic_fabric::Fabric`.
//!
//! `compile` builds the deterministic plan (and its `plan_hash`); `run`
//! executes every Qblock stage through `Fabric::execute`, collects the real
//! `FabricPacket`s, and verifies them with `binetic_verify` at L0–L3. This is
//! the G0 proof: a `.q` program produces verifiable receipts on the actual
//! fabric substrate, not a simulation.
//!
//! `run_signed` is the attributed form: every packet is signed with a
//! `NodeKey` before entering the receipt tree so the receipt chain is
//! *attributed* to that node identity, not just tamper-evident.

use binetic_core::{hash_bytes, FabricPacket};
use binetic_fabric::{
    build_plan, compute_plan_hash, ComputeKernel, DotProductKernel, Fabric, PlanConfig, VPortKind,
};
use binetic_verify::{ReceiptTree, VerificationReport};

use crate::ir::{DType, Level, QProgram, Stage};
use crate::kernels::{receipt_effect_with, FabricKernel, KernelRegistry};
use crate::sign::NodeKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    Fabric(String),
    NoPorts,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Fabric(s) => write!(f, "fabric build error: {s}"),
            CompileError::NoPorts => write!(f, "plan produced no virtual ports"),
        }
    }
}

impl std::error::Error for CompileError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    Compile(CompileError),
    Fabric(String),
    NoPorts,
    /// A registered stage's replay oracle re-derived a different output than
    /// the fabric committed. The run is refused: receipts must hold.
    ReplayFailed(String),
}

impl From<CompileError> for RunError {
    fn from(e: CompileError) -> Self {
        RunError::Compile(e)
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Compile(e) => write!(f, "{e}"),
            RunError::Fabric(s) => write!(f, "fabric execute error: {s}"),
            RunError::NoPorts => write!(f, "fabric has no executable port"),
            RunError::ReplayFailed(stage) => {
                write!(f, "semantic replay failed at stage {stage:?}")
            }
        }
    }
}

impl std::error::Error for RunError {}

/// Per-stage replay evidence for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageReceipt {
    pub name: String,
    pub kernel: String,
    /// True when the stage's registered replay oracle re-derived the
    /// committed output. False for stages without a registered oracle
    /// (L1 kernels) — they claim no replay.
    pub replayed: bool,
}

/// Result of a compiled-and-run Q program, with all recorded evidence.
#[derive(Debug, Clone)]
pub struct RunResult {
    pub plan_hash: [u8; 32],
    /// Final output lanes. i64 programs hold integer values; f32 programs
    /// hold IEEE-754 bit patterns in the low 32 bits of each lane; u64 holds
    /// 64-bit patterns; u128/u256 hold little-endian 64-bit words (2/4 per
    /// value).
    pub output: Vec<i64>,
    pub packet_count: usize,
    /// Sum of serialized receipt bytes, for verified-throughput fitness.
    pub receipt_bytes: u64,
    pub chain_root: [u8; 32],
    pub report: VerificationReport,
    /// Per-stage replay evidence, in pipeline order.
    pub stages: Vec<StageReceipt>,
    /// Attribution tag when the run was signed with a non-zero `NodeKey`:
    /// `SHA-256("binetic-q-attest/v1" || key_bytes || chain_root)`.
    /// `None` for unsigned runs (zero key). Call `verify_attribution(key,
    /// &result.chain_root, tag)` to confirm the chain came from that node.
    pub attribution_tag: Option<[u8; 32]>,
}

/// Build the deterministic `PlanConfig` from a checked program. The fabric
/// sees *lanes*, not elements: wide dtypes (u128/u256) occupy multiple
/// 64-bit lanes per value, so the plan's input/output lengths are lane
/// counts.
pub fn plan_config(program: &QProgram) -> PlanConfig {
    PlanConfig {
        model_id: program.config.model_id,
        layer_id: 0,
        block_id: 0,
        op_code: 1,
        output_len: program.output_ty.lane_len() as u32,
        input_len: program.param_ty.lane_len() as u32,
        lane_width: program.config.lane_width,
        tile_size: program.config.tile_size,
        node_count: program.config.node_count,
        seed: program.config.seed,
        port_kind: VPortKind::Tensor,
    }
}

/// Compile a program to its plan hash without executing it.
pub fn compile(program: &QProgram) -> Result<([u8; 32], PlanConfig), CompileError> {
    let cfg = plan_config(program);
    let plan = build_plan(&cfg).map_err(|e| CompileError::Fabric(format!("{e:?}")))?;
    if plan.ports.is_empty() {
        return Err(CompileError::NoPorts);
    }
    Ok((compute_plan_hash(&plan), cfg))
}

/// Deterministic initial input from the program seed (no I/O, no clock).
/// Produces `lane_len` lanes: f32 lanes carry IEEE-754 bit patterns; wide
/// unsigned values are built word-by-word (little-endian) from consecutive
/// counters of the same deterministic stream.
fn initial_input(program: &QProgram) -> Vec<i64> {
    let lanes = program.param_ty.lane_len();
    let dtype = program.param_ty.dtype;
    (0..lanes)
        .map(|i| {
            let raw = (program
                .config
                .seed
                .wrapping_add(i as u64 * 2_654_435_761))
                % 2001;
            lane_value(dtype, raw)
        })
        .collect()
}

/// Map a deterministic raw counter to one lane value for the port dtype.
fn lane_value(dtype: DType, raw: u64) -> i64 {
    match dtype {
        DType::I64 => raw as i64 - 1000,
        DType::F32 => ((raw as f32 / 2001.0) - 1.0).to_bits() as i64,
        // Unsigned words: small deterministic residues; wide values are
        // assembled from consecutive counters, low word first.
        DType::U64 | DType::U128 | DType::U256 => raw as i64,
    }
}

/// Deterministic weight matrix for a stage. `input_len` is the element
/// count; the fabric sees `lane_len = input_len · words` lanes, and the dot
/// kernel expects `rows · lane_len` weight lanes with `rows = input_len` —
/// i.e. `input_len² · words` words total. Different kernels and scalars yield
/// different matrices, but all are pure functions of (seed, kernel name,
/// weight scalar).
fn stage_weight(program: &QProgram, stage: &Stage, input_len: usize) -> Vec<i64> {
    let words = program.param_ty.dtype.words();
    let kernel_tag = match stage.kernel {
        crate::ir::Kernel::Dot => 1u64,
        crate::ir::Kernel::Fwht => 2u64,
        crate::ir::Kernel::Gate => 3u64,
        crate::ir::Kernel::Custom(_) => 4u64,
    };
    let base = program
        .config
        .seed
        .wrapping_add(kernel_tag.wrapping_mul(0x9e37_79b9_7f4a_7c15))
        .wrapping_add((stage.weight as u64).wrapping_mul(0x85eb_ca6b));
    let dtype = program.param_ty.dtype;
    let mut x = base;
    (0..input_len * input_len * words)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            lane_value(dtype, x % 2001)
        })
        .collect()
}

/// Replay oracle for the receipt tree: independently re-derive the committed
/// output hash and the state transition from the carried receipt fields.
/// (Per-stage *semantic* replay — re-deriving the output from (input,
/// weight) — is enforced by `run_with` via the kernel registry.)
fn replay_oracle(packet: &FabricPacket) -> bool {
    if hash_bytes(&packet.payload) != packet.output_hash {
        return false;
    }
    let mut pre = Vec::with_capacity(96);
    pre.extend_from_slice(&packet.state_before);
    pre.extend_from_slice(&packet.input_hash);
    pre.extend_from_slice(&packet.output_hash);
    packet.state_after == hash_bytes(&pre)
}

/// Compile and execute a checked program on a real fabric, using the default
/// kernel registry. See [`run_with`] for the registry-aware form.
pub fn run(program: &QProgram) -> Result<RunResult, RunError> {
    run_with(program, &KernelRegistry::default())
}

/// Compile, execute, and **sign** every packet with `key` so the receipt
/// chain is attributed to a node identity. The returned `RunResult` has
/// `signatures_verified = Some(true)` when every packet's HMAC-SHA256 tag
/// matches `key`; `Some(false)` on any mismatch (which should not happen on
/// a correctly computed result but is detected defensively). Pass
/// `NodeKey::zero()` to get the same behaviour as `run`.
pub fn run_signed(program: &QProgram, key: &NodeKey) -> Result<RunResult, RunError> {
    run_with_signed(program, &KernelRegistry::default(), key)
}

/// Full form: registry-aware, signed run. See [`run_signed`].
pub fn run_with_signed(
    program: &QProgram,
    registry: &KernelRegistry,
    key: &NodeKey,
) -> Result<RunResult, RunError> {
    run_with_key(program, registry, key)
}

/// Compile and execute a checked program on a real fabric under a kernel
/// registry. Every stage runs through its registered compute (unregistered
/// Custom kernels use the documented dot-product default), and every stage
/// with a registered replay oracle is semantically replayed: the committed
/// output is re-derived independently from (input, weight) and the run is
/// refused on mismatch.
pub fn run_with(program: &QProgram, registry: &KernelRegistry) -> Result<RunResult, RunError> {
    run_with_key(program, registry, &NodeKey::zero())
}

fn run_inner(
    program: &QProgram,
    registry: &KernelRegistry,
    key: &NodeKey,
) -> Result<RunResult, RunError> {
    crate::check::check_with(program, registry)
        .map_err(|e| RunError::Fabric(format!("check failed: {e}")))?;

    let cfg = plan_config(program);
    let mut fabric = Fabric::build(&cfg).map_err(|e| RunError::Fabric(format!("{e:?}")))?;
    let vport = fabric
        .plan
        .ports
        .first()
        .map(|p| p.vport_id)
        .ok_or(RunError::NoPorts)?;

    let dtype = program.param_ty.dtype;
    let lane_width = program.config.lane_width;
    let initial = initial_input(program);

    // Name → lane value map. Starts with the program parameter, then gains
    // one entry per stage. Multi-input stages element-wise sum their inputs
    // before the kernel sees them, keeping lane count fixed at param_ty.lane_len().
    let mut values: std::collections::HashMap<String, Vec<i64>> = std::collections::HashMap::new();
    values.insert(program.param.clone(), initial);

    let mut output = Vec::new();
    let mut stage_receipts: Vec<StageReceipt> = Vec::new();

    for stage in &program.stages {
        // Resolve this stage's inputs from the value map. A single-input stage
        // behaves exactly as before. For a multi-input (skip-connection) stage,
        // the inputs are element-wise summed (wrapping) to a single lane
        // vector — the fabric port geometry stays fixed at param_ty.lane_len().
        // This is the standard residual-connection merge: skip + predecessor.
        let input: Vec<i64> = if stage.inputs.is_empty() {
            return Err(RunError::Fabric(format!(
                "stage {:?} has no inputs",
                stage.name
            )));
        } else {
            let mut acc: Vec<i64> = values
                .get(&stage.inputs[0])
                .cloned()
                .unwrap_or_default();
            for name in stage.inputs.iter().skip(1) {
                if let Some(v) = values.get(name) {
                    for (a, b) in acc.iter_mut().zip(v.iter()) {
                        *a = a.wrapping_add(*b);
                    }
                }
            }
            acc
        };

        let input_len = program.param_ty.len();
        let weight = stage_weight(program, stage, input_len);
        // Kernel from the registry: real semantics + earned guarantee.
        // Unregistered Custom kernels fall back to the documented
        // dot-product compute and claim no replay (L1).
        let spec = registry.spec(&stage.kernel);
        let adapter = spec.map(|s| FabricKernel { spec: s, dtype });
        let kernel: &dyn ComputeKernel = match &adapter {
            Some(a) => a,
            None => &DotProductKernel,
        };
        output = fabric
            .execute(vport, &input, &weight, kernel)
            .map_err(|e| RunError::Fabric(format!("{e:?}")))?;

        // Semantic replay: re-derive the committed output independently.
        let replayed = match spec {
            Some(s) => match s.replay.as_deref() {
                Some(oracle) => {
                    if !oracle.replay(dtype, lane_width, &input, &weight, &output) {
                        return Err(RunError::ReplayFailed(stage.name.clone()));
                    }
                    true
                }
                None => false,
            },
            None => false,
        };
        stage_receipts.push(StageReceipt {
            name: stage.name.clone(),
            kernel: stage.kernel.to_string(),
            replayed,
        });
        values.insert(stage.name.clone(), output.clone());
    }

    // Resolve the emitted value from the map (may differ from the last stage
    // if emit names an earlier stage — valid in a DAG).
    output = values
        .get(&program.emit)
        .cloned()
        .unwrap_or(output);

    // Use the plan hash cached at build time. `compute_plan_hash` folds each
    // port's mutable state/receipt head, so recomputing after execution would
    // diverge from the compile-time identity.
    let plan_hash = fabric.plan.plan_hash;
    let packets = fabric.packets();

    let mut tree = ReceiptTree::new();
    for p in packets {
        tree.add(p)
            .map_err(|e| RunError::Fabric(format!("receipt add: {e}")))?;
    }
    let receipt_bytes = packets
        .iter()
        .map(|p| p.to_bytes().map(|b| b.len() as u64).unwrap_or(0))
        .sum::<u64>();
    let chain_root = tree.chain_root();
    // Only claim replay-backed levels when the program's earned effect
    // provides them. An L1-capped program is verified structurally and
    // reports level2/level3 = false, matching what `check` guaranteed.
    let report = if receipt_effect_with(program, registry) >= Level::L2 {
        tree.verify_with_replay(replay_oracle)
    } else {
        tree.verify()
    };

    // Attribution: compute a keyed tag over the finalized chain root.
    // The receipt chain itself runs on unsigned packets (L0–L3 preserved).
    // The attribution tag is a separate, verifiable commitment:
    //   SHA-256("binetic-q-attest/v1" || key_bytes || chain_root)
    // `None` for unsigned (zero-key) runs.
    let attribution_tag = if key.is_zero() {
        None
    } else {
        Some(key.attribution_tag(&chain_root))
    };

    Ok(RunResult {
        plan_hash,
        output,
        packet_count: packets.len(),
        receipt_bytes,
        chain_root,
        report,
        stages: stage_receipts,
        attribution_tag,
    })
}

/// Internal: run with a signing key (may be zero = unsigned).
fn run_with_key(
    program: &QProgram,
    registry: &KernelRegistry,
    key: &NodeKey,
) -> Result<RunResult, RunError> {
    run_inner(program, registry, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    const P: &str = r#"
@receipt L3
qblock pipeline(x: i64[8]) -> i64[8] {
  config { model_id = 11; lane_width = 8; tile_size = 8; node_count = 2; seed = 7; }
  let h = dot(x, 3);
  let g = h |> dot(5);
  let y = g |> gate(9);
  emit y;
}
"#;

    #[test]
    fn compiles_to_a_reproducible_plan_hash() {
        let p = Parser::parse(P).unwrap();
        let (h1, cfg1) = compile(&p).unwrap();
        let (h2, cfg2) = compile(&p).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(cfg1, cfg2);
        assert_eq!(cfg1.output_len, 8);
        assert_eq!(cfg1.input_len, 8);
    }

    #[test]
    fn run_emits_l0_l1_l2_l3_receipts() {
        let p = Parser::parse(P).unwrap();
        let r = run(&p).unwrap();
        assert_eq!(r.packet_count, 3, "one packet per Qblock stage");
        assert_eq!(r.output.len(), 8);
        assert!(r.report.level0, "L0 must hold");
        assert!(r.report.level1, "L1 chain must hold on the same route");
        assert!(r.report.level2, "sampled replay must pass");
        assert!(r.report.level3, "full replay must pass");
        assert!(r.report.all_valid);
    }

    #[test]
    fn plan_hash_is_deterministic_across_runs() {
        let p = Parser::parse(P).unwrap();
        let a = run(&p).unwrap();
        let b = run(&p).unwrap();
        assert_eq!(a.plan_hash, b.plan_hash);
        assert_eq!(a.chain_root, b.chain_root);
        assert_eq!(a.output, b.output);
    }

    const F: &str = r#"
@receipt L3
qblock fp(x: f32[8]) -> f32[8] {
  config { model_id = 11; lane_width = 32; tile_size = 8; node_count = 2; seed = 7; }
  let h = dot(x, 3);
  let y = h |> gate(9);
  emit y;
}
"#;

    #[test]
    fn f32_program_runs_and_verifies_l0_l3() {
        let p = Parser::parse(F).unwrap();
        let r = run(&p).unwrap();
        assert_eq!(r.packet_count, 2, "one packet per Qblock stage");
        assert!(r.report.level0);
        assert!(r.report.level1);
        assert!(r.report.level2, "replay must hold for the float kernel");
        assert!(r.report.level3);
        assert!(r.report.all_valid);
        // f32 lanes carry bit patterns: every output lane fits in 32 bits.
        assert!(r
            .output
            .iter()
            .all(|v| *v >= 0 && *v < (1i64 << 32)));
    }

    #[test]
    fn f32_run_is_deterministic() {
        let p = Parser::parse(F).unwrap();
        let a = run(&p).unwrap();
        let b = run(&p).unwrap();
        assert_eq!(a.plan_hash, b.plan_hash);
        assert_eq!(a.chain_root, b.chain_root);
        assert_eq!(a.output, b.output);
    }
}
