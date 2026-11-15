# UC init sequence: AP and PC, from captures and driver disassembly

Working note. Not a specification. Investigation 2026-11-15, device on the
AP personality, firmware 127.

## Sources

- Mac/AP driver captures and kext disassembly: public project
  `agfline/RME-Fireface-UC-Drivers` (cloned to a scratch dir; firmware 118
  era, driver 2.20). Key files: `cap/pluginit_full.pcapng`,
  `cap/set_sample_rate_*.pcapng`, `cap/start_stop_*.pcapng`,
  `asm/FirefaceUSB/hwInitHardware.c`, `hwSetSampleRate.c`, `xRate.c`,
  `FirefaceUSB.asm` (`initHardware`, `performAudioEngineStart`).
- Windows/PC captures: `<captures>/UC/uc-driver-only2.pcap`,
  `uc-win-coldattach-20261112.pcap`, `uc-win-driver-wdm2-20261112.pcap`,
  `uc-win-totalmix-restart-20261112.pcap`; rate sweep
  `<captures>/uc-rate-sweep3.pcap`; pitch probe
  `<captures>/uc-pitch-probe-20261111.pcap`.
- Live reads on the AP device with `tools/rme-probe.c` (built against
  system libusb; `rate` command reads 0x11/0x10 without claiming an
  interface, safe while the kernel driver is bound).

## AP init, verified (Mac driver)

Probe (`initHardware` in the kext, confirmed by `pluginit_full.pcapng`):

| Order | Write |
|---|---|
| 1 | SET_INTERFACE IF0 alt 1 |
| 2 | 0x1c firmware read |
| 3 | 36 × 0x16 wValue 0x0001, slots 0..17 and 32..49 |
| 4 | 0x10 wValue 0x0800 wIndex 0xb800 |
| 5 | 0x20 wValue 0x0001 wIndex 0 |
| 6 | DDS(44100) ×3, then 0x10 wValue 0x0010 wIndex 48 ×3 |

Step 6 is `hwSetSampleRate(44100)`: the Mac driver forces 44.1 kHz at
probe. The ×3 redundancy is the driver writing once per engine object.
The kext stores 44100 as the current rate afterwards.

Stream start (`performAudioEngineStart`, confirmed by
`start_stop_playinit_PB12-OUT12.pcapng` and `start_stop_recinit_IN1.pcapng`):
alt for the rate band, then DDS(rate) + 0x10 family/48, then
0x14 wValue 0xffff wIndex 0xc000. Both playback and capture starts do this.
Stream stop: 0x13 wValue 0xffff wIndex 0xc000.

The AP path has no 0x10/1487 commit anywhere. Polling: 0x11 and 0x10 reads
every 2 s, plus a fast 0x19 read.

## PC init, verified (Windows driver)

Probe (`uc-driver-only2.pcap`, `uc-win-coldattach-20261112.pcap`):

| Order | Write |
|---|---|
| 1 | SET_INTERFACE IF0 alt 1 |
| 2 | 60 × 0x16 wValue 0x0000, slots 0..29 and 32..61 |
| 3 | four DDS words: 0xc350/0, 0x8db9/0x2d01, 0x8235/0x5502, 0x7d00/3 |
| 4 | masked settings group: 0x0000/0x3000 ×2, 0x0800/0x0800, 0x0000/0x0800, 0x2000/0x3000, 0x0000/0x3000, 0x0800/0x0800 ×5 |
| 5 | no rate-family write, no 0x20 |

The device keeps its retained rate across the connect; the 0x11 read
reports it (index 2 = 48 kHz in both cold-attach captures). Polling:
0x1c, 0x1e, 0x1f, 0x11 at about 27 Hz.

WDM stream open (`uc-win-driver-wdm2-20261112.pcap`): 0x10 0x8000/0x8000,
0x1d 0/0, 0x14 mask/0xc000 (mask 0x0000 on firmware 127; the 0x000f form
comes from the newer-unit A2 captures). Close: 0x13 mask/0xc000.

Rate change (`uc-rate-sweep3.pcap`): SET_INTERFACE when the band changes,
0x10 family/48 ×3, 0x10 0x0001/1487 ×6. DDS words are not rewritten on a
rate change; only at connect and on pitch change.

