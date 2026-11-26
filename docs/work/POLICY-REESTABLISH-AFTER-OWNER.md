# Scheduling-policy re-establishment after an external owner

Status: open (investigation). Found during the period-lock verification
(merged #278). Not fixed there.

## Observed

The direct cushion probe briefly owned the PCM during the verification matrix.
While TotalControl stayed running, the Pro Audio nodes ended on the convenience
policy instead of IRQ/H=2P:

| Field | Observed | Expected |
|---|---|---|
| graph quantum | 512 | 1024 |
| ALSA period | 256 | 1024 |
| `api.alsa.period-num` | 16 | 32 |
| `api.alsa.headroom` | 592 (336 + period) | 2048 |
| `api.alsa.disable-tsched` | true (stale) | true |
| result | `out_underruns` +20, `ring_starved` 6, `gate overdue` 91 | clean |

A GUI restart restored 1024 / period-num 32 / headroom 2048 / IRQ; that path
then held `ring_min 1025`, `ring_starved 0`, `gate overdue 0`.

The 336/16 values are the `release_engine` convenience policy, not the
packaged WirePlumber fallback (which sets IRQ, period-num 32, headroom 0) and
not the app's IRQ policy.

## Candidate paths

- `app.rs` `Message::EngineRead`: when `detected_path()` becomes external it
  calls `relinquish_graph_overrides` and sets `external_owner_active`; when the
  owner leaves it only clears the flag. It does not re-apply the policy or
  re-prepare.
- `audio_path_service` applies the policy on `SetSchedPolicy` and re-applies it
  when the node ids change. It does not reconcile the node props against
  `last_policy` when the owner releases or the props are recreated.
- `release_engine` (convenience 336/16) is called on clean exit and `Drop`.
  The exact trigger that left it running while the GUI stayed up is the open
  question.

## Plan

1. Reproduce with the merged direct probe while TotalControl and a stream are
   present; capture the process state, owner, node props and engine log.
2. Identify the exact call that applies the convenience policy (release
   intent, node recreation, or `Drop`) with a temporary log.
3. Fix the owner-release transition: re-apply the scheduling policy to both
   nodes, confirm the period pin and `headroom`/`period-num`, and re-prepare
   when idle. The pure decision belongs in the audio-path model; the service
   executes it.
4. Test: a direct-owner enter/leave cycle with TotalControl running must leave
   the IRQ/H=2P policy on both nodes; a playing stream that starts after the
   leave must open at the pinned period. Add a host test for the decision table.
5. Keep the C13 distinction: the fix is the policy re-establishment, not a new
   cushion multiplier.
