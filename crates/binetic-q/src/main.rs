//! `binetic-q` CLI: compile / check / run a `.q` program.
//!
//! Prints one JSON object on stdout on success; errors go to stderr with a
//! non-zero exit code (mirrors the `binetic` CLI contract).

use std::path::PathBuf;

use binetic_evolver::LaneScaledThroughput;
use binetic_q::{check, compile, optimize, parser::Parser, receipt_effect, run, OptimizeConfig};
use serde::Serialize;

#[derive(Debug)]
enum Command {
    Check {
        source: PathBuf,
    },
    Compile {
        source: PathBuf,
    },
    Run {
        source: PathBuf,
    },
    Optimize {
        source: PathBuf,
        generations: usize,
        population: usize,
        seed: u64,
    },
}

fn parse_args() -> Result<Command, String> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let source = args.get(2).ok_or_else(|| {
        "usage: binetic-q <check|compile|run|optimize> <file.q> [flags]".to_string()
    })?;
    let source = PathBuf::from(source);
    let flag = |name: &str, default: &str| -> Result<String, String> {
        match args.iter().position(|a| a == name) {
            Some(i) => args
                .get(i + 1)
                .cloned()
                .ok_or_else(|| format!("{name} needs a value")),
            None => Ok(default.to_string()),
        }
    };
    match cmd {
        "check" => Ok(Command::Check { source }),
        "compile" => Ok(Command::Compile { source }),
        "run" => Ok(Command::Run { source }),
        "optimize" => Ok(Command::Optimize {
            source,
            generations: flag("--gen", "8")?
                .parse()
                .map_err(|_| "bad --gen".to_string())?,
            population: flag("--pop", "8")?
                .parse()
                .map_err(|_| "bad --pop".to_string())?,
            seed: flag("--seed", "5150")?
                .parse()
                .map_err(|_| "bad --seed".to_string())?,
        }),
        other => Err(format!(
            "unknown command {other:?}; use check|compile|run|optimize"
        )),
    }
}

#[derive(Serialize)]
struct CheckJson {
    program: String,
    stages: usize,
    demand: String,
    effect: String,
    ok: bool,
}

#[derive(Serialize)]
struct CompileJson {
    program: String,
    plan_hash: String,
    lanes: u16,
    tile_size: u32,
}

#[derive(Serialize)]
struct RunJson {
    program: String,
    plan_hash: String,
    chain_root: String,
    packets: usize,
    level0: bool,
    level1: bool,
    level2: bool,
    level3: bool,
    all_valid: bool,
}

#[derive(Serialize)]
struct OptimizeJson {
    program: String,
    generations: usize,
    population: usize,
    seed: u64,
    best_lane_width: u16,
    best_tile_size: u32,
    best_node_count: u32,
    best_genome_seed: u64,
    baseline_score: f64,
    best_score: f64,
    improved: bool,
    plan_hash: String,
    chain_root: String,
    receipt_bytes: u64,
    all_levels_valid: bool,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_source(path: &PathBuf) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))
}

fn main() {
    let result = real_main();
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<(), String> {
    let cmd = parse_args()?;
    match cmd {
        Command::Check { source } => {
            let src = read_source(&source)?;
            let p = Parser::parse(&src).map_err(|e| format!("parse: {e}"))?;
            check(&p).map_err(|e| format!("check: {e}"))?;
            println!(
                "{}",
                serde_json::to_string(&CheckJson {
                    program: p.name.clone(),
                    stages: p.stages.len(),
                    demand: p.demand.as_str().to_string(),
                    effect: receipt_effect(&p).as_str().to_string(),
                    ok: true,
                })
                .unwrap()
            );
        }
        Command::Compile { source } => {
            let src = read_source(&source)?;
            let p = Parser::parse(&src).map_err(|e| format!("parse: {e}"))?;
            let (h, cfg) = compile(&p).map_err(|e| format!("compile: {e}"))?;
            println!(
                "{}",
                serde_json::to_string(&CompileJson {
                    program: p.name.clone(),
                    plan_hash: hex(&h),
                    lanes: cfg.lane_width,
                    tile_size: cfg.tile_size,
                })
                .unwrap()
            );
        }
        Command::Run { source } => {
            let src = read_source(&source)?;
            let p = Parser::parse(&src).map_err(|e| format!("parse: {e}"))?;
            let r = run(&p).map_err(|e| format!("run: {e}"))?;
            println!(
                "{}",
                serde_json::to_string(&RunJson {
                    program: p.name,
                    plan_hash: hex(&r.plan_hash),
                    chain_root: hex(&r.chain_root),
                    packets: r.packet_count,
                    level0: r.report.level0,
                    level1: r.report.level1,
                    level2: r.report.level2,
                    level3: r.report.level3,
                    all_valid: r.report.all_valid,
                })
                .unwrap()
            );
        }
        Command::Optimize {
            source,
            generations,
            population,
            seed,
        } => {
            let src = read_source(&source)?;
            let p = Parser::parse(&src).map_err(|e| format!("parse: {e}"))?;
            let cfg = OptimizeConfig {
                seed,
                population,
                generations,
                ..OptimizeConfig::default()
            };
            let throughput = LaneScaledThroughput {
                base_tokens_per_second: 30.0,
                reference_lane_width: 8,
            };
            let r = optimize(&p, throughput, cfg.clone());
            println!(
                "{}",
                serde_json::to_string(&OptimizeJson {
                    program: p.name,
                    generations: r.generations,
                    population: r.population_size,
                    seed: cfg.seed,
                    best_lane_width: r.best_genome.lane_width,
                    best_tile_size: r.best_genome.tile_size,
                    best_node_count: r.best_genome.node_count,
                    best_genome_seed: r.best_genome.seed,
                    baseline_score: r.baseline_fitness.score,
                    best_score: r.best_fitness.score,
                    improved: r.improved(),
                    plan_hash: hex(&r.plan_hash),
                    chain_root: hex(&r.chain_root),
                    receipt_bytes: r.receipt_bytes,
                    all_levels_valid: r.verified_levels.iter().all(|&b| b),
                })
                .unwrap()
            );
        }
    }
    Ok(())
}
