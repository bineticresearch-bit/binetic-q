//! Kernel registry: guarantees are **earned, not assigned**.
//!
//! Every kernel in the registry pairs a *compute* (what the kernel does on
//! decoded fabric lanes) with an optional *replay oracle* (an independent
//! re-derivation of the output from `(input, weight)`). A kernel's
//! guaranteed receipt level is derived from that pairing, never hardcoded at
//! a call site:
//!
//! - replay oracle registered → **L3** (full semantic replay: the committed
//!   output is re-derived independently and must match)
//! - no replay oracle         → **L1** (packet integrity + chain structure,
//!   no replay claim)
//!
//! The built-ins (`dot`, `fwht`, `gate`) ship with *real* semantics —
//! integer/float dot product, fast Walsh–Hadamard transform, certified
//! (ReLU) gate — each with its own replay oracle. Any kernel, including one
//! an LLM proposed, can `register` and earn L3 by supplying a replay that
//! re-derives its output. That is the Q1 extension point, implemented.

use std::collections::HashMap;

use binetic_core::{is_supported_lane_width, pack_lanes, unpack_lanes, CoreError};
use binetic_fabric::{ComputeKernel, FabricError};

use crate::ir::{DType, Kernel, Level, QProgram};

/// What a kernel does on decoded fabric lanes. Pure function of its
/// arguments — no I/O, no clock.
pub trait QKernel {
    fn execute(
        &self,
        dtype: DType,
        input: &[i64],
        weight: &[i64],
        lane_width: u16,
    ) -> Result<Vec<i64>, String>;
}

/// Independent re-derivation: is `output` a valid committed result of this
/// kernel on `(input, weight)`? This semantic replay is what earns L3 — the
/// runner re-derives every stage output and refuses to run on mismatch.
///
/// `input`/`weight` are the *raw* lane values the runner handed to the
/// fabric; the fabric's kernel computed on their lane-decoded (quantized)
/// residues, so a correct oracle performs the same pack/unpack round-trip
/// at `lane_width` before re-deriving.
pub trait ReplayOracle {
    fn replay(
        &self,
        dtype: DType,
        lane_width: u16,
        input: &[i64],
        weight: &[i64],
        output: &[i64],
    ) -> bool;
}

/// The lane round-trip the fabric applies before a kernel sees its operands.
fn decode_lanes(v: &[i64], lane_width: u16) -> Vec<i64> {
    let packed = pack_lanes(v, lane_width).expect("valid lane width");
    unpack_lanes(&packed, lane_width, v.len() as u32).expect("consistent pack/unpack")
}

/// One registered kernel: compute + optional replay oracle.
pub struct KernelSpec {
    pub compute: Box<dyn QKernel>,
    pub replay: Option<Box<dyn ReplayOracle>>,
}

impl KernelSpec {
    pub fn new(compute: Box<dyn QKernel>, replay: Option<Box<dyn ReplayOracle>>) -> Self {
        Self { compute, replay }
    }

    /// Earned guarantee: L3 with a replay oracle, L1 without.
    pub fn guaranteed_level(&self) -> Level {
        if self.replay.is_some() {
            Level::L3
        } else {
            Level::L1
        }
    }
}

/// The registry of known kernels. `default()` carries the built-ins; apps
/// register more (their own, or LLM-proposed) and the checker, runner, and
/// evolver all consult the same registry.
pub struct KernelRegistry {
    specs: HashMap<String, KernelSpec>,
}

impl KernelRegistry {
    /// Built-ins with real semantics and replay oracles.
    pub fn builtin() -> Self {
        let mut r = Self {
            specs: HashMap::new(),
        };
        r.register("dot", KernelSpec::new(Box::new(DotKernel), Some(Box::new(DotKernel))));
        r.register(
            "fwht",
            KernelSpec::new(Box::new(FwhtKernel), Some(Box::new(FwhtKernel))),
        );
        r.register(
            "gate",
            KernelSpec::new(Box::new(GateKernel), Some(Box::new(GateKernel))),
        );
        r
    }

