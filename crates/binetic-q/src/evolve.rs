//! Q4 — graph evolution: the program graph itself is the search space.
//!
//! Q2 evolved *geometry* — how to execute a fixed QGraph. Q4 evolves the
//! *graph*: kernel choice per stage, stage weights, pipeline length and
//! order, and the execution geometry. The only fixed points are the
//! contract the intent sets: port types and the receipt demand.
//!
//! Every candidate is compiled and executed on the real fabric and scored
//! only when its receipts verify at the demanded level, so evolution can
//! never emit a program that overclaims its receipts — an impossible
//! candidate (e.g. a Custom stage under an L3 demand) scores at the invalid
//! floor, exactly like Q2's invalid genomes.
//!
//! The search is deterministic: SplitMix64 via `binetic_evolver::
//! DeterministicRng`, no clock, no thread state. Same seed + config +
//! contract always yields the same evolved graph, byte for byte.
//!
//! The fitness model is caller-chosen (any `ThroughputModel`); the score
//! formula is the evolver's canonical `100·accuracy + ln(1+tps) − 0.01·bytes`.

use std::cmp::Ordering;

use binetic_evolver::{
    DeterministicRng, Fitness, Genome, ThroughputModel, INVALID_GENOME_RECEIPT_BYTES,
    LANE_WIDTHS, MAX_NODE_COUNT, MAX_TILE_SIZE, MIN_NODE_COUNT, MIN_TILE_SIZE,
};

use crate::ir::{Kernel, Level, ProgramConfig, QProgram, Stage, TensorType};
use crate::kernels::KernelRegistry;
use crate::{check_with, run_with, RunResult};

/// Mutation strategy for graph evolution. The default searches over the
/// registry's kernel set with the built-in graph operators; apps (or an LLM
/// adapter) can supply their own operators to bias search over their own
/// kernel registries. Implementations must be deterministic in `rng`.
pub trait MutationFn {
    fn mutate(
        &self,
        genome: &StructureGenome,
        rng: &mut DeterministicRng,
        max_stages: usize,
        kernels: &[Kernel],
    ) -> StructureGenome;
}

/// The built-in operators: retarget a kernel, rescale a weight, insert a
/// stage, delete a stage, swap two stages, or mutate one geometry field.
pub struct DefaultMutation;

impl MutationFn for DefaultMutation {
    fn mutate(
        &self,
        genome: &StructureGenome,
        rng: &mut DeterministicRng,
        max_stages: usize,
        kernels: &[Kernel],
    ) -> StructureGenome {
        genome.mutate(rng, max_stages, kernels)
    }
}

/// The evolvable part of a program: kernel pipeline + weights + geometry +
/// skip-connection wiring.
///
/// `skip[i]` is an optional index `j < i` meaning stage `i` takes *both* its
/// predecessor's output *and* stage `j`'s output as inputs — a residual
/// (skip) connection. `None` means stage `i` is a simple linear stage.
/// This is the minimal DAG extension: every pipeline is a valid genome with
/// all-`None` skips, and the evolver can add/remove/retarget wires without
/// changing any pipeline-only machinery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureGenome {
    pub kernels: Vec<Kernel>,
    pub weights: Vec<i64>,
    /// Per-stage optional skip-connection source index. `skip[i] = Some(j)`
    /// adds stage `j`'s output as a second input to stage `i` (j < i).
    pub skip: Vec<Option<usize>>,
    pub lane_width: u16,
    pub tile_size: u32,
    pub node_count: u32,
    pub seed: u64,
}

/// Canonical ordinal for deterministic tie-breaking.
fn kernel_ord(k: &Kernel) -> (u8, String) {
    match k {
        Kernel::Dot => (0, String::new()),
        Kernel::Fwht => (1, String::new()),
        Kernel::Gate => (2, String::new()),
        Kernel::Custom(s) => (3, s.clone()),
    }
}

