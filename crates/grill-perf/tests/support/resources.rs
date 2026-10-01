use super::*;

fn config(source: Source) -> ResourcesConfig {
    ResourcesConfig {
        version: 1,
        sources: vec![source],
        cadence_us: 1000,
        max_gap_us: 1_000_000,
        max_read_us: 10_000,
        max_samples: 100,
        deadline_us: 10_000_000,
        per_sample_bytes: 16_384,
        raw_total_bytes: 1_048_576,
    }
}
fn process_source() -> Source {
    Source {
        id: "process".into(),
        target: Target::Process { pid: 7 },
        ownership: Ownership::ProcessAddressSpace,
    }
}
fn clock() -> Clock {
    Clock {
        id: "fixture-clock".into(),
        kind: ClockKind::LinuxMonotonic,
        unit: ClockUnit::Microseconds,
        resolution_ns: 1000,
        synchronization: Synchronization::LocalOrigin,
    }
}
fn stat(start: u64, cpu: u64) -> Vec<u8> {
    let mut fields = vec!["0".to_owned(); 50];
    fields[0] = "R".into();
    fields[11] = cpu.to_string();
    fields[19] = start.to_string();
    // Guest is already included in utime, and children are not process CPU time.
    fields[13] = "9000".into();
    fields[14] = "8000".into();
    fields[40] = "777".into();
    format!("7 (name ) with spaces)) {}\n", fields.join(" ")).into_bytes()
}
fn raw(name: &str, bytes: Vec<u8>) -> Raw {
    Raw {
        name: name.into(),
        bytes,
        error: None,
    }
}
fn process_snapshot(t: u64, start: u64, cpu: u64, rss_kb: u64) -> Snapshot {
    let source = process_source();
    let raw = vec![
        raw("stat", stat(start, cpu)),
        raw(
            "status",
            format!("VmRSS:\t{rss_kb} kB\nVmHWM:\t{rss_kb} kB\n").into_bytes(),
        ),
        raw("stat_after", stat(start, cpu)),
    ];
    let (incarnation, readings) = parse_snapshot(&source, &raw, None).unwrap();
    Snapshot {
        source: source.id,
        clock: clock().id,
        started_us: t,
        observed_us: t,
        incarnation,
        raw,
        readings,
        failure: None,
    }
}
fn observation(source: Source, snapshots: Vec<Snapshot>) -> Observation {
    let end = snapshots.last().unwrap().observed_us;
    let start = snapshots[0].observed_us;
    let raw_bytes = snapshots
        .iter()
        .flat_map(|s| &s.raw)
        .map(|r| r.bytes.len() as u64)
        .sum();
    Observation {
        version: 1,
        kind: "resource-observation-v1".into(),
        provenance: Provenance::Imported,
        adapter: ADAPTER.into(),
        binary_sha256: "1".repeat(64),
        config: config(source),
        clock: clock(),
        clk_tck: 100,
        observer_started_us: 0,
        observer_settled_us: end,
        measured: MeasuredInterval {
            clock: clock().id,
            started_us: start,
            settled_us: end,
        },
        snapshots,
        failures: Vec::new(),
        raw_bytes,
        read_overhead_us: 0,
    }
}
fn gate(metric: Metric, statistic: Statistic) -> Gate {
    Gate {
        source: "process".into(),
        metric,
        statistic,
        max_regression_bps: 100,
        max_reference_spread_bps: 0,
    }
}
fn process_observation() -> Observation {
    observation(
        process_source(),
        vec![
            process_snapshot(1000, 100, 10, 10),
            process_snapshot(2000, 100, 13, 20),
        ],
    )
}
#[test]
fn proc_final_parenthesis_guest_children_and_overflow() {
    assert_eq!(proc_stat(&stat(123, 41), 7), Ok((123, 41)));
    assert_eq!(proc_stat(&stat(123, 41), 8), Err(Failure::SourceChanged));
    assert_eq!(proc_stat(b"7 (bad) R 0\n", 7), Err(Failure::Malformed));
    assert_eq!(number("18446744073709551616"), Err(Failure::Overflow));
    assert_eq!(
        field(b"VmRSS: 18446744073709551615 kB\n", "VmRSS:", true),
        Err(Failure::Overflow)
    );
    assert_eq!(
        field(b"VmRSS: 1 MB\n", "VmRSS:", true),
        Err(Failure::Malformed)
    );
    assert_eq!(
        field(b"VmRSS: 1 kB\nVmRSS: 2 kB\n", "VmRSS:", true),
        Err(Failure::Malformed)
    );
    let source = Source {
        id: "host".into(),
        target: Target::HostCpu,
        ownership: Ownership::HostShared,
    };
    let (_, readings) = parse_snapshot(
        &source,
        &[raw(
            "stat",
            b"cpu 10 20 30 400 500 60 70 80 900 1000\nbtime 42\n".to_vec(),
        )],
        None,
    )
    .unwrap();
    assert_eq!(readings[0].value, Value::Observed { value: 270 });
}
#[test]
fn exact_cpu_denominator_and_source_specific_boundaries() {
    let mut o = process_observation();
    validate_observation(&o).unwrap();
    assert_eq!(
        summary_value(&o, &gate(Metric::CpuTime, Statistic::CpuTime)),
        Ok((30_000, 1).into())
    );
    // 30 logical CPUs is not clamped to one or divided by machine cores.
    assert_eq!(
        summary_value(&o, &gate(Metric::CpuTime, Statistic::CpuUtilizationOneCpu)),
        Ok((30, 1).into())
    );
    assert_eq!(
        summary_value(&o, &gate(Metric::ResidentMemory, Statistic::SampledMaximum)),
        Ok((20 * 1024, 1).into())
    );
    o.measured.started_us += 1;
    assert_eq!(
        summary_value(&o, &gate(Metric::CpuTime, Statistic::CpuTime)),
        Err(Failure::UnsupportedBoundary)
    );
    assert_eq!(
        summary_value(
            &o,
            &gate(Metric::LifetimePeakMemory, Statistic::SampledMaximum)
        ),
        Err(Failure::UnsupportedMetric)
    );
    o.measured.started_us = 0;
    assert_eq!(
        summary_value(&o, &gate(Metric::ResidentMemory, Statistic::SampledMaximum)),
        Err(Failure::IncompleteExposure)
    );
}
#[test]
fn pid_reuse_between_and_within_samples_and_counter_reset() {
    let changed = observation(
        process_source(),
        vec![
            process_snapshot(1000, 100, 10, 10),
            process_snapshot(2000, 101, 11, 20),
        ],
    );
    assert_eq!(
        summary_value(
            &changed,
            &gate(Metric::ResidentMemory, Statistic::SampledMaximum)
        ),
        Err(Failure::SourceChanged)
    );
    let reset = observation(
        process_source(),
        vec![
            process_snapshot(1000, 100, 10, 10),
            process_snapshot(2000, 100, 9, 20),
        ],
    );
    assert_eq!(
        summary_value(
            &reset,
            &gate(Metric::ResidentMemory, Statistic::SampledMaximum)
        ),
        Err(Failure::CounterReset)
    );
    let mut raw = process_snapshot(1000, 100, 10, 10).raw;
    raw[2].bytes = stat(101, 10);
    assert_eq!(
        parse_snapshot(&process_source(), &raw, None),
        Err(Failure::SourceChanged)
    );
    raw[2].bytes = stat(100, 9);
    assert_eq!(
        parse_snapshot(&process_source(), &raw, None),
        Err(Failure::CounterReset)
    );
}
#[test]
fn source_gaps_cancellation_permission_and_budget_never_qualify() {
    let mut o = process_observation();
    let g = gate(Metric::ResidentMemory, Statistic::SampledMaximum);
    o.config.max_gap_us = 1000;
    assert!(summary_value(&o, &g).is_ok());
    o.snapshots[1].observed_us += 1;
    o.observer_settled_us += 1;
    assert_eq!(summary_value(&o, &g), Err(Failure::Gap));
    let mut o = process_observation();
    o.snapshots[1].raw[1].error = Some(Failure::Permission);
    o.snapshots[1].readings = parse_snapshot(&process_source(), &o.snapshots[1].raw, None)
        .unwrap()
        .1;
    validate_observation(&o).unwrap();
    assert_eq!(summary_value(&o, &g), Err(Failure::Permission));
    for failure in [
        Failure::Cancelled,
        Failure::SampleBudget,
        Failure::ByteBudget,
        Failure::Deadline,
    ] {
        let mut o = process_observation();
        o.failures.push(failure);
        assert_eq!(summary_value(&o, &g), Err(failure));
    }
}
#[test]
fn clock_and_raw_replay_reject_forged_observations() {
    let mut o = process_observation();
    o.snapshots[1].clock = "other-origin".into();
    assert!(validate_observation(&o).is_err());
    let mut o = process_observation();
    o.snapshots[1].readings[0].unit = Unit::Bytes;
    assert!(validate_observation(&o).is_err());
    let mut o = process_observation();
    o.snapshots[1].readings[1].value = Value::Observed { value: 1 };
    assert!(validate_observation(&o).is_err());
    let mut c = clock();
    c.id = "another-acquisition".into();
    assert!(compatible_clocks(&clock(), &c));
    c.resolution_ns = 2000;
    assert!(!compatible_clocks(&clock(), &c));
    c = clock();
    c.synchronization = Synchronization::Unrelated;
    assert!(!compatible_clocks(&clock(), &c));
}
#[test]
fn cgroup_unlimited_missing_and_microsecond_cpu_are_distinct() {
    let source = Source {
        id: "group".into(),
        target: Target::CgroupV2 {
            path: "/sys/fs/cgroup/fixture".into(),
        },
        ownership: Ownership::CgroupMembers,
    };
    let mut raw = vec![
        raw("memory.current", b"1024\n".to_vec()),
        raw("memory.peak", b"2048\n".to_vec()),
        raw("memory.max", b"max\n".to_vec()),
        raw(
            "cpu.stat",
            b"usage_usec 1500\nuser_usec 1000\nsystem_usec 500\n".to_vec(),
        ),
    ];
    let (_, readings) = parse_snapshot(&source, &raw, Some("directory:1:2")).unwrap();
    assert_eq!(readings[2].value, Value::Unlimited);
    assert_eq!(
        readings[3],
        reading(Metric::CpuTime, Unit::Microseconds, Ok(1500))
    );
    raw[2].bytes.clear();
    raw[2].error = Some(Failure::Missing);
    let (_, readings) = parse_snapshot(&source, &raw, Some("directory:1:2")).unwrap();
    assert_eq!(
        readings[2].value,
        Value::Unavailable {
            reason: Failure::Missing
        }
    );
    raw[2].error = None;
    raw[2].bytes = b"4096\n".to_vec();
    assert_eq!(
        parse_snapshot(&source, &raw, Some("directory:1:2"))
            .unwrap()
            .1[2]
            .value,
        Value::Observed { value: 4096 }
    );
}

