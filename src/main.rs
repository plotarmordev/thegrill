mod contract;
mod grade;
mod identity;
mod lifecycle;
mod pack;
mod pilot;
mod record;
mod report;
mod run;
mod store;
mod study;
mod transport;

use clap::{Parser, Subcommand};
use contract::Result;
use serde::Serialize;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "grill",
    bin_name = "grill",
    version,
    about = "Closed-pack admission, serial client collection, grading, inspection and comparison"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Emit a deterministic, self-qualified diagnostic pack as JSON (includes answers).
    Pilot {
        #[arg(long)]
        seed: u64,
        /// Problem instances per family; two presentation variants each.
        #[arg(long)]
        units: usize,
    },
    /// Check study declarations or analyze a bound, ordered pair of saved views offline.
    Study {
        #[command(subcommand)]
        command: StudyCommand,
    },
    /// Validate and qualify an inline pack; compute separate identities.
    Check {
        pack: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Grade submitted answers, not authenticated model execution.
    Grade {
        pack: PathBuf,
        submission: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Collect one serial attempt per case from a declared endpoint; writes an initial grade view.
    Run(run::Options),
    /// Cooperatively stop admission after the active attempt drains.
    Pause { run: PathBuf },
    /// Continue only never-started cases from validated, settled run evidence.
    Resume { run: PathBuf },
    /// Prepare the same run offline; no runtime, connection or output directory.
    Plan {
        #[command(flatten)]
        options: run::Options,
        #[arg(long)]
        json: bool,
    },
    /// Read-only inspection of a run directory or grade view, including interrupted runs.
    Inspect {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Apply the built-in grader offline to saved run evidence; no credential or network.
    Regrade {
        run: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify both self-contained views and compare compatible fixed targets.
    Compare {
        left_view: PathBuf,
        right_view: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum StudyCommand {
    /// Validate a study manifest against exact pack bytes, without reading results.
    Check {
        manifest: PathBuf,
        pack: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Verify manifest bindings and report paired outcomes by family and problem unit.
    Compare {
        manifest: PathBuf,
        pack: PathBuf,
        left_view: PathBuf,
        right_view: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Serialize)]
struct Check<'a> {
    version: u32,
    label: &'a str,
    cases: usize,
    qualification: &'static str,
    identities: contract::PackIdentities,
}

// serde escapes ASCII controls. Escape every non-ASCII code point as JSON UTF-16
// escapes too, so machine output remains parseable without emitting bidi controls.
fn json(value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| format!("output encoding: {e}"))?;
    let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    for c in text.chars() {
        if c.is_ascii() && c != '\u{7f}' {
            out.write_all(&[c as u8]).map_err(|e| e.to_string())?;
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                write!(out, "\\u{unit:04x}").map_err(|e| e.to_string())?;
            }
        }
    }
    writeln!(out)
        .and_then(|_| out.flush())
        .map_err(|e| e.to_string())
}

fn line(text: &str) -> Result<()> {
    writeln!(io::stdout().lock(), "{text}").map_err(|e| e.to_string())
}

fn coverage_line(c: &report::Coverage) -> String {
    let n = c.bounds.denominator;
    format!(
        "N={n} success={} failure={} unknown={} delivered={}/{n} malformed={} refused={}; bounds [{}/{n}, {}/{n}]",
        c.success,
        c.failure,
        c.unknown,
        c.delivered,
        c.malformed,
        c.refused,
        c.bounds.lower,
        c.bounds.upper
    )
}

fn system_line(system: &contract::System) -> String {
    format!(
        "system {} model {} endpoint {}",
        report::escape(&system.name),
        report::escape(&system.model),
        system
            .endpoint
            .as_deref()
            .map(report::escape)
            .unwrap_or("(undeclared)".into())
    )
}

fn effort_label(effort: Option<contract::Effort>) -> &'static str {
    match effort {
        None => "omitted",
        Some(contract::Effort::Minimal) => "minimal",
        Some(contract::Effort::Low) => "low",
        Some(contract::Effort::Medium) => "medium",
        Some(contract::Effort::High) => "high",
        Some(contract::Effort::Xhigh) => "xhigh",
        Some(contract::Effort::Max) => "max",
    }
}

fn usage_label(include_usage: Option<bool>) -> &'static str {
    match include_usage {
        Some(true) => "true",
        Some(false) => "false",
        None => "omitted",
    }
}

fn result_line(counts: &report::ResultCounts) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for (label, count) in [
        ("correct", counts.correct),
        ("wrong-answer", counts.wrong_answer),
        ("bad-format", counts.malformed_answer),
        ("refused", counts.refused),
        ("output-limit", counts.output_limit),
        ("timeout", counts.timeout),
        ("service-error", counts.service_error),
        ("transport-error", counts.transport_error),
        ("client-limit", counts.client_limit),
        ("interrupted", counts.interrupted),
        ("local-error", counts.local_error),
        ("invalid-evidence", counts.invalid_evidence),
        ("other-ungraded", counts.other_ungraded),
    ] {
        if count != 0 {
            if !text.is_empty() {
                text.push(' ');
            }
            write!(text, "{label}={count}").expect("String write");
        }
    }
    text
}

