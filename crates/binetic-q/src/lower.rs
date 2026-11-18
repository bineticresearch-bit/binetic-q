//! Lowering: IR → `PlanConfig` → real `binetic_fabric::Fabric`.
//!
//! `compile` builds the deterministic plan (and its `plan_hash`); `run`
//! executes every Qblock stage through `Fabric::execute`, collects the real
//! `FabricPacket`s, and verifies them with `binetic_verify` at L0–L3. This is
//! the G0 proof: a `.q` program produces verifiable receipts on the actual
//! fabric substrate, not a simulation.

use binetic_core::{hash_bytes, FabricPacket};
use binetic_fabric::{
    build_plan, compute_plan_hash, DotProductKernel, Fabric, PlanConfig, VPortKind,
};
use binetic_verify::{ReceiptTree, VerificationReport};

use crate::ir::{receipt_effect, Level, QProgram, Stage};

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
        }
    }
}

impl std::error::Error for RunError {}

/// Result of a compiled-and-run Q program, with all recorded evidence.
#[derive(Debug, Clone)]
pub struct RunResult {
    pub plan_hash: [u8; 32],
    pub output: Vec<i64>,
    pub packet_count: usize,
    pub chain_root: [u8; 32],
    pub report: VerificationReport,
}

/// Build the deterministic `PlanConfig` from a checked program.
pub fn plan_config(program: &QProgram) -> PlanConfig {
    let out_len = program.output_ty.len() as u32;
    let in_len = program.param_ty.len() as u32;
    PlanConfig {
        model_id: program.config.model_id,
        layer_id: 0,
        block_id: 0,
        op_code: 1,
        output_len: out_len,
        input_len: in_len,
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
fn initial_input(program: &QProgram) -> Vec<i64> {
    let n = program.param_ty.len();
    (0..n)
        .map(|i| {
            ((program.config.seed.wrapping_add(i as u64 * 2_654_435_761)) % 2001) as i64 - 1000
        })
        .collect()
}

/// Deterministic weight matrix for a stage: `input_len` rows × `input_len`
/// cols. Different kernels and scalars yield different matrices, but all are
/// pure functions of (seed, kernel name, weight scalar).
fn stage_weight(program: &QProgram, stage: &Stage, input_len: usize) -> Vec<i64> {
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
    let mut x = base;
    (0..input_len * input_len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % 2001) as i64 - 1000
        })
        .collect()
}

/// Replay oracle: independently re-derive the committed output hash and the
/// state transition from the carried receipt fields.
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

/// Compile and execute a checked program on a real fabric.
pub fn run(program: &QProgram) -> Result<RunResult, RunError> {
    crate::check(program).map_err(|e| RunError::Fabric(format!("check failed: {e}")))?;

    let cfg = plan_config(program);
    let mut fabric = Fabric::build(&cfg).map_err(|e| RunError::Fabric(format!("{e:?}")))?;
    let vport = fabric
        .plan
        .ports
        .first()
        .map(|p| p.vport_id)
        .ok_or(RunError::NoPorts)?;

    let input_len = program.param_ty.len();
    let mut input = initial_input(program);
    let mut output = input.clone();

    for stage in &program.stages {
        let weight = stage_weight(program, stage, input_len);
        output = fabric
            .execute(vport, &input, &weight, &DotProductKernel)
            .map_err(|e| RunError::Fabric(format!("{e:?}")))?;
        input = output.clone();
    }

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
    let chain_root = tree.chain_root();
    // Only claim replay-backed levels when the program's effect provides them.
    // A Custom-capped program is verified structurally (L0/L1) and reports
    // level2/level3 = false, matching what `check` statically guaranteed.
    let report = if receipt_effect(program) >= Level::L2 {
        tree.verify_with_replay(replay_oracle)
    } else {
        tree.verify()
    };

    Ok(RunResult {
        plan_hash,
        output,
        packet_count: packets.len(),
        chain_root,
        report,
    })
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
}