    pub fn register(&mut self, name: &str, spec: KernelSpec) {
        self.specs.insert(name.to_string(), spec);
    }

    pub fn spec(&self, kernel: &Kernel) -> Option<&KernelSpec> {
        self.specs.get(&kernel_name(kernel))
    }

    /// Earned guarantee for a kernel: L3 if registered with a replay oracle,
    /// L1 otherwise (including unregistered Custom kernels).
    pub fn guaranteed_level(&self, kernel: &Kernel) -> Level {
        match self.spec(kernel) {
            Some(s) => s.guaranteed_level(),
            None => Level::L1,
        }
    }

    /// Kernel names the search may place in a stage. Sorted, so the search
    /// stream is deterministic across processes (HashMap order is not).
    pub fn searchable(&self) -> Vec<Kernel> {
        let mut names: Vec<&String> = self.specs.keys().collect();
        names.sort();
        names.into_iter().map(|n| Kernel::parse(n)).collect()
    }
}

impl Default for KernelRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

fn kernel_name(k: &Kernel) -> String {
    match k {
        Kernel::Dot => "dot".into(),
        Kernel::Fwht => "fwht".into(),
        Kernel::Gate => "gate".into(),
        Kernel::Custom(s) => s.clone(),
    }
}

/// The receipt effect of a program under a registry: the weakest *earned*
/// guarantee across its stages.
pub fn receipt_effect_with(program: &QProgram, registry: &KernelRegistry) -> Level {
    program
        .stages
        .iter()
        .fold(Level::L3, |acc, s| acc.min(registry.guaranteed_level(&s.kernel)))
}

/// Adapter: run a registered `QKernel` through the fabric's `ComputeKernel`
/// seam, round-tripping both operands through the Part 01 lane helpers so
/// lane quantization applies identically to every kernel.
pub struct FabricKernel<'a> {
    pub spec: &'a KernelSpec,
    pub dtype: DType,
}

impl ComputeKernel for FabricKernel<'_> {
    fn execute(
        &self,
        input: &[i64],
        weight: &[i64],
        lane_width: u16,
    ) -> Result<Vec<i64>, FabricError> {
        if input.is_empty() {
            return Err(FabricError::EmptyShape);
        }
        if !is_supported_lane_width(lane_width) {
            return Err(FabricError::InvalidLaneWidth(lane_width));
        }
        if weight.len() % input.len() != 0 {
            return Err(FabricError::WeightShape {
                weight_len: weight.len(),
                input_len: input.len(),
            });
        }
        let count = input.len() as u32;
        let decode = |v: &[i64], count: u32| -> Result<Vec<i64>, FabricError> {
            let packed = pack_lanes(v, lane_width)
                .map_err(|e: CoreError| FabricError::Core(format!("{e:?}")))?;
            unpack_lanes(&packed, lane_width, count)
                .map_err(|e| FabricError::Core(format!("{e:?}")))
        };
        let din = decode(input, count)?;
        let dw = decode(weight, weight.len() as u32)?;
        self.spec
            .compute
            .execute(self.dtype, &din, &dw, lane_width)
            .map_err(|e| FabricError::Core(e))
    }
}

// ---------------------------------------------------------------------------
// Wide unsigned arithmetic — hand-rolled, mod 2^(64·W), little-endian limbs.
// No bignum dependency: the fabric is a bit-lane machine, so wide values are
// just groups of 64-bit lanes and the math is schoolbook over u64/u128.
// ---------------------------------------------------------------------------

/// Wide add, mod 2^(64·W).
fn wide_add(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut out = Vec::with_capacity(a.len());
    let mut carry: u128 = 0;
    for w in 0..a.len() {
        let cur = a[w] as u128 + b[w] as u128 + carry;
        out.push((cur & u64::MAX as u128) as u64);
        carry = cur >> 64;
    }
    out
}

/// Wide subtract, mod 2^(64·W) (wrapping).
fn wide_sub(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow: u128 = 0;
    for w in 0..a.len() {
        let cur = a[w] as i128 - b[w] as i128 - borrow as i128;
        if cur < 0 {
            out.push((cur + (1i128 << 64)) as u64);
            borrow = 1;
        } else {
            out.push(cur as u64);
            borrow = 0;
        }
    }
    out
}

