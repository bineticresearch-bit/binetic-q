# Binetic Q — language design

Status: DESIGN/IMPLEMENTATION (Q0–Q4 shipped). Owner: parent.

## The vision

Q is the next intelligent programming language. Not a language *with* an
optimizer attached — a language whose core object is an **evolvable, verified
graph**, and whose compiler is a search that can only return programs the
fabric has already proven.

- A program is a graph of typed, verified transforms.
- A Qblock is one transform. A wire is a typed packet stream.
- Running the graph emits receipts for free.
- The graph itself — kernels, order, weights, length, geometry — is the
  search space. Intelligence is a language feature.

Q is not a new compute model. It is the front end over the fabric already
built in binetic-core: deterministic execution, canonical hashing, port
routes, plan hashes, and L0–L3 receipt verification. Inference, reservoir
computation, P4 pipelines, and dataflow all reduce to transforming typed
packet streams with receipts — and every one of them becomes an evolvable
graph.

Why "Q": quantized (bit-lane/codec substrate), queued (packet streams),
quality (receipt confidence), quanta (future bit-lane silicon).

## Abstraction stack

- **L0** bit-lanes, v4 `FabricPacket`, receipts, hashes — exists (Parts 01/03).
- **L1** kernels: DotProduct, FWHT, codecs, certified gate — exists/partial.
- **L2** `Qblock` = typed kernel + ports + receipt contract — Q0 seam.
- **L3** composition: graph / DSL → `PlanConfig` compiler — Q0.
- **L4** schedule, residency, failover, replay — partial (Part 02).
- **L5** intent synthesis, graph evolution, REPL, visual editor, inspector —
  Q3/Q4 shipped; REPL/editor next.
- **L6** apps: inference, evolver, dataflow — emergent.

The leverage point is L2: if a Qblock has a stable manifest and contract, it
works identically in CLI, browser, and node because all consume the same IR —
and the same IR is what the evolver searches over.

## Receipts as effects

A `@receipt Lk` demand is a compile-time contract. A result that cannot be
verified at the demanded level is an error, not a caveat. Q1 enforces this as
a hard type effect: each kernel declares its guaranteed receipt level
(built-ins `dot`/`fwht`/`gate` → L3; an unregistered `Custom` kernel → L1),
the program's effect is the weakest stage in its pipeline, and `check()`
rejects any demand above that effect — naming the stage that caps it. The
runner then verifies at exactly the level the effect guarantees, so a
Custom-capped program reports `level2/level3 = false` rather than claiming a
replay it did not perform. This invariant is inherited by Q3 (synthesis
refuses unverifiable intents) and Q4 (evolution scores unverifiable graphs at
the invalid floor). Evidence: `docs/q1-verified-effects.md`.

## Kernel registry: guarantees are earned, not assigned

The kernel registry (`binetic_q::KernelRegistry`) is the single source of
truth for what a kernel *is* and what it *guarantees*. Each registered
kernel pairs a **compute** (its real semantics on fabric lanes) with an
optional **replay oracle** (an independent re-derivation of the committed
output from `(input, weight)`, lane-quantization aware):

- replay oracle registered → **L3** (full semantic replay: the runner
  re-derives every stage output and refuses to run on mismatch)
- no replay oracle → **L1** (packet integrity + chain structure, no replay
  claim)

The built-ins ship with real semantics — `dot` (integer/float dot product),
`fwht` (fast Walsh–Hadamard transform, self-inverse up to scale), `gate`
(certified ReLU) — each with its own replay oracle. The Q0-era aliasing of
all three onto `DotProductKernel` is gone: kernels mean what they say.

Any kernel — yours, or one an LLM proposed — can `register` and **earn** L3
by supplying a replay that re-derives its output. The checker, runner, and
evolver all consult the same registry, so an earned guarantee flows
through `check` (effect), `run` (per-stage semantic replay, recorded in
`StageReceipt`), and `evolve` (registered kernels become searchable) with
no special-casing.

## Evolution as a language feature

Two levels of search, both deterministic, both receipt-gated:

- **Q2 — geometry.** The QGraph is fixed; the evolver searches
  `lane_width`/`tile_size`/`node_count`/`seed` (substrate `binetic-evolver`,
  four-integer genomes). Evidence: `docs/q2-self-optimization.md`.
