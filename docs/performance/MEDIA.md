# Media CLI benchmarking

`media preflight`, `media capture`, `media inspect` and `media compare` are
experimental and support image, audio and video workloads on Linux and macOS.
Runner markers and external probes are not supported.
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

The shared workload declares `name`, `modality` (`image`, `audio` or `video`),
`request`, `outputs`, `cells`, `slots`, `limits` and `gates`. Names and IDs use ASCII
letters, digits, `-_.`, with length 1–64. All requests contain `prompt`, `seeds`,
optional `negative_prompt` and optional `guidance_milli` (descriptive only).
Image requests require `width`, `height`, `steps` and `images_per_run`.
Audio requests require `duration_us` and `sample_rate`; video requests require
`width`, `height`, `frames` and `fps`, with optional `duration_us`.
Audio and video may declare `steps`; image-only and other-modality fields are
rejected. Bounds: prompts are control-free and at most 16 KiB, seeds 1–16,
canvas 1–16384, steps 1–1000, images per run 1–64, frames 1–100000,
integer fps 1–240, duration 1–3600000000 microseconds, sample rate positive.

Cells (1–64) declare `id`, `warmup_runs` (0–20), `measured_runs` (1–100), optional
`pause_between_runs_ms` (default zero) and optional `request`, which replaces the
whole request. Slots declare unique `id` and `arm`: `a`, `b`, `a2`, optionally
`b2`. Gates (1–16) name unique declared cells, `metric: end_to_end_us`,
`max_adverse_bps` (0–9999) and `max_reference_spread_bps` (0–1000000).
Limits are positive `run_deadline_ms`, `setup_deadline_ms`, `output_bytes`,
`stdout_bytes` and `stderr_bytes`.

Outputs declare unique `placeholder` (`output` or `output_dir`), `container`
and `expect`. Images use `png` or `jpeg` and expect `width` and `height`.
Audio uses `wav` and expects `duration_us`, `sample_rate` and positive `channels`.
Video uses `mp4` and expects `width`, `height`, `frames` and integer `fps`;
optional `duration_us` checks sample-timing `media_duration_us`, not presentation
duration. `audio: true` requires an audio track with nonzero samples, sample bytes
and media duration; `false` requires its absence. Expectations use the request
bounds and exact equality. WAV and MP4 media durations round down to whole
microseconds.
At least one output is required. The declared set must equal the output placeholders
used in sample argv. Setup arguments may use only declared output placeholders
but need not use any. `{output}` occurs at most once in each argv; samples with
multiple images require only `{output_dir}`. The file extension comes from the
`output` declaration, not the directory declaration.

The per-arm runner owns `id`, `program`, `args`, `env`, `execution`, `cache`,
`pins`, optional `cwd` and optional `setup`. `execution` is `in-process`,
`delegates-to-resident-service` or `unknown`. Programs must be executable regular
files; relative paths resolve against the invoking directory, not `cwd`, without
PATH search. Leading `~` expands to HOME. A declared `cwd` must exist as a directory.
Capture executes the declared path, not its canonical symlink target, for both
runner and setup programs. Preflight and receipts record `declared_path` (expanded
and made absolute without resolving symlinks), `resolved_path` (canonical target)
and `resolved_sha256` (SHA-256 of the target bytes). The hash covers the target at
capture time, before execution; changes to symlink targets between admission and
execution are not guarded against.
Setup declares `args`, optional `program` (defaults to the runner program) and
optional `deadline_ms` (defaults to, and cannot exceed, the workload setup limit).
Cache declares `declared_state: cold|warm|unknown` and up to 64 `paths`, each at
most 4096 bytes; preflight does not inspect caches. Pins contain `path` and
`sha256`; only their form is checked.

Only argv is templated: `{prompt}`, `{negative_prompt}`, `{seed}`, `{width}`,
`{height}`, `{steps}`, `{frames}`, `{fps}`, `{duration_s}`, `{sample_rate}`,
`{output}`, `{output_dir}`. A numeric placeholder requires its request field;
`duration_s` renders microseconds as decimal seconds. Substitution is single-pass;
spaces, quotes and shell syntax stay literal in their argv element. An absent
negative prompt substitutes an empty string without removing the element.
Unknown or unclosed placeholders fail. There is no literal opening-brace escape;
JSON-valued arguments need an adapter. Env values and paths are not templates.

### MFLUX runner

