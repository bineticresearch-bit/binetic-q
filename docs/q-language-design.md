# Binetic Q — language design

Status: DESIGN/IMPLEMENTATION (Q0 + Q1 + Q2 + Q3 shipped). Owner: parent.

## The one abstraction

> A program is a graph of typed, verified transforms. A Qblock is one
> transform. A wire is a typed packet stream. Running the graph emits receipts
> for free.

Q is not a new compute model. It is an ergonomic front end over the fabric
already built in binetic-core: deterministic execution, canonical hashing,
port routes, plan hashes, and L0–L3 receipt verification. Inference, reservoir
computation, P4 pipelines, and dataflow all reduce to transforming typed packet
streams with receipts.

Why "Q": quantized (bit-lane/codec substrate), queued (packet streams), quality
(receipt confidence), quanta (future bit-lane silicon).

## Abstraction stack

- **L0** bit-lanes, v4 `FabricPacket`, receipts, hashes — exists (Parts 01/03).
- **L1** kernels: DotProduct, FWHT, codecs, certified gate — exists/partial.
- **L2** `Qblock` = typed kernel + ports + receipt contract — **Q0 seam**.
- **L3** composition: graph / DSL → `PlanConfig` compiler — Q0.
- **L4** schedule, residency, failover, replay — partial (Part 02).
- **L5** REPL, visual editor, inspector, notebook — Q3.
- **L6** apps: inference, evolver, dataflow — emergent.

The leverage point is L2: if a Qblock has a stable manifest and contract, it
works identically in CLI, browser, and node because all consume the same IR.

## Syntax (Q0 subset)

```
program    := "@receipt" LEVEL "qblock" IDENT "(" param ")" "->" type "{" body "}"
body       := config_decl stmt* emit_stmt
stmt       := "let" IDENT "=" expr ";"
expr       := call | IDENT "|>" call
call       := IDENT "(" (IDENT | INT) ("," (IDENT | INT))* ")"
```

Pipeline sugar `a |> f(b)` desugars to `f(a, b)`. Levels are `L0`–`L3`.

## Receipts as effects

A `@receipt Lk` demand is a compile-time contract. A result that cannot be
verified at the demanded level is an error, not a caveat. Q1 enforces this as a
hard type effect: each kernel declares its guaranteed receipt level (built-ins
`dot`/`fwht`/`gate` → L3; an unregistered `Custom` kernel → L1), the program's
effect is the weakest stage in its pipeline, and `check()` rejects any demand
above that effect — naming the stage that caps it. The runner then verifies at
exactly the level the effect guarantees, so a Custom-capped program reports
`level2/level3 = false` rather than claiming a replay it did not perform.
Evidence: `docs/q1-verified-effects.md`.

## Roadmap

- **Q0 — static** (DONE): IR, lexer, parser, checker, lowering to `Fabric`,
  reproducible `plan_hash`, L0–L3 receipts. Evidence: `docs/q0-ir-compiler.md`.
- **Q1 — verified** (DONE): receipts as compiler-enforced effects; kernel
  guarantee registry; weakest-link effect; honest runtime verification.
  Evidence: `docs/q1-verified-effects.md`.
- **Q2 — self-optimizing** (DONE): first implemented intelligence step — a
  QGraph-aware `FitnessFn` for `binetic-evolver` that scores a compiled QGraph
  by verified throughput (receipts under a replay oracle), optimizing
  `lane_width`/`tile_size`/`node_count` like `FabricFitnessFn` does today.
  Evidence: `docs/q2-self-optimization.md`.
- **Q3 — intent synthesis** (DONE): structured declarative intent → verified
  `.q` programs. Synthesizes only programs that pass Q1's effect check and Q2's
  real-fabric verification; emitted source re-parses and reproduces the same
  plan/chain roots. Optional `geometry optimize` routes through Q2. Evidence:
  `docs/q3-intent-synthesis.md`. A natural-language/LLM front-end that emits the
  structured intent is a future input adapter, not claimed here.

## Open questions

- Do codecs stay annotations, or first-class quantize/dequantize Qblock pairs?
- Visual-first editor or REPL-first? Q0 recommends REPL (faster feedback loop).
- When do substrate deps switch from path (dev) to git tags (release)?