impl StructureGenome {
    /// Total order over structure genomes: pipeline, then geometry. This is
    /// the deterministic tie-breaker used for selection, mirroring the
    /// evolver's genome field order.
    fn cmp_structure(&self, other: &Self) -> Ordering {
        let ka: Vec<(u8, String)> = self.kernels.iter().map(kernel_ord).collect();
        let kb: Vec<(u8, String)> = other.kernels.iter().map(kernel_ord).collect();
        // Compare skip as Vec<u8>: None → 0xFF, Some(j) → j as u8 + 1.
        let sa: Vec<u8> = self.skip.iter().map(|s| s.map(|j| j as u8 + 1).unwrap_or(0xFF)).collect();
        let sb: Vec<u8> = other.skip.iter().map(|s| s.map(|j| j as u8 + 1).unwrap_or(0xFF)).collect();
        ka.cmp(&kb)
            .then_with(|| self.weights.cmp(&other.weights))
            .then_with(|| sa.cmp(&sb))
            .then_with(|| self.lane_width.cmp(&other.lane_width))
            .then_with(|| self.tile_size.cmp(&other.tile_size))
            .then_with(|| self.node_count.cmp(&other.node_count))
            .then_with(|| self.seed.cmp(&other.seed))
    }

    fn random(rng: &mut DeterministicRng, kernels: &[Kernel], max_stages: usize) -> Self {
        let n = 1 + rng.index(max_stages);
        // Random genomes start with no skip connections; the mutator adds them.
        Self {
            kernels: (0..n)
                .map(|_| kernels[rng.index(kernels.len())].clone())
                .collect(),
            weights: (0..n).map(|_| (rng.next_u64() % 997) as i64).collect(),
            skip: vec![None; n],
            lane_width: LANE_WIDTHS[rng.index(LANE_WIDTHS.len())],
            tile_size: (MIN_TILE_SIZE
                + (rng.next_u64() % (MAX_TILE_SIZE - MIN_TILE_SIZE + 1) as u64) as u32),
            node_count: (MIN_NODE_COUNT
                + (rng.next_u64() % (MAX_NODE_COUNT - MIN_NODE_COUNT + 1) as u64) as u32),
            seed: rng.next_u64(),
        }
    }

