use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn fixture(selected: bool) -> (PathBuf, Capture) {
    let root = std::env::temp_dir().join(format!(
        "grill-finalization-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    evidence::fresh(&root).unwrap();
    let options = BaselineOptions {
        endpoint: "http://127.0.0.1:1/v1/chat/completions".into(),
        model: "fixture".into(),
        deployment: root.join("deployment.json"),
        out: root.clone(),
        local_http: true,
        auth_env: None,
        selection: None,
        seconds: 1,
        json: false,
    };
    let workload = selection::workload(WORKLOAD).unwrap();
    let selection = serde_json::to_vec(&selection::Manifest {
        version: 1,
        id: "finalization-fixture".into(),
        workload: "workload.json".into(),
        source_sha256: evidence::digest(WORKLOAD),
        workload_sha256: evidence::digest(&serde_json::to_vec(&workload).unwrap()),
        scope: "CPU finalization-boundary fixture".into(),
        operation_scope: selection::OperationScope::Unknown,
    })
    .unwrap();
    let capture = manifest(&options, br#"{"model_revision":"fixture","runtime":"fixture","hardware":"cpu","settings":"fixture"}"#, "0".repeat(64), None, WORKLOAD, selected.then_some(selection.as_slice())).unwrap();
    (root, capture)
}

#[test]
fn finalization_deadline_fault_preserves_v1_and_records_selected_overrun() {
    let (legacy_root, legacy) = fixture(false);
    finalize_capture(&legacy_root, legacy, || Duration::from_secs(2)).unwrap();
    let legacy: Capture = serde_json::from_slice(
        &evidence::read(&legacy_root.join("capture.json"), FILE_CAP).unwrap(),
    )
    .unwrap();
    assert_eq!(legacy.status, CaptureStatus::Complete);
    assert!(legacy.stop_reason.is_none());
    assert!(!legacy_root.join("capture-timing.json").exists());

    let (expired_root, expired) = fixture(true);
    finalize_capture(&expired_root, expired, || Duration::from_secs(2)).unwrap();
    let expired: Capture = serde_json::from_slice(
        &evidence::read(&expired_root.join("capture.json"), FILE_CAP).unwrap(),
    )
    .unwrap();
    assert_eq!(expired.status, CaptureStatus::Incomplete);
    assert_eq!(expired.stop_reason.as_deref(), Some(OVERHEAD_STOP));

    let (crossed_root, crossed) = fixture(true);
    finalize_capture(&crossed_root, crossed, || {
        if crossed_root.join("capture.json").exists() {
            Duration::from_micros(1_000_001)
        } else {
            Duration::from_micros(999_999)
        }
    })
    .unwrap();
    let capture_bytes = evidence::read(&crossed_root.join("capture.json"), FILE_CAP).unwrap();
    let capture: Capture = serde_json::from_slice(&capture_bytes).unwrap();
    let timing: CaptureTiming = serde_json::from_slice(
        &evidence::read(&crossed_root.join("capture-timing.json"), FILE_CAP).unwrap(),
    )
    .unwrap();
    assert_eq!(capture.status, CaptureStatus::Complete);
    assert!(timing.elapsed_through_capture_publication_us > capture.seconds * 1_000_000);
    assert_eq!(timing.capture_sha256, evidence::digest(&capture_bytes));
    for root in [legacy_root, expired_root, crossed_root] {
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn failed_timing_publication_cannot_return_success_or_overwrite_evidence() {
    let (root, capture) = fixture(true);
    evidence::write(&root.join("capture-timing.json"), b"occupied").unwrap();
    assert!(finalize_capture(&root, capture, || Duration::from_millis(1)).is_err());
    assert!(root.join("capture.json").exists());
    assert_eq!(
        evidence::read(&root.join("capture-timing.json"), FILE_CAP).unwrap(),
        b"occupied"
    );
    std::fs::remove_dir_all(root).unwrap();
}
