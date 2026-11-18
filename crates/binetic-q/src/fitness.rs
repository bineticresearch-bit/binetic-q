//! Q2 — self-optimization: a QGraph-aware fitness function.
//!
//! A genome is fabric geometry (`lane_width`, `tile_size`, `node_count`, `seed`).
//! `QFitnessFn` applies a genome to a Q program template, compiles and runs it
//! on a REAL fabric, and scores it only if the emitted receipts verify at the
//! program's demanded level. Throughput comes from a pure model (no clock), so
//! the whole loop stays deterministic and reproducible. The evolver then searches
//! this verified-throughput landscape — the first implemented "intelligence"
//! step in Q.

use binetic_evolver::{
    Evolver, Fitness, FitnessFn, Genome, LaneScaledThroughput, ThroughputModel,
    INVALID_GENOME_RECEIPT_BYTES,
};

use crate::ir::{Level, QProgram};
use crate::{check, run, RunResult};

/// The default pure throughput model: 30 tokens/s at a reference 8-lane
/// width. Shared by the CLI and Q3/Q4 synthesis so every caller searches one
/// deterministic landscape instead of per-caller magic numbers. Callers can
/// still pass any `ThroughputModel` (fixed rate, measured lookup, ...).
pub fn default_throughput() -> LaneScaledThroughput {
    LaneScaledThroughput {
        base_tokens_per_second: 30.0,
        reference_lane_width: 8,
    }
}

fn invalid() -> Fitness {
    Fitness::from_measurements(0.0, 0.0, INVALID_GENOME_RECEIPT_BYTES)
}

/// Fitness of a compiled QGraph under one fabric geometry.
#[derive(Debug, Clone)]
pub struct QFitnessFn<T: ThroughputModel> {
    template: QProgram,
    throughput: T,
    model_accuracy: f64,
    tokens: u32,
}

impl<T: ThroughputModel> QFitnessFn<T> {
    pub fn new(template: QProgram, throughput: T, model_accuracy: f64, tokens: u32) -> Self {
        Self {
            template,
            throughput,
            model_accuracy,
            tokens,
        }
    }

    /// Overlay a genome's geometry onto the program template. Everything else
    /// (operators, pipeline shape, receipt demand) stays fixed, so the search
    /// is over HOW to execute the same QGraph, not over a different program.
    pub fn apply_genome(&self, genome: &Genome) -> QProgram {
        let mut p = self.template.clone();
        p.config.lane_width = genome.lane_width;
        p.config.tile_size = genome.tile_size;
        p.config.node_count = genome.node_count;
        p.config.seed = genome.seed;
        p
    }

    /// Like `evaluate`, but also returns the full run evidence when the
    /// receipts verified at the demanded level. Used for recorded evidence.
    pub fn evaluate_detailed(&self, genome: &Genome) -> (Fitness, Option<RunResult>) {
        if !genome.is_valid() {
            return (invalid(), None);
        }
        let program = self.apply_genome(genome);
        if check(&program).is_err() {
            return (invalid(), None);
        }
        let r = match run(&program) {
            Ok(r) => r,
            Err(_) => return (invalid(), None),
        };

        let verified = r.report.all_valid
            && match program.demand {
                Level::L0 => r.report.level0,
                Level::L1 => r.report.level1,
                Level::L2 => r.report.level2,
                Level::L3 => r.report.level3,
            };
        if !verified {
            return (invalid(), None);
        }

        let tokens = self.tokens.max(1) as f64;
        let tps = self.throughput.tokens_per_second(genome).max(0.0);
        let bytes = r.receipt_bytes as f64 / tokens;
        (
            Fitness::from_measurements(self.model_accuracy, tps, bytes),
            Some(r),
        )
    }
}

impl<T: ThroughputModel> FitnessFn for QFitnessFn<T> {
    fn evaluate(&self, genome: &Genome) -> Fitness {
        self.evaluate_detailed(genome).0
    }
}

/// Configuration for a verified-throughput search.
#[derive(Debug, Clone)]
pub struct OptimizeConfig {
    pub seed: u64,
    pub population: usize,
    pub generations: usize,
    pub model_accuracy: f64,
    pub tokens: u32,
}

