mod acquisition;
mod bundle;
mod capacity;
mod envelope;
mod evidence;
mod lifecycle;
mod metrics;
mod microbench;
mod model;
mod outputs;
mod policy;
mod prose;
mod resources;
mod retention;
mod run;
mod schedule;
mod selection;
mod sequence;
mod serving_resources;
mod startup;
mod study;
mod wire;

use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "grill-perf",
    version = concat!(env!("CARGO_PKG_VERSION"), " (source ", env!("GRILL_PERF_SOURCE"), ")"),
    about = "Measure declared serving workloads; compare saved performance evidence offline"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Finite prospective context/load and retention studies; operator recovery only.
    Capacity {
        #[command(subcommand)]
        command: capacity::Command,
    },
    /// Explicit bounded host resource observation and offline domain evidence.
    Resource {
        #[command(subcommand)]
        command: resources::Command,
    },
    /// Explicit bounded startup/restart-cache evidence; operator owns all lifecycle actions.
    Startup {
        #[command(subcommand)]
        command: startup::Command,
    },
    /// Record the default or explicitly selected bounded workload with native evidence.
    Baseline(study::BaselineOptions),
    /// Compare a declared serving change using the verified baseline settings.
    Check(study::CheckOptions),
    /// Collect bounded request waves; warmup is retained but excluded from measured results.
    Run(run::Options),
    /// Validate run declarations offline without dispatch or capture output; always JSON.
    Preflight(run::CommonArgs),
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
        reference: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Apply the captured observed-envelope policy to verified offline evidence.
    Decide {
        baseline: PathBuf,
        candidate: PathBuf,
        #[arg(long)]
        reference: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Compare replayed completion text of measured lanes between two evidence sets of one greedy workload.
    Outputs {
        a: PathBuf,
        b: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Verify the declared shared recipe bundle without network calls.
    Bundle {
        #[command(subcommand)]
        command: BundleCommand,
    },
    /// Capture, import, inspect and compare kernel or collective evidence.
    Microbench {
        #[command(subcommand)]
        command: MicrobenchCommand,
    },
}
#[derive(Subcommand)]
enum MicrobenchCommand {
    /// Run the in-process CPU reference, or one explicitly declared program.
    Capture(microbench::CaptureOptions),
    /// Validate retained artifact bytes and record them with imported provenance.
    Import(microbench::ImportOptions),
    /// Validate and print artifact identity, samples and retained failures.
    Inspect(microbench::InspectOptions),
    /// Compare compatible A/B/A2 acquisitions with exact envelope arithmetic.
    Compare(microbench::CompareOptions),
}
#[derive(Subcommand)]
enum BundleCommand {
    /// Validate one workload and print its typed pins and per-run budgets offline.
    Inspect { workload: PathBuf },
    Verify {
        manifest: PathBuf,
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
#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
fn execute(cli: Cli) -> model::Result<u8> {
    match cli.command {
        Command::Resource { command } => resources::execute(command),
        Command::Startup { command } => startup::execute(command),
        Command::Capacity { command } => capacity::execute(command),
        Command::Baseline(options) => {
            let report = study::baseline(&options)?;
            show_study(&report, options.json)
        }
        Command::Check(options) => {
            let report = study::check(&options)?;
            show_study(&report, options.json)
        }
        Command::Run(options) => show_summary(run::execute(&options)?, options.json)
            .map(|complete| if complete { 0 } else { 2 }),
        Command::Preflight(options) => {
            print_json(&run::preflight(&options)?)?;
            Ok(0)
        }
        Command::Bundle {
            command: BundleCommand::Verify { manifest, json },
        } => {
            let verification = bundle::verify(&manifest)?;
            if json {
                print_json(&verification)?;
            } else {
                bundle::show(&verification);
            }
            Ok(0)
        }
        Command::Bundle {
            command: BundleCommand::Inspect { workload },
        } => {
            let source = evidence::read(&workload, model::FILE_CAP)?;
            let admitted = selection::workload(&source)?;
            let (warmup, measured, tokens) = selection::budgets(&admitted, 1)?;
            print_json(&serde_json::json!({
                "claim":"offline-declared-workload-not-backend-qualification",
                "source_sha256":evidence::digest(&source),
                "workload_sha256":evidence::digest(&serde_json::to_vec(&admitted).map_err(|e| e.to_string())?),
                "name":admitted.name,
                "request":admitted.request,
                "warmup_requests":warmup,
                "measured_requests":measured,
                "total_output_token_ceiling":tokens,
                "limits":admitted.limits
            }))?;
            Ok(0)
        }
        Command::Pause { run } => {
            lifecycle::pause(&run)?;
            println!(
                "pause requested; active wave will drain before admission stops; inspect session run.json for paused/completed outcome"
            );
            Ok(0)
        }
        Command::Resume { run, json } => show_summary(run::resume(&run, json)?, json)
            .map(|complete| if complete { 0 } else { 2 }),
        Command::Microbench {
            command: MicrobenchCommand::Capture(options),
        } => {
            let report = microbench::capture(&options)?;
            if options.json {
                print_json(&report)?;
            } else {
                print!("{}", microbench::human_capture(&report));
            }
            Ok(report.exit())
        }
        Command::Microbench {
            command: MicrobenchCommand::Import(options),
        } => {
            let report = microbench::import(&options)?;
            if options.json {
                print_json(&report)?;
            } else {
                print!("{}", microbench::human_capture(&report));
            }
            Ok(report.exit())
        }
        Command::Microbench {
            command: MicrobenchCommand::Inspect(options),
        } => {
            let inspection = microbench::inspect(&options)?;
            if options.json {
                print_json(&inspection)?;
            } else {
                print!("{}", microbench::human_inspection(&inspection));
            }
            Ok(inspection.exit())
        }
        Command::Microbench {
            command: MicrobenchCommand::Compare(options),
        } => {
            let decision = microbench::compare(&options);
            if options.json {
                print_json(&decision)?;
            } else {
                print!("{}", microbench::human_decision(&decision));
            }
            Ok(decision.exit())
        }
        Command::Decide {
            baseline,
            candidate,
            reference,
            json,
        } => {
            let decision = policy::decide(&baseline, &candidate, reference.as_deref());
            if json {
                print_json(&decision)?;
            } else {
                // The same versioned envelope makes output completion observable.
                println!(
                    "Observed policy decision; not a statistical or causal claim. Eligibility is separate from the policy outcome."
                );
                print_json(&decision)?;
            }
            Ok(decision.decision.exit())
        }
        Command::Outputs { a, b, json } => {
            let report = outputs::compare(&a, &b)?;
            if json {
                print_json(&report)?;
            } else {
                print!("{}", outputs::human(&report));
            }
            Ok(report.exit())
        }
        Command::Compare {
            baseline,
            candidate,
            reference,
            json,
        } => {
            if study::is_capture(&baseline)
                || study::is_capture(&candidate)
                || reference.as_deref().is_some_and(study::is_capture)
                || (!baseline.join("plan.json").exists() && !candidate.join("plan.json").exists())
            {
                let report = study::compare(&baseline, &candidate, reference.as_deref());
                return show_study(&report, json);
            }
            let comparison = evidence::compare(&baseline, &candidate, reference.as_deref())?;
            if let Some(schedule) = &comparison.schedule {
                if !json {
                    println!(
                        "Named-lane schedule observations; descriptive only, not a mixed-load or fairness verdict."
                    );
                }
                print_json(&comparison)?;
                return Ok(if schedule.complete_eligible { 0 } else { 2 });
            }
            if let Some(acquisition) = &comparison.acquisition {
                if !json {
                    println!(
                        "Declared acquisition observations; descriptive only, use decide for prospective gates."
                    );
                }
                print_json(&comparison)?;
                let complete = [&acquisition.baseline, &acquisition.candidate]
                    .into_iter()
                    .chain(acquisition.reference.iter())
                    .all(|report| {
                        !report.records.is_empty()
                            && report.records.iter().all(|r| r.complete_eligible)
                    });
                return Ok(if complete && acquisition.baseline.resources.is_none() {
                    0
                } else {
                    2
                });
            }
            if json {
                print_json(&comparison)?;
            } else {
                println!("Descriptive deployment comparison; not a causal or capacity verdict.");
                if let Some(identity) = &comparison.reference_identity {
                    println!("Reference identity: {}", identity.status.as_str());
                    println!("  {}", identity.scope);
                    for reason in &identity.reasons {
                        println!("  {reason}");
                    }
                }
                for (cell, change) in comparison.changes.iter().enumerate() {
                    print!("{}: ", change.cell);
                    for (index, (name, value)) in [
                        ("wave latency", change.wave_latency_change_percent),
                        (
                            "achieved throughput",
                            change.achieved_throughput_change_percent,
                        ),
                        ("decode rate", change.decode_rate_change_percent),
                        ("prefill rate", change.prefill_rate_change_percent),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if index > 0 {
                            print!("; ");
                        }
                        match value {
                            Some(value) => print!("{name} {value:+.2}%"),
                            None if change.withheld.iter().any(|w| w.starts_with(name)) => {
                                print!("{name} withheld")
                            }
                            None => print!("{name} n/a"),
                        }
                    }
                    println!();
                    if let Some(drift) = &comparison.drift {
                        let drift = &drift[cell];
                        print!("  reference drift: ");
                        for (index, (name, value)) in [
                            ("wave latency", drift.wave_latency_percent),
                            ("achieved throughput", drift.achieved_throughput_percent),
                            ("decode rate", drift.decode_rate_percent),
                            ("prefill rate", drift.prefill_rate_percent),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            if index > 0 {
                                print!("; ");
                            }
                            match value {
                                Some(value) => print!("{name} {value:+.2}%"),
                                None => print!("{name} n/a"),
                            }
                        }
                        println!();
                        for reason in &drift.withheld {
                            println!("  reference drift withheld: {reason}");
                        }
                    }
                    for reason in change.withheld.iter().chain(&change.ineligibility_reasons) {
                        println!("  {reason}");
                    }
                }
            }
            Ok(if comparison.changes.iter().all(|c| c.eligible) {
                0
            } else {
                2
            })
        }
    }
}
fn show_study(report: &study::Report, json: bool) -> model::Result<u8> {
    if json {
        print_json(report)?;
    } else {
        print!("{}", study::human(report));
    }
    Ok(report.exit())
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
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) => {
            eprintln!("grill-perf: {}", error.escape_debug());
            std::process::ExitCode::from(1)
        }
    }
}
