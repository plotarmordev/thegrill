# Kernel and collective evidence protocol (#54)

HTTP output speed cannot prove that an isolated kernel or collective changed.
`grill-perf microbench` keeps those observations in their own closed artifact
family so the distinction is enforced by the tool rather than by a report
author's memory.

Three states are separate and never silently upgraded:

| State | Meaning |
|---|---|
| `declared` | An operator statement with no measurement bytes. A declared artifact is rejected as measurement evidence. |
| `imported` | Bytes this collector parsed but did not produce. Importing always records `imported`, even when the payload claims native execution; the payload's own claim is retained in `submitted_provenance` for audit. |
| `native_observed` | Bytes produced by a program this collector launched, or by the in-process CPU reference. This records *observation*, never authenticated execution. |

Every runtime pin inside an artifact (installed versions, loaded module hash,
device names, the parameters actually used, how many pinned sources were read
from bytes) is an unauthenticated observation of what the producing process saw.
No declared implementation pin is echoed back as observed kernel provenance. A
kernel or collective result never sets a serving-speed claim; that needs a
separately linked serving acquisition.

## Commands

```
grill-perf microbench capture --adapter <id> --plan PLAN --out DIR [--program PATH] [--authorize-device-window] [-- ARGS...]
grill-perf microbench import --artifact FILE --plan PLAN --out DIR
grill-perf microbench inspect PATH
grill-perf microbench compare --study STUDY --baseline DIR --candidate DIR --reference DIR
```

`capture` writes `plan.json`, `artifact.json` and `receipt.json` into a fresh
`DIR`; an existing directory is never overwritten. External capture additionally
retains bounded `stdout.bin` and `stderr.bin` on success **and failure**, including
malformed JSON, nonzero exit and program setup/spawn failure (empty files when
no bytes were observed). A failed launch need not have `artifact.json`.
`import` writes the original three files with `provenance: imported`; CPU and
import receipts have `program_output: null`.

External receipts carry `program_output` with `stdout_sha256`, `stderr_sha256`,
`stdout_bytes`, `stderr_bytes`, `stdout_truncated`, and `stderr_truncated`.
Hashes and lengths describe the exact retained prefixes: stdout is capped at
1 MiB and stderr at 64 KiB, independently. Neither counter claims to count bytes
discarded beyond its cap. A cap, read/setup failure or incomplete drainage is
explicitly recorded in `failure`; any truncation withholds native eligibility.
`source_sha256` keeps its source-input meaning: original imported artifact bytes
for import, retained producer stdout for native external capture, null for CPU.
It is not repurposed as a stderr or receipt hash.

Directory inspection and comparison verify present receipts and raw manifests,
bounded regular raw files, byte lengths, hashes and the native stdout/source
digest binding. Complete native stdout is parsed again and must equal
`artifact.json` after the collector's sole provenance normalization. A missing
manifest cannot qualify a native external acquisition; old receipts remain
readable, not silently upgraded. Missing/tampered raw bytes, a rewritten artifact
without matching source bytes, or truncation withhold a favorable comparison.
Failed captures' present raw manifests are checked even when no artifact exists.
This is consistency checking, not authenticated execution: rewriting an entire
self-consistent evidence directory is not prevented.

`inspect FILE` remains plain structural inspection (`acquisition_checked: false`);
it checks any present sibling raw manifest but does not establish an eligible
native acquisition. `inspect DIR` performs acquisition checks
(`acquisition_checked: true`). Neither flag is device qualification or a study
verdict; inspect reasons and compare the complete prospectively declared study.

Exit codes follow the existing observed-envelope order: `0` PASS, `1` ERROR
(invalid, corrupt, drifted or incomplete-declaration evidence), `2`
INCONCLUSIVE (retained but unavailable), `3` REGRESSION. `compare` uses the
shared `envelope.rs` exact rational arithmetic — no second statistical engine
and no floating-point decision path.

## Closed adapters and frozen cells