No Windows capture contains a nonzero 0x16 write after the clear,
including the TotalMix-connect and TotalMix-restart captures. Windows
runs with the slots at 0.

## Rate model, verified

- DDS word = 0x5f5e10000000 / rate, shifted left 1 above 57 kHz and 2
  above 114 kHz; wValue = bits 31..16, wIndex = bits 15..8 | family.
  Exact for all nine standard rates and the eight varispeed rates in the
  Mac `set_sample_rate_*` captures (32000..192000, 42336, 44056, 44144,
  45937, 46080, 47952, 48048, 50000). The kernel's ISO DDS path already
  implements this formula.
- The fourth Windows DDS word (low byte 3) is 25/24 of the 48 kHz-family
  clock: 50 kHz at nominal pitch. It tracks pitch with the other words
  (pitch probe: word2 48053 Hz, word3 50055 Hz, ratio 25/24). Its clock
  role is otherwise undecoded.
- 0x11 top byte = family + 4 × (multiplier − 1): 0=32k, 1=44.1k, 2=48k,
  4=64k, 5=88.2k, 6=96k, 8=128k, 9=176.4k, 10=192k. Verified against the
  rate-sweep capture transitions and live reads.
- The Mac driver does not trust the retained rate: it forces 44.1 kHz at
  probe and re-commits the current rate on every engine start. Windows
  sets no rate at connect and relies on the retained rate until a rate
  change or the settings applet writes one.

## Live verification on the AP device (2026-11-15)

Current main driver, AP personality, firmware 127:

- Idle read: 0x11 index 1 (44.1 kHz), matching the last graph rate.
- Graph forced 44.1 → 48 kHz: cold start wrote DDS 0x8235/0x5502, family
  0x20/48, commit 1/1487; 0x11 read during playback returned index 2.
- Graph restored to 44.1 kHz: DDS 0x8db9/0x2d01, family 0x10/48, commit;
  0x11 returned index 1. Cadence guard read 44100/48000 Hz exact.

The current AP rate path is correct: the device follows the graph rate on
every cold start. Note: a PipeWire graph locked by `clock.force-rate`
resamples test files to the locked rate; a rate test must change the graph
rate (or the force-rate metadata), not just the file rate.

## Re-reading the November PC-init experiments

The shipped conclusion "the 60 × 0x0000 block is the failing write" rests
on three tests, each with a confound:

1. "Windows full + 0x20 → heavy distortion" vs "36 × 1 + Windows DDS/0x10
   + 0x20 → clean": same rate handling in both modules, but the device's
   retained rate at each run is not recorded, and neither run verified the
   rate via 0x11.
2. "60 × 0 + known-good settings, no trio → VLC could not keep a stream
   open": without the session-open trio the UC audio path does not open
   at all. This test cannot attribute anything to the 0x16 block.
3. "ABI-3, 60 × 0 + trio → wrong rate and distortion": those modules
   predate the cold-start rate-commit fix (2026-11-14). The recorded
   symptom is literally the rate-mismatch symptom.

The Windows init contains no rate write. Replaying it on a module that
does not commit the rate at start leaves the device at its retained rate
while the graph runs at another: pitch offset 1200·log2(device/graph)
cents (44.1↔48 is ±147 cents, 32→48 is −702 cents) plus rhythmic buffer
slip, which reads as sync-like distortion. This matches the reported
"a few tones too low, heavily distorted" symptom.

The decisive experiment has not been run: 60 × 0x0000 + four DDS words +
Windows settings group + WDM trio, no 0x20, on the current rate-commit
source, at 44.1 and 48 kHz, with the 0x11 index verified before judging.

## Breakthrough: 0x16 value is the channel sample-format selector

Bench session 2026-11-15, UC PC personality, firmware 127, 44.1 kHz,
internal loopback (0x15) return recorded at 18 channels and analyzed.
The Windows init replay distorted even with the rate correctly committed
(0x11 index matched the graph). The distortion was a sample-format
mismatch, not a rate or matrix error.

### The distortion is a sign-bit square wave

