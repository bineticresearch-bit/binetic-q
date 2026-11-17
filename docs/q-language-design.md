# Binetic Q — language design

Status: DESIGN/IMPLEMENTATION (Q0 shipped; Q1–Q3 staged). Owner: parent.

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
verified at the demanded level is an error, not a caveat. In Q0 the checker
accepts any demand and the runner satisfies it via `verify_with_replay`; Q1
promotes this to a hard type effect checked against each Qblock's
`BlockContract::min_receipt`.

## Roadmap

- **Q0 — static** (DONE): IR, lexer, parser, checker, lowering to `Fabric`,
  reproducible `plan_hash`, L0–L3 receipts. Evidence: `docs/q0-ir-compiler.md`.
- **Q1 — verified** (planned): receipts as enforced types; typed wire schemas.
- **Q2 — self-optimizing** (planned): first implemented intelligence step — a
  QGraph-aware `FitnessFn` for `binetic-evolver` that scores a compiled QGraph
  by verified throughput (receipts under a replay oracle), optimizing
  `lane_width`/`tile_size`/`node_count` like `FabricFitnessFn` does today.
- **Q3 — intent synthesis** (destination): natural-language / demo → Qblocks.

## Open questions

- Do codecs stay annotations, or first-class quantize/dequantize Qblock pairs?
- Visual-first editor or REPL-first? Q0 recommends REPL (faster feedback loop).
- When do substrate deps switch from path (dev) to git tags (release)?
