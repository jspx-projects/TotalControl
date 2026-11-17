# Digiface USB support

Working note, not part of the specification. It records what is known about
the RME Digiface USB and how to add mixer control to TotalControl.

| File | Purpose | Destination once reviewed |
|---|---|---|
| [`DIGIFACE_USB_PROTOCOL_SPEC.md`](DIGIFACE_USB_PROTOCOL_SPEC.md) | Wire protocol and host binding | `docs/spec/DIGIFACE-USB-PROTOCOL-SPEC.md` |
| [`IMPLEMENTATION.md`](IMPLEMENTATION.md) | Kernel module, backend, profile, tests | Working note; the specification outcome lands in the protocol spec and the device profile spec |
| [`CAPTURE-REQUEST.md`](CAPTURE-REQUEST.md) | The evidence the device owner must provide | Working note, shareable as-is |
| [`SOURCES.md`](SOURCES.md) | Sources and the MIT licence position | `THIRD-PARTY.md` entries and code comments |

## Ownership

The work is split by who can do it best:

| Step | Owner | What |
|---|---|---|
| Protocol specification | contributor | Reaffirm and complete the spec on the unit (probe + captures); land it verified. Required. |
| Kernel control module | contributor | The thin interface-1 driver, one hwdep node, beside the mainline audio driver. No audio driver. |
| Two-driver TotalControl adaptation | maintainers | Discovery and pairing of the mainline audio card with the control card, the backend, the profile, the GUI entry, the mock and the tests. Application architecture, hardware-independent. |
| Integration and GUI completeness | contributor, with maintainer help | Test on the unit; every function the spec defines must be reachable in the GUI. Raise an issue for help. |

The contract between the two sides is the hwdep ABI header (the ioctl
commands and struct layouts; a sketch is in [`IMPLEMENTATION.md`](IMPLEMENTATION.md)).
The contributor proposes it, the maintainers review it; the maintainers build
the backend against it, the contributor implements the kernel side. The
protocol draft, the implementation plan and the capture request below are the
starting point; they are prior art and a plan, not a finished implementation.

## Specification completion (required)

Completing the protocol specification is a required part of the pull request,
not an optional extra. The draft in
[`DIGIFACE_USB_PROTOCOL_SPEC.md`](DIGIFACE_USB_PROTOCOL_SPEC.md) is prior art;
the contributor turns it into a verified specification:

1. Read it end to end and mark each section verified or open.
2. Reaffirm the known parts on the unit (status read, mixer enable, one route,
   one output fader, one meter frame) and capture the open parts (loopback
   request, meter rate, interrupt endpoint `0x83`, snapshot storage, 2x/4x
   channel maps).
3. Update the specification with the verified facts and retire the open items
   it settles; keep the rest as named open items.
4. Land it as `docs/spec/DIGIFACE-USB-PROTOCOL-SPEC.md` in the same pull
   request, with the evidence (firmware, transport, rate, steps and results).

The specification is the maintenance anchor for the code; the code and the
specification land together.

## The target mode is normal mode

The Digiface USB has two firmware modes, selected with the Flash Update Tool:

| Mode | USB declaration | Audio | Mixer control for a third-party host |
|---|---|---|---|
| Normal | vendor-specific interfaces | plain PCM, streamed by mainline `snd-usb-audio` | interface 1: vendor control requests plus bulk matrix words and meters |
| CC | class-compliant, different product id | class driver, no quirk | no TotalMix routing; EP0 vendor control requests still answer |

**Normal mode is the target.** Mainline already streams its audio, and the
mixer interface is left free for a control driver. The backend does not wait
for any CC-mode investigation.

Two earlier claims in this note were wrong and are corrected here:

- The iPad application does **not** control the Digiface USB today. RME states
  support is planned, not shipped; the App Store device list excludes it. The
  mixer therefore is not known to answer over the iPad path.
- CC mode is not a mixer-control path. RME states there is no input-to-output
  routing in CC mode; the DAW does the routing. EP0 vendor control requests do
  answer in CC mode (RME published a libusb demo that switches the optical
  output formats with request 18), but whether interface 1 and the mixer exist
  there is unknown and is not needed for the backend.

## Verified mainline facts

Read from the mainline `sound/usb` sources (see [`SOURCES.md`](SOURCES.md)):

- The quirk matches vendor-class interface 0 only, with the normal-mode
  product ids, and leaves the other interfaces unclaimed.
- The audio is plain PCM, 24-bit in 32-bit containers (`S32_LE`), not a
  proprietary packet format. Playback is isochronous OUT endpoint `0x02`,
  capture is isochronous IN `0x81`.
