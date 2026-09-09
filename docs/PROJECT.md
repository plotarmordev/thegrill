# Protocol and measurement design

## Product boundary

The Grill is a standalone measurement tool with its own protocol, local run orchestration, grading, inspection, and reporting.

No integration API or public-safe export format is implemented. Existing evidence directories preserve exact input and provider data and are not safe-to-publish exports. Local use requires neither a project account nor mandatory results upload.

## Measurement requirements

- Keep conclusions within the evaluated task set and the declared comparison.
- State what the evaluated tasks cover and what they do not.
- Do not generalize direct-answer results to tool-using or agent systems.
- Treat related variants, repeated attempts, and shared execution conditions as potential dependence, not free independent samples.
- Declare budgets, denominators, utility, missingness handling, inference scope, and practical margins before interpreting outcomes.
- Preserve complete required artifacts and attempt/termination evidence. A valid completed artifact can survive a length stop; an unknown interrupted outcome is not silently a failure or a dropped row.
- Qualify graders against both valid alternatives and wrong artifacts. Human judgment and deterministic verification are different evidence types.
- Distinguish submitted, regraded, independently replicated, and controlled results. A client receipt cannot authenticate execution on an untrusted host.

## Implementation language

The Grill uses **Rust** for its runner, CLI, grading, and result handling. Its evaluation methodology and task contracts should build on established research without requiring a particular external framework. Performance claims require measurements, not assumptions about the implementation language.

## Current validation status

The offline contracts and direct-answer runner below are implemented. Local checks exercise task admission, collection, evidence handling, grading, and comparison. The included synthetic examples exercise the format and workflow; they do not establish broad model capability.

- Local CLI and loopback checks do not qualify every real model endpoint or deployment.
- Task and grader validity require evidence beyond self-checks and passing software tests.
- Performance claims require measurements under stated conditions; receipts alone do not establish model identity, server capacity, or independent replication.

## Scope

The quality binary, `grill`, provides serial direct-answer collection, offline study manifests, deterministic synthetic examples, and paired family/unit reports. This guide describes those current capabilities, not a calibrated aggregate intelligence score. The separate serving-performance companion is documented in [its own guide](performance/README.md).

## Implemented offline contract v1

### Commands and evidence scope

`grill check PACK [--json]` admits and self-qualifies a pack and reports computed identities. `grill grade PACK SUBMISSION --out NEW_VIEW` grades explicitly submitted answer-artifact strings. `grill compare LEFT RIGHT [--json]` independently re-admits both snapshots, recomputes every digest and outcome, and rejects incompatible views; each side may be a submitted view, a run directory (its initial view) or a run-derived grade view. There is no alias command, uploaded result, or credential lookup outside `plan`/`run`. Exit status is 0 for success, 1 for admission/storage/compatibility/interruption errors and 2 for CLI usage errors. Graded wrong/malformed/refused answers and unknown outcomes are successful grading operations, not process failures. Errors are escaped text on stderr.

The synthetic files in `examples/` exercise finite alternatives, significant whitespace, Unicode escape equivalence, duplicate answer keys, and empty versus missing artifacts. They are contract fixtures, not independently validated benchmark tasks.

### Closed JSON schema

Every record is a JSON **object**, not a positional array. Unknown fields, duplicate fields (including escaped-equivalent keys and null-first duplicates), trailing values/prose, invalid UTF-8, invalid required types, integer overflow and unknown enum/version values are rejected. Lists preserve order. Unless marked nullable below, fields are required; nullable fields may be omitted or null, both encoding as null for semantic identities. An outer artifact is a string containing exact answer-artifact text, not a parsed/pregraded JSON value. JSON escape decoding happens once when reading that outer string; its decoded UTF-8 bytes go directly to the strict artifact grader.

Record notation below lists fields in **identity serialization order**. `string[]` means an array of strings; all versions are the integer `1`. There are no extension maps, asset paths, executable templates or fetched content. Any URLs in text or declared endpoints remain inert data.

| Record | Fields |
|---|---|
| Pack | `version`, `label`: string, `worlds`: string[], `groups`: string[], `cases`: Case[] |
| Case | `id`: string, `world`: string, `group`: string, `messages`: Message[], `accepted`: string[], `qualification`: Qualification |
| Message | `role`: `"system"`, `"user"` or `"assistant"`; `content`: string |
| Qualification | `valid`: artifact-string[], `wrong`: artifact-string[] |
| Submission | `version`, `system`: System, `protocol`: Protocol, `answers`: AnswerEntry[] |
| System | `name`: string, `model`: string, `endpoint`: nullable string |
| AnswerEntry | `case_id`: string, `artifact`: nullable artifact-string |
| Protocol | `profile`: `"declared-chat-completions-v1"` or `"declared-chat-completions-v2"`, `stream`: boolean, `token_cap`: TokenCap, `temperature_milli`: nullable u16, `top_p_milli`: nullable u16, `seed`: nullable i64, `reasoning_effort`/`include_usage`: profile-v2 controls (below), `collection`: Collection, `rendering`: Rendering |
| TokenCap | `field`: `"max_tokens"` or `"max_completion_tokens"`; `value`: u32 |
| Collection | `total_ms`: u32, `idle_ms`: u32, `response_bytes`: u32, `artifact_bytes`: u32 |
| Rendering | `status`: `"known"` or `"unknown"`, `template`: nullable string, `tokenizer`: nullable string |

IDs and labels are nonempty, at most 256 UTF-8 bytes. World/group declaration lists and case IDs are individually unique; cases reference declared worlds/groups, and unused declarations are rejected. Multiple cases may share a world/group, expressing dependence rather than asserting independent samples. Messages allow empty text. Accepted decoded strings allow empty text, are unique, and have no trimming/casefold/normalization. Every accepted alternative must have a valid qualification artifact; all `valid` artifacts must succeed and all `wrong` artifacts must fail. This is a self-check, not independent review.

Qualification checks only the supplied examples against the acceptance relation. The `wrong` list may contain malformed artifacts only; admission does not require a decodable-but-wrong negative or prove broader grader/task soundness. Independent reviewers must assess negative coverage and task validity before a benchmark pilot.

System name is a label; model and endpoint declarations are nonempty when present and at most 4096 bytes each. A system/endpoint declaration is never an attestation or a connectivity check. Do not put credentials or secret-bearing URLs into it.

Protocol sampling fields use integer thousandths to avoid floating-point encoding ambiguity: temperature 0..2000; top-p 1..1000; seed signed 64-bit. Null/omitted means unspecified, not a provider default inferred by Grill. Token cap is 1..1048576. Total collection time is 1..86400000 milliseconds; idle is 1..total. Response cap is 1..8 MiB and artifact cap is 1..1 MiB. These are declared protection settings, not scored delivery deadlines or measured collection evidence; offline grade enforces its engineering artifact ceiling, not a fictional collection history.