A 1 kHz sine (0.366 peak) came back as a 1 kHz square wave alternating
0 / −1.0 full scale, 50/50 duty. Measured spectrum: fundamental amplitude
0.6366 = 2/π, odd harmonics 2/(nπ) (3k 0.2122, 5k 0.1273, 7k 0.0909),
even harmonics at −64 dBFS, THD+N −6.3 dB (ideal square wave −6.31 dB).
That is the exact Fourier series of a unit square wave: the device passed
only the sign bit of each sample at full scale. The float32 sign bit (MSB)
equals the S32 sign bit, so float words fed into S32-configured channels
yield sign-only output.

### Windows sends S32 on this unit

Decoded the Windows WDM OUT audio (endpoint 0x01) in
`uc-win-driver-wdm2-20261112.pcap`: the loudest words are −0.245…−0.254 FS
as int32 but garbage (−9e18, NaN) as float32. Windows uses S32 on the
UC PC personality at firmware 127. The driver sends float32 for this
descriptor (no INT32_SAMPLES quirk, firmware below 138).

### Evidence matrix (one unit, firmware 127, 44.1 kHz)

| 0x16 block | Wire format | Loopback result |
|---|---|---|
| 60 slots × 0 (Windows) | float32 | square wave, THD −6 dB, ~12k clicks/s |
| 60 slots × 1 | float32 | clean sine, 999.98 Hz, THD −32 dB, 0 clicks |
| 60 slots × 0 (Windows) | S32 | clean sine, 1000.00 Hz, THD −65 dB, 0 clicks |
| 60 slots × 1 | S32 | distort, THD +3 dB / −6 dB, ~12k–89k clicks/s |
| 36 slots × 1 (Mac) | float32 | clean sine (shipped known-good) |

Clean iff the wire format matches the 0x16 value: 1 ↔ float32, 0 ↔ S32.
All four value/format combinations are tested; the slot count (36 vs 60)
does not affect cleanliness. The rule is airtight on this unit.

### Model and naming impact (needs confirmation before code changes)

The 0x16 wValue selects the per-channel sample format: 0 = S32, 1 =
float32. The host must send wire samples in the format it selected. This
is a host-driven contract, not auto-negotiation. The shipped 36 × 1 +
float32 path is one valid pairing; the authentic Windows 60 × 0 + S32 is
another.

This extends the sample-format model that commit #162 (Thomas) keyed to
UC_WIN_RME firmware (133 float32, 138 S32). The firmware rule and the
0x16 rule are reconcilable: the firmware likely sets the device's default
channel format, and 0x16 overrides it. #162 keyed the driver's wire format
to firmware as a proxy for the Windows choice on that descriptor. The
general rule is that the wire format follows the 0x16 init value the host
writes. Do not revert #162 blindly; the unified model needs the fourth
combination and, ideally, a UC_WIN_RME firmware-138 unit to confirm that
0x16 overrides the firmware default. A naming decision (what to call the
0x16 value field) goes through the user before code or spec changes.

### TotalMix init content is identical to the replay

The full Windows-plus-TotalMix connect burst (`uc-connect-burst3`) writes
exactly: 60 × 0x16, 4 × 0x1b DDS, 11 × 0x10 settings, 30 × 0x15 loopback,
1 × 0x17 (0x3f00/0xf03f), 22 × 0x1a, 666 × 0x12. No hidden nonzero 0x16
restore, no extra request. The scene restore is byte-identical to the
TotalControl push. The init string is ~75 driver writes (0.06..0.47 s)
plus 719 restore writes (0.64..2.74 s). The mystery was never a missing
write; it was the format pairing.

### Dormant endpoints

Windows never uses IF0 bulk (0x03/0x84), IF3 (0x88/0x09) or IF4 (0x8a) in
any capture. They are dormant in normal operation, consistent with a
service/R&D role, and are not part of the init.

## RME's sample-format heuristic

The unit does not "want" a format: it powers up in a personality-dependent
default, and the host selects the channel format via the 0x16 value and
must send matching wire samples. Both formats play clean on the same unit
when paired correctly (evidence matrix above).

Verified pattern across the product line:

| Personality / transport | Host OS native format | Wire format | 0x16 |
|---|---|---|---|
| AP / ISO (macOS Core Audio) | float32 | float32 | 1 |
| PC / interrupt (Windows PCM) | integer S32 | S32 | 0 |

