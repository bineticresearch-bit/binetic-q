//! Static checker: type, lane, and receipt-effect validation.
//!
//! Checks are conservative and deterministic — no I/O, no clock. The checker
//! enforces Q1's receipt effects: a program demanding `Lk` is rejected when a
//! stage cannot guarantee it. A receipt claim is a compile-time type effect,
//! not a runtime surprise.

use crate::ir::{Level, ProgramConfig, QProgram};
use crate::kernels::{receipt_effect_with, KernelRegistry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    EmptyProgram,
    ShapeMismatch {
        param: usize,
        output: usize,
    },
    DtypeMismatch {
        param: String,
        output: String,
    },
    UnsupportedLaneWidth(u16),
    BadGeometry(String),
    UnknownName(String),
    DuplicateName(String),
    EmptyPipeline,
    ReceiptUnsatisfiable {
        demand: Level,
        provided: Level,
        culprit: String,
    },
    OutputNotProduced(String),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::EmptyProgram => write!(f, "program has no stages"),
            CheckError::ShapeMismatch { param, output } => {
                write!(f, "input length {param} != output length {output}")
            }
            CheckError::DtypeMismatch { param, output } => {
                write!(
                    f,
                    "dtype mismatch: param is {param} but output is {output}; pipelines are dtype-homogeneous"
                )
            }
            CheckError::UnsupportedLaneWidth(w) => write!(
                f,
                "unsupported lane width {w}; the fabric supports 1..=64"
            ),
            CheckError::BadGeometry(s) => write!(f, "bad geometry: {s}"),
            CheckError::UnknownName(s) => write!(f, "unknown name {s:?}"),
            CheckError::DuplicateName(s) => write!(f, "duplicate stage name {s:?}"),
            CheckError::EmptyPipeline => write!(f, "empty pipeline"),
            CheckError::ReceiptUnsatisfiable {
                demand,
                provided,
                culprit,
            } => write!(
                f,
                "receipt effect error: demand {demand} but stage {culprit:?} only guarantees {provided}"
            ),
            CheckError::OutputNotProduced(s) => {
                write!(f, "emit name {s:?} was not produced")
            }
        }
    }
}

impl std::error::Error for CheckError {}

/// Enforce type + geometry + receipt effects under the default kernel
/// registry. See [`check_with`] for the registry-aware form.
pub fn check(program: &QProgram) -> Result<(), CheckError> {
    check_with(program, &KernelRegistry::default())
}

/// Enforce type + geometry + receipt effects. A `@receipt Lk` demand above
/// the program's computed effect is a compile error: the stage that caps the
/// guarantee is named in the error so the caller knows which kernel to change.
/// Guarantees come from the registry — earned by replay-oracle registration,
/// never hardcoded here.
pub fn check_with(program: &QProgram, registry: &KernelRegistry) -> Result<(), CheckError> {
    if program.stages.is_empty() {
        return Err(CheckError::EmptyProgram);
    }

    let p_len = program.param_ty.len();
    let o_len = program.output_ty.len();
    if p_len != o_len {
        return Err(CheckError::ShapeMismatch {
            param: p_len,
            output: o_len,
        });
    }
    // Ports are typed streams: a pipeline is dtype-homogeneous (every stage
    // transforms the same port dtype). i64 and f32 are both first-class.
    if program.param_ty.dtype != program.output_ty.dtype {
        return Err(CheckError::DtypeMismatch {
            param: program.param_ty.dtype.to_string(),
            output: program.output_ty.dtype.to_string(),
        });
    }

    check_config(&program.config)?;

    let mut names: Vec<&str> = Vec::new();
    names.push(program.param.as_str());
    for stage in &program.stages {
        if names.contains(&stage.name.as_str()) {
            return Err(CheckError::DuplicateName(stage.name.clone()));
        }
        for input in &stage.inputs {
            if !names.contains(&input.as_str()) {
                return Err(CheckError::UnknownName(input.clone()));
            }
        }
        names.push(stage.name.as_str());
    }
    if !names.contains(&program.emit.as_str()) {
        return Err(CheckError::OutputNotProduced(program.emit.clone()));
    }

    // Receipt effect: the demand may not exceed the weakest stage's earned
    // guarantee.
    let provided = receipt_effect_with(program, registry);
    if program.demand > provided {
        let culprit = program
            .stages
            .iter()
            .find(|s| registry.guaranteed_level(&s.kernel) < program.demand)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "<none>".to_string());
        return Err(CheckError::ReceiptUnsatisfiable {
            demand: program.demand,
            provided,
            culprit,
        });
    }
    Ok(())
}

fn check_config(c: &ProgramConfig) -> Result<(), CheckError> {
    if c.tile_size == 0 {
        return Err(CheckError::BadGeometry("tile_size == 0".to_string()));
    }
    if c.node_count == 0 {
        return Err(CheckError::BadGeometry("node_count == 0".to_string()));
    }
    // Mirror the substrate exactly: any width in 1..=64 packs
    // (`binetic_core::is_supported_lane_width`), so the checker adds no cap
    // of its own.
    if !(1..=64).contains(&c.lane_width) {
        return Err(CheckError::UnsupportedLaneWidth(c.lane_width));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    const P: &str = r#"
@receipt L2
qblock p(x: i64[8]) -> i64[8] {
  config { model_id = 11; lane_width = 8; tile_size = 8; node_count = 2; seed = 7; }
  let h = dot(x, 1);
  let y = h |> gate(2);
  emit y;
}
"#;

    #[test]
    fn accepts_a_valid_program() {
        let p = Parser::parse(P).unwrap();
        check(&p).unwrap();
    }

    #[test]
    fn rejects_unknown_name() {
        let bad = P.replace("dot(x, 1)", "dot(nope, 1)");
        let p = Parser::parse(&bad).unwrap();
        assert_eq!(check(&p), Err(CheckError::UnknownName("nope".to_string())));
    }

    #[test]
    fn rejects_shape_mismatch() {
        let bad = P.replace("-> i64[8]", "-> i64[4]");
        let p = Parser::parse(&bad).unwrap();
        assert!(matches!(check(&p), Err(CheckError::ShapeMismatch { .. })));
    }

    #[test]
    fn accepts_f32_ports() {
        let p32 = P.replace("i64[8]", "f32[8]");
        let p = Parser::parse(&p32).unwrap();
        check(&p).unwrap();
    }

    #[test]
    fn rejects_mixed_dtypes() {
        let bad = P.replace("-> i64[8]", "-> f32[8]");
        let p = Parser::parse(&bad).unwrap();
        assert!(matches!(check(&p), Err(CheckError::DtypeMismatch { .. })));
    }

    #[test]
    fn accepts_any_lane_width_in_substrate_range() {
        // Widths outside the old {1,4,8,16,32,64} whitelist but inside the
        // substrate's 1..=64 must pass: the checker adds no cap of its own.
        for w in [1u16, 3, 5, 32, 64] {
            let src = P.replace("lane_width = 8;", &format!("lane_width = {w};"));
            let p = Parser::parse(&src).unwrap();
            check(&p).unwrap();
        }
    }

    #[test]
    fn rejects_lane_width_outside_substrate_range() {
        for w in [0u16, 65, 256] {
            let src = P.replace("lane_width = 8;", &format!("lane_width = {w};"));
            let p = Parser::parse(&src).unwrap();
            assert!(
                matches!(check(&p), Err(CheckError::UnsupportedLaneWidth(_))),
                "lane width {w} should be rejected"
            );
        }
    }
}