| Adapter | Scope | Where it executes | Frozen cell |
|---|---|---|---|
| `cpu-sum-u64-reference` | CPU | This binary, in process | `sum_u64` over `0..4096`, exact reference `n*(n-1)/2`, warmups 2, measured iterations 5 |
| `exl3-e3-grouped` | Device | Operator program, explicitly launched | `apply_exl3_experts`, hidden 4096, intermediate 1024, experts 288, topk 8, tokens 1024, layer_seed 0, routing_seed 1, parity routing seed 3, activation seed 3, skew 1.0, tier E3 grouped, cap 32, warmups 2, measured iterations 5 |
| `nccl-allreduce-sum` | Ranks | Operator program, explicitly launched | `all_reduce` SUM, `int64`, numel 262144, declared world and ranks 0..world-1, warmups 1, measured iterations 5 |

The frozen cells are checked in code, not only documented: a plan that changes
the shape, cap, skew, tier, dtype, routing, clock, warmup count or bit-width, or
that declares an allowance below the cell's derived minimum work or memory, is
rejected before anything runs. There is no shape, cap, skew, tier or world
search, no best-median summary, no `torch.cuda.empty_cache()`, no remote
discovery and no script-hook framework. A plan names one adapter; the device
adapters additionally name one explicit `--program` whose SHA-256 the collector
hashes and checks against `plan.program_sha256`.

Device and collective capture is behind `--authorize-device-window`. Without
it, `capture` refuses before creating any output directory. Under the current
CPU-only authorization only the in-process CPU reference is exercised natively;
the device adapters are delivered callable code and remain unexercised.

## Observed identity versus declared expectations

`revision` is an **identity**, not a label:

* `kernel_revision` is the descriptor the producing process actually observed.
* `revision` is the SHA-256 hex of that descriptor's UTF-8 bytes (ASCII
  descriptor, no trailing newline), computed by the producer.
* A plan's `revision` is the operator's **expected** hash. It is validated
  structurally (64 lowercase hex characters). A mismatching expectation is
  *retained*: the artifact carries the hash it actually observed, and the
  collector rejects the mismatch when it compares evidence. It is never
  synthesized, copied from the plan, or used to suppress an otherwise valid
  body.

Descriptors:

| Adapter | `kernel_revision` descriptor |
|---|---|
| `cpu-sum-u64-reference` | `cpu-sum-u64-reference-v1;collector:<collector_binary_sha256>` (implemented by the collector in Rust; ASCII, no newline) |
| `exl3-e3-grouped` | `torch:<version>;nccl:<version>;exl3-module:<sha256 of the loaded exl3 module file>` |
| `nccl-allreduce-sum` | `torch:<version>;nccl:<version>;devices:<comma-joined participating device names>` |

The observed descriptor is the only admitted change axis: every other plan pin
(adapter, program hash, source pin set, operation, clock, warmups, iterations,
allowance, correctness contract, thresholds) must match across a study, and the
measurement *program* must not change. An A/A control that declares the same
revision for all three roles is admitted and is reported as a control; inventing
a second revision to look like a candidate is not permitted.

Every artifact also carries the complete `plan.acquisition` object verbatim, so
two acquisitions with identical duration bodies are still bound to distinct
prospective acquisitions. That object is a **declaration**, not chronology:
native chronology comes from the collector receipt's observed start and end
times, never from the declared `acquisition.started_unix_ms`.

## Pinned public sources

The two device adapters bind to reviewed public sources. The hashes below were
recorded before implementation and are re-verified at run time from the bytes
the adapter reads; a mismatch aborts before any device work.

```
MiaAI-Lab/GLM-5.3-Flash-EXL3-2x-DGX-Sparks @ f906ee990596486e10ddbe381efa6f0e496f77e3
  tests/bench_e3_microbench.py   f57e4fa6726b0110c44ce56b0b0968ba425e566ace6d89f2db3872aae13889ec
  tests/test_exl3_overlay.py     8777254d47cdbb89186ba3bd1ac2437cbebc7987e0791d36c68e7bccb3df2b35
NVIDIA/nccl-tests doc/PERFORMANCE.md
    2242493053a6f9d5db1c6542a8ce7740be919778645125b94eeae440efd31edf
```

`exl3_e3_grouped.py` imports `make_layer`, `routing`, `time_fn` and `set_cap`
from the pinned anchor and `_err_stats` / `_assert_e3_within` from the pinned
tolerance file; it never calls the anchor's `main()`. `nccl_allreduce_sum.py`
uses the pinned byte definitions for its reported bandwidth labels.