/// Full W×W product in 2W limbs (u128 headroom per limb).
fn wide_mul_full(a: &[u64], b: &[u64]) -> Vec<u128> {
    let w = a.len();
    let mut prod = vec![0u128; 2 * w];
    for i in 0..w {
        let mut carry: u128 = 0;
        for j in 0..w {
            let cur = prod[i + j] + a[i] as u128 * b[j] as u128 + carry;
            prod[i + j] = cur & u64::MAX as u128;
            carry = cur >> 64;
        }
        let cur = prod[i + w] + carry;
        prod[i + w] = cur & u64::MAX as u128;
    }
    prod
}

/// Sum of element-wise products, mod 2^(64·W). `a`/`b` are
/// `elements × words` lanes each.
fn wide_dot(a: &[u64], b: &[u64], elements: usize, words: usize) -> Vec<u64> {
    let mut acc = vec![0u128; words];
    for e in 0..elements {
        let prod = wide_mul_full(
            &a[e * words..(e + 1) * words],
            &b[e * words..(e + 1) * words],
        );
        for w in 0..words {
            // Reducing each limb mod 2^64 after every add is exactly
            // reducing the whole number mod 2^(64·W).
            acc[w] = (acc[w] + prod[w]) & u64::MAX as u128;
        }
    }
    acc.iter().map(|&x| x as u64).collect()
}

// ---------------------------------------------------------------------------
// Built-in kernels: real semantics, each with its own replay oracle.
// ---------------------------------------------------------------------------

/// One dot product per weight row. i64: saturating i128 accumulation; f32:
/// IEEE-754 float accumulation; u64/u128/u256: modular accumulation
/// (mod 2^(64·words)) — the unsigned quantized-fabric semantics.
pub struct DotKernel;

fn dot_row(dtype: DType, elements: usize, words: usize, input: &[i64], row: &[i64]) -> Vec<i64> {
    match dtype {
        DType::I64 => {
            let mut acc: i128 = 0;
            for (v, f) in input.iter().zip(row.iter()) {
                acc = acc.saturating_add((*v as i128).saturating_mul(*f as i128));
            }
            vec![i64::try_from(acc).unwrap_or_else(|_| {
                if acc > 0 {
                    i64::MAX
                } else {
                    i64::MIN
                }
            })]
        }
        DType::F32 => {
            let mut acc: f32 = 0.0;
            for (v, f) in input.iter().zip(row.iter()) {
                acc += f32::from_bits(*v as u32) * f32::from_bits(*f as u32);
            }
            vec![acc.to_bits() as i64]
        }
        DType::U64 | DType::U128 | DType::U256 => {
            let a: Vec<u64> = input.iter().map(|&v| v as u64).collect();
            let b: Vec<u64> = row.iter().map(|&v| v as u64).collect();
            wide_dot(&a, &b, elements, words)
                .into_iter()
                .map(|w| w as i64)
                .collect()
        }
    }
}

impl QKernel for DotKernel {
    fn execute(
        &self,
        dtype: DType,
        input: &[i64],
        weight: &[i64],
        _lane_width: u16,
    ) -> Result<Vec<i64>, String> {
        let words = dtype.words();
        if input.is_empty() || input.len() % words != 0 {
            return Err("dot: bad input shape".into());
        }
        if weight.len() % input.len() != 0 {
            return Err("dot: bad weight shape".into());
        }
        let elements = input.len() / words;
        let rows = weight.len() / input.len();
        let mut out = Vec::with_capacity(rows * words);
        for r in 0..rows {
            let row = &weight[r * input.len()..(r + 1) * input.len()];
            out.extend(dot_row(dtype, elements, words, input, row));
        }
        Ok(out)
    }
}