    /// One mutation: exactly one graph or geometry operator is applied.
    /// Operators 0–4 are pipeline/topology mutations; operator 5 is geometry;
    /// operators 6–7 are skip-connection mutations (add/remove/retarget).
    fn mutate(
        &self,
        rng: &mut DeterministicRng,
        max_stages: usize,
        kernels: &[Kernel],
    ) -> Self {
        let mut child = self.clone();
        match rng.index(8) {
            0 => {
                // Retarget a stage's kernel.
                let i = rng.index(child.kernels.len());
                child.kernels[i] = kernels[rng.index(kernels.len())].clone();
            }
            1 => {
                // Rescale a stage's weight.
                let i = rng.index(child.weights.len());
                child.weights[i] = (rng.next_u64() % 997) as i64;
            }
            2 => {
                // Grow the pipeline: insert a stage anywhere.
                if child.kernels.len() < max_stages {
                    let i = rng.index(child.kernels.len() + 1);
                    child.kernels.insert(i, kernels[rng.index(kernels.len())].clone());
                    child.weights.insert(i, (rng.next_u64() % 997) as i64);
                    child.skip.insert(i, None);
                    // Any existing skip that pointed at or past i must shift.
                    for s in child.skip.iter_mut() {
                        if let Some(j) = s {
                            if *j >= i {
                                *j += 1;
                            }
                        }
                    }
                }
            }
            3 => {
                // Shrink the pipeline.
                if child.kernels.len() > 1 {
                    let i = rng.index(child.kernels.len());
                    child.kernels.remove(i);
                    child.weights.remove(i);
                    child.skip.remove(i);
                    // Clear any skip that pointed at the removed stage or now
                    // has an invalid source (would be its own predecessor or later).
                    for (idx, s) in child.skip.iter_mut().enumerate() {
                        if let Some(j) = *s {
                            if j >= i || j >= idx {
                                *s = None;
                            }
                        }
                    }
                }
            }
            4 => {
                // Rewire: swap two stages (graph order is semantics).
                if child.kernels.len() > 1 {
                    let i = rng.index(child.kernels.len());
                    let mut j = rng.index(child.kernels.len());
                    if j == i {
                        j = (j + 1) % child.kernels.len();
                    }
                    child.kernels.swap(i, j);
                    child.weights.swap(i, j);
                    // After a swap, skip connections may be invalid; clear them
                    // for safety (the mutator will re-add valid ones later).
                    child.skip[i] = None;
                    child.skip[j] = None;
                }
            }
            5 => {
                // Geometry: one field, mirroring the evolver's mutation rules.
                match rng.index(4) {
                    0 => child.lane_width = LANE_WIDTHS[rng.index(LANE_WIDTHS.len())],
                    1 => {
                        let d = (rng.next_u64() % 9) as i64 - 4;
                        child.tile_size = clamp_u32(
                            child.tile_size as i64 + d,
                            MIN_TILE_SIZE,
                            MAX_TILE_SIZE,
                        );
                    }
                    2 => {
                        let d = (rng.next_u64() % 3) as i64 - 1;
                        child.node_count = clamp_u32(
                            child.node_count as i64 + d,
                            MIN_NODE_COUNT,
                            MAX_NODE_COUNT,
                        );
                    }
                    _ => {
                        let mix = rng.next_u64();
                        child.seed = if child.seed ^ mix == child.seed {
                            child.seed.wrapping_add(1)
                        } else {
                            child.seed ^ mix
                        };
                    }
                }
            }
            6 => {
                // Add or retarget a skip connection. Pick a stage i > 0 and
                // wire it to some earlier stage j < i.
                if child.kernels.len() > 1 {
                    let i = 1 + rng.index(child.kernels.len() - 1); // 1..n-1
                    let j = rng.index(i); // 0..i-1
                    child.skip[i] = Some(j);
                }
            }
            _ => {
                // Remove a skip connection: clear one that is currently set.
                let candidates: Vec<usize> = child
                    .skip
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.is_some())
                    .map(|(i, _)| i)
                    .collect();
                if !candidates.is_empty() {
                    let i = candidates[rng.index(candidates.len())];
                    child.skip[i] = None;
                }
            }
        }
        child
    }

    /// Lower this structure to a `QProgram` under the fixed contract.
    ///
    /// A stage with `skip[i] = Some(j)` takes two inputs: its linear
    /// predecessor's output *and* stage `j`'s output — a residual connection.
    /// The lowerer concatenates multi-input values before passing to the kernel.
    pub fn program(
        &self,
        name: &str,
        param_ty: &TensorType,
        output_ty: &TensorType,
        demand: Level,
    ) -> QProgram {
        // Build the name sequence: "x", "s0", "s1", ...
        let stage_names: Vec<String> = (0..self.kernels.len()).map(|i| format!("s{i}")).collect();

        let mut stages = Vec::new();
        for (i, ((kernel, weight), stage_name)) in self
            .kernels
            .iter()
            .zip(self.weights.iter())
            .zip(stage_names.iter())
            .enumerate()
        {
            // Primary input: the previous stage's output (or the program param).
            let prev = if i == 0 {
                "x".to_string()
            } else {
                stage_names[i - 1].clone()
            };
            let mut inputs = vec![prev];

            // Skip connection: add an earlier stage's output as a second input.
            // skip[i] = Some(j) means stage j's output ("s{j}") feeds stage i.
            if let Some(j) = self.skip[i] {
                if j < i {
                    let src = stage_names[j].clone(); // stage j's output is "s{j}"
                    // Only add if distinct from the primary input.
                    if !inputs.contains(&src) {
                        inputs.push(src);
                    }
                }
            }

            stages.push(Stage {
                name: stage_name.clone(),
                kernel: kernel.clone(),
                inputs,
                weight: *weight,
            });
        }

        let emit = stage_names.last().cloned().unwrap_or_else(|| "x".into());
        QProgram {
            name: name.to_string(),
            param: "x".into(),
            param_ty: param_ty.clone(),
            output_ty: output_ty.clone(),
            stages,
            emit,
            demand,
            config: ProgramConfig {
                lane_width: self.lane_width,
                tile_size: self.tile_size,
                node_count: self.node_count,
                seed: self.seed,
                ..ProgramConfig::default()
            },
        }
    }
}

