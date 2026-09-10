# Manually reviewed shared recipe report

Copy this template into a separately reviewed public report. Replace bracketed
fields only with explicitly approved public values. Omit a field or mark it
`withheld` when publication is not approved; never paste private text to explain
why it was withheld. This summary is not raw replay evidence, an authenticated
execution record or an automatic safe-to-publish export. The CLI has no exporter
or uploader.

## Publication gate

- Public pins, change description and scope approved: [yes / no]
- Privacy review completed: [yes / no]
- Entire report restricted to the approved fields below: [yes / no]

Do not publish while any gate is `no`. Do not attach private deployment JSON,
local paths, endpoints, model/provider selectors, credentials, raw prompts,
responses, raw timing/usage metrics, metrics labels, stderr or loader diagnostic
text. Do not paste complete run/compare output. Review policy identifiers and
all other operator-supplied identifiers before publishing them. Hashes also need
publication approval; a digest is not an authorization to disclose its source.

## Public provenance

| Field | Approved public value |
|---|---|
| Externally pinned full source Git SHA | [SHA or withheld] |
| Actual collector binary SHA256 | [digest or withheld] |
| Evaluator binary SHA256 | [digest or withheld] |
| Package version | [version or withheld] |
| Cargo.lock SHA256 | [digest or withheld] |
| Build profile, command and deliberate flags | [reviewed build context or withheld] |
| Rust/Cargo versions, target, OS/architecture | [reviewed context or withheld] |
| Clean native Linux ARM64 install smoke | [verified / not run / failed] |
| Manifest SHA256 | [digest or withheld] |
| Recipe | [deepseek / glm] |
| Workload | [decode / prefill] |
| Exact workload source SHA256 | [digest or withheld] |
| Normalized workload SHA256 | [digest or withheld] |
| Captured policy SHA256 and approved identifier | [pins or withheld] |
| Role-labelled A, B, A2 evidence fingerprints | [digests or withheld] |
| Source review status for this recipe/workload | [reviewed / pending] |
| Loopback fixture status for this recipe/workload | [passed / not run / failed] |
| Live qualification status for this recipe/workload | [qualified within stated scope / pending / failed] |

Source Git revision, workload source bytes and normalized workload identity are
separate pins. A binary rebuilt from the same source need not have the same
binary digest. DeepSeek qualification does not qualify GLM, or vice versa.

## Approved change and evaluated scope

- Public change identifier: [approved public issue/commit identifier or withheld]
- Public change description: [independently reviewed public description or withheld]
- Declared policy approval before collection: [yes / no / unverified]
- Same captured policy in all roles: [yes / no / unknown]
- Distinct ordered acquisitions: [qualified / unqualified / unknown]
- Declared A/A2 reference identity: [declared_match / unqualified / unknown]
- Operator-reviewed nonoverlap and restoration: [reviewed / unverified]

Repeat a row for **every** policy cell/metric, including missing or unfavorable
gates. Use only public bundled cell IDs, metric enum values, numeric policy
thresholds and coverage counts from reviewed decision output. Do not include raw
sample values, private labels or copied diagnostic prose.

| Cell | Metric | Regression tolerance (bps) | Reference spread limit (bps) | Expected / observed coverage by A, B, A2 | Gate decision | Bounded reason codes |
|---|---|---|---|---|---|---|
| [public cell] | [metric enum] | [approved integer] | [approved integer] | [reviewed coverage counts] | [PASS / REGRESSION / INCONCLUSIVE / ERROR] | [codes only] |

## Decision and limitations

- Successfully parsed versioned decision envelope: [yes / no]
- Overall decision: [PASS / REGRESSION / INCONCLUSIVE / ERROR / no decision]
- Eligibility: [qualified / unqualified / unknown]
- Aggregate bounded reason codes: [codes only]
- Complete scope retained, including missing observations: [yes / no]

The decision concerns the declared observed-envelope policy and this workload,
binary, scope and acquisition set only. It is not a significance test, confidence
interval, causal attribution, equivalence test, universal no-regression result,
intelligence score or maximum-capacity claim. `compare` success does not mean
PASS. A missing decision envelope is not a decision. Matching declarations do
not independently prove model identity, nonoverlap or physical restoration.
Thinking controls are declarations, and `observe` does not prove a cold cache.
This reviewed summary omits private replay evidence; readers cannot reconstruct
the underlying observations from it alone.
