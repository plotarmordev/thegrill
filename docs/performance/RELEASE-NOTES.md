# Performance release notes

## Version 0.5.0 — current sparkDash decode prompts and varied-text prefill

- Add `sparkdash-decode-v2` and its portable copy
  [`sparkdash-decode-portable-v2`](README.md#portable-sparkdash-copies). They
  follow sparkDash 1.8.7, which replaced the repetitive `clamp_NN` code prompt
  with a real Python task; every other prompt, cell, trial and limit is unchanged.
  The v1 workloads stay as they were, and their code cells are not comparable
  to v2's.
- Add deterministic `generated-prose-v1` fill and the
  [portable prose prefill selection](README.md#generated-prose-prefill) for
  cell-by-cell comparison with repeated-unit prefill (#97). Recorded request
  salts regenerate varied bodies; selected reports name their fill kinds.
  Existing workload bytes and identities remain unchanged; prose and filler
  results are never pooled.

Verification and limits of this version:

- Hosted CI covers the Linux x86_64 and aarch64 build, format, tests, clippy,
  release staging and installed-archive smoke, and the `grill-perf` build,
  tests and clippy on macOS. Release archives are Linux only; on macOS, build
  from a git clone at a clean commit so `--version` records the source.
- Live run of the new selections on GLM-5.3-Flash EXL3 served by TensorFold
  across two DGX Sparks (client on the serving host): `sparkdash-decode-portable-v2`
  completed 576 of 576 requests, every lane reaching its 400-token cap with no
  reasoning tokens, including the new code prompt and the count prompt.
  `prefill-prose-portable-v1` and `sparkdash-prefill-portable-v1` each completed
  128 of 128 requests with every measured trial eligible and no cached prompt
  tokens; prose prefilled 8-12% slower than the `" the"` filler at the same
  nominal sizes, with about 19% fewer prompt tokens for the same characters.
  One model and engine; not a general claim about other backends.
- The new selections have not been run on Apple Silicon servers yet.

## Version 0.4.0 — cross-platform deployment comparison and portable workloads

- Add the [portable sparkDash copies](README.md#portable-sparkdash-copies):
  `sparkdash-decode-portable-v1` and `sparkdash-prefill-portable-v1` keep the
  sparkDash prompts, sizes, cells, trials and limits byte-identical with
  portable controls, so they run on MLX servers.
- Observe macOS memory in the resource domain: `macos_process {pid}` records
  the process's physical footprint (what `footprint -p` reports) as
  `memory_used` plus its lifetime peak, and `macos_host_memory` records free
  and total memory and the new `memory_pressure_level` metric. macOS
  observations use the `macos_uptime_raw` clock and adapter
  `macos-libproc-sysctl-resource-v1`, so they never compare with Linux ones;
  Linux sources, evidence and behaviour are unchanged. Resource capture and
  serving resource attachments work on macOS with these sources, and each
  platform refuses the other's. See [macOS sources](RESOURCES.md#macos-sources).
- Add the [`long-context-recall-v1`](README.md#long-context-recall) workload:
  six ordered recall steps at about 16K, 48K and 96K tokens, graded as exact
  JSON facts, run with `grill-perf run`.
- Add the [`long-context-decode-v1` and `prefill-ladder-96k-v1`](README.md#long-context-decode-and-prefill)
  selections: decode after about 32K/48K/96K-token prompts, and a prefill
  ladder ending at 96K, both with portable `cap-reached` controls.
- Add the [`concurrency-ladder-v1`](README.md#concurrency-ladder) selection:
  one ordinary coding question at C1/C2/C4/C8 with 400-token portable
  `cap-reached` replies.
- Add the [`realistic-decode-v1`](README.md#realistic-decode) selection:
  four ordinary coding questions and two whole-file edits at C1 with portable
  `cap-reached` output, reported per cell apart from the count prompts.
- Add `grill-perf outputs A B [--json]`, an offline output-identity check
  between two run directories or two captures (paired by acquisition) of the
  same greedy workload, for example to show that speculative decoding leaves
  the text unchanged. Each measured lane's answer and reasoning text is
  replayed from verified response evidence and reported `identical`, `differs`
  (first differing character and byte, with excerpts) or `unavailable`; exit 0
  only when every lane is identical. The report kind is
  `performance-output-identity-v1`. See the
  [contract](CONTRACT.md#output-identity).
- Record the build's source commit in `grill-perf --version`:
  `grill-perf <version> (source <commit>)`. Release staging sets it through
  `GRILL_PERF_SOURCE_COMMIT` and checks the output; inside a git checkout that
  value must equal a clean HEAD. Other builds record HEAD only when the compiled
  inputs, including untracked files, match it, and otherwise print
  `(source unrecorded)` with a build warning.
- Add output mode `cap-reached`: it sends only `max_tokens` under either flat
  profile, and a lane is eligible only when the server reports completion
  tokens equal to the cap with finish reason `length`. Add thinking control
  `chat-template-thinking-v1`, which sends `thinking` and `enable_thinking`
  with the same value and is allowed on `portable-chat-v1`; with thinking
  disabled, reported reasoning makes a lane ineligible. Add the `baseline-v2`
  and `portable-v1` workloads. Existing modes, controls and stored evidence
  keep their eligibility.
- Add whole-deployment comparison. New baselines write capture v3, which records
  the collector's version, source commit and build target, the required
  `--client-placement same-host|network` and the built-in workload chosen with
  `baseline --workload baseline-v1|baseline-v2|portable-v1`. The default is now
  `baseline-v2`, which sends both thinking keys, so Qwen-family templates also
  turn thinking off; `baseline-v1` stays selectable. `check --change deployment`
  accepts another endpoint, model,
  credential name and collector binary when the collector version and recorded
  source commit match, and reports `PENDING` (exit 0). `compare A B --reference A2`
  gives a verdict only when B differs in the same direction from the baseline and
  from its unchanged control A2 captured after B; the report kind is
  `performance-deployment-comparison-v1`. `--client-placement` is now required on
  `baseline`. v1 and v2 captures load, check and compare unchanged, and cannot be
  the baseline of a deployment comparison; re-capture. The completion-rate floor
  now also covers `cap-reached` phases. See the
  [contract](CONTRACT.md#capture-v3-and-deployment-comparison).
- Add Apple Silicon macOS source support for portable `grill-perf` serving
  captures. Binary identity uses the native executable path while Unix evidence
  no-follow opens, directory synchronization and advisory locking retain their
  existing fail-closed behavior. Linux `/proc`/cgroup/NVML resource capture and
  external-program microbench capture remain unavailable on macOS; no Apple GPU
  or unified-memory claim is inferred. The dependency graph and notice text are
  unchanged; the notice ledger records the re-reviewed manifest declaration.
- Include each selected cell's median prefill rate in the human-readable
  baseline/check report alongside its achieved and decode rates. JSON evidence
  and comparison semantics are unchanged.
- Human-readable selected reports, bundle inspection and microbench decisions
  print absent values as `null` (matching the JSON) instead of Rust
  `Some(...)`/`None`, lane eligibility errors as a `;`-separated list, and a
  declared change by its `--change` name. JSON evidence is unchanged.

Verification and limits of this version:

- Hosted CI covers the Linux x86_64 and aarch64 build, format, tests, clippy,
  release staging and installed-archive smoke, and the `grill-perf` build,
  tests and clippy on macOS 15. Release archives are Linux only; on macOS,
  build from a git clone at a clean commit so `--version` records the source.
- On an Apple M5 Ultra, `portable-v1` and `baseline-v2` ran with every lane
  eligible and thinking off on oMLX and TensorFold, `outputs` found identical
  text across the two engines, the A/B/A2 deployment flow completed between
  those engines on the same Mac, and the memory observer's peak matched
  `footprint -p` (contributor report in #86). No Mac-versus-NVIDIA comparison
  has been run yet, and the portable sparkDash copies have not been run on real
  servers.
- Hosted macOS timing tests are sensitive to runner load. A wall-clock bound in
  `streaming_decode_rate_uses_post_first_text_interval_and_measured_lanes`
  failed twice and was removed (#84). `evidence_is_not_published_while_a_peer_request_is_active`
  failed once (run 36803234260) and passed on rerun; it does not involve the
  changes above.

## Version 0.3.0 — experimental KV observation, runtime source contracts and shared tool declarations

- Add an opt-in, source-pinned LMCache KV journal and `startup verify-kv`.
  Direct copy boundaries, typed operation-ID callbacks, per-key finalization,
  actual frontend/worker request bindings and finite terminal fences replace
  aggregate-event inference for this new observation identity only. The
  bounded cache-producer file set must cover every selected rank/group, and
  each cache process must persist while the corresponding observed worker
  process changes. Full-attention native-object-group LMCache-driven L1 is the
  declared scope; sliding-window/skip, GDS/fallback, engine-driven/SHM,
  L2/disk/crash durability and native backend qualification remain outside
  this implementation. The stdlib-importable hook is packaged beside the
  runtime bridge; installation remains an explicit operator action.
- Add the explicit `startup verify-kv-v2` command and runtime752
  `--kv-v2-events` integration, preserving v1 producer/consumer bytes.
  It requires prospective typed registered layout, complete physical old/new
  worker cohorts and a persistent cache journal for each ordered server slot,
  joins paged raw block/key/exclusion evidence with actual copy legs, exact
  write finalization, independently queried existing source events and weak L1
  allocation generations, and reports `INCONCLUSIVE` rather than qualification
  when evidence is incomplete. Callback delivery is not device completion.
- Select LMCache `ddc5fa34e23fe09bb9b3a5b48f67a3e7a801e130` with the existing
  vLLM 752 source set. The `verify-kv-v2` command and file names remain, but
  the journal, expectation and report contract is v3; historical captures are
  not upgraded. The verifier observes actual worker results, supported
  topology, exact successful store finalization and source-owned cache
  dispatcher termination; missing completion, swallowed drain errors, dropped
  callbacks and shutdown timeouts fail closed. Defining lifecycle and
  decorator sources are pinned; replaced allocation readers or callback
  bindings are rejected.
- Add `runtime_vllm_752a3a504_bootstrap_v1` for the explicitly selected,
  seven-file vLLM 752a3a504/Uvicorn 0.51.0 source set. It observes frontend
  startup, listening, request admission and engine-client bootstrap boundaries;
  it does not establish all-worker readiness or isolated device timings. The
  v1 KV option stays restricted to its original 487 contract.
- Workload versions 5 and 6 can explicitly share one tool declaration across a
  history (`expect.shared: true`). Factual steps keep `tool_choice: "none"`;
  tool steps still select the declared function. This avoids introducing the
  schema only at the tool step when the backend retains declarations under
  `none`. Historical workloads, correctness gates and example bytes are
  unchanged. Prefix consistency is not proof of actual cache reuse.
- Point the current installation guide at published v0.2.0 and its exact
  source revision. Published archives, bundled documentation snapshots and
  historical collector/workload pins remain unchanged.

Verification and limits of this version:

- The KV observation modules (`startup verify-kv`, `verify-kv-v2`, the v3
  contract and the runtime source checks) are exercised by the repository's
  offline Python regressions and source smokes outside CI, not by in-repo
  cargo tests; hosted CI covers the workspace build, format, tests and clippy
  only. A journaled TP2 store/restart/reload run of this exact v3 source
  identity has not been performed; the committed bytes are bound to offline
  execution and strict source-pin checks only. This is a source-compatibility
  candidate, not native admission.
- A native GLM retest confirmed the backend retains shared tool declarations
  in the rendered prompt under `tool_choice: "none"` (168 prompt tokens with
  the declaration versus 17 without). The strict GLM workload nevertheless
  stopped on its first factual response: the model emitted a complete tool
  call ending at the observation boundary token and no answer text, and the
  collector's gates rejected that response fail-closed. No measured waves
  completed and no GLM campaign is claimed; until the backend honours
  `tool_choice: "none"` for answer steps, the GLM shared-tools profile cannot
  complete.
- Strict GLM workload correctness, journaled store/restart/reload
  qualification, LMCache observer admission and the remaining issue #55
  qualification obligations remain separate open gates; none is established by
  this release.

## Version 0.2.0 — experimental claim-coverage expansion

- Add explicit policy2 first-generated, first-answer and client-completion
  latency gates on homogeneous flat workloads, plus complete-wave worst-lane
  first-answer and max/min ratio gates. Correlated peers are observations,
  not extra independent trials.
- Share checked rational envelope arithmetic without changing policy1
  serialization, tolerance boundaries, reason precedence or exit meanings.
- Preserve all A/B/A2, warmup, output amount and cache eligibility requirements.
  Actual-hit latency never fabricates cold-prefill throughput.
  Workload6 acquisition populations additionally require every declared semantic
  check; timing-eligible but incorrect steps remain explicit failed members.
- Add finite workload4 solo/mixed schedules, fixed offsets and first-generated
  triggers, named controls, actual overlap evidence and settle-before-publication
  cancellation. Preserve never-dispatched and failed peers.
  Solo report rows do not claim missing controls where none are required.
  Final barrier-clock overruns remain deadline failures even when all lanes
  completed, so native capture and offline replay agree.
- Add workload5 streamed-tool fragments and first-delta/fully-validated-call
  timing with exact retained history and replay. Tool fragments are not text.
- Accept the supported named-tool `stop` finish in streamed fixed-tool profiles,
  without accepting truncation or changing historical nonstreaming receipts.
- Add explicit bounded metrics2 provider accounting and prospective required
  telemetry gates. Missing sources, gaps and resets are not zero; shared counters
  are not attributed to one request, and an isolation declaration is not proof.
  Source/position selectors cannot bypass contradictory engine accounting;
  unrelated engine identities remain independent.
  Cooperative overhead overruns retain actual timing as unavailable telemetry
  and skip later scrapes rather than aborting model acquisition; replay cannot
  relabel an over-budget snapshot complete.
- Add fourteen explicit GLM/DeepSeek profiles: routine decode/prefill, mixed
  interference, tools, history branches, long context and p95 stress. Controls,
  source-byte pins and warmup/measured traffic ceilings are documented. The eight
  version-6 profiles completed 2,820 CPU-fixture requests and offline replay;
  their performance decisions stayed INCONCLUSIVE. Fixture usage/cache counters
  are scripted, not tokenizer, cache or model qualification.
- Add complete workload6 acquisition populations, actual parent/tool history,
  bounded larger encoded inputs and retained history, whole-conversation gates
  and explicit p95/p99 completion populations. Missing members cannot disappear
  into successful-only means or tails. Existing random namespaces and frozen
  workload meanings remain unchanged.
- Add bounded native host and opt-in NVML resource observation, retained raw
  replay and prospective domain comparisons. Multi-source windows are bracketed
  by every source; unsupported CPU-integral boundaries stay unavailable.
  Sampled maxima are not peaks, energy integration is an estimate, and
  source ownership is not model attribution.
  NVML uses lazy initialization rather than `NO_ATTACH`, which can prevent
  selected UUID resolution; versioned raw traces preserve prior failure replay.
- Add finite capacity/retention runs with successful, failed and undispatched
  cells, actual workload history and explicit source journals. Two closed public
  vLLM source sets reject mixed/unknown files. Required retention needs declared
  and executed exclusive accounting; misses, preemptions, resets and unrelated
  invalidations are not allocation-driven eviction.
- Add common startup/readiness/first-inference and store/restart/reload evidence,
  controlled-process/cache fixtures, and source-pinned vLLM/Uvicorn lifecycle
  bridges. Operators still own every lifecycle action. Supported lifecycle
  observations do not establish persisted model-KV reload, hidden-warmup
  suppression or unavailable communication substages.
- Add native CPU, E3 kernel and NCCL collective evidence with exact duration/rate
  arithmetic, source-accurate binary64 E3 parity, explicit fallback tiers and
  complete per-rank repetitions. Linux external capture retains bounded raw
  stdout/stderr and cleans its owned process group before reaping the leader;
  missing or corrupt raw evidence cannot support a favorable native comparison.
  Forced collector death and descendants escaping the group remain outside
  graceful-cleanup guarantees.
  Collective warmup failures from every rank survive gathering, even when all
  measured reductions are correct; a CPU-only producer regression covers this.
  Use PyTorch's `object_gather_list` argument when gathering completed rank
  observations; the CPU transport fixture preserves the real API signature.
- Expand the explicit archive allowlist with domain examples, offline guides and
  four opt-in Python producer modules. Installed verification shares the staging
  payload contract and checks executable permissions as well as bytes.
  Optional backend runtimes are not bundled or installed automatically.
- Preserve all published v0.1.0 artifacts, notices and frozen examples. The
  dependency lock entries and feature declarations are unchanged; the notice
  ledger was re-reviewed for the two workspace package-version changes.

Publication status and exact source/artifact identities are recorded by the
versioned GitHub Release. Exact-head packaging/installation, independent review
and real-adapter/live qualification are separate gates. Source or fixture
success does not close them, and mandatory maintainer adoption is not implied.

## Version 0.1.0 — published prerelease

[v0.1.0 is published](https://github.com/plotarmordev/thegrill/releases/tag/v0.1.0)
from explicitly approved source `4ff02a780680a8475e83db25d187173a21d51f33`.
The tag, archive receipts and native installed-smoke summaries bind that source.
The version matches the root and `grill-perf` packages; the quality CLI remains
unpackaged.

### Published artifact verification

[Publication run 34765220280](https://github.com/plotarmordev/thegrill/actions/runs/34765220280)
passed exact-source authorization, both native build/smoke paths, source/output
scope and secret gates, and upload/tag identity checks before publishing.

| Native target | Published archive SHA-256 |
|---|---|
| `aarch64-unknown-linux-gnu` | `f0174d3b3f1687eaf365f5519d1c44eb6750856bcdadb714674dd0dca5a454cf` |
| `x86_64-unknown-linux-gnu` | `6f036b78f87fa01b47d263be2848ffd202e75e5a8bcce01d7644e1735cb40572` |

All eight published assets were downloaded and checked against GitHub's asset
digests, checksum sidecars and receipts. Both archives and executables matched
the preapproval staging bytes; both native smoke summaries report success
without Rust or a source checkout. The published ARM64 binary was also unpacked
and run outside a checkout: all four GLM/DeepSeek candidate decisions reproduced
the retained gates and role identities with zero network syscalls. Its evaluator
hash is distinct from the live collector hash; this is offline replay of the
[bounded live studies](SHARED-RECIPES.md#completed-glm-live-scope), not another
live acquisition or a claim of model-quality validation.

The public HTTPS archive download was separately exercised. Released archives
remain unchanged, including their pre-publication documentation snapshot; the
current [installation guide](INSTALL.md) supplies the published download URL.

### User-visible scope

- Stage a standalone `grill-perf` binary with the existing workloads, selection
  manifests, recipe bundle, installation guide and license notices.
- Use the existing CLI outside a source checkout. `--version`, `--help`, offline
  selection/bundle verification and baseline/control/offline comparison use the
  same implementation as a source build.
- Keep stored evidence and workload identities unchanged. The release number is
  not a workload/schema version or a measurement-contract revision.
- The quality CLI remains work in progress and is not packaged.
- Invalid capture loads no longer substitute the built-in C1/exact400 scope for
  an unverified selected capture. Scope and human acquisition counts are explicitly
  unavailable; timing-window errors name the acquisition and both duration values.
  Accounting is not promoted, and timing checks, report schemas and exit codes
  remain unchanged. Offline replay does not rewrite historical reports.
- Add offline `preflight` using the same local admission as `run`, including
  optional deployment, policy, metrics and credential inputs. It returns workload
  pins and bounded budgets without network traffic or capture creation. Deployment
  validation now names the offending field and observed UTF-8 byte count; the
  4,096-byte, nonempty and control-free limits remain unchanged.
- Add flat workload v3 with an optional typed warmup output budget, separate from
  measured output and conversation v2. Phase controls bind request/evidence
  identity, weighted ceilings, usage checks and failure reporting; incompatible
  phase controls cannot be compared as matched workloads. Old examples, recipe
  hashes and absent-override behavior remain unchanged. A 32/400 declaration
  does not establish sparkDash protocol or live-backend equivalence.

### Target and compatibility boundary

Published targets are `x86_64-unknown-linux-gnu` and
`aarch64-unknown-linux-gnu`, built natively on Ubuntu 24.04 with glibc 2.39
and Rust 1.98.0. The supported installed-runtime baseline is native Ubuntu
24.04 with glibc 2.39; older libc, musl, other operating systems and emulation
are not qualified by this release mechanism. Other Linux distributions need
separate validation or a source build.
A readable system CA trust store (Ubuntu `ca-certificates`) is required for client
initialization, including loopback HTTP. The clean-runtime smoke records the
read-only native CA bundle digest; no CA contents are packaged and TLS checks
are never disabled.

Existing evidence compatibility checks remain authoritative. A newer binary
must not reinterpret or rewrite old receipts in place. Retain the exact binary,
workload bytes and source pin needed to inspect historical evidence. Selected
concurrency, conversation and historical recipe-bundle workflows remain
separate contracts; packaging does not make them mutually comparable.

### Executed staged qualification

Preparation inspected package versions, asset paths, CI definitions, pinned
upstream license/action metadata and the approved Gitleaks release checksums.
The release workflow now gates the committed source and extracted public
payload/sidecars for scope and secrets before artifact upload. The complete
redistribution ledger and required notice payload are present; staging verifies
their exact input set and hashes rather than carrying an unresolved notice
placeholder.

Both targets completed native build, existing workspace checks, clean-runtime
installed CLI smoke, and source/output scope and secret gates at source
`ef4f9e29bb081f997e795b3dded80c6965f729da` in
[staging run 34677697749](https://github.com/plotarmordev/thegrill/actions/runs/34677697749).
The independent push and pull-request CI runs at that source also passed.
The complete local workspace gate passed 206 tests (two opt-in ignores); the
15 CPU publication-boundary tests passed without making real publication writes.

| Native target | Staged archive SHA-256 |
|---|---|
| `aarch64-unknown-linux-gnu` | `fd4beba564a940dd18a5a4005514cf47937925fdc96503f6ac265193f90b93d1` |
| `x86_64-unknown-linux-gnu` | `94078e602a3528e94be0aa406f10d02d62846bb9554c03206acc3674d1953d90` |

Each public build receipt and smoke summary binds that exact source, version,
target, archive, executable and runtime/CA-store identity. Both fixtures used the
same installed binary without Rust or a source checkout: default C1 plus an
explicitly selected/authenticated control mapping; baseline, unchanged control,
candidate and network-disabled offline replay; representative input, pin,
authentication, backend-rejection and budget failures. Raw receipts stay private.
The C1 control was inconclusive on ARM and measured slower on x86; both results
are retained, not rerun until favorable or misrepresented as a server-change
effect. Selected comparisons completed with `DESCRIPTIVE`, not a performance PASS.

These hashes identify the named staged artifacts, not every later build of
version 0.1.0. A later source revision requires new exact-source staging and
receipts before approval; these historical qualification records are not rewritten.

#### Qualification after preflight and phase-output integration

Source `dd771ac36963290cf8b1079b4c2a38be40cec949` completed both native
installed-runtime workflows and source/output gates in
[staging run 34740266773](https://github.com/plotarmordev/thegrill/actions/runs/34740266773).
The local workspace gate passed 221 Rust tests (two existing opt-in ignores)
and 15 publication-boundary tests, plus build, formatting and Clippy. The merged
source tree is identical; [post-merge CI passed](https://github.com/plotarmordev/thegrill/actions/runs/34740590731).

| Native target | Staged archive SHA-256 |
|---|---|
| `aarch64-unknown-linux-gnu` | `a9d73cfbe7e3b49e29c63112c0b5d98c450fca01e8695a972a1ace056d93cfe5` |
| `x86_64-unknown-linux-gnu` | `b5a724bcd851a8fc78389a04b1b1642f63e2825cc696f690f4abc90d56d06078` |

Both summaries report the same two neutral scenarios: C1 baseline ready,
unchanged control `INCONCLUSIVE`, candidate `IMPROVED`; selected baseline ready,
control and candidate `DESCRIPTIVE`. These are controlled CPU fixtures, not
live performance claims. Both runtimes had neither Rust nor a source checkout.
Checks covered inherited inputs, network-disabled replay, corrupt downloads,
asset hashes, input/pin/authentication/backend failures and budget exhaustion.
The downloaded archives and every payload digest were independently checked
against their checksum sidecars and build receipts.

These are historical exact-source staging records, not the published release
assets and not replacements for the earlier hashes. Published assets are pinned
separately above; historical qualification records remain unchanged.

### Remaining limits

Historical Actions artifacts require access to their retained workflow run.
Use the approved release assets linked above for published downloads; do not
substitute an arbitrary archive with the same version number.

Installation and loopback fixtures are not live-backend performance qualification,
model/template qualification, GPU measurements or causal evidence. No serving,
model or GPU calls are part of release preparation. No bit-for-bit reproducible
build claim, signature or independent execution attestation is made. Checksums
bind downloaded bytes; obtain the expected digest from the reviewed release,
not an untrusted mirror alone.

See [release procedure](../RELEASES.md) and [installation](INSTALL.md).
