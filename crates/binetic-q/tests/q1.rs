//! Q1 acceptance: receipts are a compiler-enforced effect.
//!
//! A demand above the weakest stage's guarantee is a CHECK-TIME ERROR, not a
//! runtime caveat. When the demand is lowered to what the graph can provide,
//! the runner reports only the levels it actually achieved.

use binetic_q::{check, parser::Parser, receipt_effect, run, CheckError, Kernel, Level};

const L3_CUSTOM: &str = r#"
@receipt L3
qblock pipeline(x: i64[8]) -> i64[8] {
  config { model_id = 11; lane_width = 8; tile_size = 8; node_count = 2; seed = 7; }
  let h = dot(x, 3);
  let y = h |> mystery(9);
  emit y;
}
"#;

#[test]
fn unreplayable_kernel_cannot_satisfy_l3() {
    let p = Parser::parse(L3_CUSTOM).expect("parse");
    assert_eq!(
        receipt_effect(&p),
        Level::L1,
        "Custom stage caps the effect"
    );

    let err = check(&p).expect_err("L3 demand must be rejected");
    match err {
        CheckError::ReceiptUnsatisfiable {
            demand,
            provided,
            culprit,
        } => {
            assert_eq!(demand, Level::L3);
            assert_eq!(provided, Level::L1);
            assert_eq!(culprit, "y", "the opaque stage is named in the error");
        }
        other => panic!("wrong error: {other:?}"),
    }
}

#[test]
fn lowering_the_demand_to_l1_compiles_and_reports_honest_levels() {
    let src = L3_CUSTOM.replace("@receipt L3", "@receipt L1");
    let p = Parser::parse(&src).expect("parse");
    check(&p).expect("L1 is within the effect");

    let result = run(&p).expect("run");
    assert_eq!(result.packet_count, 2);
    let r = result.report;
    assert!(r.level0, "fabric integrity still holds");
    assert!(r.level1, "fabric chain still holds");
    assert!(!r.level2, "no replay oracle for a Custom stage");
    assert!(!r.level3, "no replay oracle for a Custom stage");
}

#[test]
fn builtin_pipeline_provides_l3() {
    let src = L3_CUSTOM.replace("mystery(9)", "gate(9)");
    let p = Parser::parse(&src).expect("parse");
    assert_eq!(receipt_effect(&p), Level::L3);
    check(&p).expect("builtins satisfy L3");

    let result = run(&p).expect("run");
    let r = result.report;
    assert!(r.level0 && r.level1 && r.level2 && r.level3);
    assert!(r.all_valid);
}

#[test]
fn kernel_guarantee_registry() {
    assert_eq!(Kernel::Dot.guaranteed_level(), Level::L3);
    assert_eq!(Kernel::Fwht.guaranteed_level(), Level::L3);
    assert_eq!(Kernel::Gate.guaranteed_level(), Level::L3);
    assert_eq!(Kernel::Custom("x".into()).guaranteed_level(), Level::L1);
}
