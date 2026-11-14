# UC meter work — remaining items

The meter delivery and page-validity work is complete for the PC
personality.  This note lists the work that remained after the final
validation.

## Closed in this branch

- Whole RMS/peak heartbeats delivered by the driver.
- Pair-aligned backpressure and stop drain.
- AN-record phase cue; fallback tested as a safety net.
- Output meters use the device's post-fader RMS and peak words.
- Header RMS status words rejected above the matching output peak.
- Output record mapping confirmed as block order.
- Publication selector closed as not reproduced under the paired path.
- Input 1/2 VU confirmed correct.

## AP profile

1. Capture the AP descriptor: interfaces, alternate settings, endpoint
   maximum packet sizes.
2. Verify the AP header position and phase fields.
3. Verify the RMS-then-peak pair order under the AP personality.
4. If the capture is not done, mark `uc_ap` experimental rather than
   production.

## Metering details

1. Beat-word semantics in bytes 4..8.
2. The three uses of `0x1fff`.
3. Full inventory of the header status slots and the RMS-bank
   slot-internal engine fields.

## USB transport details

1. Roles of PC interfaces 2, 3 and 4.
2. MIDI protocol on the interface 2 bulk pair.
3. Read form of request `0x10`.
4. Host-chosen interrupt URB size; document that it is not a device
   contract.
5. Status bytes 0 and 1.
6. Bulk role of endpoints `0x03` and `0x84` at 1x and 2x.

## Loopback and matrix follow-ups

1. Loopback destination indexing.
2. Metering freeze with a matrix write above 0 dB.
3. Loopback versus input-meter placement.
4. Matrix cell, per-submix source fader and output master as three
   distinct readouts.

Captures and raw state stay outside the repository.
