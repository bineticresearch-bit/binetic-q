//! Q3 acceptance: a declarative intent is synthesized into a verified `.q`
//! program. The synthesizer is constrained — it can only return a program that
//! passes Q1's effect check and Q2's real-fabric verification at the demand.

use binetic_q::{parse_intent, run, synthesize, Geometry, Level};

const INTENT: &str = include_str!("../examples/pipeline.intent");

#[test]
fn q3_intent_parses_and_synthesizes_a_verified_program() {
    let intent = parse_intent(INTENT).expect("parse intent");
    assert_eq!(intent.demand, Level::L3);
    assert_eq!(intent.geometry, Geometry::Fixed);

    let s = synthesize(INTENT).expect("synthesize");
    assert!(s.verified.iter().all(|&v| v));
    assert_eq!(s.packet_count, 3);
}

#[test]
fn q3_generated_source_recompiles_and_verifies() {
    let s = synthesize(INTENT).expect("synthesize");
    // The emitted .q source is a first-class artifact: it re-parses, checks,
    // and runs with the same plan hash and receipts.
    let p = binetic_q::parser::Parser::parse(&s.source).expect("reparse generated source");
    let r = run(&p).expect("run generated program");
    assert_eq!(r.plan_hash, s.plan_hash);
    assert_eq!(r.chain_root, s.chain_root);
    assert!(r.report.all_valid);
    assert!(r.report.level3);
}

#[test]
fn q3_is_deterministic() {
    let a = synthesize(INTENT).expect("synthesize a");
    let b = synthesize(INTENT).expect("synthesize b");
    assert_eq!(a.source, b.source);
    assert_eq!(a.program, b.program);
    assert_eq!(a.plan_hash, b.plan_hash);
    assert_eq!(a.chain_root, b.chain_root);
}

#[test]
fn q3_refuses_an_impossible_receipt_demand() {
    // A custom kernel caps the effect at L1; demanding L3 must be refused.
    let bad = INTENT.replace("gate];", "mystery];");
    let err = synthesize(&bad).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("rejected") || msg.contains("refused") || msg.contains("effect"),
        "{msg}"
    );
}

#[test]
fn q3_geometry_optimize_is_verified() {
    let opt = INTENT.replace("geometry fixed;", "geometry optimize;");
    let s = synthesize(&opt).expect("synthesize with optimized geometry");
    assert_eq!(s.geometry, Geometry::Optimize);
    assert!(s.verified.iter().all(|&v| v));
    assert_eq!(s.packet_count, 3);
}
