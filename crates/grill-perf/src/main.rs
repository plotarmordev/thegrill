mod evidence;
mod lifecycle;
mod model;
mod run;
mod wire;

use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "grill-perf",
    version,
    about = "Measure declared serving workloads; compare saved performance evidence offline"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Collect bounded request waves; warmup is retained but excluded from measured results.
    Run(run::Options),
    /// Request cooperative pause after the active whole wave is published.
    Pause { run: PathBuf },
    /// Continue only never-started waves from a verified cooperative pause.
    Resume {
        run: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Verify evidence and compare compatible workloads without network calls.
    Compare {
        baseline: PathBuf,
        candidate: PathBuf,
        #[arg(long)]
        json: bool,
    },
}
fn print_json(value: &impl serde::Serialize) -> model::Result<()> {
    let encoded = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let mut output = std::io::stdout().lock();
    for c in encoded.chars() {
        if c.is_ascii() {
            write!(output, "{c}")
        } else {
            let mut units = [0u16; 2];
            c.encode_utf16(&mut units)
                .iter()
                .try_for_each(|unit| write!(output, "\\u{unit:04x}"))
        }
        .map_err(|e| e.to_string())?;
    }
    writeln!(output).map_err(|e| e.to_string())
}
fn execute(cli: Cli) -> model::Result<bool> {
    match cli.command {
        Command::Run(options) => show_summary(run::execute(&options)?, options.json),
        Command::Pause { run } => {
            lifecycle::pause(&run)?;
            println!(
                "pause requested; active wave will drain before admission stops; inspect session run.json for paused/completed outcome"
            );
            Ok(true)
        }
        Command::Resume { run, json } => show_summary(run::resume(&run, json)?, json),
        Command::Compare {
            baseline,
            candidate,
            json,
        } => {
            let comparison = evidence::compare(&baseline, &candidate)?;
            if json {
                print_json(&comparison)?;
            } else {
                println!("Descriptive deployment comparison; not a causal or capacity verdict.");
                for change in &comparison.changes {
                    match (
                        change.wave_latency_change_percent,
                        change.achieved_throughput_change_percent,
                    ) {
                        (Some(latency), Some(rate)) => {
                            print!(
                                "{}: wave latency {latency:+.2}%; achieved throughput {rate:+.2}%",
                                change.cell
                            );
                            if let Some(decode) = change.decode_rate_change_percent {
                                print!("; decode rate {decode:+.2}%");
                            }
                            println!();
                        }
                        _ => println!(
                            "{}: {}",
                            change.cell,
                            change.ineligibility_reasons.join("; ")
                        ),
                    }
                }
            }
            Ok(comparison.changes.iter().all(|c| c.eligible))
        }
    }
}
fn show_summary(summary: run::Summary, json: bool) -> model::Result<bool> {
    let complete = summary.status == "completed";
    if json {
        print_json(&summary)?;
    } else {
        println!(
            "{}: session {} starting wave {}; {}/{} session waves published; {}/{} measured waves eligible",
            summary.status,
            summary.session,
            summary.first_wave,
            summary.published_waves,
            summary.planned_waves - summary.first_wave,
            summary.eligible_measured_waves,
            summary.measured_waves
        );
        if summary.session != 0 {
            println!(
                "Continued sessions: client pool reset and cache/warmup continuity unverified; no uninterrupted timing comparison."
            );
        }
    }
    Ok(complete)
}
fn main() -> std::process::ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = u8::from(error.use_stderr());
            if error.print().is_err() {
                return std::process::ExitCode::from(1);
            }
            return std::process::ExitCode::from(code);
        }
    };
    match execute(cli) {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::from(2),
        Err(error) => {
            eprintln!("grill-perf: {}", error.escape_debug());
            std::process::ExitCode::from(1)
        }
    }
}
