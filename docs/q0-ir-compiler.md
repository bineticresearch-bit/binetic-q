# Q0 — IR and compiler (evidence)

Status: DONE — evidence recorded. Owner: parent.

Q0 is the first shipped gate of Binetic Q. It establishes that a `.q` program
compiles to the real fabric substrate, reproduces its plan identity, and emits
verifiable receipts. Cross-cutting validation of Parts 02/03; not a new
numbered binetic part.

Scope: `crates/binetic-q/**`.

## What it proves

- A three-Qblock `.q` program lowers to a real `binetic_fabric::Fabric` and
  executes through `Fabric::execute` — real `FabricPacket`s on a virtual-port
  namespace (`model_id << 32`).
- The compile-time `plan_hash` is reproducible and agrees with the run-time
  identity (the cached `plan.plan_hash`, not a post-execution recomputation,
  because `compute_plan_hash` folds mutable port state).
- Receipts from raw fabric output verify at **L0** (per-packet integrity),
  **L1** (chain structure), **L2** (sampled replay), and **L3** (full replay),
  via a replay oracle that re-derives the output hash and state transition.
- Pipeline sugar (`h |> dot(5)`) desugars to `dot(h, 5)`.

## Test evidence

Unit tests (`cargo test --lib`, 10 passing):

| Test | What it proves |
|---|---|
| `lexer::tokenizes_a_pipeline_program` | `.q` text → tokens including `\|>` and `->`. |
| `lexer::rejects_unexpected_character` | bad bytes are diagnosed. |
| `parser::parses_pipeline_sugar` | program → IR; `\|>` yields first input; config parsed. |
| `parser::rejects_unknown_config_field` | bad config keys are rejected. |
| `check::accepts_a_valid_program` | a valid program type-checks. |
| `check::rejects_unknown_name` | undefined stage inputs are caught. |
| `check::rejects_shape_mismatch` | input/output length mismatch is caught. |
| `lower::compiles_to_a_reproducible_plan_hash` | `compile()` is deterministic; config geometry correct. |
| `lower::run_emits_l0_l1_l2_l3_receipts` | 3 stages → 3 packets; all four levels pass. |
| `lower::plan_hash_is_deterministic_across_runs` | same source → same plan hash, chain root, output. |

Integration test (`tests/g0.rs`, 1 passing):

| Test | What it proves |
|---|---|
| `g0_end_to_end` | parse → check → compile → run on `examples/three_blocks.q`; reproducible plan hash; 3 packets; L0–L3 all true; determinism. |

## Gate evidence

Recorded 2026-11-17 on macOS arm64, rustc 1.100.0:

| Gate | Command | Result |
|---|---|---|
| Unit tests | `cargo test --lib` | PASS — 10 passed, 0 failed |
| G0 integration | `cargo test --test g0` | PASS — 1 passed, 0 failed |
| CLI run | `cargo run -q --bin binetic-q -- run crates/binetic-q/examples/three_blocks.q` | PASS — `{"level0":true,...,"all_valid":true}` |

## Known limitations

- Q0 supports a single top-level qblock, `i64` ports only, equal input/output
  length, and lane widths {1,4,8,16,32,64}. Multi-block graphs are Q1+.
- The three built-in kernels (`dot`/`fwht`/`gate`) all lower to the fabric's
  `DotProductKernel` with deterministic weights. This proves the
  compiler→receipt path, not kernel semantics.
- `@receipt L2`/`L3` are satisfied at run time by a replay oracle; Q0's checker
  does not yet reject impossible demands statically — that is Q1.
- Substrate deps are path dependencies for development; release pinning is
  pending.