Profile v2 request controls sit between `seed` and `collection` in serialization order: `reasoning_effort` is one of `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, and `include_usage` is a boolean. They are optional strictly by absence: a present field must carry its real value, and unlike the nullable v1 fields an explicit `null` or wrong-type spelling is rejected, matching the frozen v1 reader's refusal of these fields entirely. Under `declared-chat-completions-v1` both must be absent; under `declared-chat-completions-v2` `include_usage` is a required explicit declaration and `include_usage: true` additionally requires `stream: true`. An absent control is omitted from the typed identity encoding rather than encoded as null, so every v1 protocol identity stays byte-stable; a v2 record always differs through its profile string. These are requested fields only: provider support, effective backend settings and usage accuracy are not verified by admission or by a committed response, and no negotiation, fallback or retry reshapes a rejected request.

The profile binds text-only, one choice, no tools, serial pack order, one attempt, no retries and omission of all other generation settings. These constants participate in protocol identity. The pack fixes equal weights and one attempt. A `"known"` rendering declaration requires nonempty template and tokenizer identifiers of at most 4096 bytes each; `"unknown"` requires both null/omitted. They are declarations only, not templates to execute. Matching unknown declarations never establish matched effective prompts. The report explicitly restricts that comparison to declared submissions/services; known matching declarations are also not independently authenticated.

### Exact grading and missingness

The relation `json-answer-set-v1` consumes exactly one object `{"answer":"..."}` with precisely one string field, matching exact decoded membership in `accepted`. JSON whitespace and escape equivalence are allowed. Duplicate keys (including null-first), extra fields, nonstrings, arrays, trailing values/prose, code fences and invalid JSON/UTF-8 are malformed. No trimming, casefold, final-line extraction or prefix rescue occurs.

A supplied artifact is `evidence: "submitted"` and grades `success`, `wrong` or `malformed`; the latter two are zero utility. An absent answer entry or missing/null artifact is `evidence: "missing"`, outcome `unknown`. An empty artifact string is delivered but malformed; an artifact string containing `{"answer":""}` can succeed if the accepted set includes the empty string. Unknown case IDs and duplicate answer entries reject the entire submission. Every admitted case remains in the fixed denominator.

### Identity encoding and frozen vectors

All digests are lowercase 64-character SHA-256 hex. Define `H(domain, bytes) = SHA256(UTF8("the-grill") || 0x00 || UTF8(domain) || 0x00 || UTF8("v1") || 0x00 || bytes)`. This is not a general JSON canonicalizer. `J(value)` is compact UTF-8 JSON from serde_json 1.x over the typed projection: tuples/lists become arrays, records emit the field order above, nulls are explicit, integers are decimal, strings use JSON control/quote/backslash escaping and emit other Unicode as UTF-8 without normalization, no insignificant whitespace or trailing newline. Incoming field order/whitespace/escape spelling does not affect typed projections. Raw source digests intentionally differ.

The exact definitions are:

```text
R = "json-answer-set-v1"
G = "grill-json-answer-set-impl-v1"
input(c) = H("case-input", J([R, c.id, c.messages, c.accepted]))
source = H("pack-source", exact admitted pack bytes)
tasks = H("tasks", J([input(c) for c in pack order]))
target = H("target", J([1, "equal-weight", "one-attempt",
                       pack.worlds, pack.groups,
                       [[c.id, c.world, c.group] for c in pack order]]))
qualification = H("qualification",
                  J([[c.id, c.qualification] for c in pack order]))
grading = H("grading", J([R, G, qualification]))
submission = H("submission-source", exact admitted submission bytes)
protocol = H("protocol", J([1, "serial-pack-order", "one-attempt-no-retry",
                           "text-only-one-choice-no-tools", "other-settings-omitted",
                           "collection-protection-not-utility", submission.protocol]))
system = H("system", J(submission.system))
artifact = H("artifact", exact decoded submitted artifact UTF-8 bytes)
```

`source`, `tasks`, `target`, `qualification`, `grading` are the ordered fields of PackIdentities. Identities has `pack`: PackIdentities, `submission`, `protocol`, `system`, in that order. Labels and source formatting do not mutate task gold. Case order/acceptance changes do; system/model/endpoint are deliberately outside the comparison gate. Qualification changes alter grading identity, not task gold.

`G` is an explicit implementation revision, not a source hash or proof of execution; it must change for any grader behavior change. The locked dependency graph and code review remain required implementation provenance. Formatting/unrelated package edits do not implicitly revise gold or the grader. `examples/identity-vectors.json` freezes independent expected vectors for the exact original pack/submission files, including protocol/system and empty/Unicode typed-encoding probes. Identity regressions compare these expected values, not hashes generated by the same implementation at test runtime.

### Grading view and publication

A successful new view directory contains fixed names `pack.json` (exact input bytes), `submission.json` (exact input bytes), and `view.json` (authoritative receipt). No original absolute input paths or manifest-selected paths are stored. Snapshots can contain private user-supplied text/system/endpoint data; this is not an automatic public-safe export.

`View = {version, claim, identities, cases}` where `claim` is `"submitted-artifacts-not-verified-execution"` and `cases` is an ordered CaseGrade array. `CaseGrade = {case_id, input, artifact, evidence, outcome}`: `input` is the per-case input digest; `artifact` is a digest or null; `evidence` and `outcome` have the values above. Receipt IDs are not trusted: comparison requalifies the pack, validates the submission, recomputes all identities, artifacts and outcomes, and requires exact typed equality. A missing/malformed required file, hash mismatch or forged grade fails verification. A fully rewritten internally consistent submission/view remains submitted evidence, not forgery-proof execution.

Storage currently supports Linux local filesystems with ordinary file and parent-directory sync. Inputs, required snapshot entries and the final view directory must be nonsymlink regular files/directories as appropriate. The parent of `--out` must already exist and must itself be a nonsymlink directory. Required file opens use libc's architecture-specific `O_NOFOLLOW | O_NONBLOCK` constants to reject final-component symlinks and avoid hanging on FIFO substitution; opened files are then checked for regular-file type. Ancestor traversal is not a sandbox against concurrent parent replacement or a hostile filesystem.

Publication exclusively creates a fresh directory (0700), synchronizes its parent, exclusively creates and syncs snapshots (0600), then syncs their directory before creating and syncing `.view.json.pending`. A no-clobber hard link publishes `view.json`; the staging link is removed and the view directory and parent are synced. Existing destinations are always refused. Failures leave the owned partial directory for inspection and never recursively delete it or unrelated data; final-sync failure may leave a visible receipt despite a nonzero exit. Absent `view.json` means no authoritative grading view. Extra unreferenced files are never read or promoted. There is no overwrite/resume/repair or power-loss/hostile-race/cross-platform durability guarantee. Move/copy all three required files together for relocation.

### Engineering ceilings and output

Pack: 16 MiB; submission: 32 MiB; receipt: 1 MiB; required view data total: 49 MiB with checked addition. A temporary hard link does not duplicate receipt payload bytes. Readers inspect file size and then read at most cap+1 bytes to detect growth; no unbounded body read. Pack admission allows 1..1024 cases; world/group lists at most 1024; 1..64 messages per case with total decoded content at most 256 KiB; 1..64 accepted alternatives; 1..128 valid and 1..128 wrong qualification artifacts. Decoded accepted strings and qualification/submitted artifacts each have a 1 MiB ceiling. JSON recursion limits remain enabled. These are engineering protection limits, not measured optimal memory/performance claims; parsing retains bounded snapshots plus typed records and allocator overhead is not a byte-exact bound.

Successful `check` admission does not guarantee a publishable receipt: pathological but valid IDs can expand during JSON escaping enough to exceed the separate 1 MiB receipt cap. `grade` explicitly refuses such views before creating the destination; it never drops cases or relaxes the cap.

Human output escapes all untrusted text to ASCII, including paths, errors, labels, ANSI/OSC and bidi controls. Error previews, including clap usage diagnostics, stop during escaping at a 1024-byte output budget including a truncation marker; the fixed `error: ` prefix and newline make stderr at most 1032 bytes per error. Underlying parser error construction is still bounded by input caps, not by this display budget. Machine JSON stdout escapes DEL and non-ASCII as UTF-16 JSON escapes and preserves JSON data semantics. Case/output counts and field lengths are bounded; no untrusted content is logged elsewhere. Errors use escaped stderr, not an error JSON schema.

### Comparison mathematics

Compatibility requires identical task, target, protocol, grading and qualification identities and ordered case inputs. Source whitespace/pack label and declared model/endpoint differences are permitted. Changed messages, accepted alternatives, grouping/selection/order, protocol settings/omissions or grader/qualification cannot silently pair. A view with a different grader revision is rejected by current recomputation, not auto-migrated.

For each side, with success count S, unknown count M and fixed N, utility bounds are `[S/N, (S+M)/N]`. For B−A, bounds are `[(S_B-S_A-M_A)/N, (S_B+M_B-S_A)/N]`. Reports store signed integer `lower`, `upper` numerators and a positive integer `denominator`; no floating-point rounding affects correctness.

Comparison JSON contains `version`, `claim`, `left_evidence`, `right_evidence`, `effective_rendering`, `left_system`, `right_system`, `n`, `left`, `right`, `paired`, `delta_b_minus_a`, `complete_case_delta_diagnostic`. Each side contains `success`, `failure`, `unknown`, `delivered`, `malformed`, `refused`, `bounds`; `refused` counts completed refusals, which are failures. `paired` contains `both_success`, `both_fail`, `gains` (known failure to success), `losses` (success to known failure), `either_unknown`. The optional diagnostic contains `(gains-losses)` as `numerator` and jointly known count as `denominator`; it is null if there are no jointly known cases. It is not the primary fixed-N estimate. The evidence labels name each side's claim (`submitted-artifacts-not-verified-execution` or `client-collected-artifacts-not-authenticated-execution`); mixed pairs are still descriptive system contrasts under one declared protocol.

Identical fully known saved outcomes give exact `[0/N, 0/N]`. Identical views with M unknowns still give `[-M/N, M/N]`; no unjustified coupled-unknown exception cancels missingness. Different stochastic submissions from the same declared system can have bounds excluding zero. There are no confidence intervals, p-values, population generalizations, repeats, unequal weights, scalar IQ or adaptive case filtering.

## Typed pack contract v2

Version 1 is an explicit archival reader with the exact `accepted` schema, grading behavior and identity formulas above. Version 2 keeps the pack's top-level fields and case `id`, `world`, `group`, `messages` and `qualification`, but replaces `accepted` with exactly one closed `acceptance` object:

```json
{"kind":"string-set","accepted":["exact decoded string"]}
```

or:

```json
{"kind":"grid-set","outputs":[[[0,1],[2,3]],[[9]]]}
```

`string-set` retains exact decoded membership, no normalization, 1..64 unique alternatives and qualification coverage of every alternative. `grid-set` is one ordered collection of **all** expected output grids, not a choice of alternate answers. The artifact is exactly `{"answer":[grid1,grid2,...]}`; every grid must match in test-input order. There are 1..8 grids, each rectangular with 1..30 rows and 1..30 columns of integer cells 0..9. Array bounds are enforced while parsing, before allocating an excess element; ragged/empty/oversized grids, floats, booleans, out-of-range cells, duplicate fields, extra fields, positional records and trailing data are malformed. Well-typed, in-bounds but incorrect cells, dimensions or grid counts are wrong. Every supplied valid qualification must succeed and every wrong qualification must fail; grid qualification is still a self-check, not task validity or independent review.

The version reader does not convert input through an untyped JSON map. V1 `accepted` and v2 `acceptance` cannot be mixed. The protocol, submission and offline grade-view envelopes remain v1 because their semantics are unchanged; they carry the new pack identities and continue to retain exact snapshots. Runner terminal receipts are independently versioned below. Only `messages` enter requests, never expected grids or qualifications.

V2 uses the same `H` byte framing and `J` typed encoding defined above, with explicitly versioned projections:

```text
(R, G) = ("json-string-set-v2", "grill-json-string-set-impl-v2")
      or ("json-grid-set-v2",   "grill-json-grid-set-impl-v2")
