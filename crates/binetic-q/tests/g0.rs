//! G0 acceptance: a three-Qblock `.q` program compiles to a real Fabric,
//! reproduces its plan hash, and emits receipts verified at L0–L3.

use binetic_q::{check, compile, parser::Parser, run};

const PROGRAM: &str = include_str!("../examples/three_blocks.q");

#[test]
fn g0_end_to_end() {
    let program = Parser::parse(PROGRAM).expect("parse");
    check(&program).expect("check");

    let (hash_a, cfg) = compile(&program).expect("compile");
    let (hash_b, _) = compile(&program).expect("compile");
    assert_eq!(hash_a, hash_b, "plan hash must be reproducible");
    assert_eq!(cfg.output_len, 8);
    assert_eq!(cfg.input_len, 8);

    let result = run(&program).expect("run");
    assert_eq!(result.packet_count, 3, "one receipt per Qblock stage");
    assert_eq!(result.output.len(), 8);
    assert_eq!(
        result.plan_hash, hash_a,
        "run and compile agree on plan hash"
    );

    let r = result.report;
    assert!(r.level0, "L0 per-packet integrity");
    assert!(r.level1, "L1 receipt-chain structure");
    assert!(r.level2, "L2 sampled replay");
    assert!(r.level3, "L3 full replay");
    assert!(r.all_valid);

    // Determinism: same source → same plan, chain root, and output.
    let again = run(&program).expect("run again");
    assert_eq!(result.plan_hash, again.plan_hash);
    assert_eq!(result.chain_root, again.chain_root);
    assert_eq!(result.output, again.output);
}
