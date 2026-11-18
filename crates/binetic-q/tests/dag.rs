//! Move 2 — Live DAG Topology integration tests.
//!
//! Verifies that:
//! 1. A multi-input (skip-connection) `.q` program parsed from source
//!    compiles, runs, and verifies at L3.
//! 2. `render_q` → `Parser::parse` round-trips a multi-input stage without
//!    loss.
//! 3. `StructureGenome::program()` with skip connections produces a valid
//!    QProgram that checks and runs clean.
//! 4. The evolver accepts an initial genome with a skip connection and still
//!    produces a verifying result.
//! 5. Determinism: two independent runs of a DAG program yield identical
//!    plan hashes, chain roots, and outputs.

use binetic_q::{
    check, run, render_q,
    evolve, EvolveConfig, StructureGenome,
};
use binetic_q::parser::Parser;

// ---------------------------------------------------------------------------
// Source-level multi-input (skip-connection) program.
// Stage `r` takes two named inputs: `g` (previous) and `h` (skip from s0).
// ---------------------------------------------------------------------------
const RESIDUAL: &str = r#"
@receipt L3
qblock residual(x: i64[8]) -> i64[8] {
  config {
    model_id = 11;
    lane_width = 8;
    tile_size = 8;
    node_count = 2;
    seed = 7;
  }
  let h = dot(x, 3);
  let g = dot(h, 5);
  let r = gate(g, h, 9);
  emit r;
}
"#;

#[test]
fn residual_program_parses_and_checks() {
    let p = Parser::parse(RESIDUAL).unwrap();
    assert_eq!(p.stages.len(), 3);
    // The skip stage takes two inputs.
    let r_stage = p.stages.iter().find(|s| s.name == "r").unwrap();
    assert_eq!(r_stage.inputs, vec!["g", "h"]);
    assert_eq!(r_stage.weight, 9);
    check(&p).unwrap();
}

#[test]
fn residual_program_runs_and_verifies_l3() {
    let p = Parser::parse(RESIDUAL).unwrap();
    let r = run(&p).unwrap();
    assert_eq!(r.packet_count, 3, "one packet per stage");
    assert_eq!(r.output.len(), 8);
    assert!(r.report.level0, "L0 must hold");
    assert!(r.report.level1, "L1 chain must hold");
    assert!(r.report.level2, "L2 sampled replay must pass");
    assert!(r.report.level3, "L3 full replay must pass");
    assert!(r.report.all_valid);
    // Replay receipts were claimed for all three stages.
    assert!(r.stages.iter().all(|s| s.replayed), "all stages must replay");
}

#[test]
fn residual_program_is_deterministic() {
    let p = Parser::parse(RESIDUAL).unwrap();
    let a = run(&p).unwrap();
    let b = run(&p).unwrap();
    assert_eq!(a.plan_hash, b.plan_hash);
    assert_eq!(a.chain_root, b.chain_root);
    assert_eq!(a.output, b.output);
}

#[test]
fn render_q_roundtrip_preserves_multi_input_stage() {
    let p = Parser::parse(RESIDUAL).unwrap();
    let src = render_q(&p);
    // The rendered source must contain the two-input stage call.
    assert!(
        src.contains("gate(g, h, 9)"),
        "rendered source must include multi-input call; got:\n{src}"
    );
    let reparsed = Parser::parse(&src).unwrap();
    assert_eq!(reparsed, p, "render→parse round-trip must be lossless");
}

// ---------------------------------------------------------------------------
// Genome-level DAG: StructureGenome with a skip connection.
// ---------------------------------------------------------------------------

use binetic_q::ir::{DType, Kernel, Level, TensorType};

fn tensor8() -> TensorType {
    TensorType::new(DType::I64, vec![8])
}

#[test]
fn genome_with_skip_produces_valid_program() {
    // 3-stage genome: s0=dot, s1=dot, s2=gate; skip[2]=Some(0) means s2 also
    // takes s0's output as a second input.
    let genome = StructureGenome {
        kernels: vec![Kernel::Dot, Kernel::Dot, Kernel::Gate],
        weights: vec![3, 5, 9],
        skip: vec![None, None, Some(0)],
        lane_width: 8,
        tile_size: 8,
        node_count: 2,
        seed: 7,
    };
    let program = genome.program("res", &tensor8(), &tensor8(), Level::L3);

    // s2 must have two inputs: s1 (predecessor) and s0 (skip).
    let s2 = program.stages.iter().find(|s| s.name == "s2").unwrap();
    assert_eq!(s2.inputs, vec!["s1", "s0"]);

    check(&program).unwrap();
    let r = run(&program).unwrap();
    assert!(r.report.all_valid);
    assert!(r.report.level3);
}

#[test]
fn genome_with_skip_is_deterministic() {
    let genome = StructureGenome {
        kernels: vec![Kernel::Dot, Kernel::Dot, Kernel::Gate],
        weights: vec![3, 5, 9],
        skip: vec![None, None, Some(0)],
        lane_width: 8,
        tile_size: 8,
        node_count: 2,
        seed: 7,
    };
    let program = genome.program("res", &tensor8(), &tensor8(), Level::L3);
    let a = run(&program).unwrap();
    let b = run(&program).unwrap();
    assert_eq!(a.plan_hash, b.plan_hash);
    assert_eq!(a.chain_root, b.chain_root);
    assert_eq!(a.output, b.output);
}

#[test]
fn evolve_with_skip_initial_produces_verifying_result() {
    // Start the evolver from a genome that already has a skip connection.
    // The evolver must still find a verifying result.
    let initial = StructureGenome {
        kernels: vec![Kernel::Dot, Kernel::Dot, Kernel::Gate],
        weights: vec![3, 5, 9],
        skip: vec![None, None, Some(0)],
        lane_width: 8,
        tile_size: 8,
        node_count: 2,
        seed: 7,
    };
    use binetic_q::fitness::default_throughput;
    let r = evolve(
        "res",
        tensor8(),
        tensor8(),
        Level::L3,
        &initial,
        default_throughput(),
        EvolveConfig {
            seed: 99,
            population: 4,
            generations: 4,
            max_stages: 4,
            ..EvolveConfig::default()
        },
    );
    assert!(r.verified_levels.iter().all(|&v| v), "must verify at L3");
    assert!(r.best_fitness.score > 0.0);
}

#[test]
fn linear_pipeline_has_all_none_skips_after_evolve_program() {
    // A genome with all-None skips lowers to a program with single-input stages.
    let genome = StructureGenome {
        kernels: vec![Kernel::Dot, Kernel::Gate],
        weights: vec![3, 9],
        skip: vec![None, None],
        lane_width: 8,
        tile_size: 8,
        node_count: 2,
        seed: 7,
    };
    let program = genome.program("lin", &tensor8(), &tensor8(), Level::L3);
    for stage in &program.stages {
        assert_eq!(stage.inputs.len(), 1, "linear stage must have one input");
    }
    check(&program).unwrap();
    let r = run(&program).unwrap();
    assert!(r.report.all_valid);
}
