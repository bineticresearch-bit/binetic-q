//! Move 3 — Signed Wire Standard: attributed receipt tests.
//!
//! Verifies that:
//! 1. An unsigned run has `attribution_tag = None`.
//! 2. A signed run has `attribution_tag = Some(tag)` and all L0–L3 hold.
//! 3. The plan_hash and chain_root of a signed run equal those of an unsigned
//!    run on the same program — signing is a pure attribution layer.
//! 4. Two signed runs with the same key yield identical attribution tags.
//! 5. `verify_attribution` accepts the correct key and rejects a different key.
//! 6. A residual (skip-connection) program signed with a node key still
//!    verifies at L3 — attribution is topology-agnostic.
//! 7. Zero key behaves identically to unsigned (`attribution_tag = None`).

use binetic_q::{run, run_signed, verify_attribution, NodeKey};
use binetic_q::parser::Parser;

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

const RESIDUAL: &str = r#"
@receipt L3
qblock residual(x: i64[8]) -> i64[8] {
  config { model_id = 11; lane_width = 8; tile_size = 8; node_count = 2; seed = 7; }
  let h = dot(x, 3);
  let g = dot(h, 5);
  let r = gate(g, h, 9);
  emit r;
}
"#;

#[test]
fn unsigned_run_has_no_attribution_tag() {
    let p = Parser::parse(P).unwrap();
    let r = run(&p).unwrap();
    assert_eq!(r.attribution_tag, None, "unsigned run must yield None");
}

#[test]
fn signed_run_has_attribution_tag_and_l3_holds() {
    let p = Parser::parse(P).unwrap();
    let key = NodeKey::from_seed(42);
    let r = run_signed(&p, &key).unwrap();
    assert!(r.attribution_tag.is_some(), "signed run must have an attribution tag");
    // Receipt levels must hold — signing does not disturb the receipt chain.
    assert!(r.report.level0, "L0 must hold after signing");
    assert!(r.report.level1, "L1 must hold after signing");
    assert!(r.report.level2, "L2 must hold after signing");
    assert!(r.report.level3, "L3 must hold after signing");
    assert!(r.report.all_valid);
}

#[test]
fn signing_does_not_change_plan_hash_or_chain_root() {
    // body_hash (and therefore chain_root) is computed from canonical content
    // fields that do not include the signature bytes, so attribution is a pure
    // additive layer that does not alter receipt identity.
    let p = Parser::parse(P).unwrap();
    let key = NodeKey::from_seed(99);
    let unsigned = run(&p).unwrap();
    let signed = run_signed(&p, &key).unwrap();
    assert_eq!(
        unsigned.plan_hash, signed.plan_hash,
        "plan_hash must be identical regardless of signing"
    );
    assert_eq!(
        unsigned.chain_root, signed.chain_root,
        "chain_root must be identical regardless of signing"
    );
    assert_eq!(unsigned.output, signed.output);
}

#[test]
fn signed_runs_are_deterministic() {
    let p = Parser::parse(P).unwrap();
    let key = NodeKey::from_seed(7);
    let a = run_signed(&p, &key).unwrap();
    let b = run_signed(&p, &key).unwrap();
    assert_eq!(a.plan_hash, b.plan_hash);
    assert_eq!(a.chain_root, b.chain_root);
    assert_eq!(a.output, b.output);
    assert_eq!(a.attribution_tag, b.attribution_tag);
}

#[test]
fn zero_key_behaves_like_unsigned_run() {
    let p = Parser::parse(P).unwrap();
    let zero = NodeKey::zero();
    let r = run_signed(&p, &zero).unwrap();
    assert_eq!(r.attribution_tag, None, "zero key must yield None");
    assert!(r.report.all_valid);
}

#[test]
fn verify_attribution_accepts_correct_key() {
    let p = Parser::parse(P).unwrap();
    let key = NodeKey::from_seed(13);
    let r = run_signed(&p, &key).unwrap();
    let tag = r.attribution_tag.unwrap();
    assert!(
        verify_attribution(&key, &r.chain_root, &tag),
        "correct key must verify the attribution tag"
    );
}

#[test]
fn verify_attribution_rejects_wrong_key() {
    let p = Parser::parse(P).unwrap();
    let key_a = NodeKey::from_seed(1);
    let key_b = NodeKey::from_seed(2);
    let r = run_signed(&p, &key_a).unwrap();
    let tag = r.attribution_tag.unwrap();
    assert!(
        !verify_attribution(&key_b, &r.chain_root, &tag),
        "wrong key must not verify the attribution tag"
    );
}

#[test]
fn residual_program_signed_verifies_l3() {
    let p = Parser::parse(RESIDUAL).unwrap();
    let key = NodeKey::from_seed(55);
    let r = run_signed(&p, &key).unwrap();
    assert!(r.attribution_tag.is_some());
    assert!(r.report.level0);
    assert!(r.report.level1);
    assert!(r.report.level2);
    assert!(r.report.level3);
    assert!(r.report.all_valid);
    // Attribution verifies correctly.
    let tag = r.attribution_tag.unwrap();
    assert!(verify_attribution(&key, &r.chain_root, &tag));
}