/// Replay: independently re-derive every row (on lane-decoded operands) and
/// compare.
impl ReplayOracle for DotKernel {
    fn replay(
        &self,
        dtype: DType,
        lane_width: u16,
        input: &[i64],
        weight: &[i64],
        output: &[i64],
    ) -> bool {
        let words = dtype.words();
        if input.is_empty() || input.len() % words != 0 || weight.len() % input.len() != 0 {
            return false;
        }
        let din = decode_lanes(input, lane_width);
        let dw = decode_lanes(weight, lane_width);
        let elements = din.len() / words;
        let rows = dw.len() / din.len();
        if output.len() != rows * words {
            return false;
        }
        (0..rows).all(|r| {
            &output[r * words..(r + 1) * words]
                == dot_row(
                    dtype,
                    elements,
                    words,
                    &din,
                    &dw[r * din.len()..(r + 1) * din.len()],
                )
                .as_slice()
        })
    }
}

/// Fast Walsh–Hadamard transform over the elements (integer: wrapping
/// arithmetic; f32: float transform on the bit-cast values; u128/u256: wide
/// modular add/sub). Self-inverse up to scale: H(H(x)) = n·x (mod 2^bits).
pub struct FwhtKernel;

fn fwht_i64(v: &mut [i64]) {
    let n = v.len();
    let mut h = 1;
    while h < n {
        for i in (0..n).step_by(2 * h) {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a.wrapping_add(b);
                v[j + h] = a.wrapping_sub(b);
            }
        }
        h *= 2;
    }
}

fn fwht(dtype: DType, input: &[i64]) -> Vec<i64> {
    let words = dtype.words();
    let elements = input.len() / words;
    match dtype {
        DType::I64 | DType::U64 => {
            // Bit-identical: wrapping add/sub on 64-bit lanes.
            let mut v = input.to_vec();
            fwht_i64(&mut v);
            v
        }
        DType::F32 => {
            let mut v: Vec<f32> = input.iter().map(|x| f32::from_bits(*x as u32)).collect();
            let n = v.len();
            let mut h = 1;
            while h < n {
                for i in (0..n).step_by(2 * h) {
                    for j in i..i + h {
                        let (a, b) = (v[j], v[j + h]);
                        v[j] = a + b;
                        v[j + h] = a - b;
                    }
                }
                h *= 2;
            }
            v.into_iter().map(|f| f.to_bits() as i64).collect()
        }
        DType::U128 | DType::U256 => {
            let mut v: Vec<Vec<u64>> = (0..elements)
                .map(|e| {
                    input[e * words..(e + 1) * words]
                        .iter()
                        .map(|&x| x as u64)
                        .collect()
                })
                .collect();
            let mut h = 1;
            while h < elements {
                for i in (0..elements).step_by(2 * h) {
                    for j in i..i + h {
                        let a = v[j].clone();
                        let b = v[j + h].clone();
                        v[j] = wide_add(&a, &b);
                        v[j + h] = wide_sub(&a, &b);
                    }
                }
                h *= 2;
            }
            v.into_iter()
                .flat_map(|w| w.into_iter().map(|x| x as i64))
                .collect()
        }
    }
}

impl QKernel for FwhtKernel {
    fn execute(
        &self,
        dtype: DType,
        input: &[i64],
        _weight: &[i64],
        _lane_width: u16,
    ) -> Result<Vec<i64>, String> {
        if input.is_empty() {
            return Err("fwht: empty input".into());
        }
        Ok(fwht(dtype, input))
    }
}

impl ReplayOracle for FwhtKernel {
    fn replay(
        &self,
        dtype: DType,
        lane_width: u16,
        input: &[i64],
        _weight: &[i64],
        output: &[i64],
    ) -> bool {
        !input.is_empty() && output == &fwht(dtype, &decode_lanes(input, lane_width))
    }
}

/// Certified gate: ReLU. i64 lanes zero out negatives; f32 lanes apply the
/// float ReLU to the bit-cast values; unsigned dtypes are the identity (every
/// value is already ≥ 0) — the certificate still holds trivially.
pub struct GateKernel;

