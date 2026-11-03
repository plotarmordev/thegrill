# Decision: the shared `sstore_tx` repair episode

**Decision: no.** Do not extract a Grill quality task from the shared
`41/120 → 120/120` episode yet. The evidence pack is useful because it replays
offline, but it cannot support an independently checkable task. It lacks a
held-out set, the prompt is ambiguous enough to cause most of the failure, and
The Grill has no execution grader that could run a submitted program safely.

This records a review of contributor evidence from
[issue #61](https://github.com/plotarmordev/thegrill/issues/61). It is not an
implemented contract, and The Grill does not vendor any of this data.

## Evidence reviewed

Gist `revofusion/8880d41a3cab5788dd2cb26390eb0cf1` at revision
`fc89372ab062d4ce7ae4c6820363058e14a31216`, MIT per its README. The prompt
excerpts carry the licenses of their EELS sources.

| File | SHA-256 |
|---|---|
| `prompt_seen.txt` | `323cd399133cecd7a0b3dd42281f71ff849c23b521be615082a7b3af9468f8a7` |
| `seed_program.py` | `33461f176ca70cc9dd90195c270cbbd51fd908484110dde95dc303ae6e36db8c` |
| `repaired_program.py` | `4d9789cdabfdeddff0feb9294b142c279be228865a73d4802e4ef6b9669266f8` |
| `truth.json` | `757c285956b95feb7c5eda79007d5a28dcffee2cc1e9724a15c1de8a26ba183c` |
| `regrade.py` | `8f6b617735c1a4304734cb4654954dff9de4546949c5ade3be81d5fce9f3e33f` |

## Offline regrade

On 2026-11-03 we regraded the pack with the contributor's unmodified scorer.
It ran on macOS under `sandbox-exec` with network access and file writes
denied, using CPython 3.12.16. The README states 3.11. Neither program imports
anything. Inside the same profile, a socket connect fails with `EPERM`.

```sh
git clone https://gist.github.com/revofusion/8880d41a3cab5788dd2cb26390eb0cf1 pack
cd pack && git checkout fc89372ab062d4ce7ae4c6820363058e14a31216
for f in seed_program.py repaired_program.py; do
  sandbox-exec -p '(version 1)(allow default)(deny network*)(deny file-write*)' \
    python3.12 -B -I -S regrade.py "$f"
done
```

| Program | Reproduced | Claimed |
|---|---|---|
| `seed_program.py` | `41/120` | `41/120` |
| `repaired_program.py` | `120/120` | `120/120` |

The six feedback-exposed inputs are the first six `FAIL` lines of the seed
regrade, in `truth.json` order. Fields are
`(original, writes, revert, warm, tx_gas)`:

1. `(1, [0], false, true, 1000000)`
2. `(1, [2], false, false, 200000)`
3. `(0, [0], false, false, 159387)`
4. `(3, [2, 1, 1], false, true, 170581)`
5. `(0, [2, 2, 3], false, false, 16918741)`
6. `(0, [2], false, false, 16926426)`

Expected outputs come from the `eels` column. In all 120 rows, `eels`, `geth`,
`nethermind` and `besu` are identical, so the pack shows that the four
implementations agree on these inputs. It does not show that they diverge
anywhere.

## Reasons

- **No held-out set.** The contributor states that none was retained. The
  `120/120` score therefore includes the six inputs whose expected outputs
  appeared in the feedback. The pack shows a fix that holds up across 114
  unshown inputs from the same generator, not one that generalizes. Producing a
  held-out set means running the reference implementation again on fresh
  inputs, and that is what a task extraction would need first.
- **Most failures come from an ambiguous prompt, not missing semantics.** The
  docstring says each write is followed by "PUSH0, PUSH0, REVERT if `revert`
  else STOP". All 79 seed failures have `revert=false`, and the seed passes
  every `revert=true` case. Every `header_gas` mismatch (62 of 79) is exactly
  4 gas over, and 66 `receipt_gas` mismatches are 3 or 4 gas over. A local
  ablation changed one line of the seed so it charges those two `PUSH0`s only
  when reverting (`if not halted:` became `if not halted and revert:`), and
  the seed then scored `107/120`. The 13 remaining failures differ only in
  `receipt_gas`. The ablation is our own probe, not contributor evidence. It
  shows that the measured repair mostly resolves the prompt's
  operator-precedence ambiguity.
- **The stated settlement omission does not match the prompt.** The issue
  describes the jump as discovering a settlement layer the model never saw.
  But `prompt_seen.txt` contains `settle_transaction_gas`, including the
  one-fifth refund cap. Only the body of `process_top_level` is missing. The
  README lists the repair READs as `GasCosts`, `StateGasCosts`,
  `charge_state_gas` and `sstore`, not `process_top_level`.
- **The repair is not replayable.** The multi-turn transcript, the READ
  payloads and the repair-generation token count were not kept. Only the
  outcome can be regraded, not the process that led to it.
- **The Grill cannot grade a submitted program.** Every relation in
  [PROJECT.md](PROJECT.md) grades an inert artifact. This scorer `exec`s
  untrusted model code in-process with no timeout or resource limit, so an
  infinite loop would hang it. Adopting it would need a new, versioned,
  isolated execution grader and its qualification fixtures. That is a protocol
  change, not a task pack.
- **The provenance would not satisfy pack v4.** The excerpts give EELS file
  paths and line ranges but no repository revision or source digest, so the
  v4 `revision` and `sha256` fields cannot be filled.

## What would change the decision

- A held-out input set with expected outputs from the pinned reference,
  generated before any candidate is shown feedback.
- A prompt whose `PUSH0`/`STOP` sequence has only one reading. Alternatively,
  the ambiguity can be declared as part of what the task measures.
- The EELS revision and digests behind every excerpt.
- An isolated execution grader in The Grill, if this direction is taken up.

Two parts of the pack are worth reusing as design input for that grader: the
truth-set shape (named argument order, points and per-implementation expected
outputs) and the 34-line regrade.