#[test]
fn cgroup_integral_and_reset_boundaries_replay_independently() {
    let source = Source {
        id: "process".into(),
        target: Target::CgroupV2 {
            path: "/sys/fs/cgroup/fixture".into(),
        },
        ownership: Ownership::CgroupMembers,
    };
    let make = |t: u64, cpu: u64, peak: u64| {
        let raw = vec![
            raw("memory.current", b"1024\n".to_vec()),
            raw("memory.peak", format!("{peak}\n").into_bytes()),
            raw("memory.max", b"max\n".to_vec()),
            raw("cpu.stat", format!("usage_usec {cpu}\n").into_bytes()),
        ];
        let (incarnation, readings) = parse_snapshot(&source, &raw, Some("directory:1:2")).unwrap();
        Snapshot {
            source: source.id.clone(),
            clock: clock().id,
            started_us: t,
            observed_us: t,
            incarnation,
            readings,
            raw,
            failure: None,
        }
    };
    let o = observation(
        source.clone(),
        vec![make(1000, 1000, 2048), make(2000, 2501, 2048)],
    );
    validate_observation(&o).unwrap();
    assert_eq!(
        summary_value(&o, &gate(Metric::CpuTime, Statistic::CpuTime)),
        Ok((1501, 1).into())
    );
    assert_eq!(
        summary_value(&o, &gate(Metric::CpuTime, Statistic::CpuUtilizationOneCpu)),
        Ok((1501, 1000).into())
    );
    let o = observation(
        source.clone(),
        vec![make(1000, 1000, 2048), make(2000, 2501, 1024)],
    );
    assert_eq!(
        summary_value(&o, &gate(Metric::MemoryUsed, Statistic::SampledMaximum)),
        Err(Failure::CounterReset)
    );
    let mut o = observation(
        source.clone(),
        vec![make(1000, 1000, 2048), make(2000, 2501, 2048)],
    );
    o.snapshots[1].incarnation = Some("directory:1:3".into());
    validate_observation(&o).unwrap();
    assert_eq!(
        summary_value(&o, &gate(Metric::MemoryUsed, Statistic::SampledMaximum)),
        Err(Failure::SourceChanged)
    );
}
fn power_source() -> Source {
    Source {
        id: "process".into(),
        target: Target::Imported {
            adapter: "fixture-power-v1".into(),
            device: Some("synthetic-device".into()),
            rank: Some(0),
        },
        ownership: Ownership::SharedMemory {
            group: "shared-allocation".into(),
        },
    }
}
fn power_snapshot(t: u64, microwatts: u64) -> Snapshot {
    let readings = vec![reading(Metric::Power, Unit::Microwatts, Ok(microwatts))];
    Snapshot {
        source: "process".into(),
        clock: clock().id,
        started_us: t,
        observed_us: t,
        incarnation: Some("source-epoch-1".into()),
        raw: vec![raw("readings.json", serde_json::to_vec(&readings).unwrap())],
        readings,
        failure: None,
    }
}
#[test]
fn sampled_energy_clips_linear_segments_with_exact_rational_arithmetic() {
    let mut o = observation(
        power_source(),
        vec![
            power_snapshot(0, 2_000_000),
            power_snapshot(1_000_000, 4_000_000),
        ],
    );
    let g = gate(Metric::Power, Statistic::SampledEnergyEstimate);
    validate_observation(&o).unwrap();
    assert_eq!(summary_value(&o, &g), Ok((3_000_000, 1).into()));
    o.measured.started_us = 250_000;
    o.measured.settled_us = 750_000;
    assert_eq!(summary_value(&o, &g), Ok((1_500_000, 1).into()));
    let o = observation(
        power_source(),
        vec![power_snapshot(0, 0), power_snapshot(1000, 1)],
    );
    assert_eq!(summary_value(&o, &g), Ok((1, 2000).into()));
    let mut o = o;
    o.snapshots.pop();
    assert_eq!(summary_value(&o, &g), Err(Failure::IncompleteExposure));
}
#[test]
fn energy_overflow_unknown_ownership_and_partial_visibility_are_unavailable() {
    let mut o = observation(
        power_source(),
        vec![
            power_snapshot(0, u64::MAX),
            power_snapshot(2_000_000, u64::MAX),
        ],
    );
    o.config.max_gap_us = 2_000_000;
    let g = gate(Metric::Power, Statistic::SampledEnergyEstimate);
    assert_eq!(summary_value(&o, &g), Err(Failure::Overflow));
    let mut o = observation(
        power_source(),
        vec![power_snapshot(0, 1), power_snapshot(1000, 1)],
    );
    o.config.sources[0].ownership = Ownership::Unknown;
    assert_eq!(summary_value(&o, &g), Err(Failure::Ownership));
    o.config.sources[0].ownership = Ownership::DedicatedDevice;
    o.snapshots[1].failure = Some(Failure::Missing);
    assert_eq!(summary_value(&o, &g), Err(Failure::Missing));
    let mut g = g;
    g.source = "unobserved-rank".into();
    assert_eq!(summary_value(&o, &g), Err(Failure::Missing));
    let mut c = config(process_source());
    c.sources[0].ownership = Ownership::HostShared;
    assert!(c.validate().is_err());
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "grill-resource-fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture_study() -> Study {
    Study {
        version: 1,
        id: "resource-envelope-fixture".into(),
        exposure_pin: "2".repeat(64),
        collector_sha256: "1".repeat(64),
        candidate_change: "declared-fixture-memory".into(),
        duration_us: 1000,
        arms: [Role::A, Role::B, Role::A2]
            .into_iter()
            .map(|role| Arm {
                role,
                deployment_pin: if role == Role::B { "4" } else { "3" }.repeat(64),
                config: config(process_source()),
                warmup_ids: vec![format!("{role:?}-warmup")],
                measured_ids: (0..3).map(|i| format!("{role:?}-measured-{i}")).collect(),
            })
            .collect(),
        gates: vec![gate(Metric::ResidentMemory, Statistic::SampledMaximum)],
    }
}
fn write_comparison(
    root: &Path,
    candidate_kb: u64,
    failed_candidate: bool,
) -> (PathBuf, [Vec<PathBuf>; 3]) {
    let plan = fixture_study();
    plan.validate().unwrap();
    let plan_path = root.join("study.json");
    let bytes = serde_json::to_vec(&plan).unwrap();
    evidence::write(&plan_path, &bytes).unwrap();
    let hash = evidence::digest(&bytes);
    let mut groups: [Vec<PathBuf>; 3] = std::array::from_fn(|_| Vec::new());
    for (arm_index, arm) in plan.arms.iter().enumerate() {
        for (ordinal, id) in arm.warmup_ids.iter().chain(&arm.measured_ids).enumerate() {
            let phase = if ordinal == 0 {
                Phase::Warmup
            } else {
                Phase::Measured
            };
            let kb = if arm.role == Role::B {
                candidate_kb
            } else {
                10000
            };
            let mut observation = observation(
                process_source(),
                vec![
                    process_snapshot(1000, 100, 10, kb),
                    process_snapshot(2000, 100, 13, kb),
                ],
            );
            observation.clock.id = id.clone();
            observation.measured.clock = id.clone();
            for s in &mut observation.snapshots {
                s.clock = id.clone();
            }
            if failed_candidate && arm.role == Role::B && ordinal == 2 {
                observation.failures.push(Failure::Cancelled);
            }
            let capture = Capture {
                version: 1,
                study_sha256: hash.clone(),
                acquisition_id: id.clone(),
                role: arm.role,
                phase,
                index: ordinal.saturating_sub(1) as u32,
                started_unix_ms: (arm_index * 8 + ordinal * 2) as u64,
                settled_unix_ms: (arm_index * 8 + ordinal * 2 + 1) as u64,
                observation,
            };
            let out = root.join(id);
            let bytes = serde_json::to_vec(&capture).unwrap();
            save(&out, &capture, Some(&bytes)).unwrap();
            groups[arm_index].push(out);
        }
    }
    (plan_path, groups)
}
#[test]
fn exact_aba2_gate_boundary_and_failure_population() {
    for (candidate, failed, expected) in [
        (10100, false, Outcome::Pass),
        (10101, false, Outcome::Regression),
        (9000, true, Outcome::Inconclusive),
    ] {
        let temp = Temp::new();
        let (plan, groups) = write_comparison(&temp.0, candidate, failed);
        let decision = compare(&plan, [&groups[0], &groups[1], &groups[2]]);
        assert_eq!(decision.decision, expected, "{:?}", decision.errors);
        assert_eq!(
            decision.claim,
            "imported-or-declared-resource-comparison-not-native-execution"
        );
        if !failed {
            assert_eq!(decision.gates[0].counts, [3; 3]);
            assert_eq!(decision.gates[0].warmup_counts, [1; 3]);
        }
    }
    let temp = Temp::new();
    let (plan, mut groups) = write_comparison(&temp.0, 10000, false);
    groups[2].pop();
    assert_eq!(
        compare(&plan, [&groups[0], &groups[1], &groups[2]]).decision,
        Outcome::Inconclusive
    );
    groups[0].swap(1, 2);
    assert_eq!(
        compare(&plan, [&groups[0], &groups[1], &groups[2]]).decision,
        Outcome::Error
    );
}
#[test]
fn import_cannot_promote_a_native_claim_and_digest_corruption_is_rejected() {
    let temp = Temp::new();
    let (plan, groups) = write_comparison(&temp.0, 10000, false);
    let mut captured = load(&groups[0][0]).unwrap();
    captured.observation.provenance = Provenance::NativeObserved;
    let input = temp.0.join("claimed-native.json");
    evidence::json(&input, &captured).unwrap();
    let out = temp.0.join("reimport");
    import(&input, &out).unwrap();
    assert_eq!(
        load(&out).unwrap().observation.provenance,
        Provenance::Imported
    );
    let file = out.join("observation.json");
    std::fs::write(file, b"{}").unwrap();
    assert!(load(&out).is_err());
    assert_eq!(
        compare(&plan, [&groups[0], &groups[1], &groups[2]]).decision,
        Outcome::Pass
    );
}

