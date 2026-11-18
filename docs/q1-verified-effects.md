# Q1 — verified effects (evidence)

Status: DONE — evidence recorded. Owner: parent.

This is the second gate of Binetic Q. Q0 proved a `.q` program lowers to a real
`binetic_fabric::Fabric` and emits L0–L3 receipts. Q1 makes the receipt demand
a **compiler-enforced effect**: a program that cannot be verified at the level
it demands is rejected at check time, not discovered as a runtime caveat.

## Purpose

In Q0, `@receipt Lk` was accepted by the checker and the runner always attempted
full replay. That meant a program containing an opaque kernel could still be
marked L3 as long as the replay oracle happened to accept the fabric's generic
state transition — the receipt claim was not tied to what the kernel could
actually prove. Q1 closes that gap:

- Each kernel declares the strongest receipt level it can guarantee.
- A program's effect is the weakest guarantee across its pipeline stages.
- `check()` rejects any demand above the effect and names the capping stage.
- The runner verifies at exactly the achieved level, so reported levels never
  exceed the static guarantee.

This turns "we can't replay that kernel" from a whitepaper caveat into a type
error — the central Q thesis.

## Guarantee registry

| Kernel | Guarantee | Why |
|---|---|---|
| `dot`, `fwht`, `gate` | L3 | Deterministic built-ins with a registered replay oracle (`replay_oracle` re-derives `output_hash` and the state transition). |
| `Custom(name)` | L1 | No registered oracle. The fabric still provides per-packet integrity (L0) and chain structure (L1), but no replay proof. |

`receipt_effect(program)` folds `min` over stage guarantees (identity L3).

## API added

```rust
use binetic_q::{receipt_effect, CheckError, Kernel, Level};

Kernel::Dot.guaranteed_level();        // Level::L3
Kernel::Custom("x".into()).is_replayable(); // false

receipt_effect(&program); // weakest stage guarantee

// check() now returns:
CheckError::ReceiptUnsatisfiable { demand, provided, culprit }
```

The `check` CLI command reports both the demand and the computed effect:
`{ "demand": "L3", "effect": "L1", "ok": false }` becomes a check error.

## Invariants

1. `receipt_effect` is pure over the stage list; inserting a `Custom` stage can
   only lower the effect, never raise it.
2. `check()` succeeds only when `program.demand <= receipt_effect(program)`.
3. `run()` calls `check()` first, so an impossible demand never reaches fabric
   execution.
4. `run()` uses `verify_with_replay` only when `receipt_effect >= L2`; otherwise
   it uses plain `verify()`. Reported `level2/level3` therefore match the static
   effect — no overclaim.
5. Built-in pipelines keep the G0 guarantee: three `dot`/`gate` stages provide
   L3 and still verify L0–L3.

## Test evidence

`tests/q1.rs` (all passing):

| Test | What it proves |
|---|---|
| `unreplayable_kernel_cannot_satisfy_l3` | A `Custom` stage caps the effect at L1; `@receipt L3` fails check with `ReceiptUnsatisfiable { demand: L3, provided: L1, culprit: "y" }`. |
| `lowering_the_demand_to_l1_compiles_and_reports_honest_levels` | The same graph at `@receipt L1` passes; `run()` reports `level0/level1 = true`, `level2/level3 = false` — honest about the missing oracle. |
| `builtin_pipeline_provides_l3` | Replacing the opaque stage with `gate` restores the L3 effect and full L0–L3 verification (G0 preserved). |
| `kernel_guarantee_registry` | The built-in vs Custom guarantee split is pinned. |

Unit tests in `ir.rs` (`effect_tests`) additionally pin the registry and the
weakest-link fold.

## Gate evidence

Recorded on macOS arm64, rustc 1.100.0:

| Gate | Command | Result |
|---|---|---|
| Full suite | `cargo test` | PASS — 12 unit + 1 G0 + 4 Q1 tests, 0 failed |
| CLI effect reporting | `binetic-q check examples/three_blocks.q` | PASS — `"demand":"L3","effect":"L3","ok":true` |
| CLI rejection | `binetic-q check /tmp/q1-l3-custom.q` | PASS — exit 1, `receipt effect error: demand L3 but stage "y" only guarantees L1` |

## Commands to verify

```bash
cd crates/binetic-q
cargo test
cargo test --test q1 -- --nocapture
cargo run -q --bin binetic-q -- check examples/three_blocks.q
```

## Known limitations

- The guarantee registry is static: built-ins are L3, any unknown identifier is
  L1. Typed wire schemas and per-Qblock `min_receipt` annotations from the
  design doc are Q2/Q3 work; Q1 does not enforce shape/dtype at the wire.
- `Custom` currently still lowers to the fabric's `DotProductKernel`, so its
  packet bytes are real; Q1 deliberately refuses to *claim* replay for it until
  an oracle is registered. Registering a `Custom` oracle is the extension point.
- L1's structural guarantee assumes receipts are inserted in wire order on one
  route, matching the Part 03 adjacency rule.