input(c) = H("case-input-v2",
             J([2, [R(c), G(c)], c.id, c.messages, c.acceptance]))
source = H("pack-source", exact admitted pack bytes)
tasks = H("tasks", J([input(c) for c in pack order]))
target = H("target", J([2, "equal-weight", "one-attempt",
                       pack.worlds, pack.groups,
                       [[c.id, c.world, c.group] for c in pack order]]))
qualification = H("qualification-v2",
                  J([[c.id, c.qualification] for c in pack order]))
grading = H("grading-v2",
            J([2, [[c.id, [R(c), G(c)]] for c in pack order], qualification]))
```

Acceptance serialization orders fields as `kind`, then `accepted` or `outputs`. Each case therefore binds its own relation and implementation, including mixed string/grid packs, and all expected grids participate in task identity. Grouping/selection and qualification remain separately bound. V1 frozen fixtures and identity vectors are unchanged.

## Final-marker pack contract v3

Version 3 packs use the version-2 case shape and admit two additional acceptance kinds, `final-string-set` (with `accepted`) and `final-grid-set` (with `outputs`). Version 2 is frozen and refuses them. The graded artifact is the full delivered final-content text; grading applies a prospectively declared boundary instead of parsing the whole artifact:

- Lines are separated by LF; one CR immediately before an LF is line termination, not content (CRLF). A bare CR does not end a line. The final line may be unterminated.
- Exactly one line's content must be exactly `===FINAL===` — no indentation, decoration or trailing spaces. Zero or multiple marker lines are malformed, wherever they render, including inside a multi-line JSON string: the scan is deliberately structure-blind, so an ambiguous boundary is a protocol failure, never a repair candidate.
- Everything before the marker line is an unrestricted, ungraded preamble that stays in the preserved artifact. Everything after the marker line's terminator must parse with the same strict typed parser as the underlying kind (`{"answer": string}` or `{"answer": grids}`) with only JSON whitespace after the object. Duplicate keys, extra fields, wrong types, fences and trailing prose remain malformed; an unterminated marker line leaves an empty suffix and is malformed.
- The suffix is graded in place as a borrowed slice of the retained artifact bytes; no duplicate answer buffer, last-object search, fence removal or shape normalization exists, and raw response bytes plus the full artifact remain archived unchanged.

Semantic equality is unchanged: the suffix answer grades `success`/`wrong` exactly like `string-set`/`grid-set`. Commitment and refusal are decided before grading, so a complete committed artifact grades normally even at a `length` stop, while refusals and uncommitted streams are never rescued by a well-formed suffix. Protocol compliance and semantic correctness stay separately visible through the existing outcome axis: `success` and `wrong` are protocol-compliant delivered answers, `malformed` is a protocol failure, and `refused`/`unknown` are not assessed for protocol compliance at all — collection/termination evidence and the fixed denominator report those separately.

The boundary is prompt-declared: case `messages` must themselves instruct the system to emit the marker, and Grill sends messages verbatim without ever appending boundary instructions. Identities bind the envelope: the marker kinds carry the relations `final-marker-json-string-set-v1`/`grill-final-marker-json-string-set-impl-v1` and `final-marker-json-grid-set-v1`/`grill-final-marker-json-grid-set-impl-v1` through the same `case-input-v2`/`grading-v2` projections, and `target` carries pack version 3, so marked and unmarked packs never silently pair. Qualification artifacts are full marked texts and self-check through the same envelope. V1/v2 packs, grades and identity vectors are byte-for-byte unchanged.

## Source-backed pack contract v4

Version 4 adds source provenance and four explicit grading relations. Versions 1–3 keep their archived readers, grades and identities; they reject the new kinds and the new `provenance` field. The transport, submission and receipt formats are unchanged. **Only case messages enter model requests**, never provenance, keys or qualification examples.

Every v4 case uses the v2 case fields plus a required, closed `provenance` object:

| Field | Meaning |
|---|---|
| `repository` | HTTPS source URL without credentials, query or fragment |
| `revision` | 40 lowercase hexadecimal characters identifying the source revision |
| `path` | Source asset path |
| `sha256` | 64 lowercase hexadecimal characters identifying the raw source asset |
| `item_id` | Original source item ID |
| `license` | Applicable rights declaration, including unresolved conflicts |
| `changes` | 1–64 nonblank descriptions of adaptations |

These are bounded declarations, not authenticated attribution or permission. The runner does not fetch repositories or follow source paths. Source preparation and rights verification are the pack author's responsibility.

### Adapted acceptance relations

```json
{"kind":"final-text-set","accepted":["example answer"]}
{"kind":"final-number","number":{"expected":12,"absolute_tolerance":0.000001,"relative_tolerance":0.000000001}}
{"kind":"json-exact","expected":{"count":3,"active":true}}
{"kind":"text-constraints","rules":[{"kind":"words","min":8,"max":12},{"kind":"ends-with","text":"done"}]}
```

The first two kinds use the existing exactly-one `===FINAL===` line, followed by a closed JSON answer object. The last two grade the entire raw reply without a marker or answer envelope.

- **`final-text-set`:** `answer` must be a string. Compare whitespace-separated word sequences using Unicode scalar lowercasing. Whitespace runs and letter case are insignificant; punctuation, word order and extra text are significant. This is neither full Unicode casefold nor Atlas's permissive extraction. `straße` and `STRASSE` remain different.
- **`final-number`:** `answer` must be a finite JSON number, parsed as binary64. Success requires `abs(actual - expected) <= max(absolute_tolerance, relative_tolerance * abs(expected))`. Expected value, tolerances and effective bound must be finite; tolerances must be nonnegative. Zero tolerance is preserved. Strings, booleans, fractions and prose-number searches are not accepted. This is an explicitly approximate numeric relation, not arbitrary-precision arithmetic.
- **`json-exact`:** parse one complete JSON value. Duplicate object keys at any depth, fences, surrounding prose and trailing content are malformed. Objects compare by their complete key sets, independent of key order; arrays are exact and ordered; strings are case-sensitive; booleans are distinct from numbers. Numbers compare by exact decimal value without binary64 conversion: `1`, `1.0` and `1e0` are equal, signed zeros are equal, and distinct large integers remain distinct. Explicit and normalized decimal exponents must fit signed 64-bit integers; overflow is rejected rather than approximated. Parsing is bounded to 1 MiB and at most 64 child-depth levels. Raw numeric tokens are retained for serialization and identities, so semantically equal spellings can still have different input identities.
- **`text-constraints`:** require all 1–16 closed rule objects. Valid UTF-8 that violates a rule is `wrong`; this does not establish factual or semantic correctness. `words` counts `split_whitespace` tokens; `characters` counts Unicode scalar values, including whitespace. Both require inclusive unsigned `min`/`max` with `min <= max`. `starts-with`, `ends-with`, `contains` and `excludes` use a literal, case-sensitive `text` field on the untrimmed reply. `uppercase` requires an uppercase Unicode character and no lowercase character; `lowercase` requires a lowercase character and no uppercase character. These two rules have no payload fields. There is no regex engine or executable dataset DSL.

Literal rule text must be nonempty. Finite-number implementation v2 uses `serde_json`'s round-trip binary64 parser; the initial best-effort parsing implementation is superseded.

All new relations use the existing positive/negative qualification mechanism. A passed self-check proves only those supplied fixture outcomes; it does not certify question correctness or coverage. Collection continues to distinguish refusals, uncommitted answers and delivered malformed artifacts. Unknown cases stay in the declared denominator.

The relation/implementation pairs are:

| Kind | Relation | Implementation |
|---|---|---|
| `final-text-set` | `final-marker-unicode-lowercase-word-set-v1` | `grill-final-marker-unicode-lowercase-word-set-impl-v1` |
| `final-number` | `final-marker-finite-number-v1` | `grill-final-marker-finite-number-impl-v2` |
| `json-exact` | `raw-json-exact-decimal-v1` | `grill-raw-json-exact-decimal-impl-v1` |
| `text-constraints` | `raw-text-constraints-v1` | `grill-raw-text-constraints-impl-v1` |

Using the existing `H` and typed `J` framing:

```text
input(c) = H("case-input-v4",
             J([4, [R(c), G(c)], c.id, c.messages, c.acceptance, c.provenance]))