Use the console script inside the virtualenv, such as
`VENV/bin/mflux-generate-...`, as `program`.
Pass `--no-exif` (or the runner's equivalent) when byte determinism by seed
matters: generation metadata changes PNG bytes.
Set `HF_HUB_OFFLINE=1` and `TRANSFORMERS_OFFLINE=1` in `env` so a capture
cannot download through Hugging Face.

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
`images_per_run` direct regular files for images, or one for audio/video.
No symlinks, subdirectories or hidden-file exceptions are allowed.
Names must be UTF-8 and are ordered lexically; a wrong count or entry
type makes the sample ineligible rather than selecting a subset.

Bounded outputs, including invalid media, are retained with SHA-256 hashes.
PNG signature, IHDR, every chunk CRC, IDAT presence and terminal IEND, or JPEG
SOI, SOF dimensions, SOS presence and terminal EOI are checked by content.
WAV checks RIFF framing, PCM/float format, sample alignment and byte rate.
MP4 checks box framing, movie and sample-description headers, sample byte ranges,
and progressive or fragmented track timing. Declared sample bytes must fit the
media-data payloads; progressive chunks must lie within a payload and fragment
runs within the following payload. Frame rate comes from sample count, durations
and timescale; retained `fps` has exact `numerator` and `denominator` fields.
Width and height expectations use coded sample-entry dimensions;
track-header display dimensions are reported separately, allowing anamorphic output.
Both sample-timing `media_duration_us` and presentation `duration_us` are retained.
A single unit-rate edit with nonnegative media time sets presentation duration;
its segment may exceed media duration by at most one movie-timescale tick for
muxer rounding. Without an edit, the durations are equal. Multi-entry or empty
edits, encrypted sample entries, multiple video tracks and unsupported handlers
fail closed.
Validation is `container_checked`, never `decoded`; it establishes neither
decoded correctness nor quality. Eligibility requires successful exit within the
deadline and every expected output with matching metadata and count.
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
rechecks retained hashes, reservations and container validations. Every run record
binds its capture's receipt digest; inspect rejects records from another receipt.
Timing count, minimum, median, mean and maximum cover only eligible measured
samples; setup, warmups and failed samples are excluded. Ineligible counts are
grouped by reason.
Determinism is reported by seed among eligible samples, unavailable without a
repeat. These observations are descriptive, not a comparison verdict.
Byte-identical MP4 output requires bit-exact muxing, including stable movie-header
creation times.
Each reported run includes `realtime_factor`: end-to-end time divided by checked
WAV duration, with values below one meaning faster than real time.
`frames_per_second_end_to_end` is checked video frames per second of end-to-end
time. Rates are null for other modalities, ineligible runs and zero durations.
These descriptive values add no gates.

Capture and inspect print JSON with capture status `complete`, `setup_failed`
or `cancelled`. The latter statuses exit unsuccessfully, as do unreadable,
incomplete or inconsistent evidence; individual sample failures remain visible
inside an otherwise completed capture.

Capture completion records require wall-clock `finished_unix_ms` and monotonic
`elapsed_ms` from capture start; the evidence digest covers the entire record
except the digest itself.
Inspect requires all process durations (setup, warmups and measured runs) to fit
within `elapsed_ms`, completion not to precede start, and the wall-clock interval
to be at least `elapsed_ms - (1000 + elapsed_ms / 1000)` milliseconds, allowing
clock drift and slew while a suspended machine may have a longer wall interval.
A backward clock step of more than about a second during a capture fails closed.

## Compare

```text
grill-perf media compare --a A --b B --a2 A2 [--b2 B2]
```

Compare fully rechecks every capture through inspect before admitting it.
All declared slots are required, with matching arms, byte-identical workloads,
and the same collector binary and platform. A and A2 must match runner bytes,
program and setup-program declared paths and resolved-bytes hashes, and deployment;
B must differ in at least one of those, and B2 must match B.
Runner bytes include setup arguments.

Comparison requires A to finish no later than B starts, then B2 if declared,
then A2. These comparisons use only wall-clock readings, so clock steps can make
ordering fail closed.
Incomplete captures and admission failures are INVALID errors, never performance
verdicts.

Each declared `end_to_end_us` gate pools eligible measured samples from A+A2
as reference and B+B2 as candidate. At least three samples per pooled arm are
required; any ineligible measured sample also withholds direction. Gates use
exact rational microseconds and the shared lower-is-better envelope rule.
Reference spread above the declared bound or an envelope crossing the tolerance
is INCONCLUSIVE. The worst gate outcome wins (ERROR > REGRESSION >
INCONCLUSIVE > PASS), before the identity cap.

Equal declared `model_revision` values select `same-checkpoint`, provided that
when both runners declare pins their sets of SHA-256 hashes match. Different
revisions or pin sets select `changed-model`: gates remain visible, but the
overall outcome is INCONCLUSIVE with `changed_model_axis`. Missing model
identity or incomplete reference deployment declarations also withhold direction.
Output quality is not compared.

The JSON report retains each capture's execution declaration and reports pooled
sample counts, minimum, median, mean, maximum, ineligible counts by reason and
within-arm determinism by seed. Seconds per image and images per second derive
from mean eligible duration and declared image count; they are display values,
not separate gates. Audio/video rates apply the per-run definitions to summed
eligible measured time and output amounts.
Cross-arm hash equality is only a byte-equality fact; it is
unavailable if either arm lacks eligible samples for the seed or is
nondeterministic.
Exit codes are **0** PASS, **1** ERROR/INVALID, **2** INCONCLUSIVE and
**3** REGRESSION.

## Private evidence

Prompts, program paths, declared env values and outputs are private evidence,
not a public export. Review preflight output before sharing it.