- **Q4 — the graph.** The contract is fixed (port types + receipt demand);
  the search space is the program: kernel per stage, stage weight, pipeline
  length and order, plus geometry. `binetic_q::evolve` runs an elitist
  population search over `StructureGenome`s (SplitMix64, no clock); every
  candidate is compiled and run on the real fabric and scored only when its
  receipts verify at the demand. Elitism makes the best-score history
  monotone; the result carries the winning graph's plan hash, chain root,
  and re-parseable `.q` source. The search runs under a caller-supplied
  `KernelRegistry` (registered kernels become searchable) with a caller-
  supplied `MutationFn` (apps bias search over their own kernel sets).
  Evidence: `docs/q4-graph-evolution.md`.

The fitness model is caller-chosen (any `ThroughputModel` — lane-scaled,
fixed, or measured); the score formula is canonical and owned by the
substrate: `100·accuracy + ln(1+tps) − 0.01·bytes_per_token`. Callers choose
the measurement; the substrate owns the ranking.

## Syntax (Q0 subset)

```
program    := "@receipt" LEVEL "qblock" IDENT "(" param ")" "->" type "{" body "}"
type       := ("i64" | "f32" | "u64" | "u128" | "u256") "[" INT "]"
body       := config_decl stmt* emit_stmt
stmt       := "let" IDENT "=" expr ";"
expr       := call | IDENT "|>" call
call       := IDENT "(" (IDENT | INT) ("," (IDENT | INT))* ")"
```

Pipeline sugar `a |> f(b)` desugars to `f(a, b)`. Levels are `L0`–`L3`.
All five dtypes are first-class. A fabric lane is 64 bits, so a wide value
occupies multiple lanes: `u64` → 1, `u128` → 2, `u256` → 4 (little-endian
words); the plan's `input_len`/`output_len` and the fabric's
`port.lane_count` are *lane* counts (elements × words). Arithmetic
semantics per dtype: `i64` saturating (dot), `f32` IEEE-754, unsigned wide
modular mod 2^(64·words) — hand-rolled schoolbook over u64/u128, no bignum
dependency. Pipelines are dtype-homogeneous; lane widths span the
substrate's full `1..=64` range.

Q3 intent grammar (declarative, structured — an LLM adapter may *produce*
it; the synthesizer is a deterministic compiler):

```
intent <name> {
  input <dtype>[<n>];
  output <dtype>[<n>];                // optional, defaults to input
  demand L<k>;                        // optional, default L3
  pipeline [ <kernel>, ... ];
  weights [ <int>, ... ];             // optional
  geometry fixed | optimize;          // optional, default fixed
}
```

## Roadmap

- **Q0 — static** (DONE): IR, lexer, parser, checker, lowering to `Fabric`,
  reproducible `plan_hash`, L0–L3 receipts. Evidence:
  `docs/q0-ir-compiler.md`.
- **Q1 — verified** (DONE): receipts as compiler-enforced effects; kernel
  guarantee registry; weakest-link effect; honest runtime verification.
  Evidence: `docs/q1-verified-effects.md`.
- **Q2 — self-optimizing geometry** (DONE): QGraph-aware `FitnessFn` for
  `binetic-evolver`; verified-throughput search over execution geometry.
  Evidence: `docs/q2-self-optimization.md`.
- **Q3 — intent synthesis** (DONE): declarative intent → *verified* `.q`
  program; the synthesizer is constrained to emit only what check + real
  fabric verification accept at the demand. Evidence:
  `docs/q3-intent-synthesis.md`.
- **Q4 — graph evolution** (DONE): the program graph is the search space;
  deterministic, receipt-gated, elitist; the winning graph ships as
  re-parseable source with receipts. Evidence:
  `docs/q4-graph-evolution.md`.
- **Q5 — interactive** (next): REPL, visual editor, inspector, notebook —
  the evolved graph as a live object.
- **Q6 — apps**: inference, reservoir, dataflow as evolvable graphs; the 23
  `binetic-quant` dtypes as annotations.

## Certificates

`binetic-q attest <file.q>` emits the certificate: the entire claim in one
deterministic artifact — source, demand, computed effect, per-stage replay
evidence (`StageReceipt`), plan hash, chain root, packet count, receipt
bytes, and the L0–L3 flags. Re-running `attest` on the same source must
reproduce the plan hash and chain root byte for byte; determinism is the
verification. Evidence: `tests/attest.rs`.

## Open questions

- Do codecs stay annotations, or first-class quantize/dequantize Qblock pairs?
- Visual-first editor or REPL-first? Q0 recommends REPL (faster feedback
  loop).
- When do substrate deps switch from path (dev) to git tags (release)?
- Should evolution grow fan-in/fan-out topology (the `QGraph` edge list),
  or stay linear pipelines until the visual surface exists?
