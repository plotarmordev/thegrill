#!/usr/bin/env python3
"""Offline C1 calibration oracle; never collects evidence or changes verdicts."""

import argparse
from collections import Counter
import hashlib
import json
import math
from pathlib import Path
import platform
import random
import statistics
import sys


N = 8
CRITICAL = 2.364624251
FIELDS = (
    "result", "log_mean_difference", "heteroscedastic_standard_error",
    "observed_change_percent", "model_based_interval_percent",
    "model_based_confidence_level",
)
SCENARIOS = {
    "unchanged_iid": {"sigma_before": 0.1, "sigma_after": 0.1},
    "unchanged_correlated": {"sigma_before": 0.1, "sigma_after": 0.1, "rho": 0.8},
    "unchanged_drifting": {"sigma_before": 0.1, "sigma_after": 0.1, "drift": 0.015},
    "controlled_slowdown": {"sigma_before": 0.1, "sigma_after": 0.1, "ratio": 0.9},
    "unchanged_heterogeneous": {"sigma_before": 0.03, "sigma_after": 0.3},
    "missing_acquisition": {"sigma_before": 0.1, "sigma_after": 0.1, "missing": True},
    "interrupted_capture": {"sigma_before": 0.1, "sigma_after": 0.1, "interrupted": True},
    "zero_variance_unchanged": {"sigma_before": 0.0, "sigma_after": 0.0},
    "zero_variance_slowdown": {"sigma_before": 0.0, "sigma_after": 0.0, "ratio": 0.5},
}