fn macos_source(target: Target) -> Source {
    Source {
        id: "process".into(),
        ownership: if matches!(target, Target::MacosProcess { .. }) {
            Ownership::ProcessAddressSpace
        } else {
            Ownership::HostShared
        },
        target,
    }
}
fn rusage(start: u64, exit: u64, footprint: u64, peak: u64) -> Vec<u8> {
    let mut bytes = vec![0xa5; RUSAGE_INFO_V4_BYTES];
    for (offset, value) in [(72, footprint), (80, start), (88, exit), (240, peak)] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
fn macos_process_snapshot(t: u64, start: u64, footprint: u64, peak: u64) -> Snapshot {
    let source = macos_source(Target::MacosProcess { pid: 7 });
    let raw = vec![raw("rusage_info_v4", rusage(start, 0, footprint, peak))];
    let (incarnation, readings) = parse_snapshot(&source, &raw, None).unwrap();
    Snapshot {
        source: source.id,
        clock: clock().id,
        started_us: t,
        observed_us: t,
        incarnation,
        raw,
        readings,
        failure: None,
    }
}
fn host_raw(pagesize: &[u8], free: u32, pressure: i32) -> Vec<Raw> {
    vec![
        raw("hw.memsize", (256u64 << 30).to_le_bytes().to_vec()),
        raw("vm.pagesize", pagesize.to_vec()),
        raw("vm.page_free_count", free.to_le_bytes().to_vec()),
        raw(
            "kern.memorystatus_vm_pressure_level",
            pressure.to_le_bytes().to_vec(),
        ),
    ]
}
#[test]
fn macos_rusage_record_is_exact_and_fails_closed() {
    let source = macos_source(Target::MacosProcess { pid: 7 });
    let parse = |bytes: Vec<u8>| parse_snapshot(&source, &[raw("rusage_info_v4", bytes)], None);
    let (incarnation, readings) = parse(rusage(99, 0, 3 << 30, 5 << 30)).unwrap();
    assert_eq!(incarnation.as_deref(), Some("pid:7:start_abstime:99"));
    assert_eq!(
        readings,
        [
            reading(Metric::MemoryUsed, Unit::Bytes, Ok(3 << 30)),
            reading(Metric::LifetimePeakMemory, Unit::Bytes, Ok(5 << 30)),
        ]
    );
    let mut truncated = rusage(99, 0, 1, 1);
    truncated.pop();
    assert_eq!(parse(truncated), Err(Failure::Malformed));
    let mut extended = rusage(99, 0, 1, 1);
    extended.push(0);
    assert_eq!(parse(extended), Err(Failure::Malformed));
    // A zeroed record has no process identity, so its zero footprint is not a reading.
    assert_eq!(
        parse(vec![0; RUSAGE_INFO_V4_BYTES]),
        Err(Failure::Malformed)
    );
    assert_eq!(parse(rusage(99, 100, 1, 1)), Err(Failure::Missing));
    let mut denied = raw("rusage_info_v4", Vec::new());
    denied.error = Some(Failure::Permission);
    assert_eq!(
        parse_snapshot(&source, &[denied], None),
        Err(Failure::Permission)
    );
}
#[test]
fn macos_pid_reuse_and_peak_reset_never_qualify() {
    let source = macos_source(Target::MacosProcess { pid: 7 });
    let g = gate(Metric::MemoryUsed, Statistic::SampledMaximum);
    let o = observation(
        source.clone(),
        vec![
            macos_process_snapshot(1000, 99, 10, 30),
            macos_process_snapshot(2000, 99, 20, 30),
        ],
    );
    validate_observation(&o).unwrap();
    assert_eq!(summary_value(&o, &g), Ok((20, 1).into()));
    let reused = observation(
        source.clone(),
        vec![
            macos_process_snapshot(1000, 99, 10, 30),
            macos_process_snapshot(2000, 100, 20, 30),
        ],
    );
    validate_observation(&reused).unwrap();
    assert_eq!(summary_value(&reused, &g), Err(Failure::SourceChanged));
    let reset = observation(
        source,
        vec![
            macos_process_snapshot(1000, 99, 10, 30),
            macos_process_snapshot(2000, 99, 20, 29),
        ],
    );
    assert_eq!(summary_value(&reset, &g), Err(Failure::CounterReset));
    // The lifetime peak is descriptive; only sampled footprint can be a gate.
    assert_eq!(
        summary_value(
            &o,
            &gate(Metric::LifetimePeakMemory, Statistic::SampledMaximum)
        ),
        Err(Failure::UnsupportedMetric)
    );
}
#[test]
fn macos_host_memory_values_are_observed_or_unavailable_never_substituted() {
    let source = macos_source(Target::MacosHostMemory);
    let values = |raw: &[Raw]| {
        parse_snapshot(&source, raw, None)
            .unwrap()
            .1
            .into_iter()
            .map(|r| (r.metric, r.value))
            .collect::<Vec<_>>()
    };
    let observed = |value| Value::Observed { value };
    let unavailable = |reason| Value::Unavailable { reason };
    assert_eq!(
        values(&host_raw(&16384u32.to_le_bytes(), 10, 1)),
        [
            (Metric::MemoryFree, observed(163_840)),
            (Metric::MemoryLimit, observed(256 << 30)),
            (Metric::MemoryPressureLevel, observed(1)),
        ]
    );
    let mut raw = host_raw(&16384u32.to_le_bytes(), 10, 3);
    assert_eq!(values(&raw)[2].1, unavailable(Failure::Malformed));
    raw[3].bytes = vec![4];
    assert_eq!(values(&raw)[2].1, unavailable(Failure::Malformed));
    raw[3].error = Some(Failure::Permission);
    assert_eq!(values(&raw)[2].1, unavailable(Failure::Permission));
    for pagesize in [0u32, 12288] {
        let raw = host_raw(&pagesize.to_le_bytes(), 10, 4);
        assert_eq!(values(&raw)[0].1, unavailable(Failure::Malformed));
        assert_eq!(values(&raw)[1].1, observed(256 << 30));
    }
    let raw = host_raw(&(1u64 << 63).to_le_bytes(), 2, 2);
    assert_eq!(values(&raw)[0].1, unavailable(Failure::Overflow));
    let mut raw = host_raw(&16384u32.to_le_bytes(), 10, 1);
    raw[0].bytes.truncate(7);
    assert_eq!(values(&raw)[1].1, unavailable(Failure::Malformed));
    let mut forged = observation(
        source,
        vec![Snapshot {
            source: "process".into(),
            clock: clock().id,
            started_us: 1000,
            observed_us: 1000,
            incarnation: Some("host-memory".into()),
            readings: parse_snapshot(
                &macos_source(Target::MacosHostMemory),
                &host_raw(&16384u32.to_le_bytes(), 10, 1),
                None,
            )
            .unwrap()
            .1,
            raw: host_raw(&16384u32.to_le_bytes(), 10, 1),
            failure: None,
        }],
    );
    validate_observation(&forged).unwrap();
    forged.snapshots[0].readings[2].unit = Unit::Bytes;
    assert!(validate_observation(&forged).is_err());
}
#[test]
fn macos_sources_never_mix_with_linux_contracts() {
    let mut c = config(macos_source(Target::MacosProcess { pid: 7 }));
    c.validate().unwrap();
    c.sources.push(Source {
        id: "host".into(),
        target: Target::HostMemory,
        ownership: Ownership::HostShared,
    });
    assert!(c.validate().is_err());
    c.sources[1] = macos_source(Target::MacosHostMemory);
    c.sources[1].id = "host".into();
    c.validate().unwrap();
    c.sources[0].ownership = Ownership::HostShared;
    assert!(c.validate().is_err());
    let mut o = observation(
        macos_source(Target::MacosProcess { pid: 7 }),
        vec![
            macos_process_snapshot(1000, 99, 10, 30),
            macos_process_snapshot(2000, 99, 20, 30),
        ],
    );
    o.provenance = Provenance::NativeObserved;
    o.adapter = MACOS_ADAPTER.into();
    assert!(validate_observation(&o).is_err());
    o.clock.kind = ClockKind::MacosUptimeRaw;
    validate_observation(&o).unwrap();
    assert!(!compatible_clocks(&clock(), &o.clock));
    o.adapter = ADAPTER.into();
    assert!(validate_observation(&o).is_err());
}
#[test]
fn native_observer_rejects_sources_of_another_platform() {
    let (native, foreign) = if cfg!(target_os = "macos") {
        (macos_source(Target::MacosHostMemory), process_source())
    } else {
        (
            Source {
                id: "host".into(),
                target: Target::HostMemory,
                ownership: Ownership::HostShared,
            },
            macos_source(Target::MacosProcess {
                pid: std::process::id(),
            }),
        )
    };
    let clock = host_clock("platform".into()).unwrap();
    assert!(Observer::start(config(foreign), Instant::now(), clock.clone()).is_err());
    Observer::start(config(native), Instant::now(), clock).unwrap();
}
#[cfg(target_os = "macos")]
fn live(target: Target) -> std::result::Result<(Option<String>, Vec<Reading>), Failure> {
    let source = macos_source(target);
    let raw: Vec<Raw> = expected_files(&source.target)
        .iter()
        .map(|name| read_macos(&source.target, name, 4096))
        .collect();
    parse_snapshot(&source, &raw, None)
}
#[cfg(target_os = "macos")]
#[test]
fn macos_observer_tracks_touched_footprint_and_replays() {
    const BLOCK: u64 = 256 << 20;
    let pid = std::process::id();
    let mut c = config(macos_source(Target::MacosProcess { pid }));
    c.sources.push(macos_source(Target::MacosHostMemory));
    c.sources[1].id = "host".into();
    c.max_read_us = 100_000;
    c.max_gap_us = 10_000_000;
    let origin = Instant::now();
    let clock = host_clock("live".into()).unwrap();
    assert_eq!(clock.kind, ClockKind::MacosUptimeRaw);
    let mut observer = Observer::start(c, origin, clock.clone()).unwrap();
    assert!(observer.sample().unwrap());
    let started_us = observer.last_observed_us().unwrap();
    // Anonymous mappings, not the allocator, so unmapping returns the pages at once.
    // SAFETY: a fresh private anonymous mapping; no existing memory is replaced.
    let block = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            BLOCK as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    assert_ne!(block, libc::MAP_FAILED);
    for offset in (0..BLOCK as usize).step_by(4096) {
        // SAFETY: offset is inside the writable mapping created above.
        unsafe { block.cast::<u8>().add(offset).write_volatile(1) };
    }
    assert!(observer.sample().unwrap());
    // SAFETY: unmaps exactly the mapping created above; nothing references it afterwards.
    assert_eq!(unsafe { libc::munmap(block, BLOCK as usize) }, 0);
    assert!(observer.sample().unwrap());
    let settled_us = observer.covered_end_us().unwrap();
    let o = observer
        .finish(
            MeasuredInterval {
                clock: clock.id,
                started_us,
                settled_us,
            },
            false,
        )
        .unwrap();
    assert_eq!(o.adapter, MACOS_ADAPTER);
    let value = |i: usize, metric| {
        numeric(
            &o.snapshots[i]
                .readings
                .iter()
                .find(|r| r.metric == metric)
                .unwrap()
                .value,
        )
        .unwrap()
    };
    let used = [0, 2, 4].map(|i| value(i, Metric::MemoryUsed));
    assert!(used[1] - used[0] >= BLOCK - (16 << 20), "{used:?}");
    assert!(used[1] - used[2] >= BLOCK - (16 << 20), "{used:?}");
    for i in [0, 2, 4] {
        assert!(value(i, Metric::LifetimePeakMemory) >= value(i, Metric::MemoryUsed));
        assert_eq!(o.snapshots[i].incarnation, o.snapshots[0].incarnation);
    }
    assert!(value(4, Metric::LifetimePeakMemory) >= used[1]);
    let mut memsize = 0u64;
    let mut len = size_of::<u64>();
    // SAFETY: fixed NUL-terminated name and an owned u64 output of the given length.
    let status = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut memsize).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    assert_eq!(status, 0);
    assert_eq!(value(1, Metric::MemoryLimit), memsize);
    assert!(value(1, Metric::MemoryFree) < memsize);
    assert!(matches!(value(1, Metric::MemoryPressureLevel), 1 | 2 | 4));
    assert_eq!(
        summary_value(&o, &gate(Metric::MemoryUsed, Statistic::SampledMaximum)),
        Ok((used[1], 1).into())
    );
}
#[cfg(target_os = "macos")]
#[test]
fn macos_exited_and_foreign_processes_are_unavailable() {
    let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    let pid = child.id();
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: waits for our own child without reaping it (WNOWAIT) into owned storage.
    let status = unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOWAIT,
        )
    };
    assert_eq!(status, 0);
    assert_eq!(live(Target::MacosProcess { pid }), Err(Failure::Missing));
    child.wait().unwrap();
    assert_eq!(live(Target::MacosProcess { pid }), Err(Failure::Missing));
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        assert_eq!(
            live(Target::MacosProcess { pid: 1 }),
            Err(Failure::Permission)
        );
    }
}