qualification = H("qualification-v4",
                  J([[c.id, c.qualification] for c in pack order]))
grading = H("grading-v4",
            J([4, [[c.id, [R(c), G(c)]] for c in pack order], qualification]))
```

Source and task aggregation use the existing domains; target binds pack version 4 and the existing selection/grouping projection. Provenance edits change v4 task identity; labels and qualification changes retain their existing separate roles.

### Source-backed evaluation data

This baseline provides the source-backed pack contract and built-in graders, not a bundled external benchmark. Pack authors must retain source revisions, verify source bytes and applicable rights, disclose prompt/scorer adaptations, and independently review answer keys. Qualification fixtures alone do not establish task correctness.

Use bound study-family summaries rather than treating a mixed total as a general model ranking. Public-source exposure, related task templates, uncalibrated difficulty labels and missing execution lanes remain limitations of any particular study. Original notices and attribution must accompany shared data; the collector retains per-case provenance but does not fetch external license files.

## Implemented runner v1

### Commands and boundary

`grill run PACK --endpoint URL --model MODEL --out NEW_RUN --token-cap N [options]` admits the pack, freezes the plan, then collects exactly one attempt per case in pack order over one reusable HTTP/1 client per collection session on a current-thread Tokio runtime, and finally writes `grades/initial` by regrading what reached disk. `grill pause RUN` requests cooperative admission stop; `grill resume RUN` continues only never-started cases from validated settled lifecycle evidence. `grill inspect RUN_OR_VIEW [--json]` is read-only and works on interrupted or crashed runs. `grill regrade RUN --out NEW_VIEW` publishes a fresh self-contained grade view from saved evidence without credentials or networking. Neither `check`, `grade`, `pause`, `inspect`, `regrade` nor `compare` opens a socket; a completed `resume` also returns before credential lookup or client construction.

Options are CLI-selected protocol data, never pack-controlled: `--stream`, `--token-cap-field max_completion_tokens|max_tokens` (default `max_completion_tokens`), `--profile declared-chat-completions-v1|declared-chat-completions-v2` (default v1), `--reasoning-effort minimal|low|medium|high|xhigh|max` (profile v2, omitted when absent), `--include-usage` (profile v2, streaming only; absent under v2 declares an explicit false), `--temperature-milli`, `--top-p-milli`, `--seed` (omitted from the request when absent), `--total-ms` (30000), `--idle-ms` (10000), `--response-bytes` (8 MiB), `--artifact-bytes` (1 MiB), `--rendering-template` with `--rendering-tokenizer` (declarations only), `--system-name` (defaults to the model selector), `--auth-env VAR` and `--local-http`. They validate under the same rules as a submission protocol and produce the same `Protocol` record and protocol identity, so a run pairs with a submitted view under identical settings and differs from one under changed settings.

`grill plan PACK --endpoint URL --model MODEL --out NEW_RUN --token-cap N [same options] [--json]` shares offline preparation with `run`; `run` consumes that preparation once before collecting. Preparation admits the pack, protocol, endpoint, credential variable and fresh destination, checks request/receipt/plan ceilings, and computes identities and sizes without creating a runtime, client, DNS lookup, connection or output. The destination parent must already exist; a successful plan does not reserve it or guarantee future publication. Credential contents are validated locally but never reported or retained.

Planning JSON version 1 contains `claim: "offline-preparation-not-dispatched"`, the would-be v1 `plan`, `protocol_identity`, actual `pack_bytes` and `plan_bytes`, total and per-case `prompt_content_bytes`, `messages_json_bytes` and `request_bytes`, and `requested_output_tokens`. Prompt-content bytes count decoded UTF-8 content only; message bytes count the exact compact JSON message array; request bytes count the complete JSON request bodies, excluding HTTP headers. Per-case sizes are in pack order. Requests are counted without retaining all bodies; collection still materializes only one request at a time. The token envelope is case count times the requested per-case cap, not measured output, billing or an equal-compute guarantee. `provider_cap_enforcement` is always `"unknown"`; requested profile controls appear only inside the embedded plan protocol and are never presented as enforced or effective settings. `plan.bound_bytes` is a conservative file-payload bound, not an actual run size or reserved capacity; its plan/receipt/reservation components use their ceilings, not the actual sizes reported alongside it. The would-be plan includes invocation time/process provenance, so later invocations need not have the same plan-source digest.

### Endpoint, client and request profile

The endpoint is one absolute URL of the Chat Completions resource. Userinfo, query strings and fragments are rejected. `https://` uses rustls with the AWS-LC provider and the platform verifier (system trust store on Linux); certificate and hostname validation are never disabled. `http://` is accepted only with `--local-http` and a literal loopback IPv4/IPv6 host; `localhost` and other names are rejected. The client sets no redirects, `retry::never`, no proxies, HTTP/1 only, `https_only` outside local mode, no compression, no referer and one idle pooled connection. Each request is `POST` with `content-type: application/json`, `accept` of `application/json` or `text/event-stream`, `accept-encoding: identity`, `user-agent: grill/<version>` and, only when `--auth-env` names a variable, `authorization: Bearer <value>` resolved while that request is built. The variable must exist at preflight and hold nonempty visible ASCII of at most 8192 bytes; its value is never written, hashed, or printed, and raw transport errors (which can embed URLs) are classified into fixed phrases instead of being displayed.

