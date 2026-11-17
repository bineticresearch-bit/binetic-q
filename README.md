# Binetic Q

> Quantized fabric compute. A systems language for verified compute.

Binetic Q (Q for short) is a C-like block language over the Binetic compute
fabric. A program is a graph of **Qblocks**; a Qblock is one typed, verified
transform; a wire is a typed packet stream. Running a graph emits
L0–L3 receipts from the real fabric — verification is a compiler-enforced
effect, not a whitepaper caveat.

Internal north star: *"C = functions over bytes linked by ABI; Q = Qblocks over
typed streams linked by receipts."* Public framing stays defensible: systems
language for verified compute.

## Status

`Q0` (static compiler) — **DONE**. `Q1` (verified effects), `Q2`
(self-optimizing via the evolver), `Q3` (intent → blocks) — planned. See
`docs/q-language-design.md` for the staged roadmap and `docs/q0-ir-compiler.md`
for the Q0 evidence.

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
cargo run -q --bin binetic-q -- check  crates/binetic-q/examples/three_blocks.q
cargo run -q --bin binetic-q -- compile crates/binetic-q/examples/three_blocks.q
cargo run -q --bin binetic-q -- run     crates/binetic-q/examples/three_blocks.q
```

`run` prints one JSON object with the `plan_hash`, `chain_root`, packet count,
and the L0/L1/L2/L3 verification flags.

## What Q0 proves

A three-Qblock `.q` program lowers to a real `binetic_fabric::Fabric`,
reproduces its compile-time `plan_hash`, and emits receipts that
`binetic_verify` accepts at L0, L1, L2, and L3. This is the fabric substrate —
not a simulation: real `FabricPacket`s flow through a virtual-port namespace.

```bash
cargo test            # 10 unit + 1 G0 integration test
```

## Dependency wiring

For development, the substrate crates (`binetic-core`, `binetic-fabric`,
`binetic-verify`) are path dependencies into a sibling `binetic-core` checkout.
For the public release these switch to git dependencies pinned to
`binetic-engine`, so the built Q compiler carries its own pinned substrate.

## License

Proprietary (see `LICENSE`). "Binetic Q", "Qblock", and the Q language design
are Binetic; the underlying fabric crates are separately licensed in their
home repository.
