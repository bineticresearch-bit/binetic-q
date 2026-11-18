//! Q2 acceptance: a QGraph-aware fitness function runs real fabric receipts,
//! the evolver searches geometry against that verified score, and the whole
//! loop is deterministic and improves on the source config.

use binetic_evolver::{FitnessFn, Genome, LaneScaledThroughput};
use binetic_q::{
    optimize, parser::Parser, receipt_effect, run, Fitness, OptimizeConfig, QFitnessFn,
};

const PROGRAM: &str = include_str!("../examples/three_blocks.q");

fn model() -> LaneScaledThroughput {
    LaneScaledThroughput {
        base_tokens_per_second: 30.0,
        reference_lane_width: 8,
    }
}

fn config() -> OptimizeConfig {
    OptimizeConfig {
        seed: 5150,
        population: 8,
        generations: 10,
        ..OptimizeConfig::default()
    }
}

#[test]
fn q2_optimize_is_deterministic_and_receipt_verified() {
    let p = Parser::parse(PROGRAM).expect("parse");
    let cfg = config();

    let a = optimize(&p, model(), cfg.clone());
    let b = optimize(&p, model(), cfg);

    assert_eq!(a.best_genome, b.best_genome, "best genome is deterministic");
    assert_eq!(a.best_fitness.score, b.best_fitness.score);
    assert_eq!(a.plan_hash, b.plan_hash, "plan hash is deterministic");
    assert_eq!(a.chain_root, b.chain_root, "chain root is deterministic");
    assert_eq!(a.history.len(), b.history.len());
    assert!(
        a.verified_levels.iter().all(|&v| v),
        "best genome verifies L0-L3"
    );
    assert!(a.packet_count > 0 && a.receipt_bytes > 0);
    assert_eq!(a.generations, 10);
    assert_eq!(a.population_size, 8);

    // The best genome's config really is a valid, runnable Q program.
    assert!(a.best_genome.is_valid());
}

#[test]
fn q2_optimization_beats_the_source_config() {
    let p = Parser::parse(PROGRAM).expect("parse");
    let result = optimize(&p, model(), config());
    assert!(
        result.improved(),
        "best ({:.4}) must beat source baseline ({:.4})",
        result.best_fitness.score,
        result.baseline_fitness.score
    );
    // Throughput model rewards wider lanes; either the best width improved or
    // the receipt-byte penalty dropped via a larger tile.
    assert!(result.best_fitness.score > result.baseline_fitness.score);
}

#[test]
fn q2_fitness_gates_on_receipts_not_just_the_model() {
    let p = Parser::parse(PROGRAM).expect("parse");
    assert_eq!(receipt_effect(&p), binetic_q::Level::L3);

    let qfit = QFitnessFn::new(p.clone(), model(), 0.95, 32);
    let good = qfit.evaluate(&Genome::new(8, 8, 2, 7));

    // An unsupported lane width cannot run on the fabric; it must be floored
    // regardless of what the throughput model would have predicted.
    let invalid = qfit.evaluate(&Genome::new(2, 8, 2, 7));
    assert!(invalid.score < good.score);
    assert_eq!(invalid.verified_tokens_per_second, 0.0);

    // The run for a valid genome really produces a receipted result.
    let r = run(&p).expect("run");
    let verified = r.report.all_valid
        && r.report.level0
        && r.report.level1
        && r.report.level2
        && r.report.level3;
    assert!(verified);
    let _ = Fitness::from_measurements(0.95, 30.0, 1.0);
}
