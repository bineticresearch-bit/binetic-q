# Q3 — intent synthesis (evidence)

Status: DONE — evidence recorded. Owner: parent.

This is the third shipped gate after Q0 (compile) and Q1 (verified effects).
Q3 adds an honest front end: a **declarative intent** becomes a *verified*
`.q` program. The intent is a structured "what I want" (port types, demanded
receipt level, kernel pipeline, geometry policy) — not prose. An LLM adapter
may *produce* the intent; the synthesizer is a deterministic, provable
compiler. Nothing in Q3 claims to understand natural language.

Scope: `crates/binetic-q/src/synth.rs`, `crates/binetic-q/tests/q3.rs`,
`crates/binetic-q/examples/pipeline.intent`, `binetic-q synth` CLI.

## What it proves

- `parse_intent` turns intent source into a typed `Intent`; `build_program`
  expands it to a `QProgram`; `synthesize` returns a `Synthesis` that is
  **constrained to be verified**: the program passes Q1's effect check and
  Q2's real-fabric verification at the demanded level, or synthesis refuses.
  There is no path that returns a program that fails verification.
- Impossible demands are refused *before* execution: an L3 demand over a
  Custom kernel (capped at L1) is a `SynthError::Check`, naming the effect.
- `geometry optimize` runs the Q2 verified-throughput search and emits the
  winning geometry; the emitted program still verifies at the demand.
- `render_q` round-trips: the emitted `.q` source re-parses to an equivalent
  program and runs with the same plan hash and chain root.
- Deterministic: same intent source ⇒ same program, source, plan hash, chain
  root, receipts.

## API added

```rust
use binetic_q::{parse_intent, synthesize, build_program, render_q,
                Intent, Synthesis, SynthError, Geometry};

parse_intent(src) -> Result<Intent, SynthError>;
synthesize(src)   -> Result<Synthesis, SynthError>;  // verified or refused
build_program(&intent, config) -> QProgram;
render_q(&program) -> String;                        // canonical .q source
```

## Invariants

1. `synthesize` never returns a program whose receipts fail at the demand.
2. Default stage weights are a pure function of the intent (fixed seed),
   never of `config.seed` — probe and final programs agree even when a
   geometry search changes the seed.
3. `render_q` output re-parses to an equivalent program (all stage inputs
   rendered).
4. Synthesis is pure over (intent source, search seed): no clock, no thread
   state.

## Test evidence

Unit tests (`synth.rs`, all passing): `parses_intent`,
`render_round_trips_through_parser`, `synthesis_is_deterministic_and_verified`,
`refuses_impossible_effect`, `rejects_weight_count_mismatch`.

Integration tests (`tests/q3.rs`, all passing):

| Test | What it proves |
|---|---|
| `q3_intent_parses_and_synthesizes_a_verified_program` | intent → verified program; all levels true; one packet per stage. |
| `q3_generated_source_recompiles_and_verifies` | emitted `.q` re-parses, runs, same plan hash + chain root, L3 holds. |
| `q3_is_deterministic` | two syntheses ⇒ identical program/source/hashes. |
| `q3_refuses_an_impossible_receipt_demand` | Custom kernel under L3 demand is refused with an effect error. |
| `q3_geometry_optimize_is_verified` | `geometry optimize` emits a verified program at the demand. |

## Gate evidence

Recorded 2026-11-18 on macOS arm64:

| Gate | Command | Result |
|---|---|---|
| Unit tests | `cargo test --lib` | PASS — 30 passed, 0 failed |
| Q3 integration | `cargo test --test q3` | PASS — 5 passed, 0 failed |
| CLI synth | `binetic-q synth examples/pipeline.intent` | PASS — `plan_hash 05f128a1…404173`, `chain_root 501d9129…2bb0c9`, 3 packets, 996 receipt bytes, L0–L3 all true, verified source emitted |

## Known limitations

- Intents describe a single linear pipeline (one input per stage); multi-input
  fan-in graphs are the Q5 visual/REPL surface.
- The intent grammar is 1-D shapes (`dtype[n]`), matching the Q0 DSL.
- `geometry optimize` uses `OptimizeConfig::default()`; per-intent search
  budgets arrive with Q5.
