//! Binetic Q — an intelligent, verified programming language over the
//! Binetic fabric.
//!
//! The one abstraction: a **typed packet stream carrying a receipt**. A Q
//! program is a graph of Qblocks; a Qblock is one typed transform; a wire is
//! a typed stream. Running the graph emits L0–L3 receipts from the real
//! fabric — and the graph itself is evolvable: the compiler searches
//! kernel pipelines, stage order, weights, and execution geometry, scoring
//! every candidate only when its receipts verify at the demanded level.
//!
//! "C = functions over bytes linked by ABI; Q = evolvable graphs of verified
//! transforms linked by receipts." — internal north star. Public framing:
//! the next intelligent programming language for verified compute.
//!
//! Modules:
//! - [`ir`] — Qblock IR: [`Qblock`], [`QGraph`], ports, receipt levels.
//! - [`lexer`] / [`parser`] — `.q` source → IR.
//! - [`check`] — type + lane + receipt-effect checking.
//! - [`kernels`] — the kernel registry: guarantees are earned by
//!   replay-oracle registration, never assigned.
//! - [`lower`] — IR → `PlanConfig` → real `Fabric`; receipts via `binetic-verify`.
//! - [`fitness`] — Q2: verified-throughput fitness over execution geometry.
//! - [`synth`] — Q3: declarative intent → verified `.q` program.
//! - [`evolve`] — Q4: the program graph itself is the search space.

pub mod check;
pub mod evolve;
pub mod fitness;
pub mod ir;
pub mod kernels;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod synth;

pub use binetic_evolver::Fitness;
pub use check::{check, check_with, CheckError};
pub use evolve::{
    evolve, evolve_with, DefaultMutation, EvolveConfig, EvolveResult, GraphFitnessFn, MutationFn,
    StructureGenome,
};
pub use fitness::{default_throughput, optimize, OptimizeConfig, OptimizeResult, QFitnessFn};
pub use ir::{receipt_effect, DType, Kernel, Level, QGraph, QProgram, Qblock, Stage, TensorType};
pub use kernels::{
    receipt_effect_with, DotKernel, FabricKernel, FwhtKernel, GateKernel, KernelRegistry,
    KernelSpec, QKernel, ReplayOracle,
};
pub use lower::{compile, run, run_with, CompileError, RunError, RunResult, StageReceipt};
pub use synth::{
    build_program, parse_intent, render_q, synthesize, Geometry, Intent, SynthError, Synthesis,
};

/// Public framing tagline (internal north star is "future of C").
pub const TAGLINE: &str = "quantized fabric compute";