fn execute(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Pilot { seed, units } => json(&pilot::generate(seed, units)?),
        Command::Study { command } => execute_study(command),
        Command::Check {
            pack: path,
            json: machine,
        } => {
            let bytes = store::read(&path, contract::PACK_CAP)?;
            let p = pack::admit(&bytes)?;
            let checked = Check {
                version: 1,
                label: &p.label,
                cases: p.cases.len(),
                qualification: "self-check-passed-not-independent-task-validation",
                identities: identity::pack(&p, &bytes)?.0,
            };
            if machine {
                json(&checked)
            } else {
                line(&format!(
                    "Qualified pack {}: {} cases (self-check only)",
                    report::escape(&p.label),
                    p.cases.len()
                ))?;
                line(&format!(
                    "source {}\ntasks {}\ntarget {}\nqualification {}\ngrading {}",
                    checked.identities.source,
                    checked.identities.tasks,
                    checked.identities.target,
                    checked.identities.qualification,
                    checked.identities.grading
                ))
            }
        }
        Command::Grade {
            pack,
            submission,
            out,
        } => {
            let view = store::publish(&pack, &submission, &out)?;
            let c = report::coverage(view.cases.iter().map(|c| c.outcome));
            line(&format!(
                "Published submitted-artifact view {} (not verified execution)",
                report::escape(&out.to_string_lossy())
            ))?;
            line(&coverage_line(&c))
        }
        Command::Plan {
            options,
            json: machine,
        } => {
            let prepared = run::plan(&options)?;
            if machine {
                return json(&prepared);
            }
            line(&format!(
                "Offline preparation: {} cases; no dispatch or output created",
                prepared.plan.cases
            ))?;
            line(&format!(
                "pack {} protocol {}",
                prepared.plan.pack.source, prepared.protocol_identity
            ))?;
            line(&format!(
                "Actual bytes: pack={} plan={} prompt-content={} messages-JSON={} request-bodies={}",
                prepared.pack_bytes,
                prepared.plan_bytes,
                prepared.prompt_content_bytes,
                prepared.messages_json_bytes,
                prepared.request_bytes
            ))?;
            for case in &prepared.cases {
                line(&format!(
                    "  {}: prompt-content={} messages-JSON={} request-body={} bytes",
                    report::escape(&case.case_id),
                    case.prompt_content_bytes,
                    case.messages_json_bytes,
                    case.request_bytes
                ))?;
            }
            line(&format!(
                "Conservative file-payload bound={} bytes (not actual size or reserved capacity)",
                prepared.plan.bound_bytes
            ))?;
            line(&format!(
                "Requested output-token envelope={} ({} per case); provider cap enforcement unknown; not measured tokens or equal compute",
                prepared.requested_output_tokens, prepared.plan.protocol.token_cap.value
            ))?;
            let protocol = &prepared.plan.protocol;
            line(&format!(
                "Requested controls: profile={:?} reasoning_effort={} include_usage={}; requested fields only, effective settings unverified",
                protocol.profile,
                effort_label(protocol.reasoning_effort),
                usage_label(protocol.include_usage)
            ))
        }
        Command::Pause { run } => line(lifecycle::pause(&run)?),
        Command::Resume { run } => {
            let summary = run::resume(&run)?;
            line(&format!(
                "{}; {} new attempts; historical receipts unchanged",
                if summary.interrupted {
                    "interrupted"
                } else if summary.paused {
                    "paused"
                } else {
                    "completed"
                },
                summary.attempts
            ))?;
            line(&coverage_line(&report::coverage(
                summary.view.cases.iter().map(|c| c.outcome),
            )))?;
            if summary.interrupted {
                return Err("immediate interruption; inspect retained evidence".into());
            }
            Ok(())
        }
        Command::Run(options) => {
            let summary = run::run(&options)?;
            let c = report::coverage(summary.view.cases.iter().map(|c| c.outcome));
            line(&format!(
                "Run {} collected {} of {} attempts (client receipts, not authenticated execution)",
                report::escape(&options.out.to_string_lossy()),
                summary.attempts,
                summary.view.cases.len()
            ))?;
            line(&format!("initial view: {}", coverage_line(&c)))?;
            if summary.paused {
                line("paused: active attempt drained; resume continues only unstarted cases")?;
            }
            if summary.interrupted {
                return Err(format!(
                    "interrupted after {} attempts; partial evidence and initial view retained",
                    summary.attempts
                ));
            }
            Ok(())
        }
        Command::Inspect {
            path,
            json: machine,
        } => {
            if store::kind(&path)? == store::Kind::Submitted {
                let verified = store::verify(&path)?;
                let c = report::coverage(verified.cases.iter().map(|c| c.outcome));
                return if machine {
                    json(&c)
                } else {
                    line(&format!(
                        "Submitted-artifact view {} verified; {}",
                        report::escape(&path.to_string_lossy()),
                        system_line(&verified.system)
                    ))?;
                    line(&coverage_line(&c))
                };
            }
            let loaded = store::load(&path)?;
            let view_path = store::view_path(loaded.kind, &path);
            let view = if std::fs::symlink_metadata(&view_path).is_ok() {
                store::verify_run(&loaded, &path).map(Some)
            } else {
                Ok(None)
            };
            let mut inspection = report::inspect(&loaded, view);
            inspection.lifecycle = Some(lifecycle::inspect(&path, &loaded));
            if matches!(inspection.view, report::ViewStatus::Mismatch)
                && lifecycle::historical_initial(&path, &loaded)
            {
                inspection.view = report::ViewStatus::Historical;
            }
            if machine {
                return json(&inspection);
            }
            line(&format!(
                "{} {}: {} ({} cases); {}",
                if loaded.kind == store::Kind::Run {
                    "Run"
                } else {
                    "Run-derived grade view"
                },
                report::escape(&path.to_string_lossy()),
                report::escape(inspection.label),
                inspection.cases,
                system_line(inspection.system)
            ))?;
            if let Some(lifecycle) = &inspection.lifecycle {
                line(&format!(
                    "lifecycle={} sessions={} next-attempt={}; {}",
                    report::escape(&lifecycle.state),
                    lifecycle.sessions,
                    lifecycle.next_attempt,
                    lifecycle
                        .detail
                        .as_deref()
                        .map(report::escape)
                        .unwrap_or_default()
                ))?;
            }
            let k = &inspection.counts;
            line(&format!(
                "profile={:?} stream={} reasoning_effort={} include_usage={} token_cap={:?}={} total_ms={} idle_ms={}; collected={} refused={} uncommitted={} unresolved={} not-started={} invalid={}",
                inspection.profile,
                inspection.stream,
                effort_label(inspection.reasoning_effort),
                usage_label(inspection.include_usage),
                inspection.token_cap.field,
                inspection.token_cap.value,
                inspection.collection.total_ms,
                inspection.collection.idle_ms,
                k.collected,
                k.refused,
                k.uncommitted,
                k.unresolved,
                k.not_started,
                k.invalid
            ))?;
            line(&format!(
                "Results: {}",
                result_line(&inspection.result_counts)
            ))?;
            let usage = &inspection.usage_coverage;
            line(&format!(
                "Usage: unvalidated provider last snapshots, not complete or validated billing; snapshots={}/{}; field present/unknown: prompt={}/{} completion={}/{} total={}/{}",
                usage.snapshots,
                inspection.cases,
                usage.prompt_tokens.present,
                usage.prompt_tokens.unknown,
                usage.completion_tokens.present,
                usage.completion_tokens.unknown,
                usage.total_tokens.present,
                usage.total_tokens.unknown
            ))?;
            for a in &inspection.attempts {
                line(&format!(
                    "  {:06} {} {:?}; grade={:?} stop={:?} metadata={:?}; {}",
                    a.attempt,
                    report::escape(a.case_id),
                    a.result,
                    a.outcome,
                    a.stop,
                    a.metadata_source,
                    a.detail.map(report::escape).unwrap_or_default()
                ))?;
                if let Some(usage) = a.usage {
                    line(&format!(
                        "    unvalidated provider snapshot: prompt={} completion={} total={}",
                        usage
                            .prompt_tokens
                            .map(|n| n.to_string())
                            .unwrap_or("unknown".into()),
                        usage
                            .completion_tokens
                            .map(|n| n.to_string())
                            .unwrap_or("unknown".into()),
                        usage
                            .total_tokens
                            .map(|n| n.to_string())
                            .unwrap_or("unknown".into())
                    ))?;
                }
            }
            match (&inspection.view, &inspection.coverage) {
                (report::ViewStatus::Verified, Some(c)) => {
                    line(&format!("grade view verified: {}", coverage_line(c)))
                }
                (status, _) => line(&format!("grade view: {status:?}")),
            }
        }
        Command::Regrade { run, out } => {
            let view = store::regrade(&run, &out)?;
            let c = report::coverage(view.cases.iter().map(|c| c.outcome));
            line(&format!(
                "Published offline regrade view {} (client-collected evidence, not authenticated execution)",
                report::escape(&out.to_string_lossy())
            ))?;
            line(&coverage_line(&c))
        }
        Command::Compare {
            left_view,
            right_view,
            json: machine,
        } => {
            let a = store::verify(&left_view)?;
            let b = store::verify(&right_view)?;
            let c = report::compare(&a, &b)?;
            if machine {
                json(&c)
            } else {
                line(
                    "Descriptive finite-pack comparison; not verified or authenticated execution",
                )?;
                line(&format!(
                    "A={} ({})\nB={} ({})",
                    system_line(c.left_system),
                    c.left_evidence,
                    system_line(c.right_system),
                    c.right_evidence
                ))?;
                line(c.effective_rendering)?;
                for (label, side) in [("A", &c.left), ("B", &c.right)] {
                    line(&format!("{label}: {}", coverage_line(side)))?;
                }
                line(&format!("A results: {}", result_line(&c.left_results)))?;
                line(&format!("B results: {}", result_line(&c.right_results)))?;
                line(&format!(
                    "B-A bounds [{}/{}, {}/{}]; both-success={} both-fail={} gains={} losses={} either-unknown={}",
                    c.delta_b_minus_a.lower,
                    c.n,
                    c.delta_b_minus_a.upper,
                    c.n,
                    c.paired.both_success,
                    c.paired.both_fail,
                    c.paired.gains,
                    c.paired.losses,
                    c.paired.either_unknown
                ))?;
                match c.complete_case_delta_diagnostic {
                    Some(d) => line(&format!(
                        "Complete-case delta (diagnostic only): {}/{}",
                        d.numerator, d.denominator
                    )),
                    None => line(
                        "Complete-case delta (diagnostic only): unavailable; no jointly known cases",
                    ),
                }
            }
        }
    }
}