The body is exactly `{"model", "messages", "stream", <token cap field>, ["temperature"], ["top_p"], ["seed"], ["reasoning_effort"], ["stream_options"]}` in that order with the case's messages verbatim and thousandths converted to decimal numbers. Bracketed fields appear only when declared. Under profile v1 neither control can appear; under profile v2 `reasoning_effort` is the declared enum string and `stream_options` is exactly `{"include_usage":true}` when usage was requested — a declared-false `include_usage` is not a request and stays off the wire, leaving the v1 byte shape. No `n`, tools, functions, response format, logprobs, modalities or extra fields are sent, and no parameter is stripped for a resend. Requests are limited to 256 KiB; a pack whose escaped request would exceed that is refused before any directory or connection exists, as are invalid options and a missing credential variable.

### Response semantics and commitment

Only a 200 status with exactly one `content-type` header whose media type matches the mode and with every `content-encoding` member equal to `identity` is parsed; repeated media types are ambiguous, other statuses (including redirects) and out-of-profile entities are captured as bounded evidence and never re-sent. The entity or event must be strict UTF-8 as a whole before parsing, one JSON object with a `choices` array of objects; unknown fields anywhere are skipped through the parser's own recursive path (so its 128-level nesting guard applies) and never copied; positional arrays where records are required are malformed. A non-null top-level `error` makes the attempt `provider_error`, uncommitted. Exactly one choice with index 0 (or no index) is allowed; zero-choice chunks are metadata. A nonempty `id`, when present, must stay identical across all chunks of one response; empty preamble ids carry no identity. `role`, when present, must be `assistant`; `content`/`refusal` must be null, absent or strings; `tool_calls`, `function_call` and `audio` that are non-null and non-empty are unsupported; a streamed choice must carry `delta` and no `message`, a complete entity `message` and no `delta`. Recognized `finish_reason` values are `stop`, `length` and `content_filter`; `tool_calls`/`function_call` and unknown values are unsupported. A nonempty `refusal` string or a `content_filter` finish is a completed refusal. Reasoning fields stay in the raw body only.

Nonstreaming commitment requires the whole entity, one closed envelope, one message and a recognized finish; a declared `content-length` above the response cap is refused before any body is read because such an entity can never commit. Streaming commitment requires a recognized finish for the single choice and then a decoded `[DONE]` event terminated by a blank line; usage and metadata chunks may follow the finish, additional choice content may not, and EOF, deadline, interruption or a malformed event before `[DONE]` leaves the attempt uncommitted. Only bytes within the response cap are parsed: a `[DONE]` that lands inside the cap commits regardless of how long the whole entity is declared to be or what was co-read after it, including invalid UTF-8; the raw offset, the surplus retained on disk (`surplus_retained`) and the surplus observed in the same read (`surplus_observed`) are recorded, reading stops there and the connection is not drained. A stream whose retained bytes reach the cap without a qualifying `[DONE]` is `response_cap`. SSE framing accepts CR, LF and CRLF line endings split across reads, one leading UTF-8 BOM, comments, multiline `data`, colonless and unknown fields, requires strict UTF-8 per line, dispatches only on a blank line and never at EOF, and caps lines and event data at 256 KiB. The total deadline runs from dispatch; the idle deadline restarts only on nonempty body reads (comments included), never on response headers; the total limit wins ties, the first interrupt wins over both, and monotonic time is rechecked after every bounded parse and immediately before commitment so a result or fault that completes after expiry settles as the deadline. Timeout wrappers are cooperative, not hard real-time enforcement. Content-filter or refusal outcomes need commitment too; a `length` stop with valid content can succeed; content absent from every delta or message is unsupported evidence, not an empty answer.

### Run directory and receipts

```text
RUN/
  pack.json                       exact admitted snapshot
  plan.json                       Plan: identities, system, protocol, transport, provenance, byte bound
  attempts/000000/
    reservation.json              Reservation: exact request body, digest, headers, auth variable name
    body.partial -> body.bin      exact received entity bytes (possibly empty, partial or capped)
    final.txt                     only for a committed non-refused attempt
    terminal.json                 Terminal: sole authoritative receipt
  grades/initial/view.json        RunView written after the loop by the offline regrade path
```

