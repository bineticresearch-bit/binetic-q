//! Wide unsigned dtypes: u64 is one fabric lane; u128 is two (little-endian
//! 64-bit words); u256 is four. All run on the real fabric, verify L0–L3
//! with semantic replay, and are deterministic.

use binetic_q::{parser::Parser, run, DType};

const U64: &str = r#"
@receipt L3
qblock w64(x: u64[8]) -> u64[8] {
  config { model_id = 11; lane_width = 64; tile_size = 8; node_count = 2; seed = 7; }
  let h = dot(x, 3);
  let y = h |> gate(9);
  emit y;
}
"#;

const U128: &str = r#"
@receipt L3
qblock w128(x: u128[4]) -> u128[4] {
  config { model_id = 11; lane_width = 64; tile_size = 8; node_count = 2; seed = 7; }
  let h = dot(x, 3);
  let f = h |> fwht(1);
  let y = f |> gate(9);
  emit y;
}
"#;

const U256: &str = r#"
@receipt L3
qblock w256(x: u256[2]) -> u256[2] {
  config { model_id = 11; lane_width = 64; tile_size = 8; node_count = 2; seed = 7; }
  let h = dot(x, 3);
  let y = h |> gate(9);
  emit y;
}
"#;

fn lanes_of(src: &str) -> usize {
    let p = Parser::parse(src).expect("parse");
    p.param_ty.lane_len()
}

#[test]
fn wide_values_occupant_their_lanes() {
    assert_eq!(lanes_of(U64), 8, "u64[8] is 8 lanes");
    assert_eq!(lanes_of(U128), 8, "u128[4] is 4 elements × 2 words = 8 lanes");
    assert_eq!(lanes_of(U256), 8, "u256[2] is 2 elements × 4 words = 8 lanes");
    assert_eq!(DType::U64.words(), 1);
    assert_eq!(DType::U128.words(), 2);
    assert_eq!(DType::U256.words(), 4);
}

#[test]
fn u64_program_runs_and_verifies_l0_l3() {
    let p = Parser::parse(U64).expect("parse");
    let r = run(&p).expect("run");
    assert_eq!(r.packet_count, 2);
    assert_eq!(r.output.len(), 8);
    assert!(r.report.all_valid);
    assert!(r.report.level3);
    // Semantic replay held at every stage.
    assert!(r.stages.iter().all(|s| s.replayed));
}

#[test]
fn u128_program_runs_and_verifies_l0_l3() {
    let p = Parser::parse(U128).expect("parse");
    let r = run(&p).expect("run");
    assert_eq!(r.packet_count, 3);
    assert_eq!(r.output.len(), 8, "4 u128 elements × 2 words");
    assert!(r.report.all_valid);
    assert!(r.report.level3, "wide modular replay must hold");
    assert!(r.stages.iter().all(|s| s.replayed));
}

#[test]
fn u256_program_runs_and_verifies_l0_l3() {
    let p = Parser::parse(U256).expect("parse");
    let r = run(&p).expect("run");
    assert_eq!(r.packet_count, 2);
    assert_eq!(r.output.len(), 8, "2 u256 elements × 4 words");
    assert!(r.report.all_valid);
    assert!(r.report.level3, "256-bit modular replay must hold");
    assert!(r.stages.iter().all(|s| s.replayed));
}

#[test]
fn wide_runs_are_deterministic() {
    for src in [U64, U128, U256] {
        let p = Parser::parse(src).expect("parse");
        let a = run(&p).expect("run a");
        let b = run(&p).expect("run b");
        assert_eq!(a.plan_hash, b.plan_hash, "{src}");
        assert_eq!(a.chain_root, b.chain_root, "{src}");
        assert_eq!(a.output, b.output, "{src}");
    }
}

#[test]
fn wide_source_recompiles_to_the_same_identity() {
    // The .q source is the artifact: re-parsing and running must reproduce
    // the plan hash byte for byte.
    let p = Parser::parse(U256).expect("parse");
    let r = run(&p).expect("run");
    let p2 = Parser::parse(U256).expect("reparse");
    let r2 = run(&p2).expect("rerun");
    assert_eq!(r.plan_hash, r2.plan_hash);
    assert_eq!(r.chain_root, r2.chain_root);
}
