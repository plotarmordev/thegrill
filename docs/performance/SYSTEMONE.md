# System One decision captures

`grill-perf systemone` is a separate version-1 domain evidence path for servers
that answer declared questions over a state in one forward pass and report zero
generated tokens. It does not change serving plan/workload meanings, grade answer
quality, or claim capacity. It measures **per-request latency** and **per-burst
request/decision rates** only; a response that reports generated tokens is a
retained failure, never a throughput measurement.

This subcommand is a **local unreleased extension**. The package version is
reported as usual, but every receipt and inspection also records
`support: local-unreleased-extension-not-released-v0.6-support` with the collector
version, source revision and executable digest. A released v0.6.0 artifact does
not contain `systemone`.

## Commands

All three commands print JSON on standard output and never launch a server.

```text
grill-perf systemone capture --plan PLAN.json --slot N --endpoint URL \
    --deployment DECLARATION.json --out NEW_CAPTURE [--auth-env NAME] [--local-http]
grill-perf systemone inspect CAPTURE
grill-perf systemone compare --plan PLAN.json --a CAPTURE [CAPTURE...] \
    --b CAPTURE [CAPTURE...] --a2 CAPTURE [CAPTURE...]
```

`--endpoint` is the full request URL; its path must end with `/v1/systemone`,
which is the declared protocol route. HTTPS is required unless `--local-http`
makes a literal loopback `http://` address explicit. `--auth-env` names an
environment variable holding a bearer credential; the value is never retained.
`--out` must be a fresh directory: existing evidence is never overwritten.

Capture exit 0 means every measured burst is complete; exit 2 retains incomplete
or failed evidence; exit 1 means invalid input or invalid evidence. `inspect`
re-verifies the retained bytes offline and reports the same inspection object
capture printed. Comparison reuses the policy contract: `PASS`=0, `ERROR`=1,
`INCONCLUSIVE`=2, `REGRESSION`=3, aggregating as
`ERROR > REGRESSION > INCONCLUSIVE > PASS`.

## Plan

The plan is a closed JSON object (`version: 1`, `kind: "systemone-plan-v1"`,
`protocol: "systemone-v1"`). Every field is required except where noted.

| Field | Meaning and bound |
|---|---|
| `name` | Short ASCII identifier for the plan |
| `requests` | 1–256 declared request bodies |
| `cells` | 1–64 bursts |
| `limits` | Timeouts and the response byte ceiling |
| `slots` | Exactly one measured slot per arm `a`, `b`, `a2`; at most one warmup slot per arm; at most 8 in total |
| `gates` | 1–16 prospective comparison gates |

A `request` is `{id, state?, questions, images?}`. `state` is any JSON value or
`null`; a request needs a non-null state or at least one image. `questions` is an
object keyed by question ID with `type` (`noul`, `choice`, `score`),
`instructions`, and `criteria` where the type requires it: `choice` needs a
non-empty object whose values are strings or `null`; `score` needs 2–10 ordered
strings; `noul` accepts two true/false descriptions or omits `criteria`.
`images` holds up to 16 `data:` URLs of at most 4 MiB each. The rendered corpus is
bounded to 256 MiB and any single encoded request to 2 MiB.

A `cell` is `{id, requests, concurrency, warmup_bursts, measured_bursts}`: the
named distinct requests dispatched together, how many may be in flight (1–64, no
more than the burst size), and how many warmup (0–20) and measured (1–1000)
bursts to run. Warmup bursts are retained and excluded from gate samples. The
plan is bounded to 1024 bursts and 10000 requests in total.

A rendered request body always carries `state` (the declared value or `null`),
`questions`, and `images` only when images are declared, in sorted-key order.
Question and criteria keys are emitted in sorted ASCII order too, so the declared
bytes are deterministic; a plan file's textual key order is not preserved.

`limits` is `{request_timeout_ms, burst_timeout_ms, response_bytes}`:
`request_timeout_ms` is 1–600000, `burst_timeout_ms` is at least
`request_timeout_ms` and at most 3600000, and `response_bytes` is 1 KiB–8 MiB.
An undispatched request at the burst deadline is retained as `total_timeout`
with `dispatched: false`; an in-flight request settles within its own deadline
and keeps whatever bytes it had read.

A `gate` is `{cell, metric, max_adverse_bps, max_reference_spread_bps}` with
`metric` one of `latency_us`, `requests_per_second`, `decisions_per_second`, and
bounds inside the observed-envelope contract (9999 and 1000000 bps).

