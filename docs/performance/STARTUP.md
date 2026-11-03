# Startup and operator-restart cache evidence

`grill-perf startup` is a separate finite domain workflow. It never starts,
stops, restarts, flushes or configures a service. Default `baseline`, `check`,
`run`, serving workload versions 1–5, their namespaces, and `lifecycle.rs`
capture ownership keep their existing meanings.

## Commands and execution boundary

```text
grill-perf startup observe PLAN --slot N --pid PID --events JOURNAL \
  --producer-source SOURCE --out NEW_DIRECTORY [--previous PREVIOUS_CAPTURE]
grill-perf startup from-log PLAN --slot N --log ENGINE_LOG \
  --producer-source LAUNCHER --out NEW_DIRECTORY [--previous PREVIOUS_CAPTURE]
grill-perf startup store PLAN --slot N --pid PID --cache-pid CACHE_PID \
  --events JOURNAL --producer-source SOURCE --out STORE_DIRECTORY \
  [--previous PREVIOUS_CAPTURE]
# Operator/test harness restarts the engine, not the cache process.
grill-perf startup reload PLAN --slot N --pid NEW_PID --cache-pid CACHE_PID \
  --events NEW_JOURNAL --producer-source SOURCE --store STORE_DIRECTORY \
  --out RELOAD_DIRECTORY [--previous PREVIOUS_CAPTURE]
grill-perf startup import SOURCE_DIRECTORY --out NEW_DIRECTORY
grill-perf startup inspect CAPTURE_DIRECTORY
grill-perf startup compare PLAN ORDERED_PATH_MANIFEST
```