impl Default for OptimizeConfig {
    fn default() -> Self {
        Self {
            seed: 5150,
            population: 8,
            generations: 8,
            model_accuracy: 0.95,
            tokens: 32,
        }
    }
}

/// Result of a verified-throughput search, with receipts for the best genome.
#[derive(Debug, Clone)]
pub struct OptimizeResult {
    pub best_genome: Genome,
    pub best_fitness: Fitness,
    pub baseline_fitness: Fitness,
    pub history: Vec<Fitness>,
    pub generations: usize,
    pub population_size: usize,
    pub plan_hash: [u8; 32],
    pub chain_root: [u8; 32],
    pub receipt_bytes: u64,
    pub packet_count: usize,
    pub verified_levels: [bool; 4],
}

impl OptimizeResult {
    pub fn improved(&self) -> bool {
        self.best_fitness.score > self.baseline_fitness.score
    }
}

/// Evolve fabric geometry for a Q program, scoring only receipt-verified runs.
pub fn optimize<T: ThroughputModel + Clone>(
    program: &QProgram,
    throughput: T,
    config: OptimizeConfig,
) -> OptimizeResult {
    let fitness = QFitnessFn::new(
        program.clone(),
        throughput,
        config.model_accuracy,
        config.tokens,
    );

    // Baseline = the geometry written in the source, so "improvement" means
    // the search beat the author's hand-picked config on the same score.
    let baseline = Genome::new(
        program.config.lane_width,
        program.config.tile_size,
        program.config.node_count,
        program.config.seed,
    );
    let baseline_fitness = fitness.evaluate(&baseline);

    let mut evolver = Evolver::new(fitness.clone(), config.seed, config.population);
    let evolved = evolver.run(config.generations);
    let best_genome = evolved.best_genome;
    let best_fitness = evolved.best_fitness;
    let (_, evidence) = fitness.evaluate_detailed(&best_genome);

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

    OptimizeResult {
        best_genome,
        best_fitness,
        baseline_fitness,
        history: evolved.history,
        generations: evolved.generations,
        population_size: evolved.population_size,
        plan_hash,
        chain_root,
        receipt_bytes,
        packet_count,
        verified_levels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;
    use binetic_evolver::LaneScaledThroughput;

    const P: &str = r#"
@receipt L3
qblock pipeline(x: i64[8]) -> i64[8] {
  config { model_id = 11; lane_width = 8; tile_size = 8; node_count = 2; seed = 7; }
  let h = dot(x, 3);
  let g = h |> dot(5);
  let y = g |> gate(9);
  emit y;
}
"#;

    fn model() -> LaneScaledThroughput {
        default_throughput()
    }

    #[test]
    fn fitness_is_deterministic_for_a_genome() {
        let p = Parser::parse(P).unwrap();
        let f = QFitnessFn::new(p, model(), 0.95, 32);
        let g = Genome::new(8, 8, 2, 7);
        let a = f.evaluate(&g);
        let b = f.evaluate(&g);
        assert_eq!(a, b);
        assert!(
            a.score > 0.0,
            "a verified run scores above the invalid floor"
        );
    }

    #[test]
    fn invalid_genome_is_penalized() {
        let p = Parser::parse(P).unwrap();
        let f = QFitnessFn::new(p, model(), 0.95, 32);
        let bad = Genome::new(2, 8, 2, 7); // lane_width 2 is not supported
        assert!(!bad.is_valid());
        let fit = f.evaluate(&bad);
        assert_eq!(fit.verified_tokens_per_second, 0.0);
        assert_eq!(fit.receipt_bytes_per_token, INVALID_GENOME_RECEIPT_BYTES);
        let good = f.evaluate(&Genome::new(8, 8, 2, 7));
        assert!(fit.score < good.score);
    }

    #[test]
    fn detail_returns_real_receipt_evidence() {
        let p = Parser::parse(P).unwrap();
        let f = QFitnessFn::new(p, model(), 0.95, 32);
        let (_, evidence) = f.evaluate_detailed(&Genome::new(8, 8, 2, 7));
        let r = evidence.expect("verified run");
        assert_eq!(r.packet_count, 3);
        assert!(r.receipt_bytes > 0);
        assert!(r.report.level0 && r.report.level1 && r.report.level2 && r.report.level3);
    }
}