Evidence: UCX spec (ISO "float32 frames", interrupt "32-bit integer
samples"); Mac kext always writes 0x16 = 1 and sends float32; Windows
capture sends S32 with 0x16 = 0 on UC PC firmware 127; the kernel
descriptor table keys INT32_SAMPLES to the interrupt descriptors (UCX
IF5, UC_WIN_RME) and float to the ISO descriptors. The UC PC descriptor
declaring float is the anomaly: it uses the Mac-style 0x16 = 1 init on a
PC personality, which works but is not the authentic Windows pairing.

Heuristic (substantiated): the wire format matches the host OS's native
audio format, selected by the personality/transport. The device default
follows the personality; 0x16 overrides it.

Firmware default change (hypothesis, not provable from current evidence):
UC_WIN_RME firmware 133 carries float32 and 138 carries S32 (Thomas,
#162). The likely reason is DSP economy: the UC DSP processes in
integer/fixed-point (12-bit linear matrix gains, integer meter and phase
words), so S32 wire feeds it without float↔int conversion at the
interface. The newer hardware revision (firmware 138, Hw Rev 6) plausibly
aligned the Windows path to S32 end-to-end to save those conversions,
while the Mac path stays float to match Core Audio. What is verified is
the pairing rule and the personality pattern; the motive for the firmware
default change is inference.

Implementation consequence: the driver's wire format, the 0x16 init value
and the personality must be one consistent decision. Choosing S32 for the
PC personality is authentic to Windows and may be DSP-cheaper; choosing
float (the shipped Mac-style init) is also valid. The host decides.

## The choice rule: how the host picks the format

The host cannot read the channel format back: flipping 0x16 between 0 and
1 leaves 0x11 (11 1e ff 01) and 0x10 (11 4a 00 00) unchanged (live test,
firmware 127). The 0x19 byte that drifts is a counter, not format. The
format is a write-only host choice. The earlier 0x11 difference between
the Windows capture (00 fe ff) and the Linux unit (11 1e ff) at the same
rate is unit/state-specific, not a format bit.

The host therefore does not read a device default; it imposes a format.
Verified choice behaviour:

- Mac kext: hwInitHardware writes 0x16 = 1 unconditionally, no firmware
  branch. Float always (Core Audio native).
- Windows UC PC: the cold-attach capture writes 0x16 = 0 immediately after
  SET_INTERFACE, with no vendor read before it (the 0x1c firmware read
  comes after the init block). S32 is hardcoded per product, not read from
  the device and not firmware-keyed at connect.

So RME's choice is the host OS native format, hardcoded per personality.
The device default is irrelevant after init because the host can impose
either format (proven). The UC_WIN_RME firmware variation (133 float, 138
S32, Thomas) is the one unexplained case; it may be driver-version or
product-specific firmware keying. It needs the Windows driver disassembly
or a UC_WIN_RME connect capture to resolve, and bench confirmation on
Thomas's unit.

## Implementation (branch fix/uc-sample-format-pairing)

Final design decision: the wire format follows the host OS's native audio
format. Linux and macOS are float-native (float32); Windows is
integer-PCM-native (S32). This driver runs on Linux, so every UC
personality carries float32. No firmware keying, no per-personality format
special case. A device that strictly requires S32 would be an
evidence-backed exception (FFUC_QUIRK_INT32_SAMPLES), not a default.

- The 0x16 wValue is derived from the resolved wire format on the UC model
  (0 = S32, 1 = float32), so the pairing can never mismatch.
- Both UC personalities use the 36-slot init (0..17, 32..49) with the
  0x0800/0xb800 settings word and 0x20 enable, float32 wire. The Windows
  60-slot S32 block is documented as the Windows-host reference, not
  shipped on Linux.
- UC_WIN_RME loses its firmware-keyed S32 (the #162 heuristic); it follows
  the host like every UC. The firmware variation was the Windows-host
  behaviour, not a device requirement.
- UCX interrupt (IF5) keeps S32: that is a documented transport protocol
  difference, a candidate host-OS-rule exception pending verification, not
  an arbitrary special case. Babyface (ISO) is float.

Rationale for float on Linux: PipeWire negotiates FLOAT_LE, so float wire
needs no sample conversion (a vectorizable copy). S32 wire forces a
per-sample integer bit-twiddle conversion in the URB completion hot path
(the kernel cannot cheaply use the FPU). Measured: float copy 0.02
ns/sample vs S32 conversion 0.97–1.33 ns/sample (50–70×); at 48 kHz × 18 ch
× 2 directions, ~0.03 ms/s CPU for float vs ~2 ms/s for S32. This is a CPU
and hot-path-cleanliness win, not a latency win: latency is set by the USB
queue, ALSA period/buffer and device round trip, all format-independent
(the conversion is ~1 µs per 48-frame buffer against a 1 ms URB period).
RME itself runs float on this hardware via the Mac personality, so float
on Linux is consistent with RME's own behaviour.

Bench evidence (UC PC, firmware 127, 44.1 kHz, internal loopback): float
wire + 36-slot init gives a clean sine, 999.99 Hz, zero discontinuities,
fundamental = RMS+3 dB; the S32 pairing was also verified clean (1000.00
Hz, THD+N −65 dB) before the design settled on float. Rate transitions
44.1↔48 kHz track the 0x11 index with zero transport errors; audio-health
CLEAN. AP path awaits a bench pass in AP mode. UC_WIN_RME float and the
UCX-IF5 question await bench confirmation (Thomas).

## Complete-init assessment

The init SEQUENCE is fully decoded for both hosts: every write, in order,
with values (Mac from the kext disassembly + pluginit captures; Windows
from the driver-only, cold-attach and WDM captures). The format-pairing
rule explains the distortion. What remains undecoded is the SEMANTICS of a
few fields, not the sequence: the masked settings-group bits (0x3000 /
0x0800 masks, bits 11..13), the fourth DDS word's clock purpose (its value
is 25/24 of the 48 kHz-family clock), the periodic status reads (Mac 0x19,
Windows 0x1e/0x1f), and the 0x10 0x0800/0xb800 settings word meaning. So:
complete sequence, partial field semantics.

## Tooling

`tools/rme-probe.c` gained a hwdep backend: control commands (info, fw,
rate, status, raw, write, set-rate) fall back to the driver's hwdep node
when the USB device node cannot be opened, or via `--hwdep NODE`. This
needs no udev rule and works while the driver is bound. Raw stream
commands still need direct USB access. The root Makefile builds rme-probe
against system libusb when the vendored header is absent. Note: the udev
rule covers the AP personality and the new vendor ID range but not the
UC PC old-VID product; the hwdep fallback makes that gap moot for control
commands.

## Open items

1. UC_WIN_RME (Thomas, firmware 133/138): confirm float32 + 0x16 = 1 plays
   clean on Linux now that the firmware-keyed S32 is removed. Prediction:
   the same unit runs float in AP mode and S32 in PC mode under RME's
   drivers (format follows the host OS, not the firmware). If his unit
   needs an exception, he owns a follow-up PR. Status: needs-device.
2. UCX interrupt (IF5): does the host-OS rule (float on Linux) apply, or is
   S32 a strict transport requirement? Kept S32 pending verification.
   Status: needs-device.
3. AP-mode bench pass of the UC float path (device was in PC mode this
   session). Status: open.
4. The device's power-on default channel format and rate (retained state
   survives USB reset; a power cycle is untested). Status: needs-capture.
5. Field semantics still undecoded: the masked settings group bits
   (0x3000/0x0800 masks, bits 11..13), the fourth DDS word's clock purpose
   (25/24 of the 48k family), the periodic status reads (0x19, 0x1e, 0x1f),
   and the 0x10 0x0800/0xb800 settings word. The init sequence itself is
   complete. Status: open.
6. AP probe-rate policy: the Mac forces 44.1 kHz at probe; the current
   driver commits the graph rate on every cold start instead, which
   covers the same failure mode. Status: optional.

Resolved: the fourth format combination (0x16 = 1 + S32) distorts as
predicted, completing the evidence matrix; the 0x16 format field is named
and the spec updated to the host-OS rule (this branch).
