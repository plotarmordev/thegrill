# Offline C1 calibration

**Conclusion:** the existing arithmetic agrees with an independent calculation;
its nominal interval is not a serving-regression certificate. Correlated
acquisition summaries substantially inflate directional conclusions in the
specified unchanged simulation. Keep measured-period language, the fixed budget
and current verdict semantics. Concurrent and conversation selections remain
descriptive; this experiment does not calibrate those scopes.

## Executed experiment

The [machine-readable report](calibration-c1-v1.json) records exact SHA-256
identities for the driver, performance sources, workload, Cargo inputs, corpus,
production results and actual Rust test executable. Hashes identify bytes, not
attestation. The source is the issue29–32 implementation based on merged
`d6aa934535fb4f658e250103edad384b40d2ec62`; the report's file hashes identify the
experiment more precisely than that ancestor commit.

Executed with seed **32**, **2,000 independent replicate experiments per scenario**,
nine scenarios, six deterministic formula cases: **18,006 capture pairs**.
The synthetic scenarios budget 288,000 acquisition-median slots before their
prospective omissions. There were **zero model requests**. Each real C1 capture
would require eight acquisitions, 32 requests including warmups and at most
12,800 requested output tokens; simulations do not authorize that traffic.

| Synthetic population | Interval coverage of true period effect | Directional outcomes | Interpretation |
|---|---:|---:|---|
| Unchanged IID, log SD 0.1 | 1,931/2,000 (96.55%) | 69/2,000 (3.45%) | Either direction is false against the zero period effect |
| Unchanged AR(1), rho 0.8, log SD 0.1 | 1,205/2,000 (60.25%) | 795/2,000 (39.75%) | Dependence violates the independent-acquisition assumption |
| Unchanged intervention with log drift 0.015/slot | 1,940/2,000 (97.00%) | 961/2,000 (48.05%) | True **period** log difference is 0.12; directions are not evidence of an intervention |
| Candidate throughput multiplied by 0.9, log SD 0.1 | 1,918/2,000 (95.90%) | 829/2,000 (41.45%), all slower | Detection frequency at this specified effect/noise, not guaranteed sensitivity |
| Unchanged heterogeneous log SDs 0.03/0.3 | 1,899/2,000 (94.95%) | 101/2,000 (5.05%) | Unequal-variance example, not universal coverage |
| Fixed missing acquisition | Unavailable in all 2,000 | Withheld in all 2,000 | Length-check behavior, not a calibrated type-I error rate |
| Fixed interruption after four candidate acquisitions | Unavailable in all 2,000 | Withheld in all 2,000 | Summary withholding, not simulated native cancellation |
| Zero estimated variance, unchanged | Unavailable in all 2,000 | Withheld in all 2,000 | No estimable uncertainty |
| Zero estimated variance, 50% slowdown | Unavailable in all 2,000 | Withheld in all 2,000 | Observed effect remains visible, confidence/direction withheld |

For unchanged IID data, the Wilson 95% simulation-frequency interval around
3.45% is **2.74–4.34%**; for AR(1), the interval around 39.75% is **37.63–41.91%**.
These quantify Monte Carlo frequency uncertainty, not a particular serving
interval. Zero denominators are null. The missing/interrupted/zero-variance
rows demonstrate control flow; their zero directional counts are not evidence
of conservative inference. No observed zero count proves zero risk.

All **18,006 rows** matched the real private `study::assess` through the ignored
Rust hook, including outcomes, unavailable fields and numeric results within
absolute/relative tolerance `1e-10`. The hook was actually invoked; file agreement
alone could be forged and would not establish execution. Build: Cargo's default
test profile, locked dependencies, rustc 1.98.0 (`88d9e12ae`,
`aarch64-unknown-linux-gnu`). The machine report pins the executable used.

## Reproduce without model access

Requires Python with `hashlib.file_digest`, Cargo and `jq`. Run from the checkout:

```sh
OUT=$(mktemp -d)
cargo test -p grill-perf --locked --bin grill-perf --no-run \
  --message-format=json > "$OUT/build.jsonl"
TEST_BINARY=$(jq -r 'select(.reason == "compiler-artifact" and .profile.test == true and .target.name == "grill-perf") | .executable' "$OUT/build.jsonl")
python3 tools/calibrate-perf.py --seed 32 --replicates 2000 --out "$OUT/oracle"
GRILL_CALIBRATION_CORPUS="$OUT/oracle/corpus.jsonl" \
GRILL_CALIBRATION_RESULTS="$OUT/production.jsonl" \
  "$TEST_BINARY" study::calibration::crosscheck_corpus --ignored --exact
python3 tools/calibrate-perf.py --seed 32 --replicates 2000 \
  --out "$OUT/checked" --production-results "$OUT/production.jsonl" \
  --production-binary "$TEST_BINARY"
```

