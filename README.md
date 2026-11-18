# Binetic Q

> The next intelligent programming language. Programs are graphs — and the
> compiler evolves them.

Binetic Q (Q for short) is a systems language for **intelligent, verified
compute**. A Q program is a graph of **Qblocks**: typed, verified transforms
connected by typed packet streams. Running a graph emits L0–L3 receipts from
the real fabric — and the graph itself is a search space: the compiler can
*evolve* kernel pipelines, stage order, weights, and execution geometry,
scoring every candidate only when its receipts verify at the level the
program demands.

Three properties make Q different from every language that came before it:

1. **Intelligence is a language feature, not a plugin.** `binetic-q evolve`
   does not tune parameters around a fixed program — it searches the program
   itself: which kernels, in what order, at what weights, on what execution
   geometry. Every candidate is compiled and executed on the real fabric and
   scores only if its receipts verify at the demanded level. The program you
   ship is the best verified graph the search found, with the receipts to
   prove it.
2. **Receipts are effects, not caveats.** `@receipt L3` is a compile-time
   contract. A pipeline that cannot be verified at the demanded level is
   rejected at `check` time, naming the stage that caps it. An impossible
   claim is a type error — in the compiler, in synthesis, and in evolution.
3. **Deterministic end to end.** No clock, no thread state, no environment
   reads participate in any plan hash, receipt, or search. Same seed + same
   source ⇒ same plan hash, same chain root, same evolved graph — byte for
   byte, on any machine.

The thesis:

> C = functions over bytes, linked by ABI.
> Q = evolvable graphs of verified transforms, linked by receipts.

## Status

`Q0` (static compiler) — **DONE**. `Q1` (verified effects) — **DONE**.
`Q2` (self-optimizing geometry) — **DONE**. `Q3` (intent → verified program)
— **DONE**. `Q4` (graph evolution) — **DONE**.

Evidence: `docs/q0-ir-compiler.md`, `docs/q1-verified-effects.md`,
`docs/q2-self-optimization.md`, `docs/q3-intent-synthesis.md`,
`docs/q4-graph-evolution.md`. Roadmap and design:
`docs/q-language-design.md`.

## Hello, blocks

```q
@receipt L3
qblock pipeline(x: i64[8]) -> i64[8] {
  config {
    model_id = 11;
    lane_width = 8;
    tile_size = 8;
    node_count = 2;
    seed = 7;
  }
  let h = dot(x, 3);
  let g = h |> dot(5);
  let y = g |> gate(9);
  emit y;
}
```

`h |> dot(5)` is pipeline sugar for `dot(h, 5)` — the piped value becomes the
first argument. Compile and run it:

```bash
cargo run -q --bin binetic-q -- check   crates/binetic-q/examples/three_blocks.q
cargo run -q --bin binetic-q -- compile crates/binetic-q/examples/three_blocks.q
cargo run -q --bin binetic-q -- run     crates/binetic-q/examples/three_blocks.q
```

`run` prints one JSON object with the `plan_hash`, `chain_root`, packet
count, and the L0/L1/L2/L3 verification flags.

## The compiler as search

```bash
# Q3 — declarative intent becomes a verified .q program
binetic-q synth examples/pipeline.intent

# Q4 — evolve the graph itself: kernels, order, weights, geometry
binetic-q evolve examples/pipeline.intent --gen 12 --pop 16

# Q2 — evolve execution geometry for a fixed .q program
binetic-q optimize examples/three_blocks.q --gen 10 --pop 8 --seed 5150

# Attest — the certificate: the entire claim in one deterministic artifact
binetic-q attest examples/three_blocks.q

# The fitness model is your choice:
binetic-q evolve examples/pipeline.intent --throughput fixed --tps 12
binetic-q optimize examples/three_blocks.q \
  --throughput lane --tps 30 --ref-lane 8 --accuracy 0.95 --tokens 32
```

`evolve` prints one JSON object: the evolved kernel pipeline, baseline vs.
best verified score, the winning graph's `plan_hash`/`chain_root`, and the
winning graph as re-parseable `.q` source. Elitism guarantees the search
never loses the best verified graph it has seen — `history` in the result is
monotone non-decreasing.

`attest` prints the certificate: per-stage replay evidence (each built-in
stage is semantically replayed — its committed output re-derived
independently from `(input, weight)`), the plan hash, chain root, and the
L0–L3 flags. Re-running on the same source must reproduce the hashes byte
for byte — determinism is the verification.

## Kernels earn their guarantees

The kernel registry is the single source of truth for kernel semantics and
receipt guarantees: a kernel registered with a **replay oracle** earns **L3**
(its output is re-derived independently on every run, and the run is refused
on mismatch); without one it holds **L1**. The built-ins ship with real
semantics — `dot`, `fwht` (fast Walsh–Hadamard), `gate` (certified ReLU) —
and any kernel you register (or that an LLM proposes) earns L3 the same way.

The score formula is canonical (owned by the substrate, not the caller):
`100·accuracy + ln(1 + verified_tps) − 0.01·receipt_bytes_per_token`. What
you choose is the *measurement* — any `ThroughputModel` (lane-scaled, fixed
rate, or a table of real per-geometry measurements).

## Types and geometry

- `i64`, `f32`, `u64`, `u128`, and `u256` ports are first-class. A fabric
  lane is 64 bits: `u64` is one lane, `u128` is two (little-endian words),
  `u256` is four. f32 lanes carry IEEE-754 bit patterns; signed `dot`
  accumulates saturating; unsigned `dot`/`fwht` are modular (mod 2^n);
  every dtype gets semantic replay at L3. Pipelines are dtype-homogeneous.
- Lane widths `1..=64` — the substrate's full range. The checker adds no cap
  of its own.
- The 23 `binetic-quant` dtypes arrive as annotations in Phase 2.

## What Q proves

A `.q` program lowers to a real `binetic_fabric::Fabric`, reproduces its
compile-time `plan_hash`, and emits receipts that `binetic_verify` accepts at
L0, L1, L2, and L3. This is the fabric substrate — not a simulation: real
`FabricPacket`s flow through a virtual-port namespace. Q4 extends the proof:
an *evolved* graph re-parses from its emitted source and runs with the same
plan hash and chain root the search recorded.

```bash
cargo test   # 37 unit + G0 + attest + Q1 + Q2 + Q3 + Q4 integration tests
```

## Dependency wiring

The substrate crates (`binetic-core`, `binetic-fabric`, `binetic-verify`,
`binetic-evolver`) are git dependencies pinned by `rev` to `binetic-engine`.
The pin
(`2e13a913bd00505998c63d7882f6d18e7b661bbe`) is the first substrate commit
that carries the L2/L3 replay and chain-linking fixes the receipt proofs
depend on, so a plain `cargo build` from a fresh clone reproduces the proofs
without any sibling checkout. Bump the `rev` in `Cargo.toml` when the
substrate advances, and re-run `cargo test` to confirm the proofs still
hold.

## License

Proprietary (see `LICENSE`). "Binetic Q", "Qblock", and the Q language design
are Binetic; the underlying fabric crates are separately licensed in their
home repository.
