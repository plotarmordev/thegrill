# Media CLI benchmarking

`media preflight` is experimental and image-only. It prints JSON without starting
processes or creating directories. Audio and video are not supported yet.

```text
grill-perf media preflight --workload W.json --runner R.json
```

The report contains raw workload/runner hashes, a typed workload hash, resolved
program hashes, each sample's argv, phase and seed, and counts. Its wall-clock
allowance sums run deadlines, pauses after every run and the setup deadline when
setup is declared; filesystem and OS stalls can exceed it. Output paths are
projections relative to a future capture directory, not executable capture argv.
Exit codes: **0** admitted, **1** rejected or unreadable, **2** CLI usage error.

## Documents

Both documents require their `kind` (`media-workload-v1` or `media-runner-v1`)
and `version: 1`. Records are JSON objects; unknown/duplicate fields and positional
records are rejected. Files and resolved programs are bounded to 4 MiB. Optional
fields are omitted, not null; absent optional workload fields are omitted from
typed serialization so later optional additions need not change existing hashes.

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
used in runner argv. `{output}` occurs only once and only for a single image;
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

## Planned

Capture, offline inspect and compare are **not available**. Capture will publish
a receipt before work, use direct exec (no shell), start a fresh process per
sample and serialize runs. Setup will run before warmups, outside measurement;
warmups and failures will be retained, warmups excluded, and failures never
retried or replaced. Declared env additions will supplement inherited values
without recording the inherited environment. Cache observations will accompany
declared labels; a fresh CLI will not prove a fresh model load.

Comparison will use A/B/A2 or A/B/B2/A2 and require at least three measured samples
per arm inside a capture (System One convention), not microbench's acquisitions
per role. Only `end_to_end_us` will gate direction. Container validation will be
`container_checked`, never `decoded`. Same-checkpoint comparisons will permit
envelope direction; changed-model or unavailable identity will remain
INCONCLUSIVE without a quality claim. Offline inspect will recheck retained bytes.

Prompts, program paths, declared env values and outputs are private evidence,
not a public export. Review preflight output before sharing it.
