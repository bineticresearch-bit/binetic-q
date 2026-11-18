# Q4 — graph evolution (evidence)

Status: DONE — evidence recorded. Owner: parent.

This is the fourth shipped gate. Q2 evolved *geometry* — how to execute a
fixed QGraph. Q4 makes the **program graph itself the search space**: kernel
choice per stage, stage weights, pipeline length and order, and execution
geometry. The only fixed points are the contract the intent sets: port types
and the receipt demand.

This is the language's central claim made executable: intelligence is a
language feature. The compiler does not just compile a graph — it searches
graphs, and it can only return a graph the fabric has already proven.

Scope: `crates/binetic-q/src/evolve.rs`, `crates/binetic-q/tests/q4.rs`,
`binetic-q evolve` CLI.

## What it proves

- `evolve` searches `StructureGenome`s (kernels + weights + geometry) with a
  deterministic elitist population search (SplitMix64 via
  `binetic_evolver::DeterministicRng`; no clock, no thread state).
- Every candidate is compiled and executed on the **real fabric** and scored
  only when its receipts verify at the demanded level — an impossible
  candidate (e.g. Custom stage under an L3 demand) scores at the invalid
  floor, exactly like Q2's invalid genomes. Evolution can never emit a
  program that overclaims its receipts.
- Mutation operators are graph operators: retarget a kernel, rescale a
  weight, insert a stage, delete a stage, **swap two stages** (order is
  semantics), or mutate one geometry field.
- Elitism: the best genome is never discarded, so the per-generation best
  score history is monotone non-decreasing and the final result is at least
  the author's baseline.
- The winning graph ships as a first-class artifact: re-parseable `.q`
  source whose plan hash and chain root match the search's recorded evidence.
- The fitness model is caller-chosen (any `ThroughputModel`); the score
  formula is the substrate's canonical
  `100·accuracy + ln(1+tps) − 0.01·bytes_per_token`.

## API added

```rust
use binetic_q::{evolve, EvolveConfig, EvolveResult, GraphFitnessFn,
                StructureGenome};

evolve(name, param_ty, output_ty, demand, initial, throughput, config)
    -> EvolveResult;   // deterministic; verified or invalid-floored

StructureGenome { kernels, weights, lane_width, tile_size, node_count, seed }
EvolveConfig    { seed, population, generations, max_stages,
                  model_accuracy, tokens }
```

CLI:

```bash
binetic-q evolve examples/pipeline.intent --gen 12 --pop 16 --max-stages 4
binetic-q evolve examples/pipeline.intent --throughput fixed --tps 12
```

## Invariants

1. `evolve` is pure over (contract, initial graph, throughput model, config):
   same inputs ⇒ same `StructureGenome`, same plan hash, same chain root,
   same history.
2. The final graph verifies at the demand (or the result carries zeroed
   evidence and an invalid-floored fitness — never a silent pass).
3. `history` is monotone non-decreasing (elitism).
4. The emitted source re-parses to the evolved program and runs with the
   same plan hash and chain root.
5. Search never loses the baseline: `best_fitness.score >=
   baseline_fitness.score`.

## Test evidence

Unit tests (`evolve.rs`, all passing): `evolution_is_deterministic`,
`evolved_graph_verifies_at_demand`, `history_is_monotone_under_elitism`,
`mutation_is_deterministic_and_bounded`.

Integration tests (`tests/q4.rs`, all passing):

| Test | What it proves |
|---|---|
| `q4_evolution_returns_a_verified_graph` | evolved graph verifies at every level; real receipt bytes; monotone history. |
| `q4_evolved_source_recompiles_with_the_same_identity` | emitted `.q` re-parses and runs with the search's recorded plan hash + chain root; L3 holds. |
| `q4_is_deterministic` | two searches ⇒ identical genome, hashes, history. |
| `q4_search_never_loses_the_baseline` | final verified score ≥ baseline; final graph verifies at demand. |

## Gate evidence

Recorded 2026-11-18 on macOS arm64:

| Gate | Command | Result |
|---|---|---|
| Unit tests | `cargo test --lib` | PASS — 30 passed, 0 failed |
| Q4 integration | `cargo test --test q4` | PASS — 4 passed, 0 failed |
| Full suite | `cargo test --all-targets` | PASS — 30 unit + G0 + Q1 + Q2 + Q3 + Q4, 0 failed |
| CLI evolve | `binetic-q evolve examples/pipeline.intent --gen 6 --pop 8` | PASS — baseline 98.1227 → best **99.0046** (`improved: true`); the search returned a *different* graph (1-stage `dot`, lane_width 16, tile 13, node_count 8) with `plan_hash b2077cfe…73d`, `chain_root 1c569d28…5e27`, 1 packet, 340 receipt bytes, L0–L3 all true, and re-parseable source |

## Known limitations

- The default search evolves over the registry's kernel set (built-ins plus
  anything the caller registered); a caller can also supply a custom
  `MutationFn` to bias search with their own operators.
- Pipelines stay linear (one input per stage); fan-in/fan-out graph topology
  is the Q5 surface.
- Throughput is a pure model unless the caller supplies a measured
  `ThroughputModel`; no wall clock is read by the search.