fn clamp_u32(v: i64, lo: u32, hi: u32) -> u32 {
    v.clamp(lo as i64, hi as i64) as u32
}

/// Scores a structure genome by verified throughput on the real fabric.
/// The fitness model (`T`) is caller-chosen; only receipt-verified runs
/// score above the invalid floor. Guarantees come from the registry.
pub struct GraphFitnessFn<T: ThroughputModel> {
    name: String,
    param_ty: TensorType,
    output_ty: TensorType,
    demand: Level,
    registry: KernelRegistry,
    throughput: T,
    model_accuracy: f64,
    tokens: u32,
}

impl<T: ThroughputModel> GraphFitnessFn<T> {
    pub fn new(
        name: impl Into<String>,
        param_ty: TensorType,
        output_ty: TensorType,
        demand: Level,
        registry: KernelRegistry,
        throughput: T,
        model_accuracy: f64,
        tokens: u32,
    ) -> Self {
        Self {
            name: name.into(),
            param_ty,
            output_ty,
            demand,
            registry,
            throughput,
            model_accuracy,
            tokens,
        }
    }

    fn invalid() -> Fitness {
        Fitness::from_measurements(0.0, 0.0, INVALID_GENOME_RECEIPT_BYTES)
    }

    /// Evaluate a structure genome; `Some(RunResult)` only when the receipts
    /// verified at the demanded level.
    pub fn evaluate_detailed(&self, genome: &StructureGenome) -> (Fitness, Option<RunResult>) {
        let program =
            genome.program(&self.name, &self.param_ty, &self.output_ty, self.demand);
        if check_with(&program, &self.registry).is_err() {
            return (Self::invalid(), None);
        }
        let r = match run_with(&program, &self.registry) {
            Ok(r) => r,
            Err(_) => return (Self::invalid(), None),
        };
        let verified = r.report.all_valid
            && match self.demand {
                Level::L0 => r.report.level0,
                Level::L1 => r.report.level1,
                Level::L2 => r.report.level2,
                Level::L3 => r.report.level3,
            };
        if !verified {
            return (Self::invalid(), None);
        }
        let geometry = Genome::new(
            genome.lane_width,
            genome.tile_size,
            genome.node_count,
            genome.seed,
        );
        let tokens = self.tokens.max(1) as f64;
        let tps = self.throughput.tokens_per_second(&geometry).max(0.0);
        let bytes = r.receipt_bytes as f64 / tokens;
        (
            Fitness::from_measurements(self.model_accuracy, tps, bytes),
            Some(r),
        )
    }

    pub fn evaluate(&self, genome: &StructureGenome) -> Fitness {
        self.evaluate_detailed(genome).0
    }
}

/// Configuration for a graph evolution search.
#[derive(Debug, Clone)]
pub struct EvolveConfig {
    pub seed: u64,
    pub population: usize,
    pub generations: usize,
    /// Upper bound on pipeline length the search may grow to.
    pub max_stages: usize,
    pub model_accuracy: f64,
    pub tokens: u32,
}

impl Default for EvolveConfig {
    fn default() -> Self {
        Self {
            seed: 5150,
            population: 8,
            generations: 8,
            max_stages: 4,
            model_accuracy: 0.95,
            tokens: 32,
        }
    }
}

/// Result of a graph evolution search, with receipts for the best graph.
#[derive(Debug, Clone)]
pub struct EvolveResult {
    pub best: StructureGenome,
    pub best_fitness: Fitness,
    pub baseline_fitness: Fitness,
    pub program: QProgram,
    pub plan_hash: [u8; 32],
    pub chain_root: [u8; 32],
    pub receipt_bytes: u64,
    pub packet_count: usize,
    pub verified_levels: [bool; 4],
    /// Best score per generation (monotone non-decreasing: elitism).
    pub history: Vec<f64>,
    pub generations: usize,
    pub population_size: usize,
}

impl EvolveResult {
    pub fn improved(&self) -> bool {
        self.best_fitness.score > self.baseline_fitness.score
    }
}

