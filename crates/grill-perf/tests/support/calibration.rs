use super::*;
use std::io::{BufRead, Write};

#[test]
#[ignore = "bounded offline calibration; requires explicit corpus and output paths"]
fn crosscheck_corpus() {
    let corpus = std::env::var_os("GRILL_CALIBRATION_CORPUS").expect("GRILL_CALIBRATION_CORPUS");
    let output = std::env::var_os("GRILL_CALIBRATION_RESULTS").expect("GRILL_CALIBRATION_RESULTS");
    let corpus = std::fs::read(corpus).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut output = std::io::BufWriter::new(
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
            .unwrap(),
    );
    let header = serde_json::json!({
        "kind":"c1-assess-crosscheck-v1",
        "corpus_sha256":evidence::digest(&corpus),
        "study_sha256":evidence::digest(&std::fs::read(root.join("src/study.rs")).unwrap()),
        "workload_sha256":evidence::digest(WORKLOAD),
        "evaluator_sha256":evidence::binary_digest().unwrap()
    });
    serde_json::to_writer(&mut output, &header).unwrap();
    writeln!(output).unwrap();
    for line in std::io::BufReader::new(corpus.as_slice()).lines() {
        let row: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
        let before: Vec<f64> = serde_json::from_value(row["before"].clone()).unwrap();
        let after: Vec<f64> = serde_json::from_value(row["after"].clone()).unwrap();
        let mut report = Report::new(PathBuf::from("offline-calibration"));
        if let Err(error) = assess(&mut report, &before, &after) {
            report.result = Outcome::Invalid;
            report.reasons.push(error);
        }
        serde_json::to_writer(
            &mut output,
            &serde_json::json!({"id":row["id"],"report":report}),
        )
        .unwrap();
        writeln!(output).unwrap();
    }
    output.flush().unwrap();
}

#[test]
fn assessment_boundaries_withhold_or_reject_without_manufacturing_confidence() {
    let mut equal = Report::new(PathBuf::new());
    assess(&mut equal, &[2.0; 8], &[2.0; 8]).unwrap();
    assert!(matches!(equal.result, Outcome::Inconclusive));
    assert_eq!(equal.observed_change_percent, Some(0.0));
    assert!(equal.model_based_interval_percent.is_none());
    let mut missing = Report::new(PathBuf::new());
    assess(&mut missing, &[0.0; 7], &[2.0; 8]).unwrap();
    assert!(matches!(missing.result, Outcome::Inconclusive));
    assert!(missing.observed_change_percent.is_none());
    assert!(missing.model_based_interval_percent.is_none());
    let mut invalid = Report::new(PathBuf::new());
    assert!(assess(&mut invalid, &[0.0; 8], &[2.0; 8]).is_err());
    assert!(invalid.observed_change_percent.is_none());
    assert!(assess(&mut invalid, &[1e-300; 8], &[1e300; 8]).is_err());
    assert!(invalid.model_based_interval_percent.is_none());
}

#[test]
fn assessment_uses_both_acquisition_variances() {
    let mut report = Report::new(PathBuf::new());
    assess(
        &mut report,
        &[1.0, 4.0, 1.0, 4.0, 1.0, 4.0, 1.0, 4.0],
        &[2.0, 8.0, 2.0, 8.0, 2.0, 8.0, 2.0, 8.0],
    )
    .unwrap();
    let log_two = 2.0f64.ln();
    let se = log_two * (2.0f64 / 7.0).sqrt();
    assert!((report.heteroscedastic_standard_error.unwrap() - se).abs() < 1e-12);
    assert!((report.observed_change_percent.unwrap() - 100.0).abs() < 1e-10);
    let [lower, upper] = report.model_based_interval_percent.unwrap();
    assert!((lower - (log_two - 2.364624251 * se).exp_m1() * 100.0).abs() < 1e-10);
    assert!((upper - (log_two + 2.364624251 * se).exp_m1() * 100.0).abs() < 1e-10);
    assert!(matches!(report.result, Outcome::Inconclusive));
}