**Sources are observed, not echoed.** A plan pins `sources[].sha256` (paths are
declared hints). The artifact retains the sources the producer actually read
with the hashes it computed from those bytes; the collector requires the
observed hash set to equal the pinned hash set exactly, so a producer cannot
drop, add or substitute a pin, while a different filesystem path for the same
bytes is fine. Each adapter also reports how many pins it verified from bytes
(`sources-verified`) and how many it could only echo as declarations
(`sources-declared`). The E3 adapter requires every pin to be verified; the
collective adapter always verifies at least its own program and may declare a
reference document it cannot resolve on the producing host. The one pin the
collector verifies directly is `program_sha256`, because it hashes the program
it launched.

## Correctness is not optional

`exl3-e3-grouped` runs the production-geometry parity check *before* any timing
with the frozen tolerances (`E3_TOL_FACTOR = 1.5`, `E3_TOL_ABS_REL = 1e-3`,
`E3_TOL_NRMSE_ABS = 1e-4`, coarse bound
`max(0.15, 0.08 * max(1.0, ref_max))`). The collector recomputes every bound
using the pinned Python helper's binary64 arithmetic and operation order on the
retained reference maximum and E2 statistics. These are observed float
statistics, not rational timer samples. For example, its nRMSE boundary
`1.5 * 0.01 + 0.0001` is `0.015099999999999999`; `0.0151` must fail.
A widened tolerance is an identity error. Non-finite values, numeric underflow
to zero, and overflowing bounds fail closed. The coarse bound is strict; the
other bounds are inclusive. Duration samples still use exact rational arithmetic.

The retained reference maximum is the **E2** value, because the pinned public
`_assert_e3_within` derives both the per-key floor `1e-3 * e2["ref_max"]` and
the coarse bound `max(0.15, 0.08 * max(1.0, e2["ref_max"]))` from
`e2["ref_max"]`. Parity statistics are emitted as the float's own
representation (`repr(float(value))`), never re-rounded to a fixed number of
decimals; scientific notation is accepted on the wire.

The effective tier must be the pinned `grouped` token, and the raw fallback
token is preserved as observed:

| Raw overlay token (`fallback-tier`) | Normalized `execution.observed_fallback` | Meaning |
|---|---|---|
| `grouped` | `e3-grouped` | the grouped tier performed the timed work |
| `kernel` | `e2-kernel` | a lower tier performed it: retained failure, withheld |
| `unavailable` (or any unknown token) | absent (`null`) | nothing usable was observed: retained failure, withheld |

A lower or unavailable tier still produces a parseable artifact: the raw token
is retained verbatim in the observation, a `tier-fallback` failure is retained,
and the collector withholds the measurement. The adapter never substitutes a
declared tier or the collector's own spelling for what the overlay actually
reported, and never aborts emission merely because the tier was not grouped.
Numerical work uses synthetic Zipf routing, labelled synthetic in the operation
identity — it is not the served routing distribution.

`nccl-allreduce-sum` verifies exact integer equality of the complete tensor
against `world*(i%251) + world*(world-1)/2`; the reduction reference has no
tolerance. Faster timing can never outweigh a correctness failure: a failed
parity or a nonzero mismatch count makes the acquisition ineligible and the
comparison ERROR or INCONCLUSIVE, never PASS.

**Warmups.** Required warmups are executed and their timings never enter the
measured samples. A correctness failure during a warmup on **any rank** remains
a failure in rank 0's merged artifact, with the originating rank and warmup
population retained. Only measured repetitions contribute to the measured
mismatch count. Warmups are required
work, not disposable work, and they are not a licence to hide a failure that
happened while the cell was warming.

## Clocks, units and samples

Every artifact carries exactly one clock identity, kind, units, declared
resolution and synchronization contract, and the plan must declare the identical
contract:

| Adapter | Clock | Units | Declared resolution | Synchronization |
|---|---|---|---|---|
| `cpu-sum-u64-reference` | `process-monotonic-ns` | nanoseconds | 1 ns | read before and after the operation |
| `exl3-e3-grouped` | `cuda-event-ms` | milliseconds | 1000 ns | CUDA events with a device synchronize before each read |
| `nccl-allreduce-sum` | `rank-monotonic-ns` | nanoseconds | 1 ns | completed collective barrier before and after the timed collective |

