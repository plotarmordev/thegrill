# Bounded host and opt-in NVIDIA resource observations

`grill-perf resource` is a separate version-1 domain evidence path. It does not
change serving plan/workload meanings or collect serving requests. This source
slice implements ordinary Linux host, [macOS process and host memory](#macos-sources)
and explicitly selected NVML producers, retained raw replay, finite
import/inspect, and prospective A/B/A2 resource comparisons. It does **not**
implement a capacity or retention-pressure runner, or establish
real-adapter/live qualification from source code or synthetic tests.

## Commands

All resource commands print JSON. Capture observes only explicitly declared
sources; it never launches a process, searches for a PID, discovers a cgroup,
reads process command lines/environment, or calls a serving endpoint.

```text
grill-perf resource capture --plan STUDY.json --role a --phase warmup --index 0 --out NEW_CAPTURE
grill-perf resource capture --plan STUDY.json --role a --phase measured --index 0 --out NEW_CAPTURE
grill-perf resource inspect CAPTURE --plan STUDY.json
grill-perf resource import CAPTURE_JSON --out NEW_IMPORTED_CAPTURE
grill-perf resource compare --plan STUDY.json --a A_WARMUP A_0 A_1 A_2 --b B_WARMUP B_0 B_1 B_2 --a2 A2_WARMUP A2_0 A2_1 A2_2
```

The plan declares exactly three ordered arms (`a`, `b`, `a2`), a fixed duration,
a source configuration for each arm, unique ordered warmup and measured IDs,
collector/exposure/deployment SHA-256 pins, a named candidate change, and every
gate. Each arm requires 1–20 warmups and 3–1000 measured acquisitions; counts and
source contracts must match. A/A2 deployment pins must match. All declared
acquisitions remain required, even when cancelled or failed. There is no retry,
replacement, upward search, selected best sample, or automatic plan relaxation.

`collector_sha256` is the actual executable SHA-256; capture checks it before
sampling. `exposure_pin` binds the operator's work declaration, not authenticated
execution of that work. Process PIDs and cgroup paths are arm-specific selections;
logical source IDs, ownership, other source/adapter fields, and budgets must
match. The current comparison requires one identical collector binary throughout;
collector/adapter implementation changes are not an admitted candidate axis.

Inspection without `--plan` verifies raw evidence but does not infer requested
gates. With `--plan`, it recomputes each declared summary. Capture exit 0 means
its requested summaries are available, not comparison PASS. Exit 2 retains
nonqualifying observations. Comparison exits reuse the policy contract:
PASS=0, ERROR=1, INCONCLUSIVE=2, REGRESSION=3, with
ERROR > REGRESSION > INCONCLUSIVE > PASS aggregate precedence. Import/inspect
exit 0 means valid storage/replay, not source completeness or native exercise.

## Explicit sources and finite budgets

`ResourcesConfig` has `version: 1`, `sources`, `cadence_us`, `max_gap_us`,
`max_read_us`, `max_samples`, `deadline_us`, `per_sample_bytes`, and
`raw_total_bytes`. All fields are required. There are at most 16 selected sources,
10,000 total source snapshots, 1 MiB raw bytes per sampling cycle, 8 MiB raw bytes
over the observer, and a one-hour observer deadline. Cadence is 1 ms–10 s; the
maximum allowed observed gap is explicit, at most 60 s. Byte ceilings bound
retained bytes, not total RSS. JSON artifacts have a separate 64 MiB ceiling.
A missing, permission-denied, truncated, malformed, changed, or late source stays
visible; exhausted budgets never become complete exposure.

A neutral selected-process source looks like this (the PID is supplied explicitly
by the owner of the ordinary fixture, not discovered by Grill):

```json
{"id":"fixture-process","target":{"kind":"process","pid":4242},"ownership":{"kind":"process_address_space"}}
```

Supported native sources:

| Target | Exact files | Semantics |
|---|---|---|
| `process {pid}` | `/proc/PID/stat`, `status`, then `stat` again | process CPU ticks, RSS, lifetime RSS high-water mark |
| `cgroup_v2 {path}` | explicit cgroup-v2 `memory.current`, `memory.peak`, `memory.max`, `cpu.stat` | used/peak/limit bytes, cumulative CPU microseconds |
| `host_cpu` | `/proc/stat` | aggregate **busy** CPU ticks, host-wide shared scope |
| `host_memory` | `/proc/meminfo` | MemFree bytes and MemTotal capacity, descriptive, not model-owned memory |
| `nvidia_memory {uuid, rank?}` | dynamically loaded NVML memory-v1 API | device used/free/total bytes; total uses `memory_limit`, not safe capacity |
| `nvidia_power {uuid, rank?}` | NVML field 186, GPU-only scope 0 | instantaneous power, converted exactly from milliwatts to microwatts |
| `macos_process {pid}` | one `proc_pid_rusage` `rusage_info_v4` record | physical footprint as `memory_used`, lifetime footprint peak |
| `macos_host_memory` | sysctls `hw.memsize`, `vm.pagesize`, `vm.page_free_count`, `kern.memorystatus_vm_pressure_level` | free-queue bytes, physical memory capacity, pressure level; descriptive |

Native source directories are held open, filesystem type is checked, and fixed
source files use bounded nonblocking/no-follow reads rather than immutable-file
size assumptions. Cgroup paths stay under `/sys/fs/cgroup`; directory inode/device
identity detects replacement. Proc stat is split after the **final** closing
parenthesis, and PID/starttime is checked both within a sampling cycle and across
cycles. Child CPU is excluded. Process `utime` already includes guest time, so
it is counted once; host guest/guest_nice fields are not added again. Host idle
and iowait are not busy CPU. CLK_TCK is observed via sysconf, never hardcoded.
Cgroup `max`, absent files, finite limits, lifetime peaks, and sampled maxima are
separate values. Native process names present in raw stat/status remain private.

Every snapshot identifies its source, clock, read start/end offsets, incarnation,
typed metric/unit/value, raw bytes, and failure. The retained read interval is
collector overhead, not atomic-source-read precision. Native observations are
unauthenticated observations; ownership labels do not prove model attribution.
Host and cgroup measurements can include unrelated activity. No metric is summed
across ranks, devices, host/device unified memory, or shared ownership groups.

Byte/count/deadline admission is finite, but userspace cannot preempt a stalled
kernel filesystem read. Deadline and cancellation checks occur between reads;
an overlong read is retained as a coverage failure, not a hard-real-time claim.

### macOS sources

A macOS collector observes only `macos_*` sources and a Linux collector only the
others; one configuration never mixes them, and the other platform's sources
fail observer start. Raw entries are the kernel's native little-endian bytes, so
any host replays them.

`macos_process` reads the footprint that `footprint -p PID` reports as
`phys_footprint` (Apple's per-process charge, including compressed and
IOKit/GPU-owned memory, not RSS) and `ri_lifetime_max_phys_footprint`.
`proc_pid_rusage` needs only the same user; `task_info` on another process needs
`task_for_pid`, which an unprivileged, unentitled collector is refused. The
incarnation is `ri_proc_start_abstime`, so a reused PID is `source_changed`; one
call returns one process, so there is no within-sample re-read. A vanished PID
or an exited, unreaped process is `missing`, another user's process is
`permission`, and a wrongly sized or zeroed record is `malformed`.

`macos_host_memory` avoids `host_statistics64`: for a non-Apple process it
returns an earlier cached copy with success after a few calls per second, which
would substitute a stale value. Free memory is `vm.page_free_count` times
`vm.pagesize` (the kernel page; Rosetta changes `hw.pagesize`), lower than
`vm_stat` "Pages free", which on memory-tagging hardware also counts free
tag-storage pages. `memory_pressure_level` is the dispatch level (1 normal,
2 warning, 4 critical, unit `dispatch_memorypressure_level`); any other value is
`malformed`.

## Clocks, exposure, and exact summaries

Offsets are integer microseconds on an identified monotonic origin: the clock
`std::time::Instant` reads, `linux_monotonic` (`CLOCK_MONOTONIC`) or
`macos_uptime_raw` (`CLOCK_UPTIME_RAW`). Native resolution is observed with
`clock_getres` and floored to the retained 1 us quantization. Acquisitions may
have different origins, but comparisons require the same clock kind, unit,
resolution, and synchronization contract. Unix provenance timestamps establish
declared A/B/A2 ordering only; they never enter a duration or integral.

The ordinary-process command samples once before its requested interval and
continues for the prospectively fixed duration. It retains actual boundaries;
clock wake/read overshoot is not relabelled as exact requested time. Comparison
requires actual duration in `[duration_us, duration_us + max_gap_us]`. This is a
prospectively bounded observation-window comparison, not exactly equal CPU work.

For multiple sequential sources, capture starts at the latest completion of the
initial cycle and ends at the earliest completion of the final cycle. This
common interval is bracketed by every source rather than only the last reader.
It does not synthesize counter readings at those boundaries: multi-source CPU
counter gates can remain `unsupported_boundary` even when memory gates qualify.

Supported lower-is-better gates:

* `sampled_maximum`: RSS, used/allocated/reserved memory, KV used bytes, or
  instantaneous power. Only in-interval samples enter the maximum, with complete
  bracketing/cadence coverage required. It is not a true peak. Lifetime peaks,
  free memory, limits, and capacity remain descriptive, not these maxima gates.
* `cpu_time`: difference of cumulative CPU counters, converted exactly to CPU
  microseconds. Both measured boundaries must equal actual source observation
  offsets. Unaligned boundaries are `unsupported_boundary`, not interpolated CPU.
* `cpu_utilization_one_cpu`: CPU time / measured wall duration as a rational
  logical-CPU ratio, allowed to exceed one. No machine-core denominator or clamp.
* `sampled_energy_estimate`: explicitly labelled piecewise-linear trapezoidal
  integral of compatible complete microwatt observations, in microjoules.
  Segments are clipped to the measured interval with exact checked rational
  arithmetic. A single power sample cannot establish energy, and this is not
  directly measured physical energy. Native NVML instantaneous power and imported
  power remain distinct provenance paths; neither establishes physical energy.

Missing endpoints, cadence gaps, permission failures, cancellation, budget
exhaustion, resets, source/clock changes, and unknown ownership prevent favorable
qualification. Rational arithmetic overflow is ERROR, not rounding or saturation.
Full per-acquisition samples feed the existing exact envelope implementation;
all required warmups must qualify, and none enters measured counts. There are no
confidence, causality, or universal-capacity claims.

## Shared serving observer API (Main integration)

The final API in `resources.rs` is:

```text
host_clock(id: String) -> Result<Clock>
Observer::start(config: ResourcesConfig, origin: std::time::Instant, clock: Clock) -> Result<Observer>
Observer::sample(&mut self) -> Result<bool>
Observer::last_observed_us(&self) -> Option<u64>
Observer::finish(self, measured: MeasuredInterval, cancelled: bool) -> Result<Observation>
validate_observation(&Observation) -> Result<()>
summarize(&Observation, &Gate) -> Summary
```

Main supplies the **existing capture-scoped monotonic Instant**, its matching
clock identity, and cadence calls in the existing acquisition lifecycle. The
observer has no thread, timer, endpoint client, request sender, or independent
serving collector. `sample` returns false after budget exhaustion and preserves
the reason. `finish` takes an explicit `{clock, started_us, settled_us}` measured
interval: first required step start through last required step settlement,
including prime/control/inter-step work, not disjoint measured-step windows.
Observer setup may precede it and final observation may follow it. Caller-supplied
`cancelled` retains cancellation. Main retains the returned observation alongside
its final workload6 acquisition types and preserves per-step coverage separately.

The conservative CPU endpoint gate can be unavailable for serving acquisitions
whose required-step boundaries do not coincide with actual CPU sample offsets;
the API does not fabricate those missing counter boundaries. Source comparisons
must retain that limitation. Serving acquisition attachment/wiring is owned by
Main and is not supplied by a second runner here.

## Import, privacy, and replay

`resource import` reads one bounded `Capture` JSON and **always** changes outer
provenance to `imported`, even if input claims `native_observed`. Original bytes
are retained as `imported.json`; the projection is independently checked on load.
Native captures retain exact `study.json`, `observation.json` (including selected
raw bytes), and the digest-bearing `resource.json` receipt. Files are exclusively
created mode 0600 inside fresh 0700 directories using the existing evidence
helpers. Native destination admission precedes source reads; unfinished output
has no authoritative receipt. Raw sampling state is bounded in memory until
publication; abrupt process/host loss before publication can leave only the
prospective study, not recoverable sampled data. Cooperative cancellation does
publish the acquired evidence.

Imported target identity is explicit:
`imported {adapter, device, rank}` plus an ownership declaration and source
incarnation. Each snapshot retains `readings.json` bytes containing the closed
`Reading` list. Replay reparses those bytes and rejects unit/metric disagreement.
Device/provider values are **imported declarations**, not authenticated adapter
execution. Imported arithmetic can yield an explicitly imported comparison but
cannot establish real-adapter exercise or live qualification. Full evidence and
plans are private replay inputs, not public-safe exports.

## Later CPU verification, not executed during concurrent authoring

After Main approves a stable integration head, run:

```sh
cargo test -p grill-perf --locked resources::tests
cargo build -p grill-perf --locked
python3 tools/smoke-resources.py target/debug/grill-perf --out /tmp/grill-resource-smoke-new
python3 tools/smoke-resources.py target/debug/grill-perf --sources 2 --out /tmp/grill-resource-multi-smoke-new
```

The smoke authors a prospectively pinned plan for its own finite 8 MiB ordinary
process, collects one warmup plus three measured acquisitions in each A/B/A2
role, exercises the actual native producer, replays every capture, compares,
and verifies import cannot upgrade provenance. It retains all failures and never
retries. It starts/terminates only its own test process; no serving process,
GPU runtime, metrics, health, model, or device access is involved. Unchanged
control variation or timing gaps may prevent PASS; inspect retained evidence
rather than replacing acquisitions. For outside-checkout verification, pass the
staged binary's absolute path and a fresh private output directory.
The `--sources 2` scenario creates two distinct owned 8 MiB processes and checks
both memory gates over their shared interval; it does not duplicate one target
under two source labels or sum their measurements.

Fixtures additionally cover final-parenthesis parsing, guest exclusion, PID reuse,
resets, permission/byte/sample/deadline/cancellation gaps, clock and unit mismatch,
overflow, unknown/shared ownership, missing rank visibility, unlimited versus
missing limits, CPU endpoint alignment, clipped rational energy, exact tolerance
boundaries, incomplete A/B/A2 populations, raw corruption, and imported provenance.
They are authored source, not an executed verification claim.

## Opt-in NVML source and runtime contract

The producer is private `resources::nvml::NvmlSource`, consumed by the unchanged
`ResourcesConfig` and `Observer` APIs above. No extra CLI selector is needed:
only `nvidia_memory` or `nvidia_power` in the admitted source list enables NVML.
An ordinary host-only capture never loads the NVML library. GPU-only adapter
identity is `linux-nvml-resource-v1`; mixed host/GPU identity is
`linux-proc-cgroup-nvml-resource-v1`; host-only evidence retains
`linux-proc-cgroup-resource-v1` and its historical claim string.

Runtime support is Linux LP64 on x86_64/aarch64 with the optional driver-installed
`libnvidia-ml.so.1` discoverable through the platform dynamic loader. There is no
hard NVML link, Cargo dependency, CUDA initialization, enumeration, index
guessing, subprocess, driver install, or service management. Library loading
executes trusted installed native code; untrusted library search paths are not
a sandbox. Runtime unsupported-host, missing-library, missing-symbol,
permissions, no-device, driver and per-field errors remain retained unavailable
observations. There is no `nvidia-smi`, CPU substitute, utilization-to-power
conversion, or import promoted to native evidence.

Selection requires the full `GPU-xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` UUID,
provided by the operator, with an optional unsigned declared `rank`. Abbreviated
UUIDs, device indices and MIG instance selectors are not supported. A rank is
not observed by NVML, and the collector never infers a rank-to-device mapping.
Separate memory and power source IDs let unsupported memory visibility withhold
the memory gate without falsifying supported power, and vice versa. Unsupported
ranks remain missing; nothing is summed across devices, ranks or host memory.

Ownership must be `dedicated_device`, `shared_memory {group}` or `unknown`.
`dedicated_device` is an operator declaration, not process attribution.
`unknown` withholds favorable gates. Prefer an honest shared-memory declaration
where ownership is shared, especially on unified-memory/NUMA systems. NVML
memory accounting may depend on the OS; pages may remain charged after process
exit, and memory attributed to the device can overlap host memory. Free/total
bytes are descriptive observations, not allocator headroom or safe serving
capacity. The v1 API's used bytes include reserved and allocated device memory:
the adapter reports only `memory_used`, `memory_free`, and `memory_limit`
(observed total), never fabricated `memory_allocated`, `memory_reserved` or KV
metrics. Zero/invalid total or inconsistent used+free/total is unavailable.

### ABI and source pins

The authoritative source is NVIDIA's
[`nvml_dev v12.9.40/nvml.h`](https://gitlab.com/nvidia/headers/cuda-individual/nvml_dev/-/raw/v12.9.40/nvml.h),
703,863 bytes, SHA-256
`5a9ed049520b02354b74280f3a1e83b27292cf9f7958beb227461c0bc8676b99`.
Bindings derive from this original header, not a wrapper-modified header or
formatted `nvidia-smi` output. The raw envelope embeds this pin and
`linux-lp64-v1`; the enclosing observation pins the actual collector binary.

The only runtime entry points are:

* `nvmlInitWithFlags(0)` (lazy v2 initialization), `nvmlShutdown()`;
* `nvmlSystemGetNVMLVersion`, `nvmlSystemGetDriverVersion` (80-byte buffers);
* `nvmlDeviceGetHandleByUUID`, `nvmlDeviceGetUUID` (96-byte buffer);
* memory: `nvmlDeviceGetMemoryInfo`, **v1**, three C unsigned long long fields in
  total/free/used order, in bytes;
* power: `nvmlDeviceGetFieldValues(device, 1, ...)`,
  **`NVML_FI_DEV_POWER_INSTANT=186`, scope 0 (GPU only)**.

`nvmlDeviceGetPowerUsage` is deliberately not used: on Ampere except GA100 and
newer devices it reports a one-second average, not consistently instantaneous
power. Field 185 (average) and scope 1 (CPU+GPU module) cannot replay as the
selected instantaneous GPU-only source. The field ABI retains field/scope IDs,
signed timestamp and latency, value type, per-field return code, and active
union scalar. Supported unsigned milliwatts multiply by 1000 with checked
arithmetic; other scalar types remain unavailable. Failed outer or field return
codes never acquire default-zero readings.

NVML initialization is balanced with shutdown for each selected source read;
the observer lazily holds the library open between reads. Both return codes,
all invoked read return codes and successful output values remain in each
bounded `nvml.json` raw trace. Shutdown-symbol absence prevents initialization.
The adapter uses no legacy init fallback that would initialize all devices.
**NVIDIA documents that UUID lookup itself may internally initialize additional
GPUs while resolving the selected UUID.** Grill does not enumerate them and
creates no CUDA context, but cannot promise that NVML internally touches only
the selected device. This is a reason to require an approved device window.

Raw call order, failure termination, balanced shutdown, string bounds, API pin,
UUID before/after the read, metric values/units and field scope are independently
parsed on replay. Observed UUID plus driver/NVML versions and ABI source pin
form a digest-bound incarnation. Runtime changes during an acquisition withhold
its gates; standalone A/B/A2 comparison also requires this incarnation to remain
compatible for each GPU gate. This conservatively excludes driver/NVML upgrades
as a candidate change in this source version. Observed version strings are not
hashes of driver binaries or an execution attestation.

Native trace version 2 records `init_flags: 0`. `NO_ATTACH` can prevent even a
present, explicitly selected UUID from resolving; it is no longer used for new
collection. Version-1 traces retain their implicit `NO_ATTACH` meaning on replay,
including failures. No legacy `nvmlInit` fallback or selector fallback is added.

The caller's capture-monotonic read end is still the observation offset.
NVML's field timestamp is retained Unix-microsecond metadata, never subtracted
from capture-monotonic time; it does not establish synchronized device timing.
Instantaneous sensor reads have hardware update cadence, latency and accuracy
limits, and repeated polling is not proof of independent or fresh physical
samples. Sampled maxima and trapezoidal energy remain explicitly sampled
estimates, not true peaks, directly measured energy, or model-only attribution.

The observer requires at least 8192 bytes of remaining raw budget **before**
calling NVML for a selected read; the bounded encoded trace consumes only its
actual bytes. Library lookup, initialization, metadata, UUID, metric and
shutdown calls are included in the snapshot read window and existing overhead.
Per-read and observer deadlines are cooperative: userspace cannot preempt a
stalled NVML/driver call. A late return remains a gap/deadline failure, not a
hard-real-time guarantee. Cadence, counts and retained bytes stay finite.

### Main-only qualification commands (not executed during source authoring)

CPU fixtures never open the NVIDIA soname. They cover raw failures, exact
units/energy, missing-library and byte-budget paths, UUID/runtime changes,
partial visibility, scalar types, replay tampering and import downgrade. One
fixture compiles a small **synthetic** C shared library with `cc`, loads its
explicit temporary path, and exercises the real C call boundary. Its missing
memory symbol and supported power field test independent capability handling;
it has no GPU dependency and is not device-execution evidence.

```sh
cargo test -p grill-perf --locked --bin grill-perf resources::nvml::tests -- --test-threads=1
cargo test -p grill-perf --locked --bin grill-perf resources::tests
cargo build -p grill-perf --locked
```

[`resources-nvml-v1.json`](../../crates/grill-perf/examples/resources-nvml-v1.json)
is a **synthetic, non-executed plan template** with one-second finite captures,
100 ms cadence, separate memory/power sources, one warmup and three measured
acquisitions in each A/B/A2 arm. Its fake UUID and digest placeholders are not
runtime facts. Before an explicitly approved GPU window, Main must write a
private prospective copy replacing the UUID, collector binary SHA-256,
deployment/exposure pins, ownership and candidate declaration. Freeze thresholds
and budgets before collection; do not tune them to salvage failures. For an
unchanged control, declare that control truthfully. Merely capturing idle-device
telemetry does not demonstrate model-resource or serving-capacity improvement.

Run the following **once per arm**, in declared `a`, `b`, `a2` order, with the
operator owning any serving-state changes between arms; the shown `a` commands
are the complete first arm, not authorization to touch a device now:

```sh
grill-perf resource capture --plan PRIVATE_STUDY.json --role a --phase warmup --index 0 --out NEW_A_W
grill-perf resource capture --plan PRIVATE_STUDY.json --role a --phase measured --index 0 --out NEW_A_0
grill-perf resource capture --plan PRIVATE_STUDY.json --role a --phase measured --index 1 --out NEW_A_1
grill-perf resource capture --plan PRIVATE_STUDY.json --role a --phase measured --index 2 --out NEW_A_2
grill-perf resource inspect NEW_A_0 --plan PRIVATE_STUDY.json
```

Repeat those four declared acquisitions with `--role b` and fresh `NEW_B_*`
paths, then `--role a2` and fresh `NEW_A2_*` paths. Retain every result and exit
status, including unsupported memory, power, permission, timing and cancellation
failures. Do not retry/replace captures or initialize an unapproved workload.
Replay **every** saved capture with `resource inspect`, then compare the
complete population using the command in the Commands section. These commands
also work with an absolute staged binary outside the checkout. Actual device
qualification needs retained native traces and independent review; a passing
CPU fixture or imported GPU trace is insufficient.
Subsequent bounded native exercises observed instantaneous power on two
explicitly selected devices. All 24 sampled energy integrals replayed and matched
independent exact arithmetic over the retained native field values. Device memory
was unsupported and remained unavailable; neither host-memory substitution nor
model attribution was used. Both resource comparisons were INCONCLUSIVE.
The template allows 32 total samples so its two sources can bracket a one-second
interval at 100 ms cadence without exhausting a 16-sample ceiling.


### NVIDIA notice for the derived ABI declarations

Copyright 1993-2025 NVIDIA Corporation. All rights reserved.

NVIDIA MAKES NO REPRESENTATION ABOUT THE SUITABILITY OF THIS SOURCE
CODE FOR ANY PURPOSE. IT IS PROVIDED "AS IS" WITHOUT EXPRESS OR
IMPLIED WARRANTY OF ANY KIND. NVIDIA DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOURCE CODE, INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY, NONINFRINGEMENT, AND FITNESS FOR A PARTICULAR PURPOSE.
IN NO EVENT SHALL NVIDIA BE LIABLE FOR ANY SPECIAL, INDIRECT, INCIDENTAL,
OR CONSEQUENTIAL DAMAGES, OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE
OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE
OR PERFORMANCE OF THIS SOURCE CODE.

U.S. Government End Users. This source code is a "commercial item" as
that term is defined at 48 C.F.R. 2.101 (OCT 1995), consisting of
"commercial computer software" and "commercial computer software
documentation" as such terms are used in 48 C.F.R. 12.212 (SEPT 1995)
and is provided to the U.S. Government only as a commercial end item.
Consistent with 48 C.F.R.12.212 and 48 C.F.R. 227.7202-1 through
227.7202-4 (JUNE 1995), all U.S. Government End Users acquire the
source code with only those rights set forth herein.

## Remaining capacity and retention dependencies

No capacity or retention command is implemented by this independent host slice.
Capacity requires a finalized workload6 acquisition integration with finite
prospective cells, success/quality/resource conditions, explicit stop condition
and OOM response, retained failed cells, and operator-owned recovery. Retention
requires the same sequence runner, prime/pressure/probe/recovery cases, actual
acquired histories, and finalized #51 accounting/continuity evidence. A miss
alone is not eviction. Missing continuity cannot qualify eviction. Neither
feature can be replaced by host resource JSON or a small fixture claim; issue52
and broad coverage remain open until those dependencies, independent review,
CPU/native staging, real adapters, and separately authorized qualification finish.