- Playback is asynchronous with **implicit feedback** from the capture
  endpoint (`.implicit_fb = 1`, `.sync_ep = 0x81`) — the same mechanism the
  Fireface URB engine uses.
- Three rate families: 32–48 kHz (34 out / 32 in), 64–96 kHz (18 / 16),
  128–192 kHz (10 / 8). The last two outputs are the phones pair.
- A boot quirk writes a fixed state on every probe: mixer off, internal
  clock, all outputs ADAT, 48 kHz, loopback off, unity output gain. A control
  app must re-enable the mixer and full-sync after connect.
- ALSA mixer controls already exist for `Sync Source` (rw), `Current Sync
  Source`, `System Rate` / `Current Rate` (r), `Input N Sync/Format/Rate` (r)
  and `Output N Format` (rw). They use the same vendor requests 16 and 18 and
  the status read 17 the protocol draft describes, with matching bitmasks.
- The matrix, output faders, meters, loopback and snapshots are deliberately
  left to userspace; that is the control module's scope.

## Two-card model and pairing rule

The control module cannot attach hwdep to the mainline audio card. It
registers its own ALSA card with one hwdep node and no PCM:

| Card | Owner | Contents |
|---|---|---|
| Audio card | `snd-usb-audio` | PCM and the standard mixer controls |
| Control card | the new module | one hwdep node, no PCM |

The application pairs them into one visible device. Pairing rule: match the
control card by its fixed id (proposed `DigifaceControl`) and the audio card
by the mainline Digiface card id. Store ids, never card numbers. Both present
→ one device; audio card only → "Digiface USB (no mixer interface)", no
backend; control card only → an error state.

## Status

| Item | State |
|---|---|
| Normal-mode audio, endpoints, implicit feedback | Verified in mainline `sound/usb` |
| Mainline ALSA clock/format/status controls | Verified in mainline `mixer_quirks.c` |
| Boot quirk fixed state | Verified in mainline `quirks.c` |
| CC mode has no TotalMix routing | Verified statement, RME forum |
| CC mode EP0 vendor control answers | Verified, RME libusb demo (request 18) |
| CC-mode interface 1 / mixer presence | Unknown; optional descriptor capture |
| Mixer interface and endpoints | Public prior art, not verified here |
| Control requests and register bits | Public prior art, partly confirmed by mainline values |
| Matrix word format | Public prior art, not verified here |
| Gain law | Public prior art, unity confirmed by the mainline write |
| Meter frame layout | Public prior art, measured on a unit by Openface-Mixer (vendor mode) |
| Loopback request | Conflict between prior art and mainline; needs a capture |
| Snapshot storage | Unknown; needs a capture |
| Device profile | Not written |

## Recommendation

Use a thin control-only kernel module that binds interface 1 and exposes one
hwdep node. Keep audio on `snd-usb-audio`. Keep the mixer model, routing
policy and scenes in userspace. This follows the repository's support policy;
a direct-USB userspace backend is a useful capture and prototype tool, not
part of the supported set.

Do not start the kernel work before the captures in
[`CAPTURE-REQUEST.md`](CAPTURE-REQUEST.md) confirm the prior-art protocol. A
quick first probe is enough to start: RME's published `usb_vendor` libusb demo
and the Openface-Mixer / rmectl prior art exercise the control requests
directly. Reaffirm the status read, mixer enable, one route, one output fader
and one meter frame on the unit before writing the module against them.

## What the device is

- 32 ADAT inputs at 1x: four optical ports of eight channels, each port also
  usable as stereo S/PDIF (the pair aliases the port's first two channels).
- 34 outputs at 1x: the same four optical ports plus one stereo Phones output.
- 34 software playback channels at 1x.
- TotalMix FX with routing, faders, mutes, pans, groups, snapshots and
  loopback.
- No microphone preamps, no instrument inputs, no EQ, no dynamics and no
  reverb or echo. The manual states that FX is not available on this model.
- No MIDI ports. A DIN Mackie surface cannot connect; USB controllers (ARC
  USB, a USB X-Touch) connect through the computer and use the generic
  controller engine.
- Channel counts fall with the sample-rate family because ADAT uses S/MUX.

## Open items

| Status | Item |
|---|---|
| needs-capture | Normal-mode descriptors for interface 1 |
| needs-capture | TotalMix operation capture: routing, fader, mute, pan, clock, format, snapshot, loopback |
| needs-capture | Meter frames and rate |
| needs-capture | Loopback request semantics |
| open | Channel topology at 2x and 4x |
| open | Snapshot and setup storage in the device |
| open | Whether the mixer can be enabled while audio streams |
| open | Interrupt endpoint `0x83` purpose |
| optional | CC-mode descriptor tree (settles the CC-mode question; not needed for the backend) |