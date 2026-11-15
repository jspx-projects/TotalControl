# Fireface UC — open work backlog

A single, shared backlog for the Fireface UC (and UCX where noted), organized
by component. Anyone can pick an item, implement it, and update its status
here. This is a working note, not a specification; the authoritative protocol
model lives in `docs/spec/FIREFFACE-UC-PROTOCOL-SPEC.md` and the detailed
investigations in the sibling notes referenced below.

Conventions: one Fireface at a time; preserve the user scene and gui.json;
keep captures and raw device state outside the repository; no personal data,
serials or USB IDs in commits or notes. Protocol items need a capture before
code — state the implemented mapping positively in the spec and keep the open
question here.

Status legend: **open** (actionable), **needs-capture** (blocked on evidence),
**optional** (not required for correct behaviour), **done** (recently closed).

## Driver (kernel)

The PC and ISO streaming cores are verified clean and share the gated signal
path, ring-pressure instruments and transferred-frame validation. No open
streaming defect.

### Transport / protocol decode (spec §11.11, need captures)

| Item | Status | Notes |
|---|---|---|
| UC MIDI | done | Interface 2 carries two logical MIDI ports on bulk OUT `0x07` / IN `0x86` (`kernel/ffuc-midi.c`). Both directions use USB-MIDI code index numbers; RME's Windows driver sends length codes instead and the unit accepts both. A bridge from MIDI 1 out to MIDI 1 in returned three messages and a 20-byte LCD SysEx through the rawmidi devices. The unit parses each bulk transfer as a separate stream, so a message must fit one transfer. The AP personality has no MIDI row. |
| Mackie Control host | done | Userspace host on the rawmidi surface: handshake, faders/VPOT/buttons, bank/row/submix navigation, control room, snapshots, cue, simple CC/note, LCD + meter feedback, loopback guard, MIDI AND OSC settings page (`mackie.rs`, `midi.rs`, `app.rs`). Codec and mixer mapping unit-tested; live-surface verification and the remaining open items are in `TOTALCONTROL-MIDI-SPEC.md` §13. |
| MIDI descriptors and wire (AP, Babyface, UCX) | needs-capture | AP personality IF2 class; Babyface normal-mode IF2 class and jacks; UCX class-codec wire verification and whether it shares the per-transfer parsing rule; UC port 2 physical output. See MIDI spec §13. |
| Interfaces 3/4 roles (PC) | needs-capture | Endpoints present, unclaimed/undecoded. |
| Read form of request 0x10 | needs-capture | Only the write is implemented. |
| Status bytes 0/1 | needs-capture | Front-panel events use bytes 2/3. |
| Bulk role of 0x03/0x84 at 1x/2x | needs-capture | Bulk there, interrupt at 4x. |
| PC interrupt URB size | open | Documentation only: 48/96/192 frames is the host's choice, not a device contract. |

### Mode and init (see `UC-MODE-INIT-TODO.md`)

| Item | Status | Notes |
|---|---|---|
| Windows rate sweep comparison | needs-capture | Run the Windows rate sweep, compare the PC init; keep the shared init until proven. |
| PC init proof on Linux | open | Confirm the PC-mode open path and default sink end to end. |

### Metering, driver side (see `UC-METER-REMAINING.md`, `UC-METER-DECODE.md`)

Heartbeat delivery, pairing (shape-check realign), calibration and ballistics
are done and verified. Digital inputs read the source exactly; analog inputs
correctly include the input-reference gain. These are robustness items, not
defects.

| Item | Status | Notes |
|---|---|---|
| Slip cause | needs-capture | Intermittent; alt-change confirmed, no duplicate page across 8891 pages. A bus analyser at the data-toggle level across a slipping start decides it. The shape check realigns regardless. |
| Shape check with live ADAT 3..7 | needs-capture | The RMS-half AN-range spans ADAT 3..7; decidable with live AN5/SPDIF (748/748), the exact ADAT case needs an ADAT source. |
| Bounded deferral (interrupt transport) | optional | The zero-slack corner is accepted with the manual warning (quantum >= 2 ms at 96/192 kHz). Not required. |

## GUI (userspace)

### Gated controls (see `UC-TODO.md`)

`needs_protocol_features` drops these messages on the UC. For each: keep
gated (no device write), un-gate (device or host-side operation exists), or
hide.

| Item | Status | Notes |
|---|---|---|
| Pitch | open | Spec §7 documents the 0x1b DDS mapping; `caps.pitch` is false and the gate drops it. Implement the write and un-gate. |
| Output phase | open | Check whether it is a matrix sign bit or an output word; input phase is already excepted. |
| Width / MS Proc / per-channel width | open | Host-computed matrix operations (§9); need a UC matrix rewrite to be audible, not just host state. |
| Dim, Speaker B, Talkback receive | open | Confirm host-side monitor operation vs unsupported. |
| Input link, AN 1>2 | open | Confirm host-side pairing vs a device word. |
| EQ / dynamics / FX / AutoSet / reference-level switch | open | UC has no DSP (§9); keep gated or hide. |

### Meter display

| Item | Status | Notes |
|---|---|---|
| Meter handling across UC/UCX | open | Share ballistics/peak-hold/OVR/display; delegate the device-specific page decode (UC heartbeat vs UCX self-describing). Per-device ballistics already exist (`MeterBallistics`). |

## Validation & captures

Host-side measurements and protocol evidence; not driver code unless a
capture reveals a defect.

| Item | Status | Notes |
|---|---|---|
| Physical-loopback bit-exact check (both transports) | needs-capture | The audio-content gate; needs a cable. Scheduling is sound on the dither substrate; pitch/level/mapping under load unverified until this runs. |
| Mode-2 early capture-content zeros | open | Likely a channel-mapping artifact: ADAT 5/6 are ALSA indices 12/13; the old trace checksummed channel 15 (ADAT 7), silent in that scene. Verify against the PC scene routing. See `FIREFACE-UC-WIN-RME.md`. |
| q48 escalation experiment | open | A skipped follower cycle is necessary but not sufficient for an audible zero-run; the aggravator is client-side buffering. Shrink the recorder buffer to force escalation. Also q128 failure rate, output-vs-captured. |
| Beat-word semantics (bytes 4..8) | needs-capture | Session-dependent engine fields; no page-identity role. |
| The three uses of 0x1fff | open | No-update sentinel, RMS no-value, beat-word value; whether one code is intentional is unknown. |
| Header status-slot / RMS-bank engine-field inventory | needs-capture | Drift within a session, do not track the signal; no law published. |
| AP meter profile verification | needs-capture | Verify RMS-then-peak order and header fields under AP; else mark `uc_ap` meters experimental. |

## Tooling & consolidation

| Item | Status | Notes |
|---|---|---|
| Shared gate / ring-pressure / payload-validation | done | Common to both transports. |
| Counters into the hwdep UAPI | open | Move the session counters into `ffuc_hwdep_counters` and extend `ffuc-counters` so a non-root reader sees them. |
| Session scripts into `scripts/` | open | Generalise the battery/phase-lead/loopback procedures; document in `DEVELOPMENT-TOOLS.md`. |

## Recently closed

- ISO transport consolidation and closure (#176, #177).
- Meter heartbeat slip realign (#175), ballistics timing + per-device rates
  (#174), example build fix (#173).
- Configurable VU fallback release (#178, #179).
- PC-mode rate commit + Windows-session timing verified.