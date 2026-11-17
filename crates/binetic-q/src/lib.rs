//! Binetic Q — a C-like verified block language over the Binetic fabric.
//!
//! The one abstraction: a **typed packet stream carrying a receipt**. A Q
//! program is a graph of Qblocks; a Qblock is one typed transform; a wire is
//! a typed stream. Running the graph emits L0–L3 receipts from the fabric.
//!
//! "C = functions over bytes linked by ABI; Q = Qblocks over typed streams
//! linked by receipts." — internal north star. Public framing: a systems
//! language for verified compute.
//!
//! Modules:
//! - [`ir`] — Qblock IR: [`Qblock`], [`QGraph`], ports, receipt levels.
//! - [`lexer`] / [`parser`] — `.q` source → IR.
//! - [`check`] — type + lane + receipt-effect checking.
//! - [`lower`] — IR → `PlanConfig` → real `Fabric`; receipts via `binetic-verify`.

pub mod check;
pub mod ir;
pub mod lexer;
pub mod lower;
pub mod parser;

pub use check::{check, CheckError};
pub use ir::{DType, Kernel, Level, Qblock, QGraph, QProgram, Stage, TensorType};
pub use lower::{compile, run, CompileError, RunResult};

/// Public framing tagline (internal north star is "future of C").
pub const TAGLINE: &str = "quantized fabric compute";