fn gate(dtype: DType, input: &[i64]) -> Vec<i64> {
    match dtype {
        DType::I64 => input.iter().map(|v| (*v).max(0)).collect(),
        DType::F32 => input
            .iter()
            .map(|v| {
                let f = f32::from_bits(*v as u32);
                f.max(0.0).to_bits() as i64
            })
            .collect(),
        DType::U64 | DType::U128 | DType::U256 => input.to_vec(),
    }
}

impl QKernel for GateKernel {
    fn execute(
        &self,
        dtype: DType,
        input: &[i64],
        _weight: &[i64],
        _lane_width: u16,
    ) -> Result<Vec<i64>, String> {
        if input.is_empty() {
            return Err("gate: empty input".into());
        }
        Ok(gate(dtype, input))
    }
}

impl ReplayOracle for GateKernel {
    fn replay(
        &self,
        dtype: DType,
        lane_width: u16,
        input: &[i64],
        _weight: &[i64],
        output: &[i64],
    ) -> bool {
        !input.is_empty() && output == &gate(dtype, &decode_lanes(input, lane_width))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fwht_is_self_inverse_up_to_scale() {
        // H(H(x)) = n·x for n a power of two.
        let x: Vec<i64> = (0..8).map(|i| i as i64 * 3 - 5).collect();
        let once = fwht(DType::I64, &x);
        let twice = fwht(DType::I64, &once);
        assert_eq!(twice, x.iter().map(|v| v * 8).collect::<Vec<_>>());
    }

    #[test]
    fn dot_replay_matches_compute() {
        let input = vec![1, -2, 3, 4];
        let weight = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let k = DotKernel;
        let out = k.execute(DType::I64, &input, &weight, 64).unwrap();
        // lane_width 64: no truncation, raw == decoded.
        assert!(k.replay(DType::I64, 64, &input, &weight, &out));
        let bad = out.iter().map(|v| v + 1).collect::<Vec<_>>();
        assert!(!k.replay(DType::I64, 64, &input, &weight, &bad));
    }

    #[test]
    fn replay_accounts_for_lane_quantization() {
        // At lane_width 8 the fabric computes on 8-bit residues; the oracle
        // must re-derive from those residues, not the raw values.
        let input = vec![300i64, 5]; // residues: 44, 5
        let weight = vec![1i64, 1];
        let k = DotKernel;
        let din = decode_lanes(&input, 8);
        let dw = decode_lanes(&weight, 8);
        let out = k.execute(DType::I64, &din, &dw, 8).unwrap();
        assert_eq!(out, vec![49]); // 44 + 5, not 300 + 5
        assert!(k.replay(DType::I64, 8, &input, &weight, &out));
        // A replay from the *raw* values would disagree.
        let raw_out = k.execute(DType::I64, &input, &weight, 64).unwrap();
        assert_eq!(raw_out, vec![305]);
        assert_ne!(raw_out, out);
        assert!(!k.replay(DType::I64, 8, &input, &weight, &raw_out));
    }

    #[test]
    fn gate_output_is_non_negative_and_replays() {
        let input = vec![-5, 3, -1, 9];
        let k = GateKernel;
        let out = k.execute(DType::I64, &input, &[], 64).unwrap();
        assert!(out.iter().all(|v| *v >= 0));
        assert!(k.replay(DType::I64, 64, &input, &[], &out));
    }

    #[test]
    fn wide_arithmetic_wraps_modularly() {
        // add: MAX + 1 wraps to 0.
        assert_eq!(wide_add(&[u64::MAX], &[1]), vec![0]);
        // sub: 0 - 1 wraps to MAX.
        assert_eq!(wide_sub(&[0], &[1]), vec![u64::MAX]);
        // mul: (2^64-1)^2 = 2^128 - 2^65 + 1 → limbs [1, 2^64-2].
        assert_eq!(
            wide_mul_full(&[u64::MAX], &[u64::MAX]),
            vec![1u128, (u64::MAX - 1) as u128]
        );
        // 128-bit dot: (2^64)^2 = 2^128 → 0 mod 2^128.
        let a = vec![0u64, 1]; // 2^64
        assert_eq!(wide_dot(&a, &a, 1, 2), vec![0u64, 0]);
        // 128-bit dot: (2^64 + 1)^2 = 2^128 + 2^65 + 1 → 2^65 + 1 mod 2^128.
        let b = vec![1u64, 1]; // 2^64 + 1
        assert_eq!(wide_dot(&b, &b, 1, 2), vec![1u64, 2]);
    }

    #[test]
    fn fwht_u128_is_self_inverse_up_to_scale() {
        // H(H(x)) = n·x mod 2^128. Small non-negative limbs keep every
        // intermediate carry-free, so the expectation is per-limb 4·x.
        let x: Vec<i64> = vec![7, 1, 3, 5, 3, 2, 9, 4]; // 4 u128 elements
        let once = fwht(DType::U128, &x);
        let twice = fwht(DType::U128, &once);
        let expected: Vec<i64> = x.iter().map(|v| v * 4).collect();
        assert_eq!(twice, expected);
    }

    #[test]
    fn wide_dot_matches_manual_modular_math() {
        // u128 dot, two elements of (2^64+1): (2^64+1)^2 + (2^64+1)^2
        // = 2·2^128 + 2^66 + 2 ≡ 2^66 + 2 (mod 2^128).
        // 2^66 + 2 = 2 + 4·2^64 → limbs [2, 4].
        let a: Vec<u64> = vec![1, 1, 1, 1];
        let out = wide_dot(&a, &a, 2, 2);
        assert_eq!(out, vec![2u64, 4]);
    }

    #[test]
    fn gate_is_identity_on_unsigned() {
        let input = vec![5i64, -3, 0, u64::MAX as i64];
        assert_eq!(gate(DType::U128, &input), input);
        assert_eq!(gate(DType::U64, &input), input);
        assert_eq!(gate(DType::U256, &input), input);
    }

    #[test]
    fn registry_earns_levels_from_registration() {
        let r = KernelRegistry::builtin();
        assert_eq!(r.guaranteed_level(&Kernel::Dot), Level::L3);
        assert_eq!(r.guaranteed_level(&Kernel::Fwht), Level::L3);
        assert_eq!(r.guaranteed_level(&Kernel::Gate), Level::L3);
        // Unregistered Custom: L1, no replay claim.
        assert_eq!(r.guaranteed_level(&Kernel::Custom("mystery".into())), Level::L1);

        // A user kernel earns L3 by registering a replay oracle...
        let mut r2 = KernelRegistry::builtin();
        r2.register("mystery", KernelSpec::new(Box::new(GateKernel), Some(Box::new(GateKernel))));
        assert_eq!(r2.guaranteed_level(&Kernel::Custom("mystery".into())), Level::L3);
        // ...and stays at L1 without one.
        let mut r3 = KernelRegistry::builtin();
        r3.register("opaque", KernelSpec::new(Box::new(GateKernel), None));
        assert_eq!(r3.guaranteed_level(&Kernel::Custom("opaque".into())), Level::L1);
    }

    #[test]
    fn effect_folds_registry_guarantees() {
        use crate::ir::{ProgramConfig, Stage, TensorType};
        let p = QProgram {
            name: "p".into(),
            param: "x".into(),
            param_ty: TensorType::new(DType::I64, vec![4]),
            output_ty: TensorType::new(DType::I64, vec![4]),
            stages: vec![
                Stage {
                    name: "a".into(),
                    kernel: Kernel::Dot,
                    inputs: vec!["x".into()],
                    weight: 1,
                },
                Stage {
                    name: "b".into(),
                    kernel: Kernel::Custom("opaque".into()),
                    inputs: vec!["a".into()],
                    weight: 2,
                },
            ],
            emit: "b".into(),
            demand: Level::L1,
            config: ProgramConfig::default(),
        };
        let r = KernelRegistry::builtin();
        assert_eq!(receipt_effect_with(&p, &r), Level::L1);
        // Register "opaque" with a replay oracle and the effect rises to L3.
        let mut r2 = KernelRegistry::builtin();
        r2.register("opaque", KernelSpec::new(Box::new(GateKernel), Some(Box::new(GateKernel))));
        assert_eq!(receipt_effect_with(&p, &r2), Level::L3);
    }
}
