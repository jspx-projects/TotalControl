# Fireface UC PC (interrupt) driver — open work items

Working note, not part of the specification. Backlog for closing the UC PC
mode topic. The streaming core is verified clean; what remains is protocol
decode features, host-side measurement questions, and optional consolidation.

## Verified / closed

- PC-mode duplex verified: `in_overruns 0`, `out_underruns 0`, `bad_length 0/0`,
  `delta_zero 0`, `hw_ptr_mismatch 0`, gate `overdue 0 / max_pending 1`.
- Cold-start rate commit and the `FFUC_QUIRK_WIN_SESSION` timing (settle +
  close gaps) verified in PC mode (firmware 127, interrupt transport).
- Shared gate, ring-pressure instruments and transferred-frame validation are
  merged and common to both transports (#166, #169, #170). The q48 zero-run
  investigation concluded the driver is contract-correct; the residual
  zero-runs are host-side (PipeWire follower), not a driver defect.

## Protocol decode gaps (features, not defects)

Spec `FIREFFACE-UC-PROTOCOL-SPEC.md` §11.11. Each needs a capture before code.

1. **Interfaces 3, 4 roles.** Endpoints present, unclaimed/undecoded.
2. **Read form of `0x10` outside the options bits and the rate family.** The
   bits under `0x05cf` read back as written and the rate commit uses them;
   bits 4-5 carry the rate family; the other bits are undecoded.
3. **Status bytes 0 and 1.** Front-panel events use bytes 2/3; 0/1 undecoded.
4. **Bulk role of `0x03`/`0x84` at 1x/2x.** Bulk there, interrupt at 4x; the
   bulk role is undecoded.
5. **PC interrupt URB size.** Documentation only: 48/96/192 frames is the
   host's choice, not a device contract.

## Host-side measurements (not driver code)

1. **Mode-2 early capture-content zeros.** Hypothesis: a channel-mapping
   artifact. ADAT 5/6 are ALSA indices 12/13; the original trace checksummed
   channel 15 (ADAT 7), a silent channel in that scene. Verify against the PC
   scene routing; if confirmed, this closes as a measurement artifact, not a
   second failure mode.
2. **q48 escalation experiment.** A skipped follower cycle is necessary but
   not sufficient for an audible zero-run; the aggravator is client-side
   buffering. Shrink the recording client buffer to force escalation and get a
   powered audible-event test. Also: q128 failure rate, and whether the zeros
   affect the device output or only captured/monitored streams.
3. **Physical-loopback bit-exact check.** The audio-content gate for both
   transports; needs a cable. Scheduling claims are sound on the dither
   substrate; pitch/level/mapping under load are unverified until this runs.

## Consolidation status

- Shared gate / ring-pressure / payload-validation: done.
- Bounded deferral for the interrupt transport: optional. The zero-slack
  corner is accepted with the manual warning (quantum >= 2 ms at 96/192 kHz),
   so this is not required.
- Tooling promotion: move the session counters into the hwdep UAPI struct and
  the session scripts into `scripts/` (see the tooling promotion plan in
  `FIREFACE-UC-WIN-RME.md`). Follow-up, not a blocker.

## Disposition

The PC streaming driver has no open defect. The items above are protocol
decode features (need captures), host-side measurements, and optional
consolidation; none obliges a driver change for correct audio.