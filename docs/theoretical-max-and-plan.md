# Binetic Q — Theoretical Max & Realization Plan

Status: strategy/design. Cross-cutting: covers the `binetic-q` front-end **and**
the `binetic-engine` substrate it pins (`binetic-core`, `binetic-fabric`,
`binetic-verify`, `binetic-evolver`). Every "measured" claim below is reproduced
in the substrate repo; every "projected" claim is flagged as such.

This document has four parts:
1. What exists today (grounded).
2. The core breakthrough (the one inversion everything is downstream of).
3. The theoretical achievable max.
4. A realization plan (evidence-gated, in the project's own culture).

Plus a final section: **could this build AGI?**

---

## 1. What exists today

### 1.1 The stack, layer by layer

**`binetic-core` (Part 01) — canonical deterministic primitives.** A fixed,
big-endian, length-delimited, `BNF4`-framed **v4 packet** with a canonical
preimage; SHA-256 `body_hash` (the hash slot is zeroed in the preimage so a hash
never hashes itself), CRC32 `checksum` (over the body *with* the real hash), and
a hand-rolled **HMAC-SHA256** for signatures. Lane packing at **1..=64 bits**,
MSB-first, unsigned residues. `verify()` runs 7 integrity checks. No wall clock,
no RNG, no env state anywhere — pinned by `no_wall_clock_value_enters_the_preimage`
and `every_committed_field_is_integrity_checked`.

**`binetic-fabric` (Part 02) — the execution model.** A `PlanConfig` compiles to a
deterministic `FabricPlan` (one virtual port per output tile, nodes assigned
round-robin). `Fabric::execute` runs a tile through a `ComputeKernel`, hashes the
input/output, chains `state_after = H(state_before, input_hash, output_hash)`, and
emits a `FabricPacket` whose `prev_hash` is the previous packet's `body_hash` on
that route. **Explicitly "in-memory and deterministic"**: no sockets, clocks, RNG,
or global mutable state in any plan/receipt hash. `migrate()` rebinds a failed port
to another node, preserving `state_hash`/`receipt_head` so it can replay.

**`binetic-verify` (Part 03) — the receipt tree + L0–L3.**
- **L0** per-packet integrity (delegated to `FabricPacket::verify`).
- **L1** chain structure (`seq` advances by 1, `prev_hash == previous.body_hash`).
- **L2** sampled replay (every 8th packet + last, through a caller oracle).
- **L3** full replay (every packet, through the oracle).

The crate is explicit: *"Receipts are tamper-evident records, not zero-knowledge
proofs of execution… it does not prove that a node actually performed the claimed
computation."* The substrate **orchestrates** the oracle; it does not interpret
payload semantics. The trust root is the hash function + the caller's oracle.

**`binetic-evolver` (Part 04) — deterministic search.** SplitMix64
`DeterministicRng`, four-integer geometry genomes, a canonical fitness
`100·accuracy + ln(1+tps) − 0.01·bytes_per_token`, elitist selection.

**`binetic-q` — the language front-end.** A `.q` DSL (lexer → parser → `QProgram`
IR → checker → lowering to a real `Fabric`). Q0 static compiler, Q1 receipts-as-
compile-time-effects, Q2 geometry evolution, Q3 intent synthesis, Q4 graph
evolution. The kernel registry is the single source of truth: a kernel **earns**
L3 by registering a replay oracle, else it holds L1. `attest` emits the
certificate. All 67 tests pass; determinism is enforced, not asserted.

### 1.2 The substrate is bigger than the four crates

The `binetic-engine` repo the crates come from is a **multi-language, P4-backed
"network compute fabric"**, not a Rust-only library:

- **P4 data-plane programs** (`netcomputer_*.p4`) — the fabric is realized as
  **programmable network hardware**; quantized bit-lane compute as packet
  processing. `netcomputer_cascade.p4` holds cross-packet recurrence in P4
  `register()` state and writes the receipt *into the payload*.
- **Python** (`binetic/` — `receipt.py`, `bitslice.py`, `signing.py`, `net/`),
  **WASM** (`binetic-wasm` + browser demos), **.NET** (`dotnet/`), **hardware/**.
- **Golden vectors** (`tests/fabric/test_golden_vectors.py`), **specs**
  (`NETWORK_COMPUTE_FABRIC_SPEC.md`, `FABRIC_BUILD_SPEC.md`), **whitepapers**
  (`WHITEPAPER.md`, `WHITEPAPER_BITLANES.md`), `RECEIPT_INDEX.md`.
- A sibling language (`docs/weave-language-design.md`) — Q is one front-end, not
  the only one.

### 1.3 What is *measured* (not projected)

From `WHITEPAPER_BITLANES.md` and `WHITEPAPER.md` (all reproducible via the
commands in those docs):

- **Bit-exact across four independent runtimes** (Python / C / WASM / .NET),
  **across OS+compiler** (macOS clang == Linux aarch64 gcc), **inside a real
  browser** (Chromium WASM), and **in a live P4/BMv2 switch data plane** — same
  trace, same bits, every field of every receipt.
- **The switch genuinely computes.** The P4 mix register computes `u(t-1)·v(t-1)`
  in silicon, 100% of packets; that nonlinearity is worth more than any delay-line
  memory (NMSE 0.0000 with the mix term vs 0.1515 without).
- **Closed-loop reservoir through the switch.** The readout's own prediction rides
  the packet back into the data plane; the switch multiplies it with the input in
  silicon; the receipt returns the product. NARMA-10: **0.015–0.021 NMSE — 8× over
  the input-only floor** (provably unreachable without `y(t-1)`) and ~30× over a
  200-node ESN. The loop *adds information* because the state now contains `y(t-1)`.
- **The data-plane program itself is evolvable.** A genome parameterizes a P4
  skeleton that compiles by construction; 0.25 s reload; fast enough to evolve
  *against physical measurements on the device*.
- **Reservoir quality.** NARMA-30 at 0.025 NMSE with 8 lattice registers vs 0.546
  for a 200-node ESN (21.8×), shown to be *structural* (a delay line provably
  can't reach it).
- **Conserved ledger.** In the CASCADE fold, **3 of 4 receipt fields are pure
  functions of the input/timestamp log and verify with no device replay** — only
  the state register (`phase`) needs replay.

### 1.4 What is *projected* (honest)

- **BMv2 is software.** All live-switch results are on a software switch. The
  timing channel is non-stationary on software; real silicon (Tofino/NetFPGA)
  would be stationary and bounded. Tighter closed-loop / timing-as-dynamics
  results are *expected* on silicon, not yet measured there.
- **The closed loop is not a single streaming pass.** The readout runs host-side,
  step by step. Making the whole loop one streaming inference (host out of the
  per-token path) is engineering, not yet done.
- **The fabric spec is a design target.** `NETWORK_COMPUTE_FABRIC_SPEC.md` is
  explicit: "a design target, not a claim that the fabric is already
  implemented." The 8 GB laptop / 60 B demo is the Phase 7 target.

---

## 2. The core breakthrough

Everything is downstream of one inversion:

> **The packet is simultaneously the data, the state, the computation, and the
> proof — and order is a value, not a clock reading.**

In a normal system these four are separate: you compute, you store state in
memory, you log (maybe), you verify (rarely, by re-running). Binetic collapses
them into one object. Three properties make the collapse real:

1. **State in motion.** The reservoir state is not in memory — it is *carried in
   the packet* (`phase`). "The state transition and the data movement are the
   same event." The portability lemma makes this exact: `replay` (persistent
   registers) == `replay_carried` (only the previous receipt's phase), bit-for-bit.
   **The trace determines the device.**

2. **Order as a value.** `seq` is intrinsic logical time; no wall clock enters any
   hash. A computation is a **deterministic fold over an ordered, receipted
   trace.** The device is reconstructable from the trace alone.

3. **The conserved ledger.** Most receipt fields are pure functions of the log and
   verify with **no device replay**; only the small state register needs replay.
   **Verification cost decouples from computation cost.** This is the difference
   between "verifiable compute" as a curiosity and as something you can do on a
   1000-token, 60 B-model generation.

**Why it's novel** vs. the cited prior art (photonic reservoirs, ESNs, P4 in-network
compute, network coding): those exist separately. The new thing is that *state +
computation + verification are the same packet, order is a value, and the trace
determines the device* — which is what lets them **compose** instead of merely
coexist.

---

## 3. The theoretical achievable max

Take the inversion to its limit and the end-state is one system:

> **A portable, self-verifying, state-carrying computation that is a packet,
> executed at line rate by an *evolved* programmable network, where the network is
> the processor, the packet is the state, the receipt is the proof, and correctness
> is a property the compiler enforces rather than a test you run.**

Five capabilities follow, each load-bearing:

1. **Computation-as-communication.** Routing a packet through the data plane *is*
   the computation. Any deterministic, bit-lane-expressible transform can be a
   packet path through programmable network hardware — and the transit *is* the
   receipt. The network stops being transport for compute and becomes the compute.
2. **Verification that scales.** Because most of the check is log-attestation and
   only the state register replays, verifying an arbitrarily long trace costs
   orders of magnitude less than re-running it. This is what makes "verify the 60 B
   output on the 8 GB laptop" feasible at all.
3. **The device is a function of the trace.** The same trace + plan runs
   bit-identically on a laptop, a browser, a WASM runtime, a .NET host, and a
   switch. A **runtime-agnostic compute artifact** — the "TCP/IP of verifiable
   compute": a *standard*, not a product.
4. **State-in-motion dissolves the memory wall.** Lanes stay resident (no per-step
   repack); throughput = concurrent chains ÷ round-trip latency. Throughput scales
   with the *network's* parallelism, not the *device's* memory bandwidth.
5. **The compiler searches the data plane.** Q2/Q4 evolve geometry and graph; the
   reservoir track evolves the *switch program* against *measured* line-rate
   performance. The max is a compiler that searches data-plane programs + network
   geometry + quantization, scores by real hardware, and emits a deterministic,
   receipted, verified plan — "intelligence is a language feature," grounded in
   measurement, not a model.

Deepest ML consequence: **the network is a *recurrent* computer, not just
feed-forward** — genuine cross-packet recurrence and closed-loop feedback executed
in switch silicon and verified by receipts. No one else has a working, measured
instance of this.

---

## 4. Realization plan

The project's culture is **evidence-gated**: each phase ships an *evidence
artifact* and a **gate** (a check that must pass, in the style of the existing
`acceptance criteria` and per-part proof docs). Phases below follow that.
**E** = engineering (doable now), **R** = research (open). The two hardest,
most load-bearing phases are **P2** and **P3**.

### P0 — Consolidate the substrate + front-end (E)
Get the whole stack (substrate + Q) building and testing in one place, with the
golden vectors as the contract. Today the substrate is a git dep pinned by `rev`;
the golden vectors live in the substrate repo.
- **Work.** Pin + vendor policy; a single `make verify` that runs the substrate
  golden vectors *and* the Q suite; CI across Rust/Python/WASM/.NET.
- **Gate.** Cross-language golden vectors (Rust/Python/WASM/.NET) all pass on the
  *same* trace; `binetic-fabric` network-computation proof passes; Q 67/67.
- **Evidence.** `docs/p0-consolidation.md` with the consolidated evidence JSON.

### P1 — Make the packet a real, signed wire standard (E) — *critical path*
The v4 packet is the strategic asset. Turn it from "a format we happen to share"
into "a standard the ecosystem can build on."
- **Work.** Formal packet spec + versioning; golden vectors in CI for all four
  runtimes; a small `receipt` verifier library per language; **signing by default**
  (node public keys over `body_hash`) so receipts are *attributed*, not just
  tamper-evident. Q receipts are currently unsigned (`verify()` uses the zero key).
- **Gate.** A receipt produced in Python verifies in Rust/WASM/.NET/browser; a
  signed receipt from node A is attributable to A and rejects a wrong key.
- **Evidence.** `docs/p1-wire-standard.md`; the cross-language verify matrix.

### P2 — Close the loop into a single streaming pass (E, hard) — *load-bearing*
State-in-motion is only as good as "the host is out of the per-step path." Today
the closed loop is host-driven, step by step.
- **Work.** Keep the `receipt(t) → input(t+1)` feedback entirely in the data plane;
  host only at stream start/end. Resident lanes end-to-end (realize the 2× SIMD
  widening that §6.14 measured in-kernel but not yet in the full pipeline).
- **Gate.** A recurrent model streams at line rate through the switch with the host
  out of the per-token path; receipts verify L0–L3 on the full stream; measured
  throughput = concurrent chains ÷ round-trip latency.
- **Evidence.** `docs/p2-streaming-loop.md` with the latency table (in-fabric vs
  host-driven) and the full-stream receipt root.

### P3 — Verification that scales + adversarial soundness (E + R) — *load-bearing*
This is what turns "tamper-evident" into "trustless" and makes verification cheap.
- **Work (E).** Generalize the **conserved ledger**: for each kernel, declare which
  output fields are log-derivable (cheap) vs which need replay (expensive); always
  verify the cheap ones, sample/full-replay the expensive ones per the demanded
  level. **Merkle-ize** the receipt aggregate (fan-in/fan-out DAGs, inclusion
  proofs) — today it's a linear chain.
- **Work (R).** **Adversarial soundness**: independent oracles (a *different*
  implementation for compute vs replay — today Q's built-ins use the *same*
  function for both, so L3 is a determinism check, not independent semantic
  verification), a TEE-attestation seam, or zk-proofs over the fold.
- **Gate.** Verify a long trace for a fraction of the cost of re-running it; a
  malicious node's wrong answer is caught *without trusting its oracle*; a
  fan-in/fan-out DAG yields an inclusion proof.
- **Evidence.** `docs/p3-verification.md`: cost-of-verify vs cost-of-rerun curve;
  the adversarial-node test; the Merkle inclusion-proof test.

### P4 — The compiler searches the data plane (E + R)
Unify the three search tracks into one, and make the fitness *measured*.
- **Work.** One search over (data-plane program × network geometry × quantization),
  scored by **measured** line-rate performance + accuracy, gated on receipts.
  Replace Q's synthetic throughput model with real per-geometry measurements (the
  substrate already exposes a `ThroughputModel` seam for this).
- **Gate.** The compiler emits a plan that beats hand-written baselines on
  *measured* verified-tokens/s on real hardware, and the winning plan re-runs
  bit-identically (same plan hash + chain root).
- **Evidence.** `docs/p4-compiler-search.md`: baseline-vs-best measured table +
  the re-run identity proof.

### P5 — The real graph + the interactive surface (E)
Today Q's `QGraph` (blocks + edges) is defined but the live model is a linear
`Stage` pipeline. Wire the DAG in; ship Q5.
- **Work.** Multi-input stages and fan-in/fan-out in the Q parser/checker/lowerer;
  REPL + visual editor + inspector — the evolved graph as a live object.
- **Gate.** A non-linear Q program (fan-in/fan-out) compiles, runs, verifies
  L0–L3, and evolves; the REPL round-trips a live graph.
- **Evidence.** `docs/p5-graph-and-repl.md`.

### P6 — The flagship demo (E)
The spec's Phase 7, made reproducible.
- **Work.** 8 GB laptop + browser + network fabric; **60 B-class quantized** model;
  receipt-verified output; published **verified-tokens/s**; privacy modes visibly
  distinct.
- **Gate.** Third-party reproducible; active-parameters-per-token, local-vs-remote,
  node count, network mode, quality delta, and receipt-bytes-per-token all
  disclosed (the spec's "acceptable vs unacceptable headline" rule).
- **Evidence.** `docs/p6-demo.md` + the published benchmark table.

### P7 — The AGI bet (R)
Where "could this build AGI" is actually tested, not asserted. See §5.
- **Gate.** A system that **self-modifies its own compute graph** under the receipt
  gate and **measurably improves** across a held-out task suite while its receipts
  verify at the demanded level *throughout*.

**Critical path.** P1 (signed, cross-language wire standard) and P3 (adversarial
soundness + Merkle) are what turn "a cool Rust substrate" into "a standard the
ecosystem can build AGI infrastructure on." P2 (streaming loop) is what makes
"the network is the processor" true at line rate. Do P0 → P1 → P3 in parallel with
P2; P4–P6 build on those.

---

## 5. Could this build AGI?

**Short answer: not by itself — it's a substrate, not a mind. But it is one of the
more interesting *substrates* for the hardest *engineering* part of AGI:
verifiable, self-improving, long-horizon, portable compute. Its unique AGI
contribution is *sound self-modification*.**

### 5.1 The substrate is the "how," not the "what"
AGI requires general learning, reasoning/world-modeling, long-horizon memory,
self-modification, grounding, and scalability. This substrate answers the
*mechanism* questions — how do you run, verify, optimize, and safely
self-modify computation — not the *cognitive* question of what computation
produces general intelligence. It's orthogonal-enabling, the way a fast, reliable
CPU is orthogonal to whether the software on it is intelligent. It **enables** AGI
infrastructure; it does not **constitute** a mind.

### 5.2 Where it is genuinely AGI-relevant
Three of the six AGI requirements map unusually well onto properties this
substrate already has or is built to have:

- **Self-modification (the novel one).** The evolvable data-plane program +
  receipts-as-effects means: when the system rewrites its own compute graph, the
  rewrite is **verified before it's accepted** — an impossible candidate scores at
  the invalid floor and is never shipped. This is a *structural* answer to a core
  AGI-safety/scaling problem most stacks have no answer to: **how do you let a
  system improve itself without silent regression?** "Intelligence is a language
  feature" taken seriously = an intelligence that evolves its own graph under a
  receipt gate that provably preserves its guarantees. That is **sound
  self-improvement**, and it's the single most novel AGI-relevant capability here.
- **Long-horizon memory.** State-in-motion + "the trace determines the device" +
  the conserved ledger means *memory is a receipted log that is replayable and
  cheap to verify integrity of*. That's a substrate for very long-horizon,
  auditable, editable memory — better than a GPU's volatile memory for
  long-horizon reasoning and persistent world models.
- **Portable, trustworthy deployment.** If a computation is a runtime-agnostic,
  receipted artifact, then an AGI's "brain" is a *verifiable, reproducible,
  shippable* artifact — you can verify it did what it claims, reproduce its
  reasoning, and audit its decisions. That's an AGI-*infrastructure* contribution.

### 5.3 Why it's not obviously AGI (the honest risks)
- **Bit-lanes favor quantized/integer/branch-free work.** The current AGI frontier
  (dense float, large continuous representations) is not the natural fit. You'd be
  doing AGI in a *quantized* regime. That's a bet: quantization is increasingly
  enough for inference, but the "thinking" may still want the float tensor path,
  with the network for fast verified bulk.
- **Reservoir computing is not the proven AGI architecture.** It's excellent for
  recurrent/time-series/function-approximation. General reasoning, composition,
  abstraction, and planning — the things that make something *AGI* — are not
  demonstrated in reservoirs. The substrate gives a *novel compute paradigm*;
  whether that paradigm produces *general* intelligence is open science.
- **Verification is replay/tamper-evidence, not proof.** For AGI self-modification
  to be *trustworthy*, the verification must be *sound* (adversarial) — which
  needs independent oracles / TEEs / zk (P3). Until then, "verified
  self-improvement" is "self-improvement checked by trusted code": good, not airtight.

### 5.4 The research agenda that would test it (P7)
If the goal is to find out whether this substrate is AGI-capable, the experiments
are:
1. **Cognitive:** does a *quantized, recurrent, in-network* model reach
   general-reasoning benchmarks (not just time-series)? This is the question the
   substrate does *not* answer.
2. **Verifiable self-improvement (the novel bet):** can the system evolve its own
   compute graph under the receipt gate and *improve* on a held-out task suite
   while its receipts verify at the demanded level throughout? If yes, you have a
   self-improving system that *cannot silently regress* — a first.
3. **Long-horizon memory:** does trace-as-memory + the conserved ledger support
   long-horizon reasoning/consistency at scale (10^5–10^6 step traces) at
   sub-rerun verification cost?

### 5.5 Bottom line on AGI
The substrate is a **necessary-but-not-sufficient** contribution. It could be the
*body* of an AGI — verifiable, self-optimizing, portable, with long-horizon
auditable memory — and it solves a hard part of the problem (safe self-improvement)
that most AGI stacks don't. But it is not the *mind*. The open question it leaves
standing is the real one: **is a quantized-recurrent-in-network mind general
enough?** The substrate's gift to that question is that it makes exploring it
*cheaper and safer* — verifiable, portable, and self-improving without silent
regression — even though it doesn't decide it.

---

## Appendix A — Load-bearing assumptions (what the max depends on)

| # | Assumption | Status | If it fails |
|---|---|---|---|
| 1 | Quantized/integer/binary is enough for the target compute | Partial (BNN, fixed-point measured; dense float not) | Network handles bulk, tensor path handles "thinking" |
| 2 | Replay oracle is *independent* of kernel code | **Not yet** (Q built-ins use the same fn) | L3 = determinism check, not semantic proof |
| 3 | Verification is sound under adversarial nodes | **Not yet** (tamper-evidence + trusted oracle) | Trustless compute needs independent oracle / TEE / zk |
| 4 | Real silicon ≈ software switch (stationary timing) | **Projected** (BMv2 only) | Timing-as-dynamics may not transfer |
| 5 | Closed loop can be a single streaming pass | **Not yet** (host-driven) | "Network is processor" bounded by host round-trips |
| 6 | Receipt aggregate supports DAGs | **Not yet** (linear chain) | Fan-in/fan-out needs Merkle structure |
| 7 | Geometry/program search generalizes across tasks | **Not yet** (task-specific) | Per-task re-evolution |

## Appendix B — The two things that most determine the ceiling
1. **Oracle independence (A2).** The value of L3 is only as strong as the
   independence of the replay path from the execution path. Today it's independent
   of *fabric state* but not of *kernel code*. Making oracles independent is the
   single highest-leverage correctness improvement.
2. **The conserved ledger generalized (P3).** The claim that "most verification is
   log-checking" is what makes the whole thing scale. Generalizing it per-kernel
   and measuring the verify-vs-rerun cost ratio is what turns the vision into an
   engineering fact.