The driver is standard-library-only and separate from the fast test suite. It
writes into fresh directories, uses independently seeded scenario streams and
checks source identities again before publication. The hook calls the production
function, ignores oracle expectations and maps errors to `INVALID` as the CLI
does. Its header pins corpus, study source, workload and actual test executable;
the driver checks every ordered row. Keep build output, invocation and generated
artifacts with the report. Different source/doc bytes change identity fields;
compatible Python/math runtimes are needed for exact corpus reproduction.

This is a fixed prospective experiment, not retry-until-PASS. Rebuilding or
cross-checking the **same corpus** verifies implementation identity; it does not
add samples or improve a disappointing detection rate. Do not replace a seed,
exclude slow outcomes or extend collection after viewing results.

## Method, assumptions and claim boundaries

The unit is a synthetic **acquisition median**, not a request, lane or token.
Production's C1 median summarizes three measured waves; the simulation generates
medians directly and does not model those inner request distributions. Each
capture supplies eight positive finite values. On their logs:

- `d = mean(after) - mean(before)`;
- `SE = sqrt((sample_variance(before) + sample_variance(after)) / 8)`;
- observed change is `100 * expm1(d)`;
- nominal interval endpoints are `100 * expm1(d +/- 2.364624251 * SE)`.

The oracle uses independent `statistics.mean`/`statistics.variance` calculations;
Rust uses centered summation. The fixed conservative df7 critical value is the
existing method, not a newly fitted threshold. Zero SE withholds confidence;
missing counts withhold assessment before value validation; nonpositive or
numerically overflowing complete observations are invalid. Deterministic
powers-of-two fixtures check nonzero variances independently.

IID Gaussian log-medians, stationary AR(1), drift and variance ratios are stated
simulation assumptions—not qualified backend models. Coverage is against the
**period** effect. Under drift, `direction_without_intervention` measures the
risk of interpreting a period shift causally; `false_direction` against the
nonzero period effect answers a different question. Fresh clients and matching
declarations do not establish independence or remove carryover/confounding.

Direction versus zero, ruling out loss beyond a prospective tolerance, and
establishing improvement beyond a prospective useful-effect threshold are three
different claims. The latter two need justified bounds crossing their declared
margins; neither follows from a directional label, a zero-overlapping interval
or completed collection. No equivalence/noninferiority gate is added.

Results are marginal per named scenario. Multiple cells, history steps or repeated
checks require a prospective family and justified joint error control before a
wider claim; shared-wave lanes are not independent repetitions. Selected captures
therefore do not use this C1 interval. If the finite budget cannot resolve a
useful effect, report insufficient precision. Formula agreement, simulated
coverage and live serving qualification remain separate statuses.

## Output variation and optional diagnostics

C1 currently enforces transport/usage eligibility, **not semantic equality**.
Before any separately authorized live study, an operator must freeze its semantic
audit rule: for counting, ordered requested content without unrelated material,
with whitespace-only layout differences permitted. Check whether full completion
is compatible with the fixed exact-token contract; otherwise state that limitation.
This is a manual protocol requirement, not an implemented C1 grader. Conversation
v2 instead implements its explicitly bounded factual/strict checks.

Retain actual reported completion-token exposure, missing-usage counts, raw bytes,
channels and termination state. Equal declarations need not yield byte-identical
output or equal work. Blank lines alone are not serving-regression evidence;
stale content, additional reasoning, truncation and failures must not be repaired
or excluded after inspecting timing. Requested token ceilings are not measured
exposure. This calibration does not establish semantic-output stability.

Optional speculative telemetry is not required for a C1 result. The following is
an **operator analysis protocol**, not a changed metrics schema or automatic
product filter. Fix backend/counter definitions and scope before collection.
For same-scope proposed draft tokens `D` and accepted draft tokens `A`, work
acceptance is `A/D`. Use a prospective minimum `D >= 1000` for presenting that
ratio in this protocol; below it retain counts and report insufficient exposure.
Existing native raw diagnostics are not silently reinterpreted or suppressed by
this reporting convention. The floor concerns ratio granularity, not power.

Count emitted completion tokens `E` separately: `A/E` is not acceptance probability.
Only if accepted-but-not-emitted terminal work `T` is independently bounded and
`0 <= T <= A <= D` is established may `[(A-T)/D, A/D]` be called a bookkeeping
bracket—never a confidence interval. Without that evidence, do not invent a
terminal correction. Keep raw counts, missing/reset/wrap/label states and invalid
counter relationships; do not clamp them into plausible rates. Process-wide
snapshots need traffic and reset accounting and cannot identify a per-step cause.
Longer output improves granularity but establishes neither independence nor
unbiased inference. No live backend, concurrent/history inference or mandatory
merge gate was qualified by this experiment.