`Plan = {version, claim, pack: PackIdentities, cases, system, protocol, transport, provenance, bound_bytes}` with `transport = {profile, http, tls, auth_env, redirects, retries, proxy, content_encoding, commitment, request_cap, sse_line_cap, sse_event_cap}` and `provenance = {grill, lockfile, os, arch, started_unix_ms, pid}`; `lockfile` is a digest of the Cargo.lock compiled into the binary and is declared build provenance, not execution authentication. `Reservation = {version, attempt, case_id, input, plan, endpoint, method, headers, auth_env, request, body, started_unix_ms}` where `request = H("request", body)`. `Terminal = {version, attempt, case_id, input, reservation, request, dispatched, reason, detail, commitment, stop, http, body, final, usage, timing}`: `reservation = H("reservation-source", exact reservation bytes)`; `reason` is one of `committed`, `refused`, `unsupported_response`, `malformed_envelope`, `provider_error`, `http_status`, `http_entity`, `transport_failure`, `stream_incomplete`, `total_deadline`, `idle_deadline`, `response_cap`, `artifact_cap`, `frame_cap`, `interrupted`, `local_io`; `commitment` is `committed` only for `committed`/`refused`; `stop` is `stop`, `length`, `content_filter` or null; `http = {status, version, content_type, content_length}` or null; `body = {bytes, sha256, complete, truncated, terminal_offset, surplus_retained, surplus_observed}` covers precisely the retained bytes with plain SHA-256; `final = {bytes, sha256}` or null; `usage` retains prompt/completion/total counts when present; `timing = {started_unix_ms, reserve_ms, headers_ms, first_body_ms, settle_ms, local_ms}` separates preparation, header and first-body observation, settlement and local publication. `detail` is a fixed phrase of at most 256 bytes.

New terminal receipts use **version 2**; plans, reservations and grade views remain version 1. A complete, otherwise supported response with a recognized finish but no final content remains `unsupported_response` with `commitment: uncommitted`, no artifact and an `unknown` grade. Version 2 additionally retains its `stop` (`stop` or `length`) and last `usage` snapshot instead of discarding those facts. Such metadata is permitted only on a complete nonstreaming entity or a complete stream with its DONE boundary. Earlier parsing, transport and deadline failures do not acquire completion facts by inference. The version-1 reader preserves its original restrictions; old receipts are never upgraded or rewritten in place.

Older readers that accept only terminal version 1 cannot inspect or regrade new version-2 runs. Use the updated reader for those runs; the updated reader continues to support existing version-1 evidence.

Ordering: the pack and plan are written and synced before `attempts/` exists; each attempt exclusively creates its directory, syncs `attempts/`, writes and syncs `reservation.json`, syncs the attempt directory, and only then builds and sends the request, so a reservation means *may have been sent*. Received bytes are buffered through a 64 KiB writer and hashed incrementally; at settlement the partial body is flushed, synced and linked to `body.bin` without clobbering, `final.txt` is written and synced when a qualified artifact exists, the directory is synced, and only then is `.terminal.json.pending` written, synced, linked to `terminal.json`, and the attempt and `attempts/` directories synced. Every error is checked. A failure while writing captured bytes settles the attempt as `local_io` regardless of any earlier status or media-type verdict and stops admission of further cases; a failure during publication aborts the run; in both cases the partial directory is left for inspection and the process exits 1. Rename/link controls visibility; power-loss durability requires separate platform qualification.

Interruption: a SIGINT handler sets a latch before the first dispatch, and Tokio's signal receiver wakes pending network awaits. The latch survives synchronous publication and is checked before reservation, before dispatch, and before commitment. An interrupt during publication preserves an already committed attempt and starts no further case; an interrupt before commitment leaves the active attempt unknown. Partial bytes and an explicit terminal are retained when publication succeeds, the session records `interrupted`, and the command exits 1. There is no second-interrupt hard kill, abort of kernel I/O, or server-side cancellation. Forced termination can leave a reservation with no terminal; inspection reports it as unresolved, regrade grades it unknown, and resume refuses the uncertain session rather than guessing whether the request was sent.

### Cooperative pause and continuation sessions

Use `grill pause RUN`, then `grill inspect RUN --json`. `pause_requested` means an active admitted attempt is still draining, not that it has stopped. The request keeps its original deadline and response semantics; pause never injects a transport interrupt. The admission gate serializes pause publication with the next reservation, so no further attempt is admitted after acknowledgement. The final attempt may complete the entire run before a pause takes effect; inspection then reports `completed`, not `paused`. `Ctrl-C` remains immediate client interruption, not a synonym for pause.

Execution state is independent of attempt exhaustion. A final-attempt interruption remains `interrupted`, and a final-attempt local capture failure remains `blocked`, even with no unstarted suffix. Resume refuses these exhausted unsuccessful executions without creating a session or sending requests. Only normal exhaustion, including a benign final cooperative-pause race, becomes `completed`.

`grill resume RUN` accepts no changed endpoint, model, pack, protocol, or credential-variable selector. It re-admits the saved pack and plan, verifies request/response/terminal/artifact lineage and every historical session snapshot, and requires a contiguous terminal prefix followed only by never-started cases. Completed, failed, and interrupted terminal attempts are consumed, never retried. Reserved-but-unsettled attempts, unfinished sessions, changed evidence, foreign attempt names, and unsupported lifecycle versions fail closed. An unfinished session is refused even if its last attempt appears settled: no operator-accounting or repair switch is provided for native evidence. Original v1 runs without lifecycle sidecars remain inspectable and regradeable, but are not implicitly adopted for continuation.

Linux `flock` ownership is held on the original run directory descriptor for the whole collector/resumer lifetime. Kernel lock release on process exit avoids PID-reuse ownership guesses and stale lock-file deletion. A separate permanent `lifecycle/admission.lock` serializes short admission/control operations; there is no daemon, polling loop, retry scheduler, or signal sent by `pause`. Advisory locking coordinates these collectors, not a hostile writer replacing directory ancestors or bypassing the protocol.

New runs add `lifecycle/NNNNNN/start.json`, optional `pause.request` (the session-start digest), `view.json`, and `end.json`. Session starts bind version 1, the original plan digest, previous session-end digest, first attempt, time, PID, binary version and compiled lockfile digest. End receipts bind the start digest, next attempt, state, and exact session-view digest. Each session view preserves the full fixed denominator and is independently checked against the corresponding immutable evidence prefix. Files are no-clobber publications with directory synchronization; credentials never enter these records. The fixed ceiling is 1025 sessions, including the first; the advisory byte bound includes all session metadata and view ceilings. No timing continuity or warmed-client equivalence is claimed across sessions.

`grades/initial/view.json` is written once and never replaced. After continuation, inspection labels a validated older initial snapshot `historical`, not corrupt and not current. `compare RUN ...` refuses a historical initial view; publish a new self-contained view with `grill regrade RUN --out NEW_VIEW` and compare that explicit snapshot. A changed historical receipt is a mismatch and blocks resume; it is never repaired or regenerated in place.

Damaged lifecycle history remains visible independently of initial-view classification: inspection reports `lifecycle.state: "blocked"` with the history error in `lifecycle.detail`, even when the stale initial view is reported as `mismatch` rather than `historical`. Continuation stays refused; no historical receipt is regenerated to repair that diagnostic.

### Run views, regrade and inspection

`RunView = {version, claim, identities, cases}` with `claim = "client-collected-artifacts-not-authenticated-execution"`, `identities = {pack: PackIdentities, plan: H("plan-source", plan bytes), protocol, system}` using the offline protocol/system encodings, and ordered `cases` of `{case_id, input, attempt, terminal, reason, artifact, evidence, outcome}` where `terminal = H("terminal-source", terminal bytes)`, `artifact = H("artifact", final bytes)` and `evidence` is `collected` (graded `success`/`wrong`/`malformed`), `refused` (outcome `refused`, zero utility, no artifact), `uncommitted` (any other terminal, `unknown`), `unresolved` (reservation without terminal, `unknown`) or `not_started` (no reservation, `unknown`). Every admitted case stays in the denominator.