Keep plan files, states, images and endpoints outside the repository: they are
private inputs. A plan is not a benchmark prompt set shipped with the tool.

## Response contract

A capture accepts HTTP 200 with exactly one `application/json` content type. The
body must carry `answers` keyed by the declared question IDs and `usage` with the
integer `output_tokens` equal to zero plus an optional integer `input_tokens`.
Each answer must satisfy its declared type: a `noul` answer carries `noul`; a
`choice` answer carries `choice` (one of the declared criteria) and
`probabilities` over exactly the declared criteria keys; a `score` answer carries
`score` inside the declared level range and `probabilities` over exactly the keys
`"0"`–`"n-1"`. Every probability must be a finite number in `[0, 1]`, and a
declared `type` echo must match.

The attempt fails closed, recording one or more reason codes, on a malformed or
unparseable body (`malformed_response`), a missing or invalid `usage`
(`missing_usage`, `missing_usage_output_tokens`, `invalid_usage_output_tokens`,
`invalid_usage_input_tokens`), a nonzero generated-token count
(`output_tokens_nonzero`), a missing or unknown answer (`missing_answers`,
`missing_answer:ID`, `unknown_answer:ID`), and any answer violation
(`invalid_answer:ID`, `answer_type_mismatch:ID`, `invalid_criteria:ID`,
`missing_choice:ID`, `unknown_choice:ID`, `missing_probabilities:ID`,
`probability_keys_mismatch:ID`, `invalid_probability:ID`, `invalid_score:ID`).
Transport faults, non-200 status, a foreign media type, a response over the
declared byte ceiling, or an expired deadline are retained with their attempt
status and detail. The inspection reports the union of the recorded reason codes
and the non-complete attempt statuses.

## Evidence

A capture directory holds `plan.json` and `deployment.json` (the exact source
bytes), `receipt.json` (identity and declared counts, published before traffic),
one `wave-NNNNNN.json` per burst, and one `request-NNNNNN/` directory per attempt
containing `body.bin` (the exact request bytes sent) and `response.bin` (the exact
retained response bytes, empty when nothing was read).

`evidence_sha256` is the digest of a manifest line holding the receipt digest
followed by every retained or missing burst digest in order, so two captures of
the same server differ. `endpoint_sha256` binds the endpoint without printing it
in a shared summary; the URL itself stays in the local receipt. `inspect`
re-reads the plan, deployment, receipt, every burst and every request/response
pair, re-checks digests and byte counts, and re-validates each complete response
against the declared contract before reporting. A missing
burst is reported as `missing_wave:N` and withholds completeness; a changed byte
is an error.

The `cells` section reports, per cell, the warmup and measured burst counts, the
eligible bursts, requests and decisions observed, and `latency_us`,
`requests_per_second` and `decisions_per_second` summaries over eligible bursts
only. Latency samples are per-request microseconds from dispatch to the complete
body. Rates are `count * 1_000_000 / makespan_us` for one burst, where `count` is
the dispatched requests or the validated answers. These summaries are display
values; every gate decision uses exact integer arithmetic.

## A/B/A2 comparison

`compare` admits only evidence that binds the same plan: one identical collector
binary throughout, exactly one capture per declared slot (including warmups and
failed captures), distinct capture evidence, an unchanged reference arm sharing
one clock, endpoint and deployment between `a` and `a2`, and a candidate arm whose
endpoint or deployment differs from the reference. A repeated slot, a capture
from another plan, or a reused directory is rejected before any gate arithmetic.
A change in the reported `input_tokens` for one declared state between
acquisitions is drift, and yields `INCONCLUSIVE` rather than a comparison.

Each gate pools the candidate arm's measured samples and compares them against
the observed reference envelope of `a` joined with `a2` using the declared
bounds, with the observed-envelope rules and a minimum of three samples per arm.
A latency gate is lower-better; rate gates are higher-better. The gate reports
`reference`, `candidate`, `counts`, outcome and any reason
(`ReferenceSpreadExceeded` is the reference drift check). An incomplete or
under-sampled arm cannot pass on survivors alone.

`PASS` means the candidate envelope is not adverse beyond the declared tolerance
under these capture periods; `REGRESSION` means it is. Neither is a certified
serving improvement, a causal claim, a capacity statement, or answer-quality
evidence. Agreement with any reference answers is not task accuracy. Retain
unchanged controls, failed captures and budget stops rather than rerunning until a
preferred verdict appears.
