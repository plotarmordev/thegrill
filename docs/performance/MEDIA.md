# Media CLI benchmarking

`media preflight`, `media capture` and `media inspect` are experimental and
image-only. Capture supports Linux and macOS; audio and video are not supported.
Preflight prints JSON without starting processes or creating directories.

```text
grill-perf media preflight --workload W.json --runner R.json
```

The report contains raw workload/runner hashes, a typed workload hash, resolved
program hashes, each sample's argv, phase and seed, and counts. Its wall-clock
allowance sums run deadlines, pauses after every run and the setup deadline when
setup is declared; filesystem and OS stalls can exceed it. Output paths are
projections relative to a future capture directory, not executable capture argv.
Exit codes: **0** admitted; **1** rejected, unreadable or CLI usage error.

## Documents

Both documents require their `kind` (`media-workload-v1` or `media-runner-v1`)
and `version: 1`. Records are JSON objects; unknown/duplicate fields and positional
records are rejected. Documents are bounded to 4 MiB; resolved programs to 1 GiB.
Optional fields are omitted, not null; absent optional workload fields are omitted
from typed serialization so later optional additions need not change existing hashes.

The shared workload declares `name`, `modality: image`, `request`, `outputs`,
`cells`, `slots`, `limits` and `gates`. Names and IDs use ASCII letters, digits,
`-_.`, with length 1–64. Requests contain `prompt`, `seeds`, `width`, `height`,
`steps`, `images_per_run`, optional `negative_prompt` and optional
`guidance_milli` (descriptive only; no guidance substitution). Bounds: prompts
are control-free and at most 16 KiB (no newlines/tabs), seeds 1–16, canvas
1–16384, steps 1–1000, images per run 1–64.

Cells (1–64) declare `id`, `warmup_runs` (0–20), `measured_runs` (1–100), optional
`pause_between_runs_ms` (default zero) and optional `request`, which replaces the
whole request. Slots declare unique `id` and `arm`: `a`, `b`, `a2`, optionally
`b2`. Gates (1–16) name unique declared cells, `metric: end_to_end_us`,
`max_adverse_bps` (0–9999) and `max_reference_spread_bps` (0–1000000).
Limits are positive `run_deadline_ms`, `setup_deadline_ms`, `output_bytes`,
`stdout_bytes` and `stderr_bytes`.

Outputs declare unique `placeholder` (`output` or `output_dir`), `container`
(`png` or `jpeg`) and `expect: {width, height}` with the same canvas bounds.
At least one is required. The declared set must equal the output placeholders
used in sample argv. Setup arguments may use only declared output placeholders
but need not use any. `{output}` occurs at most once in each argv; samples with
multiple images require only `{output_dir}`. The file extension comes from the
`output` declaration, not the directory declaration.

The per-arm runner owns `id`, `program`, `args`, `env`, `execution`, `cache`,
`pins`, optional `cwd` and optional `setup`. `execution` is `in-process`,
`delegates-to-resident-service` or `unknown`. Programs must be executable regular
files; relative paths resolve against the invoking directory, not `cwd`, without
PATH search. Leading `~` expands to HOME. A declared `cwd` must exist as a directory.
Setup declares `args`, optional `program` (defaults to the runner program) and
optional `deadline_ms` (defaults to, and cannot exceed, the workload setup limit).
Cache declares `declared_state: cold|warm|unknown` and `paths`; preflight does not
inspect or purge caches. Pins contain `path` and `sha256`; only their form is checked.

Only argv is templated: `{prompt}`, `{negative_prompt}`, `{seed}`, `{width}`,
`{height}`, `{steps}`, `{output}`, `{output_dir}`. Substitution is single-pass;
spaces, quotes and shell syntax stay literal in their argv element. An absent
negative prompt substitutes an empty string without removing the element.
Unknown or unclosed placeholders fail. There is no literal opening-brace escape;
JSON-valued arguments need an adapter. Env values and paths are not templates.
Non-image fields, marker fields/placeholders and probes are not admitted.

## Capture and inspect

```text
grill-perf media capture --workload W.json --runner R.json --deployment D.json --slot a --out capture
grill-perf media inspect capture
```

Capture publishes the declarations and receipt before setup or sample execution.
It uses direct exec, declared environment additions and cwd, and an owned process
group per invocation. Runs are serial, with the declared pause after each run;
deadlines and cancellation kill the owned group, including descendants that
remain in it. A runner can escape the group; this is not a sandbox.

Optional setup runs once before warmups and has separate timing and streams.
Every sample has a reservation, status, exit code, Grill-monotonic
`end_to_end_us`, retained stdout/stderr prefixes and truncation flags. Truncation
is nonfatal. Failures are retained, never retried or replaced; cancellation also
records unstarted samples as cancelled. Warmups are excluded from summaries.
One capture contains the measured samples for one declared slot.

`{output}` names a collector-owned file. `{output_dir}` is a fresh empty directory
that must contain exactly `images_per_run` direct regular files, with no symlinks,
subdirectories or hidden-file exceptions; all names are retained in lexical byte
order with their original filename bytes recorded. Invalid counts or entry types
make the sample ineligible rather than selecting a subset.

Bounded output files are copied into each run's `retained/` directory with SHA-256
hashes. PNG signature, IHDR, chunk CRCs and terminal IEND, or JPEG SOI, SOF
dimensions and terminal EOI are checked by content, not extension. Validation is
`container_checked`, never `decoded`; it does not establish pixel correctness or
quality. Eligibility requires successful exit within the deadline and every
expected output with matching dimensions and count. Oversized or missing files
are recorded as failures, not loaded into retained output buffers.

Declared cache paths are observed before setup, after setup and after runs:
existence, regular-file count, bytes and newest mtime. Walks are bounded and do
not follow symlinks; incomplete observations carry errors. Growth during measured
runs is recorded separately from setup/warmup changes. Nothing is flushed or
purged, and a fresh CLI does not prove a fresh model load. The execution
declaration accompanies timing reports.

Inspect launches nothing and requires no original runner or cache files. It
rechecks retained hashes, sample argv projections and container validations,
then reports measured timing count, minimum, median, mean and maximum, eligible
count and determinism by seed among eligible samples. Timing summaries include
failed measured samples; warmups and setup are excluded. A seed with no repeat
has unavailable determinism. These are descriptive observations, not a verdict.
Capture and inspect print JSON and exit successfully even when samples failed;
unreadable, incomplete or inconsistent evidence is an error.

## Planned

Compare remains unavailable. It will use A/B/A2 or A/B/B2/A2 and require at least
three measured samples per arm. Only `end_to_end_us` will gate direction.
Same-checkpoint comparisons will permit envelope direction; changed-model or
unavailable identity will remain INCONCLUSIVE without a quality claim.

Prompts, program paths, declared env values and outputs are private evidence,
not a public export. Review preflight output before sharing it.
