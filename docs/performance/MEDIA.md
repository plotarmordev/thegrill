# Media CLI benchmarking

Status: experimental, **preflight only** in this release. `capture`, offline
`inspect` and envelope `compare` are planned, not available. Audio and video
return a “not supported yet” error. No media process is started by preflight.

```
grill-perf media preflight --workload W.json --runner R.json [--json]
```

Preflight admits closed JSON objects, rejects unknown and duplicate fields,
resolves program symlinks, hashes the resolved bytes, and prints each planned
sample's argv, phase and seed. It reports raw workload/runner hashes, the typed
workload hash, warmup and measured counts, and a per-capture wall-clock allowance:
run deadlines plus declared pauses after each run, plus setup's deadline when
present. This is a declared work ceiling, not a guarantee against filesystem or
OS stalls. Output paths are relative projections into a future capture directory;
preflight creates no directories. Exit status is success for an admitted plan,
error for invalid input or unavailable program bytes; CLI usage follows the
existing command convention. JSON contains the same plan as text, with separate
fields for environment additions, working directory and declared cache paths.

## Documents

The shared `media-workload-v1` document declares `version`, `name`, image
`modality`, `request`, `outputs`, `cells`, `slots`, `limits` and `gates`.
The per-arm `media-runner-v1` declaration separately owns `version`, `id`,
`program`, `args`, `env`, nullable `cwd` and `setup`, `cache`, nullable `markers`
and `probe`, and `pins`. Workload data never selects an executable.

Requests contain `prompt`, nullable `negative_prompt`, `seeds`, `width`, `height`,
`steps`, nullable `guidance_milli`, and `images_per_run`. Image requests reject
non-null `frames`, `fps`, `duration_us` and `sample_rate`. A cell's optional
`request` replaces the whole request, never merges it. Cells declare `id`,
`warmup_runs`, `measured_runs` and optional `pause_between_runs_ms` (default zero).
Gates name a cell, `end_to_end_us`, `max_adverse_bps` and
`max_reference_spread_bps`. Outputs declare `placeholder`, `container` (`png` or
`jpeg`) and `expect: {width, height}`; placeholders are unique `output` or `output_dir`.
`{output}` may appear at most once in argv and only for a single image per run.
Multiple images require `{output_dir}` instead; collection from that directory
belongs to the planned capture command, not preflight.
Limits declare `run_deadline_ms`, `setup_deadline_ms`, `output_bytes`,
`stdout_bytes` and `stderr_bytes`.

Runner argv uses only `{prompt}`, `{negative_prompt}`, `{seed}`, `{width}`,
`{height}`, `{steps}`, `{frames}`, `{fps}`, `{duration_s}`, `{sample_rate}`,
`{output}`, `{output_dir}` and `{marker_file}`. Modality-inapplicable placeholders
and unknown placeholders fail preflight. Substitution is single-pass; prompt
text is not a second template. Arguments remain separate strings, including
spaces, quotes and shell syntax. Planned execution is direct exec, never a
shell. Program declarations are file paths, not PATH searches. Only leading
`~` path expansion is supported. Environment additions supplement the inherited
environment; inherited values are not recorded.

Setup declares `args`, optional `program` (defaults to the runner program), and
optional `deadline_ms` bounded by the workload setup deadline. It is planned
once before warmups and excluded from measurements. Cache declares
`declared_state: cold|warm|unknown` and `paths`; no cache is inspected or purged
by preflight. Pins declare `path` and `sha256`; preflight checks their form but
does not verify their bytes. Marker collection and external probes are planned;
non-null probes and enabled markers are rejected now.

## Planned measurement contract

Each sample starts a fresh process; samples run serially. `execution` must be
`in-process`, `delegates-to-resident-service` or `unknown`, and accompanies
end-to-end numbers: a fresh CLI process does not prove a fresh model load.
Warmups and failures are retained, warmups excluded from every statistic, and
failed samples never retried, dropped or replaced. Progress-output truncation
is a retained observation rather than an artifact-validation failure.

Slots declare `id` and `arm`: A/B/A2, optionally A/B/B2/A2. Each capture contains
its arm's measured samples. Directional gates require at least three measured
samples per arm, following System One, not microbench's three acquisitions per
role with repeated iterations inside each acquisition. Smaller admitted plans
are descriptive only. Candidate envelopes pool B and B2; reference envelopes
pool A and A2. Excess reference spread or missing eligible samples is
INCONCLUSIVE, not evidence of equality.

The planned capture publishes a receipt before work, retains exact declarations
and outputs, and observes declared cache paths before/after setup and after runs.
Observed cache growth during measurement withholds a directional verdict.
Offline inspect rechecks retained bytes. The sole initial gate is
`end_to_end_us` (spawn to observed exit, lower is better); rates and setup time
are descriptive. Container validation will be labelled `container_checked`,
never `decoded`; neither hashes nor structural checks establish output quality.

Comparison uses existing deployment declarations. A and A2 must match runner
bytes, program hash and deployment; B must differ. A same-checkpoint axis can
support envelope direction. A changed-model axis remains INCONCLUSIVE and
makes no quality comparison; unavailable model identity also withholds direction.

Prompts, program paths, declared environment values and outputs are private
evidence, not a public export. Preflight output itself can expose these values.
Do not publish it without review.