Each sample retains two things:

* `raw` — the timer's own decimal representation, preserved verbatim. This
  includes scientific notation (`1.234e-05`) and any number of fractional
  digits the timer returned;
* `duration` — the exact rational nanosecond conversion of `raw`, reduced, with
  `{numerator, denominator}`. A fractional nanosecond is retained as a
  fraction, never rounded.

Concrete examples, all accepted:

| `raw` (ms) | `duration` (ns) |
|---|---|
| `31.234` | `31234000/1` |
| `31.2345678` | `156172839/5` |
| `0.123456789012345` | `24691357802469/200000000` |
| `1.234e-05` | `617/50` |

The declared clock resolution describes the advertised timer class. It **never**
rounds, quantizes or gates a retained value: a digit finer than the declared
resolution is preserved rather than rejected, and the exact decimal expansion
of a value a timer returned as a float is not a precision or accuracy claim.
Zero, negative, non-finite, over-long, out-of-range and non-representable values
fail closed (`DurationOutOfRange`, `SamplePrecision`, `SampleOverflow`); a
duration that cannot be represented as two `u64` members is retained as a
failure rather than truncated, and the sample grid then stays incomplete.

Durations from different clock identities are never subtracted. Rank scope
records only per-rank durations; one completed world repetition is one sample,
and the acquisition statistic is the exact mean of the complete per-repetition
maxima. Rank absolute timestamps are never compared across ranks. The
collective duration is a synchronized host-monotonic completion latency, not
device-only kernel time, and is labelled as such.

## Bytes, bandwidth and the corrected definitions

Algorithmic payload bytes are `numel * sizeof(dtype)`; for the frozen
collective that is `262144 * 8 = 2097152` bytes. Algorithm bandwidth is
`payload_bytes / duration`, an exact rational.

`2*(world-1)/world` is the nccl-tests **bus bandwidth** normalization for
all-reduce. It is retained and labelled separately
(`bus_scope: "nccl-tests all-reduce bus-bandwidth normalization; not measured
physical link traffic"`) and is never described as measured link traffic, nor
substituted for the algorithm payload. `microbench inspect` prints both.

## Work, memory and time allowances

Every plan declares finite `work_units`, `memory_bytes` and `deadline_ms`
before execution. The collector rejects a plan whose declared allowance cannot
cover the frozen cell, and withholds evidence whose observed work or memory
exceeds the declaration. That admission is now derived from the frozen
operation alone — never from a measured outcome — and it happens before
anything runs: Rust `check_plan` refuses the plan before the output directory
exists, and each producer refuses the same plan in its own `check_plan`, before
it imports a device runtime or opens a rank, window or device. A tiny positive
budget is therefore no longer admitted only to fail after the work has already
run. Retained accounting is checked the same way: an artifact whose
`work_units` or `memory_bytes` is below the derived minimum is invalid
(`ALLOWANCE_EXCEEDED`), not a small favorable number.

| Adapter | Minimum `work_units` | Minimum `memory_bytes` |
|---|---|---|
| `cpu-sum-u64-reference` | `bound * (warmups + iterations)` = 4096 * 7 = 28672 `u64` adds | `bound * 8` = 32768 |
| `exl3-e3-grouped` | `tokens * topk * 3 * 2 * hidden * intermediate` per pass, for 2 warmups + 5 measured + 3 parity passes = 2061584302080 | fp16 activation payload `tokens * hidden * 2` = 8388608 |
| `nccl-allreduce-sum` | `world * numel * (warmups + iterations)` = 2 * 262144 * 6 = 3145728 reduced `int64` elements | `numel * (8 * 5 + 1)` = 10747904 per rank |

