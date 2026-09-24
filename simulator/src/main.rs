//! Simulator entrypoint for deterministic consensus and fault-injection runs.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::env;
use std::process::ExitCode;

use simulator::scenario::{ScenarioReport, ScenarioRunner, ScenarioType};

fn print_usage() {
    eprintln!(
        "Usage: simulator [OPTIONS]\n\
         \n\
         Options:\n\
         \x20 --seed <u64>         Deterministic PRNG seed (default: 42)\n\
         \x20 --steps <u64>        Maximum simulated virtual ticks (default: 200)\n\
         \x20 --scenario <name>    Scenario to run: partition | crash | chaos | all (default: all)\n\
         \x20 --fuzz <count>       Run fuzz campaign across N consecutive seeds\n\
         \x20 --help, -h           Show this help message"
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let mut seed = 42u64;
    let mut steps = 200u64;
    let mut scenario_arg = "all".to_string();
    let mut fuzz_count = None;

    let mut i = 1;
    while i < args.len() {
        let Some(current_arg) = args.get(i) else {
            break;
        };
        match current_arg.as_str() {
            "--seed" => {
                if let Some(val) = args.get(i + 1) {
                    seed = val.parse().unwrap_or(42);
                    i += 2;
                } else {
                    print_usage();
                    return ExitCode::FAILURE;
                }
            }
            "--steps" => {
                if let Some(val) = args.get(i + 1) {
                    steps = val.parse().unwrap_or(200);
                    i += 2;
                } else {
                    print_usage();
                    return ExitCode::FAILURE;
                }
            }
            "--scenario" => {
                if let Some(val) = args.get(i + 1) {
                    scenario_arg = val.clone();
                    i += 2;
                } else {
                    print_usage();
                    return ExitCode::FAILURE;
                }
            }
            "--fuzz" => {
                if let Some(val) = args.get(i + 1) {
                    fuzz_count = Some(val.parse().unwrap_or(20));
                    i += 2;
                } else {
                    print_usage();
                    return ExitCode::FAILURE;
                }
            }
            "--help" | "-h" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("Unknown option: {current_arg}");
                print_usage();
                return ExitCode::FAILURE;
            }
        }
    }

    println!("================================================================================");
    println!(" TrustLedger Deterministic Simulation Testing (DST) Harness");
    println!(" Method: Deterministic discrete-event seeded pseudo-random injection");
    println!("         reproducing partitions, crashes, packet loss, duplicates, and recovery");
    println!("================================================================================");

    if let Some(count) = fuzz_count {
        println!("==> Starting fuzz run across {count} randomized simulation seeds...");
        for iteration in 0..count {
            let cur_seed = seed.wrapping_add(iteration);
            let scenarios = [
                ScenarioType::NetworkPartition,
                ScenarioType::CrashTornWrite,
                ScenarioType::ChaosSoak,
            ];
            for sc in scenarios {
                match ScenarioRunner::run(sc, cur_seed, steps) {
                    Ok(rep) => {
                        println!(
                            "  [PASS] Seed: {:<12} | Scenario: {:<18} | Ops: {:<4} | Packets: {:<4}",
                            cur_seed,
                            format!("{:?}", rep.scenario),
                            rep.ops_committed,
                            rep.packets_delivered
                        );
                    }
                    Err(violation) => {
                        eprintln!("\n❌ SIMULATION FAILURE DETECTED!");
                        eprintln!("   Violation: {violation}");
                        eprintln!("   Reproduce with:");
                        eprintln!("   cargo run -p simulator -- --seed {cur_seed} --scenario {:?} --steps {steps}\n", sc);
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
        println!("\n✅ Fuzz campaign successful! 0 violations found across {count} seeds.");
        return ExitCode::SUCCESS;
    }

    let scenarios = match scenario_arg.as_str() {
        "partition" => vec![ScenarioType::NetworkPartition],
        "crash" => vec![ScenarioType::CrashTornWrite],
        "soak" => vec![ScenarioType::ChaosSoak],
        "all" => vec![
            ScenarioType::NetworkPartition,
            ScenarioType::CrashTornWrite,
            ScenarioType::ChaosSoak,
        ],
        other => {
            eprintln!("Unknown scenario: {other}. Available: partition, crash, soak, all");
            return ExitCode::FAILURE;
        }
    };

    let mut reports = Vec::new();
    for sc in scenarios {
        println!(
            "\n--> Running scenario: {:?} (seed: {seed}, max_ticks: {steps})...",
            sc
        );
        match ScenarioRunner::run(sc, seed, steps) {
            Ok(report) => {
                print_report(&report);
                reports.push(report);
            }
            Err(violation) => {
                eprintln!("\n❌ SIMULATION FAILURE DETECTED!");
                eprintln!("   Violation: {violation}");
                eprintln!("   To reproduce this exact failure:");
                eprintln!(
                    "   cargo run -p simulator -- --seed {seed} --scenario {scenario_arg} --steps {steps}\n"
                );
                return ExitCode::FAILURE;
            }
        }
    }

    println!("\n================================================================================");
    println!("✅ All {} simulation scenarios PASSED!", reports.len());
    println!("   Invariants verified: Wealth Conservation (Zero Drift), Log Agreement.");
    println!("================================================================================");

    ExitCode::SUCCESS
}

fn print_report(rep: &ScenarioReport) {
    println!("    * Status:             PASSED (Invariants Conserved)");
    println!("    * Simulated Ticks:    {}", rep.total_ticks);
    println!("    * Ops Proposed:       {}", rep.ops_proposed);
    println!("    * Ops Committed:      {}", rep.ops_committed);
    println!("    * Packets Delivered:  {}", rep.packets_delivered);
    println!("    * Packets Dropped:    {}", rep.packets_dropped);
}