/// Evolve the program graph under a fixed contract (port types + receipt
/// demand), using the default kernel registry and the built-in mutation
/// operators. Deterministic: same seed/config/contract => same result.
///
/// Selection is elitist — the best genome is never discarded — so `history`
/// is monotone non-decreasing and the final `best` is at least as good as
/// every candidate ever seen.
pub fn evolve<T: ThroughputModel + Clone>(
    name: &str,
    param_ty: TensorType,
    output_ty: TensorType,
    demand: Level,
    initial: &StructureGenome,
    throughput: T,
    config: EvolveConfig,
) -> EvolveResult {
    evolve_with(
        name,
        param_ty,
        output_ty,
        demand,
        initial,
        throughput,
        config,
        KernelRegistry::default(),
        &DefaultMutation,
    )
}

/// Full form: the search runs under a caller-supplied kernel registry
/// (guarantees are earned there — a kernel the caller registered with a
/// replay oracle becomes searchable *and* verifiable) with a caller-supplied
/// mutation strategy. Deterministic and elitist, exactly like [`evolve`].
pub fn evolve_with<T: ThroughputModel + Clone>(
    name: &str,
    param_ty: TensorType,
    output_ty: TensorType,
    demand: Level,
    initial: &StructureGenome,
    throughput: T,
    config: EvolveConfig,
    registry: KernelRegistry,
    mutation: &dyn MutationFn,
) -> EvolveResult {
    let kernels = registry.searchable();
    let fitness = GraphFitnessFn::new(
        name,
        param_ty.clone(),
        output_ty.clone(),
        demand,
        registry,
        throughput,
        config.model_accuracy,
        config.tokens,
    );

    // Baseline = the author's starting graph, so "improvement" means the
    // search beat the hand-picked pipeline on the same verified score.
    let baseline_fitness = fitness.evaluate(initial);

    let mut rng = DeterministicRng::new(config.seed);
    let population = config.population.max(2);
    let mut pop: Vec<StructureGenome> = (0..population)
        .map(|_| StructureGenome::random(&mut rng, &kernels, config.max_stages))
        .collect();

    let mut best = initial.clone();
    let mut best_fitness = baseline_fitness;
    let mut history = Vec::new();

    for _ in 0..config.generations {
        // Rank the population: verified score desc, structure order asc.
        let mut scored: Vec<(StructureGenome, Fitness)> = pop
            .drain(..)
            .map(|g| {
                let f = fitness.evaluate(&g);
                (g, f)
            })
            .collect();
        scored.sort_by(|(ga, fa), (gb, fb)| {
            fb.score
                .total_cmp(&fa.score)
                .then_with(|| ga.cmp_structure(gb))
        });

        if let Some((g, f)) = scored.first() {
            if f.score.total_cmp(&best_fitness.score) == Ordering::Greater
                || (f.score.total_cmp(&best_fitness.score) == Ordering::Equal
                    && g.cmp_structure(&best) == Ordering::Less)
            {
                best = g.clone();
                best_fitness = *f;
            }
        }
        history.push(best_fitness.score);

        // Next generation: keep the top half (elitism), refill half with
        // mutations of the current best and half with random restarts.
        let keep = scored.len() / 2;
        let mut next: Vec<StructureGenome> = scored.into_iter().take(keep).map(|(g, _)| g).collect();
        while next.len() < population {
            let restart = next.len() % 2 == 1;
            next.push(if restart {
                StructureGenome::random(&mut rng, &kernels, config.max_stages)
            } else {
                mutation.mutate(&best, &mut rng, config.max_stages, &kernels)
            });
        }
        pop = next;
    }

    // Final evidence for the best graph: real receipts, real plan identity.
    let (best_fitness, evidence) = fitness.evaluate_detailed(&best);
    let program = best.program(name, &param_ty, &output_ty, demand);
    let (plan_hash, chain_root, receipt_bytes, packet_count, verified_levels) = match evidence {
        Some(r) => (
            r.plan_hash,
            r.chain_root,
            r.receipt_bytes,
            r.packet_count,
            [
                r.report.level0,
                r.report.level1,
                r.report.level2,
                r.report.level3,
            ],
        ),
        None => ([0u8; 32], [0u8; 32], 0, 0, [false; 4]),
    };

    EvolveResult {
        best,
        best_fitness,
        baseline_fitness,
        program,
        plan_hash,
        chain_root,
        receipt_bytes,
        packet_count,
        verified_levels,
        history,
        generations: config.generations,
        population_size: population,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fitness::default_throughput;
    use crate::ir::{DType, Level, TensorType};

    fn contract() -> (TensorType, TensorType, Level) {
        (
            TensorType::new(DType::I64, vec![8]),
            TensorType::new(DType::I64, vec![8]),
            Level::L3,
        )
    }

    fn initial() -> StructureGenome {
        StructureGenome {
            kernels: vec![Kernel::Dot, Kernel::Gate],
            weights: vec![3, 9],
            skip: vec![None, None],
            lane_width: 8,
            tile_size: 8,
            node_count: 2,
            seed: 7,
        }
    }

    #[test]
    fn evolution_is_deterministic() {
        let (p, o, d) = contract();
        let a = evolve("t", p.clone(), o.clone(), d, &initial(), default_throughput(), EvolveConfig::default());
        let b = evolve("t", p, o, d, &initial(), default_throughput(), EvolveConfig::default());
        assert_eq!(a.best, b.best);
        assert_eq!(a.plan_hash, b.plan_hash);
        assert_eq!(a.chain_root, b.chain_root);
        assert_eq!(a.history, b.history);
    }

    #[test]
    fn evolved_graph_verifies_at_demand() {
        let (p, o, d) = contract();
        let r = evolve("t", p, o, d, &initial(), default_throughput(), EvolveConfig::default());
        assert!(r
            .verified_levels
            .iter()
            .all(|&v| v),
            "evolved graph must verify at every level");
        assert!(r.best_fitness.score > 0.0);
        assert!(r.packet_count >= 1);
    }

    #[test]
    fn history_is_monotone_under_elitism() {
        let (p, o, d) = contract();
        let r = evolve("t", p, o, d, &initial(), default_throughput(), EvolveConfig::default());
        for w in r.history.windows(2) {
            assert!(w[0] <= w[1], "elitism must never lose the best: {:?})", r.history);
        }
    }

    #[test]
    fn mutation_is_deterministic_and_bounded() {
        let mut rng = DeterministicRng::new(42);
        let parent = initial();
        let ks = KernelRegistry::builtin().searchable();
        for _ in 0..64 {
            let child = parent.mutate(&mut rng, 4, &ks);
            assert!(!child.kernels.is_empty());
            assert!(child.kernels.len() <= 4);
            assert_eq!(child.kernels.len(), child.weights.len());
            assert_eq!(child.kernels.len(), child.skip.len());
            assert!(LANE_WIDTHS.contains(&child.lane_width));
            // All skip connections must be valid: skip[i] < i.
            for (i, s) in child.skip.iter().enumerate() {
                if let Some(j) = s {
                    assert!(*j < i, "skip[{i}] = {j} must be < {i}");
                }
            }
        }
    }

    #[test]
    fn evolution_searches_a_registered_custom_kernel() {
        // A user kernel earns L3 by registering a replay oracle — and the
        // search may then place it in evolved graphs.
        use crate::kernels::{GateKernel, KernelSpec};
        let mut registry = KernelRegistry::builtin();
        registry.register(
            "mygate",
            KernelSpec::new(Box::new(GateKernel), Some(Box::new(GateKernel))),
        );
        assert!(registry.searchable().contains(&Kernel::Custom("mygate".into())));
        let (p, o, d) = contract();
        let r = evolve_with(
            "t",
            p,
            o,
            d,
            &initial(),
            default_throughput(),
            EvolveConfig {
                generations: 4,
                population: 8,
                ..EvolveConfig::default()
            },
            registry,
            &DefaultMutation,
        );
        // Whatever the search found, it verifies at the demand under the
        // registry that earned the guarantees.
        assert!(r.verified_levels.iter().all(|&v| v));
    }
}
