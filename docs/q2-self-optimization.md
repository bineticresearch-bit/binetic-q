# Q2 — self-optimization (evidence)

Status: DONE — evidence recorded.
Owner: parent.

Q2 is the first implemented intelligence step in Binetic Q. Q0 proved a `.q`
program lowers to a real `binetic_fabric::Fabric` and emits L0–L3 receipts; Q1
made receipt demand a compiler-enforced effect. Q2 closes the loop: a
**QGraph-aware fitness function** runs the program on a real fabric for a
candidate geometry, scores it only when receipts verify at the demanded level,
and uses `binetic-evolver` to search geometry for a higher verified score.

## What is being optimized

A `Genome` is execution geometry, not a different program:

| Field | Range | Meaning |
|---|---|---|
| `lane_width` | {1, 4, 8, 16} | SWAR bit-lane width |
| `tile_size` | 1..=64 | output rows per vport (affects packet count) |
| `node_count` | 1..=8 | nodes in the plan |
| `seed` | any u64 | plan/state derivation seed |

`QFitnessFn::apply_genome` overlays these onto the Q program's `ProgramConfig`.
The pipeline, operators, and `@receipt` demand are unchanged — the search is
over HOW to execute the same QGraph, not over a different QGraph.

## The verified-throughput score

For each genome, `QFitnessFn::evaluate`:

1. Reject `!genome.is_valid()` → `Fitness::from_measurements(0, 0, 1e9)`.
2. Clone the template, apply geometry, `check()` and `run()` it on a real
   `Fabric`. Any error → the same invalid floor.
3. Require `report.all_valid` **and** the report to achieve the program's
   demanded level (L0/L1/L2/L3). Failure → invalid floor.
4. Otherwise:
   - `accuracy = model_accuracy` (an input to the search, not a claim),
   - `verified_tokens_per_second = throughput(genome)` (a pure model, no clock),
   - `receipt_bytes_per_token = actual serialized receipt bytes / tokens`.

The score is the substrate's canonical formula
(`binetic_evolver::compute_score`):

```
score = 100 * accuracy + ln(1 + verified_tps) - 0.01 * receipt_bytes_per_token
```

Two properties matter. First, throughput is **gated on receipts**: a genome
that would score well on the model alone is floored if its receipts fail.
Second, throughput is a deterministic function of the genome — no wall clock —
so the entire evolution is reproducible. The throughput model used in the CLI
and acceptance tests is `LaneScaledThroughput { base: 30.0, ref_lane: 8 }`.

This is honest about what it measures: it is a **model** of throughput under a
receipt-validity gate, not a wall-clock benchmark. The receipts are real; the
tokens/sec number is a model (see Known limitations).

## Public API

```rust
use binetic_evolver::{FitnessFn, Genome, LaneScaledThroughput};
use binetic_q::{optimize, OptimizeConfig, QFitnessFn};

// Single-point evaluation, with optional run evidence.
let qfit = QFitnessFn::new(program.clone(), throughput, 0.95, 32);
let fitness = qfit.evaluate(&Genome::new(8, 8, 2, 7));
let (fitness, maybe_run) = qfit.evaluate_detailed(&Genome::new(8, 8, 2, 7));

// Search: evolve geometry from the source config.
let result = optimize(&program, throughput, OptimizeConfig {
    seed: 5150, population: 8, generations: 10,
    model_accuracy: 0.95, tokens: 32,
});
assert!(result.improved());
assert!(result.verified_levels.iter().all(|&v| v));
```

`OptimizeResult` carries `best_genome`, `best_fitness`, `baseline_fitness`,
`history`, `plan_hash`, `chain_root`, `receipt_bytes`, `packet_count`, and
`verified_levels` — enough to record the winning genome and re-run it.

`binetic-evolver` is a pinned git dependency at the same substrate rev as the
other fabric crates (`2e13a913bd00505998c63d7882f6d18e7b661bbe`).

## CLI

```bash
binetic-q optimize <file.q> [--gen 10] [--pop 8] [--seed 5150]
```

Prints one JSON object: best genome, baseline vs best score, improvement flag,
plan hash, chain root, receipt bytes, and whether all levels verified.