All output is JSON. `observe` only accepts a startup study; `from-log` only an
`engine_log_v1` plan ([engine-launcher logs](#engine-launcher-logs)); `store` and `reload`
only accept a restart-cache study. The path manifest is a JSON array of capture
directory paths in the plan's A/B/A2 slot order. `--previous` binds the preceding
whole acquisition's receipt, including failures; store and reload use the same
predecessor. The first slot has no predecessor. A store alone is not a completed
restart-cache acquisition and cannot enter comparison.

Use new output directories. Captures retain the exact `plan.json`, pinned source
bytes in `producer.bin`, raw `events.bin`, authoritative `capture.json`, and each
`request-NN/body.bin`, `response.bin`, and actual metrics2 companions when
selected. Reload embeds the verified store under `store/`; it is independently
replayable without the original store path. Existing evidence read/write,
non-symlink regular-file checks, exclusive private creation and publication
primitives are reused. Filesystem failure can leave an inspectable partial
output without an authoritative receipt; OS/filesystem stalls are not hard
real-time deadlines.

`import` always writes an imported wrapper and preserves the original receipt
as `imported-capture.json`, including on repeated import. Original acquisition
links remain checkable. A payload claiming native execution is still imported.
Native records are unauthenticated observations, **not cryptographic execution
attestations**. Possessing producer source with the correct hash does not prove
that an untrusted producer ran it.

## Neutral executable fixture

`tools/startup-fixture.py` supplies `neutral_journal_v1`, an explicit local-process
adapter. The **operator or test harness** invokes its `engine` and, for restart
studies, separate `cache` modes. The Rust CLI does not execute the script or
interpret input paths as commands. No source discovery or plugin registry exists.

The engine independently delays binding/listening, communication initialization,
readiness and inference, emits an append-only NDJSON journal, and exposes a
bounded ordinary loopback Chat Completions response and synthetic metrics.
The cache process really retains a computed synthetic answer across engine
restart; this is neither model KV nor an approximation of a model's performance.
The exact synthetic operation is the sum of integers zero through six, answer
`21`. Request response correctness is checked against the plan's exact answer,
not inferred from a model name, health string or listening socket.

Native attachment reads only the explicitly selected engine/cache PIDs and
journal. PID identity is `{pid,start_ticks,boot_id}`; parsing `/proc/PID/stat`
starts after its final closing parenthesis. Zombies, process exit, PID reuse,
journal inode replacement, rewritten prefixes, malformed lines, exhausted
budgets, failed responses and missing completion are retained as nonqualifying
observations. Polling is every 10 ms, with finite event count, raw bytes and a
whole observation deadline. There are no inference retries or replacement
acquisitions. Only the prospective request list is dispatched, after supported
readiness; all declared metrics snapshots are budgeted separately.

The script's `sealed` event means **inference admission has closed**, while
read-only metrics remain available. It is emitted after all accepted inference
requests have been recorded. Complete coverage is therefore a property of this
specific instrumented producer, not of arbitrary logs with similarly named
fields. Smoke suppression, shape-warmup suppression and stable hash control
must be source-observed; an operator attestation cannot replace them.

## Opt-in serving-runtime bridge

`tools/startup-runtime.py` implements the closed public source variants
`runtime_vllm_v1`, `runtime_vllm_487ecf187_v1`,
`runtime_vllm_487ecf187_bootstrap_v1`, and
`runtime_vllm_752a3a504_bootstrap_v1`. It is an explicit
**operator-invoked, in-process entrypoint**, not a command that Grill launches.
It calls the existing single-worker `api_server.run_server` with ASGI lifespan
enabled. The bridge adds no wrapper subprocess, restart, service manager or
arbitrary-module hook; vLLM itself may launch engine-core processes. The neutral
producer above remains a separate CPU fixture.

Supported public source is byte-pinned, not selected by model name or image tag:
The plan selects **one whole set**; there is no per-file fallback or automatic
version detection. Version/revision names are source metadata, not qualification.

Original set: `adapter:"runtime_vllm_v1"`,
event contract `vllm-0.27.0-uvicorn-0.34.0-sha256-v1`:

| Source | Revision | Required SHA-256 |
|---|---|---|
| [vLLM api_server.py](https://github.com/vllm-project/vllm/blob/v0.27.0/vllm/entrypoints/openai/api_server.py) | `v0.27.0` | `cd4b83e85dc9d5aae808348d336e59b3064bee697012750d23c786de082a1c53` |
| [vLLM launcher.py](https://github.com/vllm-project/vllm/blob/v0.27.0/vllm/entrypoints/launcher.py) | `v0.27.0` | `caf4c4517a62f05abe5998409de73af70e91d99dfab3819a4b359ba44af0da91` |
| [vLLM async_llm.py](https://github.com/vllm-project/vllm/blob/v0.27.0/vllm/v1/engine/async_llm.py) | `v0.27.0` | `81a0cae6d5da22140f509a59d6c6bb8fc6ee1572da2a2cd793b5830222d18bcc` |
| [Uvicorn server.py](https://github.com/encode/uvicorn/blob/0.34.0/uvicorn/server.py) | `0.34.0` | `8dd3d150523fd140a9981c41f0fae963b869b20c064dd94ab7422bc453748e6f` |

Additional reviewed set: `adapter:"runtime_vllm_487ecf187_v1"`,
event contract `vllm-487ecf187-uvicorn-0.52.4-sha256-v1`:

| Source | Revision | Required SHA-256 |
|---|---|---|
| [vLLM api_server.py](https://github.com/vllm-project/vllm/blob/487ecf187/vllm/entrypoints/openai/api_server.py) | `487ecf187` | `cd4b83e85dc9d5aae808348d336e59b3064bee697012750d23c786de082a1c53` |
| [vLLM launcher.py](https://github.com/vllm-project/vllm/blob/487ecf187/vllm/entrypoints/launcher.py) | `487ecf187` | `94566e08afbe40aef653184aa49bb1cfc6bdc8e9bf10881ae05cab65475bcd2e` |
| [vLLM async_llm.py](https://github.com/vllm-project/vllm/blob/487ecf187/vllm/v1/engine/async_llm.py) | `487ecf187` | `bceed0b3f5f0c834fef79525f2462a092f082390f0070526280abc95945837dd` |
| [Uvicorn server.py](https://github.com/encode/uvicorn/blob/0.52.4/uvicorn/server.py) | `0.52.4` | `7c1dbd656835c9cdd6f92078ffc80bcc6007824ab322aff15435bd89c068e0be` |

Bootstrap set: `adapter:"runtime_vllm_487ecf187_bootstrap_v1"`,
event contract `vllm-487ecf187-uvicorn-0.52.4-sha256-bootstrap-v1`.
This is a **new closed seven-file set**: all four files in the preceding 487
table plus all three files below, with no optional member or per-file fallback.
Hashes were computed from these public URLs, not installed-local source.

| Additional source | Revision | Required SHA-256 |
|---|---|---|
| [vLLM core_client.py](https://github.com/vllm-project/vllm/blob/487ecf187/vllm/v1/engine/core_client.py) | `487ecf187` | `afb2b627cf9ef861f05b411494156cc6ad5076ea770c6f2a4c8a941ca82103ec` |
| [vLLM core.py](https://github.com/vllm-project/vllm/blob/487ecf187/vllm/v1/engine/core.py) | `487ecf187` | `86b8f3b3826504549ef8bea2e7fbf7553728339803abcb3e7d051d146963ff9c` |
| [vLLM utils.py](https://github.com/vllm-project/vllm/blob/487ecf187/vllm/v1/engine/utils.py) | `487ecf187` | `6ce83b0552b6207505c9bd7ac1fd67d2771628eac01572738765923aa91c1c0f` |

The original four-file contracts retain their original meanings and do not
emit bootstrap events. Persisted evidence is never upgraded or reinterpreted.
The new bootstrap modules' imported locations are checked against the pinned
distribution paths before installing the bootstrap hook.

Additional bootstrap set: `adapter:"runtime_vllm_752a3a504_bootstrap_v1"`,
event contract `vllm-752a3a504-uvicorn-0.51.0-sha256-bootstrap-v1`.
This is a separate **closed seven-file set**, not an alias for either 487 set.
Its public revisions are vLLM `752a3a504485790a2e8491cacbb35c137339ad34`
and Uvicorn `e4d0b05eb8c6459b7ba27ad13a2c2f4f8d4ece50` (0.51.0).

| Source | Required SHA-256 |
|---|---|
| [vLLM api_server.py](https://github.com/vllm-project/vllm/blob/752a3a504485790a2e8491cacbb35c137339ad34/vllm/entrypoints/openai/api_server.py) | `29f8a544a1b780c327b40fee0ab639921f69ce5bb6084ef22132ae6777ab995d` |
| [vLLM launcher.py](https://github.com/vllm-project/vllm/blob/752a3a504485790a2e8491cacbb35c137339ad34/vllm/entrypoints/launcher.py) | `f2340520aa886ff8d2e4b53d6cd06614deaeb0afb91e0b258e3eb0fa0cb0a1a7` |
| [vLLM async_llm.py](https://github.com/vllm-project/vllm/blob/752a3a504485790a2e8491cacbb35c137339ad34/vllm/v1/engine/async_llm.py) | `69ea05aea497134204f1c6fd5f53994f4f7bbc35d24f90a768553dd8d6ba0b1a` |
| [vLLM core_client.py](https://github.com/vllm-project/vllm/blob/752a3a504485790a2e8491cacbb35c137339ad34/vllm/v1/engine/core_client.py) | `2d83952580e23abca33c3bb5f93edc349ea0e10da358bcb41b385f8d066e2a0b` |
| [vLLM core.py](https://github.com/vllm-project/vllm/blob/752a3a504485790a2e8491cacbb35c137339ad34/vllm/v1/engine/core.py) | `27b23827a86fd488f1f7cc9a2722fe76d284087ca7cfefef5a30c5e74f21bc71` |
| [vLLM utils.py](https://github.com/vllm-project/vllm/blob/752a3a504485790a2e8491cacbb35c137339ad34/vllm/v1/engine/utils.py) | `bc3fbc0fa6d8feb776b43ef008919ae4f28d732ce69bcd3b93649cd38795fc26` |
| [Uvicorn server.py](https://github.com/encode/uvicorn/blob/e4d0b05eb8c6459b7ba27ad13a2c2f4f8d4ece50/uvicorn/server.py) | `6a8fe07e699543f225cbc7d0c0027ffd26fec95797d5c6a10446d38c7929ed57` |

This runtime contract does **not** enable the v1 `--kv-events` option, which
remains restricted to the separately supported 487 contracts. The explicit
`--kv-v2-events` option below selects a different registered-state/L1 protocol;
runtime source compatibility alone does not establish cache transfer support.

The original and 487 API sources are shared; the 752 API source differs. Every
file must belong to the selected set. A mixed installation, a complete set under
the other adapter, missing/extra source members, or changed bytes are rejected.
Rechecking uses the same prospective selection; it never switches contracts.
The ordinary CPU ASGI fixture has separate `runtime_asgi_fixture_v1` and
`runtime_asgi_fixture_bootstrap_v1` contracts; neither can select a public runtime
set through the real entrypoint.

Pins are checked against bounded installed source files before serving imports
and again before sealing. Imported module locations must match those files.
The producer source retained as `producer.bin` binds all exact pin sets and
the plan-selected event contract. Replay requires that precise contract. This is native
**unauthenticated** source observation, not execution authentication or proof
that every dependency is unmodified. Different upstream/recipe-patched bytes
are unsupported; do not replace a pin with the local hash to make it pass.
Review a source-contract revision instead. No deployment-local source is copied.

The 487 set was reviewed at its concrete hook boundaries:

- `launcher.serve_http` creates `NoSignalServer`, a subclass that overrides
  only `capture_signals`; it inherits the hooked `uvicorn.Server.startup`.
  The bridge does not replace or infer the launcher's signal/shutdown policy.
- Uvicorn 0.52.4 retains `startup(self, sockets=None)`, awaits ASGI lifespan
  before creating listeners, retains `config.loaded_app`, `servers`, and
  `started`, and sets `started` only after listener creation. Failed lifespan
  now raises `SystemExit(STARTUP_FAILURE)` rather than returning with
  `should_exit`: the existing entrypoint's `BaseException` path retains failure,
  and the post-startup listening hook is not reached.
- `AsyncLLM.add_request` remains an awaited coroutine returning an output
  collector and adds optional `session_id`; the existing `*args, **kwargs`
  wrapper forwards it without changing the request. Its event observes entry
  to the frontend API, not downstream scheduler completion, cache transfer or
  backend-worker coverage. Successful inference still requires collected wire
  evidence. Streaming input, multi-choice expansion and direct engine-core
  traffic do not acquire new qualification from this source revision.

The 752 set has separately reviewed compatible boundaries, not inferred ones:

- `api_server.run_server` still enters `run_server_worker` in the same frontend;
  that constructs the async engine client before `build_and_serve` initializes
  app state and awaits `serve_http`. ASGI lifespan is imported from
  `serve.utils.server_utils`, rather than defined in the API module.
- `launcher.serve_http` creates `uvicorn.Server` directly, not `NoSignalServer`.
  The same `Server.startup` hook applies. Its existing watchdog, signal and
  shutdown behavior is not replaced by the bridge.
- Uvicorn 0.51.0 has `startup(self, sockets=None)`, reads the loaded ASGI app
  through its lifespan configuration, awaits startup before creating listeners,
  and sets `started` only after listener creation. Failed lifespan exits with
  `SystemExit(STARTUP_FAILURE)`; it cannot emit successful listening evidence.
- `AsyncLLM.add_request` remains an awaited coroutine with the existing request,
  reasoning and data-parallel arguments, but no 487 `session_id` parameter.
  The unchanged forwarding wrapper neither adds nor removes arguments. This
  observes frontend request entry only, not downstream completion or KV copying.

The imported lifespan helper and other transitive dependencies are not additional
members of the seven-file set. These reviewed boundaries do not authenticate every
dependency or substitute for an actual serving observation.

### Launch, readiness and inference

The bridge records `runtime_start` at its journal entrypoint before importing
serving libraries. It is explicitly **not OS process launch**: interpreter,
bridge imports and initial pin checking precede this anchor. Inspection reports
`launch_anchor:"bridge-entrypoint-not-os-launch"`; anchor-based milestone durations
begin there. Bootstrap instead uses its own entry-to-return pair below.
Full OS exec-to-ready remains unavailable.

The outermost ASGI wrapper records `ready` only on
`lifespan.startup.complete` (`asgi-lifespan-startup-complete-v1`). In the pinned
vLLM entrypoint this follows engine-client construction and app-state
initialization. This is ASGI/application readiness, not independent proof of
model correctness. `listening` is emitted separately after the pinned
`uvicorn.Server.startup` returns with `server.started` and actual sockets whose
`SO_ACCEPTCONN` is one. Uvicorn completes lifespan **before** starting listeners;
ready can therefore precede listening. The collector waits for both. No
health string, log line, model name or merely bound socket substitutes.

`inference_complete` hashes the actual ASGI response entity after its final
body send. Favorable first-valid inference still requires the existing Rust
wire collector's complete valid response, exact expected answer, usage and
matching digest. A `communication` gate remains unavailable on real runtimes,
not total startup or bootstrap time relabelled as communication.

### Frontend-observed engine bootstrap

The 487-bootstrap and 752-bootstrap contracts hook the synchronous static method
`EngineCoreClient.make_async_mp_client`: `bootstrap_start` immediately before
the call, `bootstrap_end` only after successful return. It forwards the original
arguments and result without adding a worker launcher or changing engine behavior.
An exception propagates to the existing entrypoint failure handler, leaving no
successful end or fabricated duration.

The actual public `487ecf187` source establishes this boundary, independently of
the older 0.27 review:

- `async_llm.py` calls `make_async_mp_client` during frontend construction.
- `core_client.py` selects `AsyncMPClient`, `DPAsyncMPClient`, or
  `DPLBAsyncMPClient`; each constructor synchronously reaches `MPClient.__init__`.
  Its ready-message loop receives and applies a response from every engine
  identity managed by **this client** before returning; timeout raises.
- The managed-process `utils.py::launch_core_engines` context also waits for
  engine startup on exit. The client ready-message loop applies even when
  engines are managed externally or through the Ray branch.
- `core.py::EngineCoreProc` calls the base engine constructor (including KV
  initialization) before starting the input thread that emits ready responses.

The public 752 source preserves this constructor boundary: `AsyncLLM` calls the
same static factory; its three concrete client variants synchronously reach
`MPClient.__init__`, whose managed-engine identity loop waits for and applies
every ready response before returning. Managed process launch also waits for
startup; the Ray and externally managed paths still pass through the client
ready loop. `EngineCoreProc` initializes its base engine before starting the
input thread that sends these responses. The factory's tracing decorator is
retained because the bridge wraps the original callable rather than replacing
its body. Failure still produces no successful bootstrap end.

This is frontend-observed synchronous engine-client construction through
readiness, including setup, IPC wait and constructor work. It is **not** a pure
backend compute span, per-worker readiness/PIDs, NCCL/communication initialization,
all-cluster readiness, or persisted model-KV proof. A client attaching to already
running engines measures that attachment, not their earlier startup. Later ASGI
app construction/readiness remains a separate milestone.

Replay requires a single ordered pair after `runtime_start`, followed by ASGI
`ready`, all with the same frontend monotonic clock and incarnation and strictly
increasing boundary timestamps. Missing/failed boundaries remain unavailable;
duplicates, reordered boundaries and incompatible frontend clocks invalidate the
pair and never contribute a bootstrap sample. Bootstrap samples use the existing
duration, import/replay and lower-is-better envelope comparison paths.

### Complete finite request exposure

Declare `runtime_window_us` in the plan (1 ms or more, strictly below
`deadline_us`). From entrypoint initialization through at least this duration
after ASGI ready, instrumentation records all mutating/unknown HTTP routes and
all websocket admissions, including requests without Grill headers. Only
GET/HEAD `/health`, `/metrics`, `/v1/models` bypass body observation. Unknown
routes are counted conservatively, not guessed to be harmless. A websocket is
recorded and rejected as unsupported; overlap closes admission and withholds
serial-order qualification instead of allocating unbounded concurrent bodies.

Grill adds `x-grill-startup-request` and `x-grill-startup-plan` for correlation;
it does not change the exact body or stable `cache_salt`. Unmarked requests
receive `external-N` IDs and cannot masquerade as first reload just by arriving
first. Headers remain unauthenticated correlations, not bearer credentials.
`AsyncLLM.add_request` is wrapped before engine initialization:
`runtime_engine_request` records actual admissions in the active ASGI context.
Internal/background calls without a live context produce a retained failure.
Missing engine admissions cannot be repaired by a 200 response alone.

At the prospective timer boundary the bridge closes **its inference admission**
and drains already admitted requests before emitting `runtime_coverage`, then
`sealed`. It does not stop the server; read-only metrics stay accessible.
Later inference attempts get 503 and cannot reach the wrapped app. This
behavior is opt-in and must be accepted when the operator selects the bridge.
It never seals just because the expected count has arrived: external requests
after the final Grill response but before the timer still invalidate ordering.
Timeout, changed source, partial drain, absent instrumentation or missing seal
withhold the claim. Byte/event caps and per-request deadlines fail closed.

The complete supported scope is **one frontend's ASGI and AsyncLLM admissions**.
Additional frontends, direct engine-core clients and backend/device graph/shape
warmup are not proven absent by these hooks. The bridge therefore does not emit
favorable smoke/shape suppression controls, and it cannot currently qualify
cross-restart KV persistence. `no_warming_attestation` cannot change that.

### Shared namespace identity and operator invocation

Run collector and producer in the **same PID and time namespaces**, with the
same `/proc` process view: for example, both inside the operator-owned serving
container, or both in a deliberately shared host namespace. `--pid` is the
actual producer PID in that namespace, not a container PID copied into a host
command. The collector checks `/proc/self/ns/{pid,time}` against
`/proc/PID/ns/{pid,time}`, saves both namespace identities, and requires
the runtime event's identity plus PID/start ticks/boot ID to agree.
Different namespaces fail closed. Automatic host/container PID translation,
namespace entry and container control are not implemented.

The observed `engine` process for this adapter is the API/frontend process,
not every backend worker. `--cache-pid` retains the kernel-observed identity of
the explicitly selected cache-server process, but this bridge does not infer
which cache process served a request.
Filesystem residency, compiled artifact/weight/prefix-offload identity and
temperature stay unknown. A matching frontend `PYTHONHASHSEED=0` is recorded
only as `frontend-only`; cache-server seed and KV transfer are unobserved.
Real response cached-token usage and selected metrics2 evidence are retained,
but they cannot identify external persisted KV by themselves.
`cache_result` is `unavailable`, never a synthetic answer-cache hit or an
inferred transfer. Store/reload remain callable and honestly nonqualifying for
these missing predicates.

For a separately authorized launch, prepare the normal complete startup plan and
select one of the four public adapters above. Select the exact 487-bootstrap or
752-bootstrap adapter and a `bootstrap` gate for the frontend span.
Pin `adapter_sha256` to the exact bridge file and `collector_sha256`
to the exact binary, and prospectively set the complete runtime
window/deadline/request allowances. Invocation is identical for every public plan;
no extra source-version flag or package-version string can override selection.
Source-only example:

```sh
# Operator-owned serving invocation, not a Grill action:
PYTHONHASHSEED=0 python3 tools/startup-runtime.py \
  --plan PLAN --events NEW_JOURNAL --stage startup -- \
  --model MODEL --host 127.0.0.1 --port 8000
# A separate shell in the SAME PID/time namespaces; actual running frontend PID:
grill-perf startup observe PLAN --slot 0 --pid PID \
  --events NEW_JOURNAL --producer-source tools/startup-runtime.py --out NEW_CAPTURE
```

Use the existing `store`/`reload` commands and matching bridge `--stage` for
restart evidence; operator alone replaces the serving invocation between them.
Do not invoke recipe smoke/shape warmups hoping the bridge will ignore them.
The existing recipe launcher remains intact and owns deployment lifecycle.
This source support has not itself exercised or qualified a recipe/image.


## Plan v1 and finite exposure

`startup::Plan` is a closed `version:1`, `kind:"startup-study-v1"` object. Its
required fields are:

- `study_id`, collector binary SHA-256, adapter enum and exact adapter source
  SHA-256; every capture rechecks these pins.
- `runtime_window_us` is required for every `runtime_*` adapter above, including
  `runtime_asgi_fixture_bootstrap_v1`; it is absent for existing non-runtime
  adapters. `log_ranks` (1–64) is required for `engine_log_v1` and absent
  otherwise. These explicit adapter contracts extend the closed plan without
  reinterpreting old bytes.
- `setup_axis` and `setup_sha256:[A,B,A2]`: prospectively declared setup
  fingerprints. A and A2 must match. These remain declarations, not observed
  proof that the only effective difference was that axis.
- `slots:[{id,arm,warmup,index}]`: arms `a`, `b`, `a2`, in that order, each with
  1–20 warmups followed by 3–100 measured acquisitions. Equal counts in all arms,
  contiguous indices within warmup/measured phases, unique IDs, at most 360 slots.
- `study:{kind:"startup"}` or the explicit restart-cache contract below.
- `states:[{class,declared,required_observed}]`: 1–6 unique prospective class
  conditions. Temperatures are `cold`, `warm`, `unknown`. An unknown observation
  never proves a requested cold or warm state. Both declared and observed state
  remain visible; a compiled-artifact cold condition is not a KV miss.
- `request:{model,prompt,max_tokens,expected_answer}`: bounded exact prompt and
  expected answer. The narrow producer uses nonstreaming, one-choice text,
  temperature zero, output cap 1–32768, without hidden sampling or namespace
  changes. All repeated list entries use this same exact request body.
- `endpoint`, `local_http`, optional credential environment **name**, optional
  finalized `metrics::v2::Config`; no credential value is retained and model
  credentials are never forwarded to metrics. Metrics2 is mandatory for restart.
- `max_events` 8–1024; `event_bytes` 1 KiB–4 MiB; `deadline_us` 1 ms–1 hour;
  `limits` with total HTTP 1–600000 ms, idle bounded by total, response at most
  1 MiB, aggregate retained-response allowance at most 16 MiB. Admission checks
  the full request/scrape deadline allowance and request/response byte ceilings.
  Encoded requests stay within the existing 2 MiB ceiling. These are serialized
  buffer ceilings, not an RSS guarantee.
- Nonempty `gates`, unique targets among `listening`, `ready`,
  `first_valid_inference`, `communication`, `bootstrap`, `reload`, each with prospective
  `max_regression_bps` and `max_reference_spread_bps`. Reload is only legal in a
  restart study. There is no token-rate or C1 conversion for these gates.

`tools/smoke-startup.py` contains a complete generated prospective plan example
with every field and the exact finalized metrics2 configuration. It pins the
binary and adapter file actually supplied to the fixture, rather than embedding
an obsolete sample fingerprint in documentation.

## Events, clocks and milestones

Each NDJSON event has `version:1`, a contiguous `sequence`, `clock`, `offset_us`,
engine incarnation, and a tagged `event`. The clock has an ID/origin,
`kind:"monotonic"`, `units:"microseconds"`, positive `resolution_us`, and
`synchronization:"single-process-origin"`. Resolution describes timestamp
quantization, not a claim of hardware timer precision.

| Event | Supported meaning |
|---|---|
| `launched` | First event from the attached process incarnation |
| `listening` | Producer has bound/listened; never model-ready or valid inference |
| `communication_start/end` | Explicit paired communication-initialization substage |
| `bootstrap_start/end` | New bootstrap adapters only: synchronous frontend engine-client entry/successful return |
| `ready` | Exact `neutral-ready-v1` producer readiness contract; no model-name heuristic |
| `controls` | Source-observed smoke/shape suppression and hash-seed contract, before requests |
| `state` | Per-class declared/observed temperature plus appropriately typed identity |
| `request_start` | Request ID and SHA-256 of exact received body, in observed order |
| `inference_complete` | Request ID, retained response digest and supported per-request cache result |
| `failure` | Retained producer failure; cannot disappear behind faster samples |
| `sealed` | Final complete request count after inference admission closes |
| `runtime_start` | Bridge-entrypoint anchor, producer pin, source contract and observed namespaces; not OS launch |
| `runtime_engine_request` | Actual frontend engine admission correlated with the current ASGI request |
| `runtime_coverage` | Prospective minimum window elapsed, admission closed, active requests drained, hook installed |


For the neutral producer, `launched` is its first explicit source event, not the OS
`exec` instant. Interpreter/import work preceding the journal origin is outside
these source-clock durations; full OS-process-launch latency is unavailable.
First-valid-inference requires an actual native-collected, complete valid
response with exact expected answer, supported usage, stop termination and
matching source response digest. Failed/missing client evidence cannot be
replaced by an adapter event. Header/body arrival and health checks do not count.

Anchor-to-listen, anchor-to-ready and anchor-to-first-valid-inference subtract
only timestamps with identical clock contract/origin **and** process incarnation.
Communication and bootstrap durations each use their own distinct supported
start/end pair. Missing or cross-clock boundaries stay unavailable. HTTP reload
duration is separately client-clock `settle_us`; it is never subtracted from a
source-clock timestamp.
All request/metrics offsets within a capture share a capture-scoped client
origin. Across acquisitions, duration comparison requires matching clock kind,
units, resolution and synchronization, but not the same origin ID.

## Explicit restart-cache study

The `restart_cache` study adds:

```text
identity: CacheIdentity {
  version: 1, prompt_sha256, source_sha256, cache_salt,
  hash_seed_contract: "PYTHONHASHSEED=0:engine+cache_server"
}
post_restart: [{id:"first_reload"}, ...]  # 1–16 unique ordered requests
classes: [{class:"engine",transition:"changed"},
          {class:"cache_server",transition:"persisted"}, ...]
no_warming_attestation: boolean         # declaration only, never sufficient
```

The exact cache salt and prompt/source pins are shared by store and reload.
There is no random acquisition namespace, salt suffix, seed increment or
per-acquisition derivation. The prompt hash is checked against retained prompt
bytes; the source hash is a reviewed source declaration pinned in the plan.
Existing conversation namespaces are unchanged.

Engine/cache-server identity is a process incarnation. Filesystem,
compiled-artifact cache, weights and prefix-offload identities are instead
`{content_sha256,generation}` artifact identities. Required class transitions
are independently assessed. Same engine means invalid restart evidence;
missing relevant class evidence means unavailable; restarting the cache server
is not persistent KV across an engine-only restart.


The neutral producer observes engine/cache-server process identities only.
Its nonprocess identity and temperature fields remain explicitly unavailable
rather than hashing class names or pretending synthetic weights were observed.
Artifact identity predicates can be replayed from imported evidence, but native
qualification for those classes needs an actual supported source.
The first observed post-restart request must be `first_reload`. Source request
starts and finishes must exactly match the finite collected list, received
body hashes and retained response hashes, with a complete admission-closure
record and no preceding/extra request. Missing suppression or ordering evidence
withholds persistence even when the operator asserts there was no warming.

Actual `persisted_hit`, `recomputed`, `stored`, and `unavailable` results stay
distinct. A favorable persisted result requires the supported neutral producer's
actual cache lookup plus positive request cached-token evidence, matched stable
identity, changed engine, persisted cache server, and actual metrics2 snapshots.
The narrow neutral source additionally requires one complete full-label external
query/hit diagnostic pair with positive deltas; multiple-label or absent/zero
diagnostics withhold the claim, without summing or attributing server counters.
Metrics2 parsing, cache diagnostics, continuity views and accounting consistency
are reused without substituting a parallel schema. **Server-wide metrics are
still diagnostic, not per-request attribution**; no local/external counter alias
or inferred hit from speed is introduced. A missing promised hit or recomputation
is nonqualifying, not a zero-valued successful reload sample.

## Public recipe source mapping versus qualification

The pinned public DeepSeek recipe at
[`f3d76450e075d5f89698aed2e865655f0b7eeb1c`](https://github.com/MiaAI-Lab/DeepSeek-v4-Flash-DSpark-2x-DGX-Spark/tree/f3d76450e075d5f89698aed2e865655f0b7eeb1c)
provides these **source-reviewed constraints only**:

- [`lmcache/README.md`](https://github.com/MiaAI-Lab/DeepSeek-v4-Flash-DSpark-2x-DGX-Spark/blob/f3d76450e075d5f89698aed2e865655f0b7eeb1c/lmcache/README.md):
  stable `PYTHONHASHSEED=0` on engines and cache servers; independently surviving
  cache servers; a ZMQ port answer is listening, not ready or valid inference.
- [`docs/ENVS.md`](https://github.com/MiaAI-Lab/DeepSeek-v4-Flash-DSpark-2x-DGX-Spark/blob/f3d76450e075d5f89698aed2e865655f0b7eeb1c/docs/ENVS.md):
  launcher smoke precedes the default shape warmup; qualifying first reload
  would need no smoke and `DSPARK_BOOT_SHAPE_WARMUP=0`, or an operator launch path
  without those requests, **with actual complete observation of that fact**.

These uninstrumented recipe logs cannot establish complete request coverage,
substage boundaries, hidden-warming absence, actual restart identity or a
persisted first reload. `uninstrumented_recipe_logs` is an explicitly
nonqualifying imported/declaration source category, not a parser or a native
producer for those logs; the separate `engine_log_v1` contract below parses only
its own closed line grammar. No real DeepSeek/GLM startup adapter or backend has been
exercised or qualified by this source delivery. Production source support for
those unavailable facts requires separately reviewed instrumentation and an
explicit disruptive observation window; do not translate the neutral fixture's
result into model-KV persistence or communication speedup.

## Engine-launcher logs

An engine started by its own launcher (for example a multi-host recipe) has no
Grill-attached process or journal. Instead the launcher writes a finished startup
log, and `startup from-log` builds a capture from it. Grill never starts, waits
for or reads anything but that file; `--log` is an ordinary file, not a command.

The log is the engine's own `grill-startup-v1` lines and nothing else, one per
line, UTF-8, each ending in `\n`, separated by single spaces:

```text
grill-startup-v1 <microseconds> <host> <rank> <event>
```

`<microseconds>` is that host's monotonic clock; `<rank>` is `0..log_ranks`;
both are unsigned decimal. `<host>` is a short ASCII ID. Events are closed:

| Event | Rule |
|---|---|
| `launch` | Rank 0, the first line; origin of `ready` |
| `weights_start` | Once per rank, after `launch` |
| `weights_end` | Once per rank, after its `weights_start`, strictly later |
| `engine_ready` | Rank 0, the last line, after every rank's `weights_end`; the whole multi-host engine is ready |

Hosts have independent clocks, so a rank keeps one host, each host's timestamps
never regress, and every span subtracts only timestamps from one host. Lines of
different hosts may interleave; file order across hosts is not ordering evidence.
Any other line (including ordinary engine output), unknown event or rank, missing,
duplicate or out-of-order event, or text after `engine_ready` rejects the whole log.
Filter the engine's output to these lines before capture; never trim a rejected log.

The capture retains the plan, the pinned launcher source as `producer.bin`, the
raw log bytes as `events.bin`, and a `capture.json` with `provenance:"log_derived"`;
it has no engine identity or requests. A rejected log is still retained, as an
`ERROR` acquisition. Inspection reports `ready` (rank 0 launch to `engine_ready`,
clock `engine-log-head-host`, `launch_anchor:"engine-log-launch-line-not-os-launch"`)
and `weight_loads:[{rank,host,duration_us}]` for every rank.

An `engine_log_v1` plan is a startup study whose gates are all `ready` and whose
states all require `unknown`: logs observe no requests, cache state or
communication. `compare` gives the [usual verdicts](#offline-decision-and-limitations),
for example A/A2 on one loader setting and B on another, with scope
`log-derived-unauthenticated-domain-comparison-not-live-qualification`. The same log
bytes in two slots are not independent acquisitions and are rejected. Log lines are
unauthenticated launcher output, not native observation; import keeps them imported.

## Pinned model-KV source observations

`tools/kv-journal.py` and `startup verify-kv` add the separate observation identity
`vllm-487ecf187-lmcache-3e11b8ed-kv-copy-v1`. They do **not** rewrite, upgrade or
remove the unavailable semantics of any previous startup capture.

The supported source scope is
`lmcache-driven-full-attention-l1-engine-restart`: vLLM revision
`487ecf187d3dfe74d2cf6119a92881dba403c219`, LMCache revision
`3e11b8ed191631e6f098b8038235823f1a410b24`, native object-group transfers,
complete chunks with no token skip, all selected ranks and object groups.
Actual worker metadata must report `ParallelStrategy.mla_only = false` and
`n_servers = 1`, with vLLM world/rank equal to the KV world/ordinal. MLA and
multi-server rank remapping are rejected even if their operation counts fit.
An explicit multi-producer journal union does not qualify remapped ordinal
domains.
The whole Python source sets and their SHA-256 values are explicit constants
in the hook and Rust consumer. These pins identify source, not a running
deployment or a qualified native binary.

### Operator installation and observation windows

Importing the hook uses only Python's standard library. Installation is a
separate, explicitly authorized backend action, not performed by Grill:

- In each cache-server process, before a transfer module or native callback
  dispatcher exists, construct `Journal(NEW_PATH, role="cache", ...)` and invoke
  `install_lmcache(journal)`. Keep **that same journal and cache process** alive
  across the store and reload. Seal it only after the finite selected reload
  window using `journal.seal(timeout=SECONDS)`, with `0 <= SECONDS <= 60`.
- In each spawned engine-worker process, use vLLM's existing
  `parallel_config.worker_extension_cls = "kv-journal.WorkerExtension"` path.
  Make the selected tools directory importable in **every worker environment**,
  and explicitly provide `THEGRILL_KV_WORKER_DIR` (an existing private directory)
  and `THEGRILL_KV_WINDOW_US` (1,000–3,600,000,000). The extension verifies the pinned
  `WorkerWrapperBase` source and installs before the worker initializer can
  construct `LMCacheMPWorkerAdapter`; it does not rely on inheriting a frontend
  monkeypatch through `fork`. Files are exclusive
  `worker-{boot_id}-{pid}-{start_ticks}.jsonl` journals. Retain all selected ranks'
  separate pre/post-restart files. No environment propagation or remote
  installation is performed automatically.
- For the pinned 487ecf187 startup runtime, supply
  `--kv-events NEW_FRONTEND_JOURNAL --kv-producer-sha256 SHA256_OF_KV_JOURNAL`
  before `--`. The bridge installs the actual `_add_request` observer. At its
  existing finite admission-coverage boundary it invokes the existing engine
  `collective_rpc("grill_kv_seal", ...)`, requires all returned worker seals to
  complete within the remaining deadline, then seals its frontend journal.
  Both `add_request` and the final engine-submission boundary remain observed;
  a client/header ID or an OpenAI response ID is not assumed to be a session ID.

The low-level `install_vllm_worker(journal)` API remains available to an
operator-owned bootstrap that installs before adapter construction and seals
before worker shutdown. The concrete extension uses that same API. Existing
custom worker extensions or allocation paths that bypass this extension's
`__new__` are not silently composed or assumed covered: missing installation,
RPC failure or missing worker journals is nonqualifying.

These APIs do not supply a backend entrypoint, patch a deployment or launch a
service. Operators must establish the exact loaded source and selected transfer
mode and place cache installation before construction. Duplicate/late
installation is rejected; there is no post-warmup re-arm or journal rotation.
The complete frontend journal must contain exactly one measured non-child
admission. Additional frontend warmups or other admissions inside that window
disqualify it; do not trim records or reinterpret them as absent. A zero-frontend-
warmup plan does not prove internal warmup suppression or admission eligibility.

Observation closure does not stop inference or cache operations. New source
work or unbound native callbacks during the pending drain makes the journal
fail closed while preserving the original operation/handler. Work after a
completed observation fence is outside that finite window. Keep producers and
their existing native dispatcher alive until all tracked callbacks drain and
the journal seals; stopping the dispatcher first can lose terminal delivery.
No GPU-wide synchronization is added. Shutdown, restart and native
qualification require their own authorization.

The producer directly observes the selected ObjectKeys and expected token/byte
extents, successful nonempty native-plan calls, returned operation outcome,
existing stream-ordered callback delivery and successful per-key finalization.
`device_complete` is observed callback completion, not a separately sampled
device fence: copy completion follows by the pinned same-stream host-callback
ordering (the cache context's external stream wraps its transfer stream).
That transitive source claim still requires native qualification. Store-side
per-key write finalization is independently observed. Retrieve-side per-key
read-lock-release status is outside this copy scope; an already completed H2D
copy is not a claim that all read locks were successfully released.
Observed callbacks use two fixed kinds in the **existing** dispatcher, carrying
the operation ID and exact typed key list. Each still invokes the original
backend handler once. Unobserved work keeps its original callback kind.
Missing native delivery remains pending even if the aggregate event bus has
contiguous sequence numbers. Empty success, tensorless cleanup, partial or
failed copy, unsupported mode, wrong key/rank/salt/group, observation exception,
exhaustion and incomplete drain cannot establish copy support.

### Offline consumer and retained eligibility

Create a prospective expectation file, pinning the exact hook source:

```json
{
  "version": 1,
  "source": "vllm-487ecf187-lmcache-3e11b8ed-kv-copy-v1",
  "scope": "lmcache-driven-full-attention-l1-engine-restart",
  "producer_sha256": "SHA256_OF_KV_JOURNAL",
  "model": "ACTUAL_MODEL_NAME",
  "salt": "ACTUAL_CACHE_SALT",
  "world_size": 2,
  "groups": 1,
  "chunk_size": 256,
  "start": 0,
  "end": 256,
  "store_request": "store",
  "reload_request": "DECLARED_FIRST_RELOAD_REQUEST"
}
```

The illustrative uppercase strings are operator inputs, not valid evidence.
Worker ordinals are exactly `0..world_size`, groups exactly `0..groups`; no
inference from which ranks happen to report is allowed. A retained ObjectKey's
`kv_rank` is a **different**, packed topology field: the pinned converter calls
`ComputeKVRank(world_size, worker_id, world_size, worker_id)`, encoding
`(world_size << 24) | (worker_id << 16) | (world_size << 8) | worker_id`.
Even the single-worker ordinal 0 has `kv_rank = 16777472`, not zero.
This source scope supports one full-range store and one reload per rank, and
one non-child frontend admission per selected request. Multiple/incremental
operations, child sampling, MLA, sliding windows, skipped prefixes,
fallback/GDS transfers, SHM/engine-driven transfer, L2/server restart, disk
durability and crash persistence are explicitly unsupported, not silently
counted as the broader retention goal.

```text
grill-perf startup verify-kv EXPECTATION.json \
  --cache-events CACHE_PRODUCER_0.jsonl --cache-events CACHE_PRODUCER_1.jsonl \
  --store-frontend STORE_FRONTEND.jsonl --reload-frontend RELOAD_FRONTEND.jsonl \
  --store-worker STORE_WORKER_0.jsonl --store-worker STORE_WORKER_1.jsonl \
  --reload-worker RELOAD_WORKER_0.jsonl --reload-worker RELOAD_WORKER_1.jsonl \
  --producer-source tools/kv-journal.py --reload-capture RELOAD_DIRECTORY
```

The explicit source-file union is bounded to 64 ranks, 64 groups, 1,024 chunks,
10,000 records per journal, 65,536 bytes per record and 16 MiB total retained
journal input. There is no receiver, automatic discovery or network aggregation.
Every rank's store and reload must occur in the same cache-process journal;
duplicate cache processes, missing ranks, changed cache incarnation, or any old
worker process reappearing in the reload cohort are nonqualifying. Old and new
worker-process sets must be disjoint, even when ranks are permuted.
`{boot_id,start_ticks,pid}` and the
independent journal incarnation are retained; PID or adapter UUID alone is
insufficient.

The result separately exposes `source_copy_supported`,
`admission_warmup_eligible`, and both historical startup inspections. Missing
`--reload-capture` still allows source replay, but cannot qualify admission.
An existing single-cache-PID capture cannot qualify a distributed admission
scope. Runtime evidence still cannot establish hidden warmup suppression or
the cache-server hash-seed contract; successful KV observations do not invent
those facts. `PASS` requires both source support and independently eligible
retained startup evidence. Even then it is a source observation, not a
cryptographic attestation or live-backend qualification. Unknown is never
converted to recomputation or a zero-work assertion.

### CPU source-boundary verification

`tools/kv-source-smoke.py` executes the installed hooks around unchanged pinned
source bodies with controlled CPU buffers, allocator, IPC and native-kernel/
completion-queue boundaries. It executes the actual ObjectKey/IPC schemas and
key converter, actual ParallelStrategy properties, and the
`WorkerWrapperBase.init_worker` construction plus `AsyncLLM.collective_rpc`
sealing boundaries in spawned children. It does not import vLLM, LMCache, torch or a
device backend. The optional `--binary` path executes the real Rust consumer
on source-generated frontend/worker/cache journals from separate CPU processes.
Cases cover multi-rank/multi-producer/multi-group copies, underflow, genuine
partial copies, same-key arrivals during drain, frontend context/multiplicity,
and malformed or cross-process journal joins. Consumer mutation cases are
deliberately untrusted fixture evidence, not claims of native execution.
Source files remain external:

```text
python3 -I -S tools/kv-source-smoke.py \
  --verified-sources VERIFIED_SOURCE_DIRECTORY \
  --additional-sources SUPPLEMENTAL_PINNED_SOURCE_DIRECTORY \
  --binary target/debug/grill-perf --out NEW_PRIVATE_DIRECTORY
```

The supplemental directory contains the pinned
`vllm/v1/engine/async_llm.py`, `vllm/v1/worker/worker_base.py` and
`lmcache/v1/gpu_connector/gpu_ops.py`; their bytes are hash-checked before
compilation. The corrected verified source set supplies the remaining files,
including `distributed/api.py` and `multiprocess/custom_types.py`. Native
transport/codec dependencies are controlled leaves in the standard-library
smoke; this is not native CUDA or transport qualification. CPU success does
not establish deployment mode, native installation, actual model KV,
admission/warmup, first-reload performance, or the required A/B/A2 acquisitions.

## Candidate registered-state/L1 source observations v3

The existing `tools/kv-journal-v2.py` entrypoint and `startup verify-kv-v2`
command now select journal/expectation/report version **3**, identity
`vllm-752a3a504-lmcache-ddc5fa34-kv-copy-v3`, scope
`registered-state-l1-engine-restart-v3`. Filenames and launch flags are retained;
the source identity and wire version are not aliases for the previous protocol.
V1 bytes and frozen examples are unchanged. V2 captures are not upgraded or
reinterpreted: use their matching historical artifact. Candidate source identity
is neither independent review nor native admission.

Freeze a prospective expectation **before** the qualifying journals. Its required
fields are `version: 3`, `source`, `scope`, `producer_sha256` (the exact
`kv-journal-v2.py` bytes), `registered_state_v2`, `salt`, `store_request`,
`reload_request`, `start` and `end`. The selected range contains 1–1024 whole
LMCache chunks, with zero prefix skip. Source hashes for adjacent observer
components are additionally bound to the verifier's compiled artifact.

`registered_state_v2` is a closed object containing `schema:
"thegrill.kv.registered-state.v2"`, `physical_template`, `model_flags`,
`worker_template` and `cache_template`. Physical topology supplies
`vllm_world_size`, `tp_size`, `pp_size`, `dp_size`, and `n_servers`; flags supply
`mla_enabled`, `is_hybrid`, and `mla_only`. Worker identity includes exact model,
engine type, required nullable layout hint, ordered final registered layers and
wire engine groups. Cache identity includes those wire groups, tensor/exclusion
inventory, server policy, chunk size, all resolved kernel shapes/dtypes/formats,
allocation components, ordered object groups and their window policy.
The typed records in `startup_kv_registration.rs` define the exact closed shapes.
Only observed `shape.nb` is omitted from the prospective cache shape; block stride
and every other layout field remain identity. Never fill any field from a model
name, quantization label or guessed default. An initial collection run may inform
a separately reviewed template, but cannot validate itself: freeze the template
and collect fresh worker processes and engine instances.

The closed v2 geometry projection is retained unchanged within the new source
protocol. The required `observation_policy` in every start row binds DCP size 1,
ordinary attention groups only, absent request-scoped configuration, the default
CUDA event backend, and a single registration generation. Auxiliary and recurrent
groups, partial-prefix copies, DCP sharding, non-ZMQ/engine-driven contexts,
experimental dispatchers and lazy offload are explicitly unsupported. Their
runtime behavior is not silently projected into attention-only geometry.
Worker submissions and cache operations additionally carry `num_kv_readers`;
the consumer checks it against the physical MLA/non-MLA reader projection.

The event backend is checked before instrumentation, including the cached
selection: isolated/timeline IPC and unknown backends fail closed. Only the
source-verified default CUDA event query is used; the observer never creates an
event, copies device data or synchronizes a stream. A successful transfer result,
the exact source transfer generation, a unique recorded event and the existing
callback/lifetime checks are all necessary for completion. False results, missing
handles, reused events, worker exceptions, load errors, dropped requests,
unhealthy drains and context invalidation cannot produce a complete fence.

Cache fencing additionally requires the exact source-owned completion dispatcher
to have stopped and drained: a still-live thread, failed periodic run, swallowed
native drain error, unknown callback kind, or decode/handler exception invalidates
the observation. An explicit cache seal before the owning stop cannot qualify.
The observer reads lifecycle state only; it adds no joins or device synchronization.
The defining periodic-thread, tracing-decorator and NVTX Python sources are pinned.
L1 synchronization, tracing and NVTX decorator shells retain their original
behavior and must match their source bodies and delegate closures; `__wrapped__`
metadata alone is not accepted. Allocation readers and finalization callbacks
must retain both their class and instance bindings throughout the observation.

Run the supplemental CPU source-path regressions with
`python3 tools/kv-observer-regression.py --source-root SELECTED_LMCACHE_ROOT`.
It executes pinned adapter completion/submission bodies and the pinned default
event query with inert transport/device inputs. It is not an installer test,
native copy proof, serving qualification or admission decision; full source
installer and consumer validation remain separate obligations.

Operator-owned installation is explicit:

- Keep all six adjacent v2 files together: `kv-journal-v2.py`,
  `kv-transfer-v2.py`, `kv-lifetime-v2.py`, `kv-registration.py`,
  `kv-registration-hooks.py`, and `kv-v2-pins.json`.
- Configure the existing vLLM worker extension as
  `kv-journal-v2.WorkerExtension`, with the tools directory importable in each
  worker and explicit `THEGRILL_KV_V2_WORKER_DIR` and
  `THEGRILL_KV_V2_WINDOW_US`. Directory creation, serving configuration and
  process lifecycle remain operator actions.
- For the runtime752 bootstrap only, use
  `--kv-v2-events NEW_FRONTEND.jsonl --kv-v2-producer-sha256 SHA256`
  before the existing `--` serving arguments. Mixing v1 and v2 options fails.
  After the runtime admission window closes and active requests drain, the existing
  collective RPC invokes `grill_kv_v2_seal`; all worker seals must succeed before
  the frontend journal seals. Early runtime closure records failure instead.
  No endpoint is added.
- In each persistent cache process, before transfer/dispatcher construction,
  explicitly import the module, create `Journal(path, role="cache")`, and call
  `install_lmcache(journal)`. After both store and reload phases, the pinned
  transfer module's existing `close()` seals with no wait after its source-owned
  `dispatcher.stop()` returns and before cache contexts are released. A manual
  seal before that owning stop fails closed. Successful termination, final drain,
  callback delivery and event readiness are all required at this boundary;
  shutdown is not an excuse to invent completion. The observer neither initiates
  shutdown nor changes its ordering. The operator must retain the same enumerated
  cache processes across restart; closing a cache between phases cannot qualify.
- Every participating interpreter must actually observe `PYTHONHASHSEED=0`;
  frontend-only environment inheritance is not evidence of worker/cache state.

```text
grill-perf startup verify-kv-v2 --expectation EXPECTATION.json \
  --cache SLOT_0_CACHE.jsonl --cache SLOT_1_CACHE.jsonl \
  --store-frontend STORE_FRONTEND.jsonl --reload-frontend RELOAD_FRONTEND.jsonl \
  --old-worker OLD_RANK_0.jsonl --old-worker OLD_RANK_1.jsonl \
  --reload-worker NEW_RANK_0.jsonl --reload-worker NEW_RANK_1.jsonl
```

`--cache` order is prospective server-slot order, exactly `0..N-1`; a consistently
swapped observed mapping fails. Supply every physical worker in each phase,
including MLA nonwriters; stores are required only from strategy-derived writers,
reloads from all physical readers. Worker argument order is immaterial because
physical rank is observed. Cache-local ranks and keys may repeat on different
slots without merging their residency evidence.

The producer captures raw CPU block IDs before source mutation, records paged
keys/blocks/exclusions/callbacks, observes actual nonempty native or per-kernel
execution and both fallback staging legs, and binds weak L1 allocation generations.
The consumer checks exact window suffixes and per-kernel nonnull stored
counterparts. Reservation failure or a missing object is never an all-null witness.
Callback delivery, write finalization and device completion are distinct: the
existing source event is queried nonblocking from the existing dispatcher drain
or before the next operation, before weak objects are dereferenced. No event,
device synchronization, polling thread, tensor retention or allocator lease is
introduced. Free/invalidate/resize/replacement before required completion fails;
retirement after the final required completion need not invalidate past evidence.
Recovery/re-registration disqualifies this bounded fresh-registration scope.
The optional native object-group callable is pinned at installation only when
the pinned source selects that path, then checked by identity after execution
and at sealing. A missing optional native symbol preserves the verified
per-kernel torch fallback; a selected Python replacement is rejected at install.
This callable-origin check is not a native-binary qualification.

`observe` callbacks run synchronously under the journal condition lock and retain
no arguments afterward. They must not block waiting for the dispatcher, MQ loop
or any thread that may need that lock; event queries must remain nonblocking and
lifetime locks reentrant/leaf-only. The pinned dispatcher releases its registry
lock before calling handlers, so no reverse registry-lock/condition wait is added.

Rows are capped at 65,536 bytes, journals at 10,000 rows, and all supplied evidence
at 16 MiB. Large operation lists use fixed 64-item pages with exact offsets and
totals. The row cap applies to every row, including registration: an oversized
row fails the journal closed and is never truncated or emitted as valid evidence.
Oversized expectation metadata also fails its bounded input check.
Object records must be JSON objects, including nested layouts, topology, process
identity, keys and elements of record arrays; positional arrays are not alternate
record encodings. Missing/duplicate/unknown fields, wrong roles, floating-point
integers, source mismatch, incomplete pages, unbound scheduling and incomplete
fences fail closed.
Scheduled-step observation is deliberately bounded to at most 64 request IDs;
it is not an arbitrary busy-production-load recorder. More IDs fail the journal,
and scheduling unrelated to the one prospectively selected request is
nonqualifying even below that cap. Do not enlarge, trim or drop scheduler records
to make a busy run qualify.

The pinned source ordering is load-bearing: `group_view.py` expands IDs in wire
group order, and `kv_layer_groups.py` checks the corresponding ordered layer
inventory when constructing kernel groups. Its `slots_per_block == shape.bs`
and validated chunk divisibility make the observed per-chunk block count exactly
`chunk / tokens_per_block`. The source records an event before callback submission,
and the existing final dispatcher drain runs before cache-context release.
These are public-source facts, not observations of a deployed native backend.

A complete v3 copy join sets `source_copy_supported`, **not** native or admission
eligibility. Its overall outcome remains `INCONCLUSIVE` (exit 2), with
`admission_warmup_eligible: false`: existing single-cache-PID admission evidence
does not qualify distributed G7, and metadata/CPU fixtures do not prove real
model contents, deployment-global absence of unlisted caches, or native execution.

## Offline decision and limitations

Comparisons reuse `envelope::range` and `envelope::assess` with exact rational
microsecond samples and lower-is-better direction. A and A2 form the reference
range; every declared warmup and measurement must qualify. Native captures must
have distinct engine incarnations; the acquisition chain and plan fingerprints
must agree. Actual prompt/completion-token exposure must match. No dropping failed
trials, best-case selection, threshold adjustment or survivor-only gate is allowed.

Outcomes and exits retain shared policy precedence:
`ERROR` (1) > `REGRESSION` (3) > `INCONCLUSIVE` (2) > `PASS` (0).
An imported startup arithmetic PASS is explicitly `imported-comparison-only`,
not adapter exercise. Restart-cache persistence cannot PASS on imported evidence.
No outcome establishes causality, universal no-regression, production percentile,
serving speed, real-adapter exercise or live qualification.

## Later CPU verification

These exact commands are intended for the integration owner on a stable head:

```sh
cargo build -p grill-perf --locked
python3 tools/smoke-startup.py --binary target/debug/grill-perf --case startup
python3 tools/smoke-startup.py --binary target/debug/grill-perf --case restart
python3 tools/smoke-startup.py --binary target/debug/grill-perf --case comparison
# Or all three in one finite invocation:
python3 tools/smoke-startup.py --binary target/debug/grill-perf --case all
# Real bridge code, ordinary ASGI fixture; no vLLM/Uvicorn/device imports:
python3 tools/smoke-startup-runtime.py --binary target/debug/grill-perf
```

The smoke authors actual delayed ordinary processes, source failures and
malformed data, wrong clocks and stale/same/missing incarnations, identity drift,
hidden/extra/reordered requests, missing reload/hit/metrics evidence, full
recomputation, imported-provenance rejection, exact 5% boundary arithmetic,
insufficient acquisitions and broken order. It uses only synthetic loopback
fixtures. Its arithmetic inputs are explicitly imported synthetic mutations,
not relabelled live observations. Execution is a separate verification gate;
source authoring alone does not claim these commands passed.

The runtime smoke preserves the existing neutral cases and adds real bridge
lifecycle/admission execution through `tools/startup-asgi-fixture.py`
(`runtime_asgi_fixture_v1`, explicitly not vLLM execution): independent
ASGI-ready/listen/inference delays, external requests before and after the
planned request, direct internal engine admissions, failed ASGI startup,
container-namespace mismatch, source drift and missing seal. A separate real
store/operator-owned process replacement/first-reload fixture preserves stable
request identity and verifies unavailable KV accounting never becomes PASS.
It compares actual CLI capture with offline replay. Both CPU execution and
pinned-library/live qualification remain separate checks; authoring these
fixtures is not a pass.

The new `runtime_asgi_fixture_bootstrap_v1` / `ordinary-asgi-fixture-bootstrap-v1`
CPU contract adds an independently delayed synchronous constructor through the
real bootstrap hook, followed by separately delayed ASGI readiness. The runtime
smoke retains the old ASGI/communication/cache fixtures and adds missing start,
missing end, missing pair, thrown construction failure, duplicate start/end,
reordered and late boundaries. Offline imported mutations cover changed clocks,
stale incarnation, pre-runtime ordering, a failure followed by a false end,
legacy-contract rejection, unavailable communication and an exact 100000-us
bootstrap arithmetic oracle. Native capture and imported evidence are replayed
with the same consumer. These are authored regression scenarios, not executed
verification or public-vLLM qualification.

The runtime smoke also authors source-set distinction cases using tiny
synthetic file bytes and substituted expected pin tables (never substituted
hash/read/selection functions): all three exact sets, a valid set under the wrong
plan adapter, every mixture of the three original changed files, each missing
member, extra members, unsupported/fixture selectors, and each file drifting on
recheck. Actual public pins are also checked against wrong synthetic bytes.
These CPU cases do not execute or qualify either serving library; the complete
smoke command above remains the Main-owned verification entrypoint.