Loading a run recomputes the pack identities, requires the plan to match them, re-derives the bound endpoint and the fixed transport declaration from the plan's own system/protocol, and requires `attempts/` and every present `attempts/NNNNNN`, `grades/` and `grades/initial` to be nonsymlink directories. Each attempt is validated against the frozen plan and case rather than trusted from its receipts: the reservation's endpoint, headers, credential variable name and exact request body must equal the projection this binary would build, and its request digest must match; the terminal's lineage must match that reservation, and the receipt must be physically and semantically consistent (dispatch and HTTP facts versus reason, an HTTP/1.0 or HTTP/1.1 version, the mode's media type on every parsed 200 (a `local_io` receipt keeps whatever status and media type were actually observed before capture failed), a declared content length equal to a completely retained entity or bounding a partial or DONE-terminated one, commitment only with a complete body, `committed` only with a `stop`/`length` finish and a final artifact, stream offsets and surplus accounting on committed, refused or unsupported-at-settlement streams only, truncation only at the cap, no artifacts on uncommitted attempts); `body.bin` size and hash are checked when raw bodies are expected and `final.txt` size and hash before grading. An attempt directory with no reservation is not started only when it is empty; any other surviving evidence without its reservation is invalid. Any tampering, missing file, symlink, FIFO or oversize entry marks that attempt invalid: inspection reports it read-only, while regrade, initial-view writing and comparison refuse to produce a view. Orphan bodies, extra files and `body.partial` are never read or promoted. A regrade view directory contains exact copies of `pack.json`, `plan.json`, every `reservation.json`, `terminal.json` and `final.txt` (rechecked against the loaded digests during the copy), plus `view.json`; raw bodies are referenced by hash only, so a regrade view is verified without them. Regrading identical evidence with the same grader revision produces a byte-identical `view.json`; a grader revision change produces a different grading identity and stops silent pairing with older views. This is internal consistency of client receipts, not authentication of remote execution.

Run inspection JSON **version 5** retains system/protocol declarations, attempt state/reason/detail, timing, coverage and grade-view status, and adds `lifecycle` with `state`, session count, next-attempt boundary and optional blocking detail. Lifecycle states distinguish `running`, `pause_requested`, `paused`, `interrupted`, `blocked`, `completed`, `uncertain`, and archival evidence without lifecycle support. Each attempt retains its `result` explanation and `metadata_source`; observed `stop` and `usage` come from `terminal`, `verified_raw_body`, or remain `unavailable`. Version-1 absent-content responses can recover metadata only by verified complete raw-body replay through the existing parser; no answer or grade is promoted. Grading relations, protocol identities, terminal formats, and fixed example bytes are unchanged by lifecycle control.

For raw-body-free version-1 regrades, facts discarded by the old writer are unavailable. These views remain grade-verifiable, but cannot reproduce richer diagnostics available from the original raw run. Version-2 regrades retain the new terminal metadata without needing raw bodies. Inspecting or regrading old evidence preserves the original terminal bytes; grade-view bytes are unchanged under the same grader.

`usage` exposes nullable `prompt_tokens`, `completion_tokens` and `total_tokens`. Missing usage remains null; absent fields remain unknown; an explicitly reported zero remains present. `usage_status` is `unvalidated-provider-snapshot` when a snapshot exists and `unknown` otherwise. `usage_coverage` counts available snapshots and separate `{present, unknown}` pairs for each field over **all pack cases**, including invalid, unresolved and not-started cases. Earlier streaming snapshots are never merged into the last one. No token totals or bills are inferred, and grade-view verification does not authenticate provider accounting.

`result_counts` contains one diagnostic category per case, independent of the unchanged `outcome`/`coverage` grading axis:

| Result | Meaning |
|---|---|
| `correct`, `wrong_answer`, `refused` | The canonical grade; a valid answer can remain correct or wrong even at a reported length stop. |
| `malformed_answer` | An answer artifact violated its declared format, without a reported length stop. |
| `output_limit` | A reported `length` stop without a gradeable final answer: a malformed artifact or a complete absent-content response. This reports an observed limit, not a claim that the limit caused an otherwise wrong answer or that hidden thinking alone consumed it. |
| `timeout` | Client total or idle deadline; the original `reason` distinguishes them. |
| `service_error` | Provider error envelope or non-200 HTTP response. |
| `transport_error` | Transport failure. |
| `client_limit` | Client response, artifact or framing limit—not the provider's output limit. |
| `interrupted`, `local_error`, `invalid_evidence` | Explicit interruption, local I/O error or failed evidence validation. |
| `other_ungraded` | Other unsupported/incomplete responses, unresolved attempts or unstarted cases; inspect `state` and `reason`. |

These explanations do not change grades, denominators, task/protocol identities or grader revisions. In particular, `output_limit` can accompany canonical `malformed` or `unknown`; neither is silently regraded. Human output names the categories and metadata source and continues to escape untrusted text.

Human-readable output is for interactive inspection, not a stable parsing interface. Use the versioned `--json` output for scripts.

### Limits

Ceilings: request 256 KiB; raw entity at most the declared response cap (8 MiB ceiling); final artifact at most the declared artifact cap (1 MiB ceiling); SSE line and event 256 KiB; plan 64 KiB; reservation 2 MiB; terminal 64 KiB; receipt 1 MiB (checked at preflight against a worst-case view so no case is dropped later). `bound_bytes` is a checked worst-case sum for the run, advisory rather than reserved capacity; no free-space probe or evidence auto-deletion exists. Memory holds the admitted pack, one request, one response buffer (nonstreaming) or one framing/artifact buffer (streaming) at a time; library, TLS and kernel buffers are outside that accounting.

The test suite exercises endpoint admission, request shaping, streaming boundaries, response limits, deadlines, interruption, filesystem safety, evidence tampering, and offline replay. These checks establish behavior under the tested conditions, not conformance of every model provider or validity of a capability score.

## Study manifests and pilot design

### Commands

```sh
grill pilot --seed 42 --units 8 > pilot.json
grill check pilot.json --json
grill study check study.json pilot.json --json
grill study compare study.json pilot.json left-view right-view --json
```

`pilot` writes a self-qualified pack as JSON to stdout, including answer keys and qualification artifacts. `study check` reads only the manifest and pack, validates their relationship, and reports the study, pack, protocol and system identities without reading outcomes. `study compare` verifies both saved views through the same evidence loader used by `compare`, then checks the manifest's bindings. Submitted views, run directories with an initial grade view, and relocated regrade views are supported. Neither study command makes a request, fetches a source, reads credentials, or writes files; shell redirection is explicit.

`examples/synthetic-study.json` is a complete manifest for the existing synthetic pack and submissions A/B. It intentionally declares exposed fixtures and a pilot purpose. To author a different study:

1. Freeze the pack bytes. Copy `identities.source` from `grill check PACK --json` into `pack_source`.
2. Declare the exact ordered `left_system` and `right_system`, and their shared `protocol`. These use the existing System and Protocol schemas, including profile-v2 controls when applicable. For collected runs, use the planned `system` and `protocol` from `grill plan ... --json` (`plan.system` and `plan.protocol`), before running the same options. System labels and endpoints are part of identity; swapping sides requires a different manifest.
3. Write the intended construct, sampling design, exclusions and stopping rule. State which effects the comparison can and cannot identify. The tool records these declarations; it does not enforce prose instructions during collection.
4. Describe every pack group in `families`, in pack order, including source, revision and rights. `world` is the declared problem unit and cannot span multiple groups in one study. Presentation variants of a problem share a world; their separate case rows must not be counted as independent samples.
5. List every case in `exposure`, in pack order, with its exposure status separately for the left and right systems. Use `unknown` when freshness cannot be established. A different seed is not proof of freshness, disjoint tasks or absence of contamination.
6. Run `study check` and retain the exact manifest and returned identity before collecting outcomes. Retain evidence of timing/exposure separately if a claim requires it. A content hash alone cannot prove preregistration.
7. Run each declared collection once with the frozen pack and settings, or grade submitted artifacts with their truthful system declarations. Analyze the saved views with `study compare`. Retain the manifest, pack and views together for offline replay; the JSON report alone is not a self-contained evidence archive.

### Closed manifest v1

The manifest uses the existing strict JSON reader: object records only; no unknown or duplicate fields, positional records, trailing values or alternative enum spellings. The top-level file is at most 1 MiB. `label` is at most 256 UTF-8 bytes; each descriptive text field is at most 4096 bytes. Required prose must contain non-whitespace text; only `exclusions` may be empty. All manifest-level fields below are required. Nested System/Protocol nullable fields follow their existing omission/null rules.

| Record | Fields |
|---|---|
| Manifest | `version: 1`, `label`, `purpose`, `construct`, `sampling`, `exclusions`, `stopping_rule`, `pack_source`, `protocol: Protocol`, `left_system: System`, `right_system: System`, `families: Family[]`, `exposure: Exposure[]` |
| Family | `group`, `construct`, `source`, `revision`, `rights`: strings |
| Exposure | `case_id`: string; `left`, `right`: `"fresh"`, `"exposed"` or `"unknown"` |

`purpose` is `"pilot"`, `"screening"` or `"confirmation"`. Confirmation admission requires every case to be declared fresh for both systems. This is an internal-consistency gate, **not independent verification of exposure, holdout secrecy, timing, sampling or scientific validity**. Pilot and screening studies allow exposed and unknown cases. A confirmation report remains descriptive and cannot issue a capability certificate.

Family and exposure arrays must completely cover the declared pack groups and ordered cases. Duplicate, omitted, additional or reordered entries fail. Source and rights strings are inert declarations, not downloaded assets or grants of rights. A family source revision should identify the actual generator revision/parameters or dataset revision and its separately retained provenance record.

The study identity is `H("study-source", exact manifest bytes)` using the existing domain-separated `H`. It is separate from all pack identities. Changing a study declaration changes the study identity without changing any task, qualification or grading identity. The study binds **exact pack source bytes**, including labels and JSON formatting: a label-only or whitespace-only edit requires a new source binding even where ordinary `compare` would accept semantically identical packs.

Study comparison requires each view's source, task, target, qualification, grading and protocol identities to match the admitted study, its system identity to match the declared side, and every ordered case ID/input digest to match. It does not trust precomputed result rows. Existing `compare` compatibility and arithmetic remain unchanged; study binding is an additional gate.

### Reports and interpretation

JSON study report **version 2** contains `claim`, the admitted `study` (manifest and computed identities), the verified `comparison`, and `families`. Ordinary comparison output is also **version 2** and adds `left_results`/`right_results` diagnostic counts alongside unchanged grading arithmetic. Each family contains `group`, a paired `summary`, and `units`. Every summary includes the same left/right result counts. Each unit contains `world`, its summary, and case rows with canonical `left`/`right` outcomes plus `left_result`/`right_result` explanations. Families follow pack group order; units follow pack world order within each group; cases retain pack order within each unit. Output order never depends on hash-map traversal.

Every summary uses the same accumulator as whole-pack comparison: `n`, left/right coverage, paired both-success/both-fail/gains/losses/either-unknown counts, fixed-denominator `delta_b_minus_a` bounds, and nullable complete-case delta explicitly labeled diagnostic. Wrong, malformed and provider-signaled refused outcomes are known failures; unknowns remain in the declared denominator at every level. `wrong` counts can be obtained as `failure - malformed - refused`; case rows preserve the exact outcome. Gain/loss counts exclude any pair with an unknown side.

Result explanations are observational and can differ between a legacy raw run and a raw-body-free regrade even when their grade views are byte-identical. They never enter compatibility identities, success bounds or gains/losses. Use `inspect` for metadata provenance and the detailed cause of an ungraded case.

For a subset of size `N`, side bounds are `[S/N, (S+M)/N]`; paired change bounds are `[(S_B-S_A-M_A)/N, (S_B+M_B-S_A)/N]`. These describe uncertainty from missing evidence on that fixed set, not sampling uncertainty. They are computed from counts, not averages of family percentages. Case weighting is unchanged by grouping. Unit/family summaries expose dependence and offsetting flips; they do not establish statistical independence or support a confidence interval by themselves.

No p-values, confidence intervals, effective sample sizes, inferred refusal classifications, cap-enforcement claims or weights-only verdicts are generated. Provider usage remains an unvalidated observation under the existing inspection contract. A study manifest does not authenticate weights or equal effective compute. Human output escapes all untrusted labels; JSON follows the existing ASCII-safe encoding.

### Pilot recipe v1

`--seed` is a required unsigned 64-bit integer; `--units` is required and must be 1..=256. The recipe emits `4 * units` cases under pack v3, two groups and `2 * units` worlds:

- **Ledger:** three initial balances and nine credits, debits and transfers, with negative balances allowed. The target is one final account balance. The second variant renames accounts and reverses display order while retaining numbered-operation semantics.
- **Reachability:** eight nodes with a reachable directed cycle and an isolated node; remaining edges vary. The target is the count of distinct other nodes reachable from the start, excluding the start even when a cycle returns to it. The second variant renames nodes and reverses edge/node listing order.

Each `(seed, family, unit)` selects a parameter block with SHA-256 over the ASCII prefix `the-grill/pilot/v1`, a zero byte, the seed as eight big-endian bytes, family UTF-8 bytes, a zero byte and the zero-based unit index as eight big-endian bytes. Selection is deterministic, not a random-population model. World IDs include recipe, seed, family and index; case IDs add the presentation variant. Different IDs/seeds do not guarantee distinct semantic problems. Generator families can remain dependent even across different worlds.

Both variants accept a canonical decimal answer string or the deliberately permitted equivalent `value=<decimal>`, as stated in the prompt. The relation is `final-string-set`: exactly one standalone `===FINAL===` line followed by one strict answer object. Generated qualifications exercise both accepted forms, reasoning before a valid answer, incorrect finals mentioning the correct number in reasoning, conflicting finals, prose refusal, absent boundary, duplicate keys, numeric instead of string values, and trailing prose.

Generation serializes the pack and runs it through the real pack reader and grader before emitting it. That self-check does not establish that the prompt describes the intended construct or that the oracle is correct; independent solver checks are separate. Recipe v1 has narrow, synthetic coverage and no calibrated difficulty, population validity or contamination guarantee. It is suitable for exercising the study workflow and collecting pilot diagnostics, not assigning model tiers. Any future change to parameter selection, prompt semantics or expected answers needs an explicit new recipe identity rather than silently changing v1.
