# Media CLI benchmarking

`media preflight`, `media capture`, `media inspect` and `media compare` are
experimental and image-only. Capture supports Linux and macOS.
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
Cache declares `declared_state: cold|warm|unknown` and up to 64 `paths`, each at
most 4096 bytes; preflight does not inspect caches. Pins contain `path` and
`sha256`; only their form is checked.

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

Capture publishes declarations and a receipt before work. It uses direct exec,
declared environment additions and cwd, with a fresh owned process group for
each invocation. Runs are serial and pause as declared. Normal exit, deadline
and cancellation all kill the group, including descendants still in it; a
runner can escape the group, so this is not a sandbox. Cancellation interrupts
pauses and records remaining samples as unstarted, without replacements.

Optional setup runs once before samples, with separate timing, streams and
declared outputs. Failed setup stops the capture before any sample. Each sample
retains its reservation, exit status, failures and stdout/stderr prefixes;
truncation is nonfatal, and failures are never retried. `end_to_end_us` measures
Grill-monotonic time through observed process exit; the polling interval is
2 ms, not microsecond resolution. One capture represents one declared slot.

`{output}` names a collector-owned file. `{output_dir}` must contain exactly
`images_per_run` direct regular files: no symlinks, subdirectories or hidden-file
exceptions. Names must be UTF-8 and are ordered lexically; a wrong count or entry
type makes the sample ineligible rather than selecting a subset.

Bounded outputs, including invalid images, are retained with SHA-256 hashes.
PNG signature, IHDR, every chunk CRC, IDAT presence and terminal IEND, or JPEG
SOI, SOF dimensions, SOS presence and terminal EOI are checked by content.
Validation is `container_checked`, never `decoded`; it establishes neither pixel
correctness nor quality. Eligibility requires successful exit within the
deadline and every expected output with matching dimensions and count.
Oversized and missing outputs are recorded as failures.

Declared cache paths are observed before setup, after setup and after runs:
existence, regular-file count, bytes, newest mtime and symlink metadata/targets.
Symlinks are recorded without following them or stopping the walk. Incomplete
bounded comparisons report `cache_mutated_during_measurement` as `null` (unknown),
not `false`. Both observed growth and unknown growth withhold measured-sample
eligibility. Setup/warmup changes are separate. Nothing is flushed or purged,
and a fresh CLI does not prove a fresh model load. Reports retain the runner's
`execution` declaration.
The walk is bounded at 100,000 entries per declared cache; larger caches make
observations incomplete and measured samples ineligible, so declare the model's
own cache directory rather than a whole hub.

Inspect launches nothing and requires no original runner or cache files. It
rechecks retained hashes, reservations and container validations. Timing count,
minimum, median, mean and maximum cover only eligible measured samples; setup,
warmups and failed samples are excluded. Ineligible counts are grouped by reason.
Determinism is reported by seed among eligible samples, unavailable without a
repeat. These observations are descriptive, not a comparison verdict.

Capture and inspect print JSON with capture status `complete`, `setup_failed`
or `cancelled`. The latter statuses exit unsuccessfully, as do unreadable,
incomplete or inconsistent evidence; individual sample failures remain visible
inside an otherwise completed capture.

## Compare

```text
grill-perf media compare --a A --b B --a2 A2 [--b2 B2]
```

Compare fully rechecks every capture through inspect before admitting it.
All declared slots are required, with matching arms and byte-identical workloads.
A and A2 must match runner bytes, program hash and deployment; B must differ
in at least one of those, and B2 must match B.

Capture completion records require `finished_unix_ms`; inspect rejects missing
completion times or completion before the receipt's `started_unix_ms`.
Comparison requires A to finish before B starts, then B2 if declared, then A2;
equal boundary timestamps are allowed. This is wall-clock ordering, so clock
steps can make it fail closed. Incomplete captures and admission failures are
INVALID errors, never performance verdicts.

Each declared `end_to_end_us` gate pools eligible measured samples from A+A2
as reference and B+B2 as candidate. At least three samples per pooled arm are
required; any ineligible measured sample also withholds direction. Gates use
exact rational microseconds and the shared lower-is-better envelope rule.
Reference spread above the declared bound or an envelope crossing the tolerance
is INCONCLUSIVE. Gate outcomes aggregate using the shared policy ordering.

Equal declared `model_revision` values select `same-checkpoint`. Different
revisions select `changed-model`: gates remain visible, but the overall outcome
is INCONCLUSIVE with `changed_model_axis`. Missing model identity or incomplete
reference deployment declarations also withhold direction. Output quality is
not compared.

The JSON report retains each capture's execution declaration and reports pooled
sample counts, minimum, median, mean, maximum, ineligible counts by reason and
within-arm determinism by seed. Seconds per image and images per second derive
from mean eligible duration and declared image count; they are display values,
not separate gates. Cross-arm hash equality is only a byte-equality fact.
Exit codes are **0** PASS, **1** ERROR/INVALID, **2** INCONCLUSIVE and
**3** REGRESSION.

## Planned

Audio/video, runner markers and external probes are not supported.

Prompts, program paths, declared env values and outputs are private evidence,
not a public export. Review preflight output before sharing it.