Units are fixed per adapter and are never converted between adapters. One
`work_units` is one elementary operation of that cell: a `u64` add per element
per repetition for the CPU reference, one reduced `int64` element per
participating rank per repetition for the collective (the `world` factor is why
rank 0's single retained number already covers every rank), and for E3 one
multiply-accumulate counted as two over the projection shape, which is how the
cell already counted it. Required warmups are charged, and so are the parity
passes the cell must execute before timing: the E3 adapter runs
`apply_exl3_experts` once as the E2 kernel reference and twice as the E3
candidate and its repeat, while its pinned Python reference loop is a different
implementation and is not counted as grouped-expert work. No adapter derives an
allowance from an observed duration, tier, mismatch or any other outcome.

`memory_bytes` counts **PyTorch tensor payloads only**. For the CPU reference
that is the `u64` vector; for the collective it is the five live per-rank
`int64` payloads (`index`, `partial`, `expected`, `input_template`, `buffer`)
plus the boolean element payload the exactness comparison allocates; for E3 the
minimum is the fp16 activation the timed kernel cannot run without, while the
**retained observation stays the process-local PyTorch allocator peak**
(`memory_allocated` / `max_memory_allocated`), which is larger. None of these
figures is total device, driver, NCCL or CUDA-context memory; none is a
reserved, physical or hard-OOM bound; and non-PyTorch workspaces — the CUDA
context, NCCL channels/rings/buffers, allocator caches and every other process
— stay outside every allowance by construction. The E3 artifact's memory figure
is documented as a process-local allocator peak, never as a device total.

The accounting a run retains is re-derived from the same frozen figures that
admitted the plan, so it cannot drift from the admission bound or be understated
relative to it. In the collective every rank derives its own accounting and
retains an `allowance-exceeded` failure before the gather, and rank 0 flattens
the retained failures of **all** ranks, so an exceeded allowance, a peer failure
or a missing participant is never hidden behind a favorable result.

Linux external capture places only the explicitly spawned program in a fresh
process group. Deadline expiry, observed SIGINT or SIGTERM cancellation,
capture failure, and normal leader exit all finish by signalling that owned
group with SIGKILL, including its normal descendants.
The collector never signals the invoking shell's group, unrelated PIDs,
operator-started peers or services. A graceful **collector cancellation** means
the collector retains a failed receipt; it does not promise a producer shutdown
hook or a distributed collective's graceful teardown.

The collector observes leader exit with `waitid(WNOWAIT | WNOHANG)` and sends
its sole group signal **before reaping the leader**. The unreaped leader reserves
the numeric PID/PGID, avoiding signal-after-reap reuse races even on normal exit.
An ignored SIGCHLD, auto-reap setting or competing SIGCHLD handler fails setup
closed; unexpected loss of the waitable leader prevents any numeric group
signal and records a cleanup failure. No group signal occurs after `try_wait`.

There are no blocking reader threads or joins. Both output pipes are nonblocking;
each polling turn reads at most 16 chunks of 8192 bytes per stream, then checks
exit, caps, cancellation and the monotonic deadline. The poll sleep is 2 ms.
After normal leader exit and the group SIGKILL, drainage continues until pipe EOF
or the run's remaining deadline expires, whichever comes first. This lets killed
descendants close inherited pipes when scheduled, without imposing a shorter
drain window. An escaped descendant holding a pipe past the deadline still fails
with `stdout drainage incomplete at cleanup boundary` or
`stderr drainage incomplete at cleanup boundary`: retained prefixes are marked
truncated. Failure, deadline and cancellation cleanup, and nonblocking leader
reaping, retain their separate 100 ms allowance. Under ordinary scheduling, the
collection budget is the declared deadline plus one bounded polling turn and at
most 100 ms cleanup plus its final polling turn; receipt `duration_ms` includes
launch and cleanup, not program hashing or evidence publication.

These are algorithmic wait bounds, **not hard real-time OS guarantees**.
Scheduling stalls, blocked kernel execution (including spawn/exec or filesystem
I/O), and uninterruptible tasks cannot be preempted by this collector. A task
still unreapable after cleanup is reported rather than awaited indefinitely.
The collector catches SIGINT/SIGTERM from external setup through the collection
decision before final publication; a later signal does not revoke an already
committing result. Storage failure can leave partial evidence without a final
receipt. SIGKILL of the collector, power loss and crashes cannot run cleanup or
guarantee a receipt. A producer that deliberately escapes its process group
(for example with `setsid`) is outside group containment; no process discovery,
subreaper or daemon is introduced to chase it. Ordinary descendants in the
owned group receive the cleanup signal, but this is not a hostile-program sandbox.

Native external launch fails closed on non-Linux platforms rather than using
direct-child-only cleanup. This platform gate does not apply to CPU reference,
import or offline inspection paths.

## Launching the collective: direct pinned execution, explicit environment

The collector hashes whatever `--program` names and requires it to equal
`plan.program_sha256`, and this adapter verifies the same hash on itself. A
launcher such as `torchrun` has a different hash, so it is **not** a valid
`--program`: the program must be the pinned adapter file, executed directly.

Each participant is started explicitly, one process per rank, with the
process-group environment already set:

```sh
RANK=<rank> WORLD_SIZE=<world> MASTER_ADDR=<host> MASTER_PORT=<port> \
  [LOCAL_RANK=<local device index>] \
  python3 tools/microbench/nccl_allreduce_sum.py \
    --plan plan.json [--stdout | --out <file>] [--checkout <bytes-source root>]
```

* `RANK`, `WORLD_SIZE`, `MASTER_ADDR` and `MASTER_PORT` are required; a missing
  variable is a bounded refusal, not a hang. `LOCAL_RANK` selects the device
  when supplied (otherwise `CUDA_VISIBLE_DEVICES` decides).
* The process-group rendezvous timeout is the plan's declared deadline, so a
  missing or dead peer fails inside a bounded wait instead of hanging.
* Rank 0 emits the merged artifact — to stdout when the collector launched it,
  or to `--out` for the operator-owned flow. Other ranks print nothing.
* A participant that fails init or passes its deadline exits non-zero with its
  traceback; the survivors time out; rank 0's artifact retains the missing
  participants as `rank-missing` failures. Failed participants stay visible.
* No launcher, cluster manager, remote control or discovery is invented here or
  by the collector. Placement and process ownership belong to the operator.

Collector-launched local pair (the operator starts the peer; the collector
inherits its own environment and launches rank 0):

```sh
RANK=0 WORLD_SIZE=2 MASTER_ADDR=127.0.0.1 MASTER_PORT=29500 \
  grill-perf microbench capture --adapter nccl-allreduce-sum \
    --plan plan.json --program tools/microbench/nccl_allreduce_sum.py \
    --authorize-device-window -- --stdout --checkout /path/to/nccl-tests
```

Operator-owned placement, where the collector only imports the result:

```sh
RANK=0 WORLD_SIZE=2 MASTER_ADDR=host-a MASTER_PORT=29500 \
  python3 tools/microbench/nccl_allreduce_sum.py --plan plan.json --out rank0-artifact.json &
RANK=1 WORLD_SIZE=2 MASTER_ADDR=host-a MASTER_PORT=29500 \
  python3 tools/microbench/nccl_allreduce_sum.py --plan plan.json &
wait
grill-perf microbench import --artifact rank0-artifact.json --plan plan.json --out evidence/
```

## Study shape: at least three complete acquisitions per role

A native domain comparison needs a **prospectively declared study**, written
before the measured acquisitions run:

```json
{
  "kind": "microbench-study-v1",
  "version": 1,
  "adapter": "cpu-sum-u64-reference",
  "revision_axis": { "baseline": "<sha256 hex>", "candidate": "<sha256 hex>" },
  "thresholds": { "adverse_bps": 500, "spread_bps": 2000 },
  "minimum_acquisitions": 3,
  "roles": {
    "baseline": ["a01", "a02", "a03"],
    "candidate": ["b01", "b02", "b03"],
    "reference": ["r01", "r02", "r03"]
  },
  "started_unix_ms": 1757000000000
}
```

* `minimum_acquisitions` must be at least `3`, and every role must declare at
  least that many slots; each slot names one acquisition directory inside the
  role directory, and the slot id must equal that acquisition's plan
  `acquisition.id`.
* The declared `revision_axis` is the only permitted difference between the
  arms: baseline and reference members must carry `baseline`, candidate members
  `candidate`, and every other pin must match across the entire study. Both
  entries may be the same observed hash — that is an A/A control, which the
  collector admits and labels as a control.
* Declared starts must be strictly increasing inside each role, and every
  baseline start must precede every candidate start, which must precede every
  reference start. No acquisition may start before `started_unix_ms`.
* Every acquisition in a study must have been recorded by one collector
  identity; a mixed set is `CollectorMismatch`.

Each acquisition directory is what `capture`/`import` produce
(`plan.json`, `artifact.json`, `receipt.json`, and for native external captures
`stdout.bin` and `stderr.bin`), so a CPU role directory looks like:

```
baseline/
  a01/{plan.json,artifact.json,receipt.json}
  a02/...
  a03/...
```

`compare` loads exactly the declared membership. A directory that is present but
not declared is an error (`UnexpectedAcquisition`); a declared slot that is
missing, failed, timed out, incomplete or unusable withholds the comparison
(`MissingAcquisition` plus the specific reason). **Missing or failed
acquisitions are never replaced, filtered or dropped, and there is no
one-acquisition-per-role shortcut.**

## Comparison contract

Within one acquisition the statistic is the exact arithmetic mean of the
measured samples, or for rank scope the exact mean of the complete
per-repetition maxima. Roles are then compared by statistic:

* the reference envelope pools the acquisition statistics of the **baseline and
  reference (A/A2) roles**;
* the candidate envelope is the candidate role's acquisition statistics;
* the shared envelope assessment applies the study's `adverse_bps` and
  `spread_bps` in exact rational arithmetic.

The result retains every acquisition statistic, both role revisions, the
per-role and pooled ranges, the adverse bounds, the uniform collector identity,
the evaluator binary digest, the study digest and every reason code. These are
finite empirical statistics of one declared cell, not precision, confidence,
IID or production-percentile guarantees, and not universal capacity or adoption
verdicts.

## Shipped examples: placeholders versus observed identity

The example documents under `crates/grill-perf/examples/` are **templates**, and
every identity field in them is a placeholder:

| Document | Placeholder | Becomes observed when |
|---|---|---|
| `microbench-cpu-sum-u64.json` | `revision` all zeros | derive the expected descriptor hash from the collector binary; capture checks it before timing |
| `microbench-exl3-e3-grouped.json`, `microbench-nccl-allreduce-sum.json` | `revision` all zeros | the first device capture records the loaded runtime descriptor hash |
| `microbench-study-cpu.json` | both `revision_axis` entries all zeros, `started_unix_ms` fixed | the smoke writes its study from the observed hash and a real clock reading |
| all plans | `acquisition.id`, `acquisition.started_unix_ms` | the smoke writes one plan per acquisition |

`program_sha256` and the adapter's own `sources` entry are **not** placeholders:
they are the shipped adapter's real hash and are re-pinned whenever the adapter
changes. Native CPU capture rejects a mismatching expected revision before
allocation or timing; a placeholder capture cannot discover that identity.
External captures retain observed runtime identity for subsequent validation.

## CPU reference smoke plan

Run on a stable build, without concurrent writers. Derive the CPU implementation
identity from that binary, write every plan and the study before execution,
then acquire all A, all B, and all A2. Actual receipt intervals—not declaration
timestamps—prove execution order. Keep the output directory even on failure.

```sh
cargo build --offline -p grill-perf
python3 - target/debug/grill-perf /path/to/new-private-smoke-directory <<'PY'
import hashlib, json, os, pathlib, subprocess, sys, time

os.umask(0o077)
binary = pathlib.Path(sys.argv[1]).resolve(strict=True)
out = pathlib.Path(sys.argv[2]).resolve()
out.mkdir(mode=0o700)  # Must be fresh; never replace a failed run.
collector = hashlib.sha256(binary.read_bytes()).hexdigest()
descriptor = "cpu-sum-u64-reference-v1;collector:" + collector
revision = hashlib.sha256(descriptor.encode("ascii")).hexdigest()
examples = pathlib.Path("crates/grill-perf/examples")
template = json.loads((examples / "microbench-cpu-sum-u64.json").read_text())
study = json.loads((examples / "microbench-study-cpu.json").read_text())
declared = time.time_ns() // 1_000_000
study["started_unix_ms"] = declared
study["revision_axis"] = {"baseline": revision, "candidate": revision}
for role, slots in study["roles"].items():
    (out / role).mkdir()
    for slot in slots:
        plan = dict(template, revision=revision,
                    acquisition={"id": slot, "started_unix_ms": declared})
        (out / (slot + ".json")).write_text(json.dumps(plan, indent=2) + "\n")
(out / "study.json").write_text(json.dumps(study, indent=2) + "\n")
commands = []

def invoke(name, args, allowed=(0,)):
    command = [str(binary), "microbench", *map(str, args), "--json"]
    commands.append(command)
    (out / "commands.json").write_text(json.dumps(commands, indent=2))
    result = subprocess.run(command, capture_output=True, timeout=30)
    (out / (name + ".stdout.json")).write_bytes(result.stdout)
    (out / (name + ".stderr.txt")).write_bytes(result.stderr)
    if result.returncode not in allowed:
        raise RuntimeError(f"{name}: exit {result.returncode}; evidence retained in {out}")
    return json.loads(result.stdout)

for role in ("baseline", "candidate", "reference"):
    for slot in study["roles"][role]:
        destination = out / role / slot
        captured = invoke(slot + "-capture", [
            "capture", "--adapter", "cpu-sum-u64-reference",
            "--plan", out / (slot + ".json"), "--out", destination])
        inspected = invoke(slot + "-inspect", ["inspect", destination])
        assert captured["provenance"] == inspected["provenance"] == "native_observed"
        assert captured["samples"] == inspected["samples"] == 5
        assert inspected["plan_verified"] and not inspected["invalid"]
        artifact = json.loads((destination / "artifact.json").read_text())
        assert artifact["kernel_revision"] == descriptor
        assert artifact["revision"] == revision
        for sample in artifact["samples"]:
            assert sample["duration"] == {"numerator": int(sample["raw"]), "denominator": 1}

report = invoke("comparison", [
    "compare", "--study", out / "study.json", "--baseline", out / "baseline",
    "--candidate", out / "candidate", "--reference", out / "reference"], (0, 2, 3))
assert all(group["complete"] == 3 for group in report["roles"].values())
assert report["axis"] == "same-implementation-control"
print(json.dumps({"result": "CPU_PROTOCOL_SMOKE_PASSED", "decision": report["decision"],
                  "collector_sha256": collector, "evidence": str(out),
                  "device_or_serving_qualification": False}))
PY
```

Each capture runs a real `sum_u64` reduction over `0..4096` with warmups 2 and
five measured iterations, records exact nanosecond rational samples on the
process monotonic clock, and validates the exact `n*(n-1)/2` reference. Because
this build's descriptor is the same for every arm, the smoke is an A/A control.
A PASS means agreement within the declared envelope; timing variability can
instead produce INCONCLUSIVE or REGRESSION. The smoke reports that decision
unchanged, without retries or wider thresholds. It is not evidence of a kernel
change. Device or kernel changes require a separately authorized window and
plans whose expected revisions match the observed runtime descriptors.

Device adapters are *not* exercised by this plan; the collective launch contract
is above, and the E3 adapter is launched the same way as a single explicit
program:

```sh
grill-perf microbench capture --adapter exl3-e3-grouped \
  --plan crates/grill-perf/examples/microbench-exl3-e3-grouped.json \
  --program tools/microbench/exl3_e3_grouped.py \
  --authorize-device-window \
  -- --checkout /path/to/GLM-5.3-Flash-EXL3-2x-DGX-Sparks
```

## What this protocol deliberately does not claim

* Device, fabric or collective exercise. The adapters are delivered callable
  and unexercised; every device artifact is `imported` or a synthetic CPU
  fixture until a separately authorized window produces real samples.
* Serving-speed impact. A kernel or collective PASS is a measurement-layer
  observation only and needs a separately linked serving acquisition.
* Precision, confidence, IID, production percentiles, capacity or adoption.
* Authenticated execution. `native_observed` means this collector launched a
  program and retained its bytes within a bounded deadline, and that the
  program's own observations were structurally consistent with the frozen cell;
  it cannot attest what that program actually did.
* Any executed result on this branch. The collector-side Rust that parses these
  documents, hashes revisions and runs the comparison is Main's; this document
  describes the contract the adapters and examples implement, and the smoke
  above is a plan to execute, not a captured result.