fn execute_study(command: StudyCommand) -> Result<()> {
    let (manifest_path, pack_path) = match &command {
        StudyCommand::Check { manifest, pack, .. }
        | StudyCommand::Compare { manifest, pack, .. } => (manifest, pack),
    };
    let pack_bytes = store::read(pack_path, contract::PACK_CAP)?;
    let pack = pack::admit(&pack_bytes)?;
    let manifest_bytes = store::read(manifest_path, study::MANIFEST_CAP)?;
    let study = study::admit(&manifest_bytes, &pack, &pack_bytes)?;
    match command {
        StudyCommand::Check { json: machine, .. } => {
            if machine {
                json(&study)
            } else {
                line(&format!(
                    "Study {}: {} cases, {} families, {} declared problem units",
                    report::escape(&study.manifest.label),
                    pack.cases.len(),
                    pack.groups.len(),
                    pack.worlds.len()
                ))?;
                line(&format!(
                    "study {} pack {}",
                    study.identity, study.pack.source
                ))?;
                line(
                    "Declarations checked, not authenticated provenance, exposure or preregistration.",
                )
            }
        }
        StudyCommand::Compare {
            left_view,
            right_view,
            json: machine,
            ..
        } => {
            let left = store::verify(&left_view)?;
            let right = store::verify(&right_view)?;
            let analysis = report::study_analysis(&study, &pack, &left, &right)?;
            if machine {
                return json(&analysis);
            }
            line("Descriptive paired study; no confidence intervals or causal verdict.")?;
            line(&format!("study {}", study.identity))?;
            line(&format!(
                "A={}\nB={}",
                system_line(&left.system),
                system_line(&right.system)
            ))?;
            line(analysis.comparison.claim)?;
            line(analysis.comparison.effective_rendering)?;
            line(
                "Family provenance, exposure and independent problem units are declarations, not verified facts.",
            )?;
            for (name, c) in [
                ("A", &analysis.comparison.left),
                ("B", &analysis.comparison.right),
            ] {
                line(&format!("{name}: {}", coverage_line(c)))?;
            }
            let c = &analysis.comparison;
            line(&format!("A results: {}", result_line(&c.left_results)))?;
            line(&format!("B results: {}", result_line(&c.right_results)))?;
            line(&format!(
                "All cases: gains={} losses={} either-unknown={}; B-A bounds [{}/{}, {}/{}]",
                c.paired.gains,
                c.paired.losses,
                c.paired.either_unknown,
                c.delta_b_minus_a.lower,
                c.n,
                c.delta_b_minus_a.upper,
                c.n
            ))?;
            for family in &analysis.families {
                line(&format!(
                    "Family {}: {} declared problem units",
                    report::escape(family.group),
                    family.units.len()
                ))?;
                study_summary_line(&family.summary)?;
                for unit in &family.units {
                    line(&format!("  Unit {}", report::escape(unit.world)))?;
                    study_summary_line(&unit.summary)?;
                    for case in &unit.cases {
                        line(&format!(
                            "    {}: {:?} ({:?}) -> {:?} ({:?})",
                            report::escape(case.case_id),
                            case.left,
                            case.left_result,
                            case.right,
                            case.right_result
                        ))?;
                    }
                }
            }
            Ok(())
        }
    }
}

fn study_summary_line(summary: &report::PairSummary) -> Result<()> {
    line(&format!(
        "  N={} gains={} losses={} either-unknown={}; B-A bounds [{}/{}, {}/{}]",
        summary.n,
        summary.paired.gains,
        summary.paired.losses,
        summary.paired.either_unknown,
        summary.delta_b_minus_a.lower,
        summary.n,
        summary.delta_b_minus_a.upper,
        summary.n
    ))?;
    line(&format!(
        "  A: {}; {}",
        coverage_line(&summary.left),
        result_line(&summary.left_results)
    ))?;
    line(&format!(
        "  B: {}; {}",
        coverage_line(&summary.right),
        result_line(&summary.right_results)
    ))
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                return if line(&e.to_string()).is_ok() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                };
            }
            let _ = writeln!(
                io::stderr().lock(),
                "error: {}",
                report::error_preview(&e.to_string())
            );
            return ExitCode::from(2);
        }
    };
    match execute(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(
                io::stderr().lock(),
                "error: {}",
                report::error_preview(&error)
            );
            ExitCode::FAILURE
        }
    }
}
