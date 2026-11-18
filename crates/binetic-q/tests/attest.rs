//! Attestation: the certificate is the entire claim in one deterministic
//! artifact. Re-running on the same source must reproduce the plan hash and
//! chain root byte for byte — determinism is the verification.

use binetic_q::{run, parser::Parser, KernelRegistry, KernelSpec, GateKernel};

const P: &str = include_str!("../examples/three_blocks.q");

#[test]
fn attest_is_reproducible() {
    let p = Parser::parse(P).expect("parse");
    let a = run(&p).expect("run a");
    let b = run(&p).expect("run b");
    assert_eq!(a.plan_hash, b.plan_hash);
    assert_eq!(a.chain_root, b.chain_root);
    assert_eq!(a.receipt_bytes, b.receipt_bytes);
    assert!(a.report.all_valid);
    // Per-stage replay evidence: every built-in stage is semantically replayed.
    assert!(a.stages.iter().all(|s| s.replayed));
    assert_eq!(a.stages.len(), 3);
}

#[test]
fn attest_records_honest_replay_evidence_for_l1_kernels() {
    // An unregistered Custom kernel runs (dot-product default compute) but
    // claims no replay: its stage receipt says replayed = false.
    let src = P.replace("gate", "mystery");
    let src = src.replace("@receipt L3", "@receipt L1");
    let p = Parser::parse(&src).expect("parse");
    let r = run(&p).expect("run");
    assert!(r.report.level0 && r.report.level1);
    assert!(!r.report.level2, "L1-capped program must not claim replay levels");
    assert!(
        r.stages.iter().any(|s| s.kernel == "mystery" && !s.replayed),
        "unregistered kernel must be recorded as not replayed"
    );
}

#[test]
fn registered_kernel_earns_replay_evidence() {
    // Register "mystery" with a replay oracle: the same source now runs at
    // L3 and the stage is recorded as replayed.
    let src = P.replace("gate", "mystery");
    let p = Parser::parse(&src).expect("parse");
    let mut registry = KernelRegistry::builtin();
    registry.register(
        "mystery",
        KernelSpec::new(Box::new(GateKernel), Some(Box::new(GateKernel))),
    );
    let r = binetic_q::run_with(&p, &registry).expect("run under registry");
    assert!(r.report.level3, "registered replay earns L3");
    assert!(
        r.stages
            .iter()
            .any(|s| s.kernel == "mystery" && s.replayed),
        "registered kernel must be recorded as replayed"
    );
}
