//! `binetic-q` CLI: compile / check / run a `.q` program.
//!
//! Prints one JSON object on stdout on success; errors go to stderr with a
//! non-zero exit code (mirrors the `binetic` CLI contract).

use std::path::PathBuf;

use binetic_q::{check, compile, parser::Parser, run};
use serde::Serialize;

#[derive(Debug)]
enum Command {
    Check { source: PathBuf },
    Compile { source: PathBuf },
    Run { source: PathBuf },
}

fn parse_args() -> Result<Command, String> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let source = args
        .get(2)
        .ok_or_else(|| "usage: binetic-q <check|compile|run> <file.q>".to_string())?;
    let source = PathBuf::from(source);
    match cmd {
        "check" => Ok(Command::Check { source }),
        "compile" => Ok(Command::Compile { source }),
        "run" => Ok(Command::Run { source }),
        other => Err(format!("unknown command {other:?}; use check|compile|run")),
    }
}

#[derive(Serialize)]
struct CheckJson {
    program: String,
    stages: usize,
    demand: String,
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
    }
    Ok(())
}