## Test evidence

| Test | What it proves |
|---|---|
| `fitness::tests::fitness_is_deterministic_for_a_genome` | Same genome → identical `Fitness`; a verified run scores above the invalid floor. |
| `fitness::tests::invalid_genome_is_penalized` | Unsupported lane width → tps 0, receipt bytes 1e9, score below a valid genome. |
| `fitness::tests::detail_returns_real_receipt_evidence` | A valid genome returns 3 real receipt packets, >0 receipt bytes, L0–L3 all true. |
| `tests/q2.rs::q2_optimize_is_deterministic_and_receipt_verified` | Two identical searches give the same genome, score, plan hash, chain root, and history; best genome verifies L0–L3. |
| `tests/q2.rs::q2_optimization_beats_the_source_config` | Evolution improves on the hand-written source config's score. |
| `tests/q2.rs::q2_fitness_gates_on_receipts_not_just_the_model` | The program's effect is L3; an invalid genome is floored regardless of the model; a real run verifies. |

The Q0 (1), Q1 (4), and 15 unit suites remain green alongside Q2's 3 tests.

## Gate evidence

Recorded 2026-11-17 on macOS arm64, rustc 1.100.0, against the pinned substrate:

| Gate | Command | Result |
|---|---|---|
| Full crate suite | `cargo test` | PASS — 15 unit + 1 G0 + 4 Q1 + 3 Q2 tests, 0 failed |
| Optimize CLI | `binetic-q optimize examples/three_blocks.q --gen 10 --pop 8 --seed 5150` | PASS — improved, all levels valid |
| Determinism | two identical CLI runs | PASS — byte-identical JSON (same best genome, plan hash, chain root, scores) |

Recorded CLI output (both runs identical):

```json
{"program":"pipeline","generations":10,"population":8,"seed":5150,
 "best_lane_width":16,"best_tile_size":26,"best_node_count":4,
 "best_genome_seed":17775193301025956299,
 "baseline_score":98.12273720448515,"best_score":98.79212386417332,
 "improved":true,
 "plan_hash":"fba2893c44eef44d37ae1caa30b724c7b366e445663da5427f11ffd288383a3d",
 "chain_root":"a445f7dda6e13886a995de73a920436f33e4118eb27ec3caef6bcf4fbc260cb3",
 "receipt_bytes":1020,"all_levels_valid":true}
```

The search moved `lane_width` 8 → 16 and improved the canonical score from
98.12 to 98.79, while the winning genome's receipts verify at L0/L1/L2/L3.

## Invariants

- Fitness is a pure function of (template program, genome, config). No clock,
  no randomness outside the evolver's seeded `DeterministicRng`.
- A genome that fails validity, check, run, or the demanded receipt level
  always receives the invalid floor — never a model-only score.
- `apply_genome` changes only geometry fields; the program's operators and
  demand are fixed, so scores across genomes are comparable.
- `Evolver` recomputes `score` with the canonical formula on every evaluation
  (`with_canonical_score`), so `QFitnessFn`'s returned score field is
  authoritative only through the canonical ranking.

## Known limitations

- **Throughput is modelled, not wall-clock measured.** The tps term is
  `LaneScaledThroughput`, a linear function of `lane_width`. Q2 proves the
  search loop and the receipt gate, not a speedup claim. A measured-throughput
  model (e.g. from hardware traces) can be swapped in via the
  `ThroughputModel` trait without changing the evolver.
- **Geometry only.** The search does not mutate operators or pipeline shape;
  that belongs to a later gate (Q3 intent synthesis) and to the substrate's
  separate evolver work.
- **Validity gate is binary.** A run either verifies at the demanded level or
  is floored; partial credit for degraded receipts is not modelled.
- **Scores are comparable only within one config.** `model_accuracy`, `tokens`,
  and the throughput model are inputs; changing them changes the scale.

## Commands to verify

```bash
cd binetic-q
cargo test
cargo run -q --bin binetic-q -- optimize crates/binetic-q/examples/three_blocks.q \
  --gen 10 --pop 8 --seed 5150
```
