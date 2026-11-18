//! Q4 acceptance: the program graph itself is the search space. Evolution
//! searches kernels, stage order, weights, pipeline length, and geometry —
//! and can only return a graph whose real fabric receipts verify at the
//! demanded level.

use binetic_q::{
    default_throughput, evolve, parse_intent, render_q, run, EvolveConfig, StructureGenome,
};

const INTENT: &str = include_str!("../examples/pipeline.intent");

fn initial_from_intent() -> StructureGenome {
    let intent = parse_intent(INTENT).expect("parse intent");
    let starting = binetic_q::build_program(&intent, Default::default());
    StructureGenome {
        kernels: starting.stages.iter().map(|s| s.kernel.clone()).collect(),
        weights: starting.stages.iter().map(|s| s.weight).collect(),
        lane_width: starting.config.lane_width,
        tile_size: starting.config.tile_size,
        node_count: starting.config.node_count,
        seed: starting.config.seed,
    }
}

fn evolve_default() -> binetic_q::EvolveResult {
    let intent = parse_intent(INTENT).expect("parse intent");
    evolve(
        &intent.name,
        intent.input.clone(),
        intent.output.clone(),
        intent.demand,
        &initial_from_intent(),
        default_throughput(),
        EvolveConfig::default(),
    )
}

#[test]
fn q4_evolution_returns_a_verified_graph() {
    let r = evolve_default();
    // The evolved graph verifies at every level — receipts, not caveats.
    assert!(r.verified_levels.iter().all(|&v| v));
    assert!(r.receipt_bytes > 0);
    assert!(r.best_fitness.score > 0.0);
    // Elitism: the best score never decreases across generations.
    for w in r.history.windows(2) {
        assert!(w[0] <= w[1]);
    }
}

#[test]
fn q4_evolved_source_recompiles_with_the_same_identity() {
    let r = evolve_default();
    // The emitted .q source is a first-class artifact: it re-parses and runs
    // with the same plan hash and chain root the search recorded.
    let source = render_q(&r.program);
    let p = binetic_q::parser::Parser::parse(&source).expect("reparse evolved source");
    let run = run(&p).expect("run evolved program");
    assert_eq!(run.plan_hash, r.plan_hash);
    assert_eq!(run.chain_root, r.chain_root);
    assert!(run.report.all_valid);
    assert!(run.report.level3);
}

#[test]
fn q4_is_deterministic() {
    let a = evolve_default();
    let b = evolve_default();
    assert_eq!(a.best, b.best);
    assert_eq!(a.plan_hash, b.plan_hash);
    assert_eq!(a.chain_root, b.chain_root);
    assert_eq!(a.history, b.history);
}

#[test]
fn q4_search_never_loses_the_baseline() {
    // Elitism over the baseline: the final verified score is at least the
    // author's starting pipeline, and the final graph verifies at demand.
    let intent = parse_intent(INTENT).expect("parse intent");
    let cfg = EvolveConfig {
        generations: 12,
        population: 16,
        ..EvolveConfig::default()
    };
    let r = evolve(
        &intent.name,
        intent.input.clone(),
        intent.output.clone(),
        intent.demand,
        &initial_from_intent(),
        default_throughput(),
        cfg,
    );
    assert!(r.best_fitness.score >= r.baseline_fitness.score);
    assert!(r.verified_levels.iter().all(|&v| v));
}
