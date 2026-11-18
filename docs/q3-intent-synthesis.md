# Q3 — intent synthesis (evidence)

Status: DONE — evidence recorded.
Owner: parent.

Q3 is the third gate of Binetic Q. Q0 proved a `.q` program lowers to a real
`binetic_fabric::Fabric` and emits L0–L3 receipts; Q1 made receipt demand a
compiler-enforced effect; Q2 evolved geometry against verified throughput.
Q3 closes the loop: a **declarative intent** is synthesized into a `.q` program
that is checked, compiled, run on the real fabric, and receipt-verified before
it is returned.

## What Q3 is (and is not)

Q3 is **structured intent → verified Qblocks**, not natural language → blocks.
No LLM, keyword matcher, or weight-fitting theater is involved. A small intent
language describes *what* is wanted (shape, depth, kernel class, receipt
demand, geometry policy); the deterministic synthesizer decides *how* (concrete
stages, deterministic weights, config), and refuses to emit a program it cannot
verify. An LLM/NL front-end that emits this structured intent is a future input
adapter and explicitly out of scope for this gate.

A synthesized result is **proof-carrying**: the returned `Synthesis` includes
the emitted `.q` source plus the plan hash, chain root, packet count, receipt
bytes, and the actual L0–L3 verification bits from a real `run()`.

## Intent language

```text
intent pipeline {
  input i64[8];
  output i64[8];
  demand L3;
  pipeline [dot, dot, gate];
  weights [3, 5, 9];
  geometry fixed;        // or: optimize (Q2 picks geometry)
}
```

Grammar (Q3 subset):

```text
intent        := "intent" IDENT "{" field* "}"
field         := input_field | output_field | demand_field
               | pipeline_field | weights_field | geometry_field
input_field   := "input" type ";"
output_field  := "output" type ";"
demand_field  := "demand" LEVEL ";"
pipeline_field:= "pipeline" "[" ident_list "]" ";"
weights_field := "weights" "[" int_list "]" ";"
geometry_field:= "geometry" ("fixed" | "optimize") ";"
type          := ("i64" | "f32") "[" INT "]"
```

The synthesizer reuses the existing `.q` lexer token set; no new lexer was
introduced. `geometry optimize` invokes Q2's `optimize()` and writes the winning
`lane_width`/`tile_size`/`node_count`/`seed` into the generated config.

## Effect-aware refusal

Synthesis routes every intent through `check()` (Q1) and `run()` (Q2) before
returning. An intent demanding `L3` whose pipeline contains a kernel with no
replay guarantee (an unregistered `Custom` kernel, capped at `L1`) is rejected
with `"rejected: ..."`. The synthesizer cannot produce a result whose receipt
claim exceeds the program's enforced effect — the guarantee is structural, not
a documentation promise.

## Public API

```rust
use binetic_q::{parse_intent, synthesize, Geometry, Intent, Level, Synthesis};

let intent: Intent = parse_intent(source)?;
let s: Synthesis = synthesize(source)?;
assert_eq!(intent.demand, Level::L3);
assert!(s.verified.iter().all(|&v| v)); // real L0–L3 bits from run()
// s.source is a first-class artifact: it re-parses, checks, and runs to the
// same plan_hash and chain_root.
```

- `parse_intent(&str) -> Result<Intent, SynthError>`
- `synthesize(&str) -> Result<Synthesis, SynthError>`
- `build_program(&Intent) -> ...` and `render_q(...)` are the deterministic
  lower/emit halves, exposed for tooling.

## CLI

```bash
binetic-q synth examples/pipeline.intent
```

Emits one JSON object with `intent`, `geometry`, `stages`, generated `source`,
`plan_hash`, `chain_root`, `packets`, `receipt_bytes`, `demand_level`, and
`all_levels_valid`.

## Test evidence

`tests/q3.rs` (5 tests) plus 5 inline `synth::tests`:

| Test | What it proves |
|---|---|
| `q3_intent_parses_and_synthesizes_a_verified_program` | A structured intent parses, synthesizes, and yields all-valid receipts with 3 packets. |
| `q3_generated_source_recompiles_and_verifies` | The emitted `.q` source re-parses, runs, and reproduces the same plan hash + chain root. |
| `q3_is_deterministic` | Same intent → byte-identical source, program, plan hash, chain root. |
| `q3_refuses_an_impossible_receipt_demand` | An L3 intent over an opaque kernel is refused. |
| `q3_geometry_optimize_is_verified` | `geometry optimize` produces a verified program through Q2. |
| `synth::tests::*` | Parser, weight/shape validation, render round-trip, effect refusal, deterministic verified synthesis. |

## Gate evidence

Recorded 2026-11-17 on macOS arm64, rustc 1.100.0:

| Gate | Command | Result |
|---|---|---|
| Full suite | `cargo test` | PASS — 20 unit + 1 G0 + 4 Q1 + 3 Q2 + 5 Q3, 0 failed |
| CLI synth | `binetic-q synth examples/pipeline.intent` | PASS — valid JSON, `all_levels_valid: true` |

## Known limitations

- **No natural-language front end.** Q3 consumes a structured intent; an LLM
  adapter that produces that intent is future work. This is by design, not a
  gap hidden behind a claim.
- **Deterministic weights.** Stage weights are pure functions of the intent's
  weight scalars and seed; synthesis does not fit weights to data.
- **`geometry optimize` reuses Q2's model-based throughput** (no wall-clock), so
  "optimized" means best on the canonical score, not fastest in silicon.

## Roadmap status

With Q3, the four staged Q gates are complete: Q0 (compiler + receipts), Q1
(verified effects), Q2 (self-optimization), Q3 (intent synthesis). The long-term
destination — intent expressed naturally and compiled by an intelligent
front-end — remains future work and is not claimed here.