def encoded(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode()


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def oracle(before, after):
    # statistics uses an independent summation path, not Rust's centered loop.
    report = dict.fromkeys(FIELDS)
    report["result"] = "INCONCLUSIVE"
    if len(before) != N or len(after) != N:
        return report
    if any(not math.isfinite(x) or x <= 0 for x in before + after):
        report["result"] = "INVALID"
        return report
    a, b = [list(map(math.log, values)) for values in (before, after)]
    difference = statistics.mean(b) - statistics.mean(a)
    se = math.sqrt((statistics.variance(a) + statistics.variance(b)) / N)
    try:
        interval = [100 * math.expm1(difference + sign * CRITICAL * se) for sign in (-1, 1)]
        observed = 100 * math.expm1(difference)
    except OverflowError:
        report["result"] = "INVALID"
        return report
    if not all(map(math.isfinite, interval + [observed])):
        report["result"] = "INVALID"
        return report
    report.update(log_mean_difference=difference, heteroscedastic_standard_error=se,
                  observed_change_percent=observed)
    if se == 0:
        return report
    report.update(model_based_interval_percent=interval, model_based_confidence_level=0.95)
    report["result"] = "IMPROVED" if interval[0] > 0 else "REGRESSED" if interval[1] < 0 else "INCONCLUSIVE"
    return report


def close(actual, expected):
    if isinstance(expected, list):
        return isinstance(actual, list) and len(actual) == len(expected) and all(
            close(a, e) for a, e in zip(actual, expected))
    if isinstance(expected, float):
        return isinstance(actual, (int, float)) and not isinstance(actual, bool) and math.isclose(
            actual, expected, rel_tol=1e-10, abs_tol=1e-10)
    return actual == expected


def formula_checks():
    log2 = math.log(2)
    se = log2 * math.sqrt(2 / (N - 1))
    before, after = [1.0, 4.0] * (N // 2), [2.0, 8.0] * (N // 2)
    checks = [
        ("analytic_log_moments", before, after, {
            "log_mean_difference": log2, "heteroscedastic_standard_error": se,
            "observed_change_percent": 100.0,
            "model_based_interval_percent": [100 * math.expm1(log2 - CRITICAL * se),
                                              100 * math.expm1(log2 + CRITICAL * se)],
            "result": "INCONCLUSIVE"}),
        ("zero_se_shift", [1.0] * N, [2.0] * N, {
            "result": "INCONCLUSIVE", "observed_change_percent": 100.0,
            "heteroscedastic_standard_error": 0.0, "model_based_interval_percent": None,
            "model_based_confidence_level": None}),
        ("zero_se_equal", [1.0] * N, [1.0] * N, {
            "result": "INCONCLUSIVE", "observed_change_percent": 0.0,
            "heteroscedastic_standard_error": 0.0, "model_based_interval_percent": None}),
        ("missing_precedes_value_check", [0.0], [1.0] * N, {
            "result": "INCONCLUSIVE", "observed_change_percent": None}),
        ("nonpositive_observation", [0.0] * N, [1.0] * N, {"result": "INVALID"}),
        ("numeric_overflow", [1e-300] * N, [1e300] * N, {"result": "INVALID"}),
    ]
    rows = []
    for name, before, after, expected in checks:
        actual = oracle(before, after)
        for field, value in expected.items():
            if not close(actual[field], value):
                raise ValueError(f"formula check {name}: {field}: {actual[field]!r} != {value!r}")
        rows.append({"id": "formula/" + name, "before": before, "after": after, "expected": actual})
    return rows


def sample(rng, spec):
    rho = spec.get("rho", 0.0)
    state = rng.gauss(0, 1)
    logs = []
    for index in range(2 * N):
        if index:
            state = rho * state + math.sqrt(1 - rho * rho) * rng.gauss(0, 1)
        candidate = index >= N
        sigma = spec["sigma_after" if candidate else "sigma_before"]
        logs.append(sigma * state + spec.get("drift", 0.0) * index
                    + (math.log(spec.get("ratio", 1.0)) if candidate else 0.0))
    before, after = [math.exp(x) for x in logs[:N]], [math.exp(x) for x in logs[N:]]
    if spec.get("missing"):
        before.pop(N // 2)
    if spec.get("interrupted"):
        after = after[:N // 2]
    return before, after


def proportion(numerator, denominator):
    if denominator == 0:
        return {"numerator": numerator, "denominator": denominator, "rate": None,
                "monte_carlo_se": None, "wilson_95": None}
    p = numerator / denominator
    z = statistics.NormalDist().inv_cdf(0.975)
    divisor = 1 + z * z / denominator
    center = (p + z * z / (2 * denominator)) / divisor
    half = z * math.sqrt(p * (1 - p) / denominator + z * z / (4 * denominator**2)) / divisor
    return {"numerator": numerator, "denominator": denominator, "rate": p,
            "monte_carlo_se": math.sqrt(p * (1 - p) / denominator),
            "wilson_95": [center - half, center + half]}


def cross_check(path, binary, corpus, pins):
    expected_header = {
        "kind": "c1-assess-crosscheck-v1", "corpus_sha256": digest(corpus),
        "study_sha256": pins["crates/grill-perf/src/study.rs"],
        "workload_sha256": pins["crates/grill-perf/examples/baseline-v1.json"],
        "evaluator_sha256": digest(binary),
    }
    count = 0
    with path.open() as results, corpus.open() as inputs:
        if json.loads(next(results)) != expected_header:
            raise ValueError("production cross-check header does not match runtime identities")
        for line in inputs:
            row = json.loads(line)
            actual = json.loads(next(results))
            if actual["id"] != row["id"]:
                raise ValueError("production row identity/order mismatch")
            for field in FIELDS:
                if field not in actual["report"] or not close(actual["report"][field], row["expected"][field]):
                    raise ValueError(f"production mismatch: {row['id']} / {field}")
            count += 1
        if next(results, None) is not None:
            raise ValueError("unexpected extra production rows")
    return {"status": "matched", "rows": count, "results_sha256": digest(path),
            "evaluator_sha256": expected_header["evaluator_sha256"],
            "build_provenance": "operator must independently verify binary/source correspondence"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True, help="new output directory; never overwritten")
    parser.add_argument("--seed", type=int, default=32)
    parser.add_argument("--replicates", type=int, default=2000)
    parser.add_argument("--production-results", type=Path)
    parser.add_argument("--production-binary", type=Path)
    args = parser.parse_args()
    if not 1 <= args.replicates <= 100000:
        parser.error("--replicates must be in 1..100000; choose before inspecting results")
    if bool(args.production_results) != bool(args.production_binary):
        parser.error("production results and binary must be supplied together")
    root = Path(__file__).resolve().parents[1]
    paths = [Path(__file__).resolve(), root / "docs/performance/CALIBRATION.md",
             root / "Cargo.toml", root / "Cargo.lock", root / "crates/grill-perf/Cargo.toml",
             root / "crates/grill-perf/examples/baseline-v1.json"]
    paths.extend(sorted((root / "crates/grill-perf/src").glob("*.rs")))
    pins = {str(path.relative_to(root)): digest(path) for path in paths}
    workload = json.loads((root / "crates/grill-perf/examples/baseline-v1.json").read_bytes())
    checks = formula_checks()
    args.out.mkdir(parents=True, exist_ok=False)
    corpus = args.out / "corpus.jsonl"
    scenarios = []
    with corpus.open("wb") as output:
        for row in checks:
            output.write(encoded(row))
        for name, spec in SCENARIOS.items():
            seed = int.from_bytes(hashlib.sha256(f"{args.seed}/{name}".encode()).digest(), "big")
            rng = random.Random(seed)
            counts = Counter()
            truth = math.log(spec.get("ratio", 1.0)) + N * spec.get("drift", 0.0)
            for replicate in range(args.replicates):
                before, after = sample(rng, spec)
                result = oracle(before, after)
                output.write(encoded({"id": f"{name}/{replicate}", "before": before, "after": after,
                                      "expected": result}))
                counts[result["result"]] += 1
                interval = result["model_based_interval_percent"]
                directional = result["result"] in ("IMPROVED", "REGRESSED")
                counts["directional"] += directional
                counts["false_direction"] += directional and (
                    truth == 0 or (truth < 0 and result["result"] == "IMPROVED")
                    or (truth > 0 and result["result"] == "REGRESSED"))
                if interval is not None:
                    counts["available"] += 1
                    counts["covered"] += interval[0] <= 100 * math.expm1(truth) <= interval[1]
            total, available = args.replicates, counts["available"]
            scenarios.append({
                "name": name, "parameters": spec, "rng_seed": seed,
                "true_period_log_difference": truth,
                "true_intervention_log_difference": math.log(spec.get("ratio", 1.0)),
                "outcomes": {key: counts[key] for key in ("IMPROVED", "REGRESSED", "INCONCLUSIVE", "INVALID")},
                "interval_available": proportion(available, total),
                "coverage_given_available": proportion(counts["covered"], available),
                "covered_and_available": proportion(counts["covered"], total),
                "directional": proportion(counts["directional"], total),
                "false_direction_per_planned": proportion(counts["false_direction"], total),
                "false_direction_given_directional": proportion(counts["false_direction"], counts["directional"]),
                "direction_without_intervention": proportion(counts["directional"], total)
                    if spec.get("ratio", 1.0) == 1.0 else None,
                "interval_withheld": total - available,
            })
    production = {"status": "not_run", "reason": "requires actual study::assess Rust hook; see CALIBRATION.md"}
    if args.production_results:
        production = cross_check(args.production_results, args.production_binary, corpus, pins)
    if any(digest(root / name) != sha for name, sha in pins.items()):
        raise ValueError("identity files changed during calibration; report withheld")
    requests = N * sum(cell["warmup_trials"] + cell["trials"] for cell in workload["cells"])
    report = {
        "kind": "c1-offline-calibration-v1", "seed": args.seed, "replicates_per_scenario": args.replicates,
        "identities_sha256": pins, "corpus_sha256": digest(corpus),
        "runtime": {"python": sys.version, "platform": platform.platform()},
        "formula_checks": {"status": "passed", "cases": [row["id"] for row in checks]},
        "production_cross_check": production,
        "method": {"acquisitions_per_capture": N, "critical_value": CRITICAL,
                   "nominal_confidence": 0.95, "sample_unit": "synthetic acquisition median",
                   "estimator": "difference of means of log acquisition medians; unpooled SE",
                   "assumptions": "independent stable log-scale acquisition variation; violated in named stress scenarios"},
        "budget": {"planned_pairs": args.replicates * len(SCENARIOS),
                   "planned_medians": args.replicates * len(SCENARIOS) * 2 * N,
                   "formula_pairs": len(checks), "live_requests": 0,
                   "workload_requests_per_complete_capture": requests,
                   "workload_output_token_ceiling_per_complete_capture": requests * workload["request"]["output"]["tokens"],
                   "stopping": "fixed replicates; no extension, replacement, retries or outcome-conditioned exclusions"},
        "scenarios": scenarios,
        "qualification": "offline synthetic oracle only; not live qualification or a product gate",
        "limitations": ["request timing, semantic output and receipt validation are not simulated",
                        "Monte Carlo bands describe simulation frequency uncertainty, not serving uncertainty",
                        "per-scenario marginal summaries; no simultaneous multi-cell coverage claim",
                        "no noninferiority, equivalence, useful-effect or guaranteed-sensitivity claim"],
    }
    (args.out / "report.json").write_bytes(encoded(report))
    print(args.out / "report.json")


if __name__ == "__main__":
    main()
