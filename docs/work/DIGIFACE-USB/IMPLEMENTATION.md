# Digiface USB implementation plan

Working note, not part of the specification. The protocol is in
[`DIGIFACE_USB_PROTOCOL_SPEC.md`](DIGIFACE_USB_PROTOCOL_SPEC.md). The captures that must come first are in
[`CAPTURE-REQUEST.md`](CAPTURE-REQUEST.md).

## 1. Architecture decision

Use a thin kernel module that owns the mixer interface and exposes one hwdep
node. Keep audio on `snd-usb-audio`. Keep the matrix, faders, routing model,
scenes and profiles in userspace.

The target is normal mode. RME's iPad application reaches the mixer in CC mode
through a private protocol over USB, so the mixer answers there. That does not
expose a control interface a host can claim. A CC-mode descriptor capture
decides whether one exists; the normal mode backend does not wait for it.

Why not a userspace USB backend:

- the support policy in `docs/CONTRIBUTING-NEW-DEVICE.md` excludes direct-USB
  crates;
- a userspace process cannot share interface 1 with a kernel driver cleanly;
- udev rules and permissions become part of the install;
- the project already has an hwdep ABI style to follow.

Why not a full audio driver:

- `snd-usb-audio` already streams the audio interface correctly;
- the mixer is a separate vendor interface;
- writing an isochronous audio engine would duplicate mainline work.

A userspace libusb tool is still useful for captures and for a prototype
before the kernel module exists. It is not the supported path.

## 2. The two-card consequence

The kernel module cannot attach an hwdep node to `snd-usb-audio`'s card. It
registers its own ALSA card with one hwdep device and no PCM.

| Card | Owner | Contents |
|---|---|---|
| Audio card | `snd-usb-audio` | PCM, standard mixer |
| Control card | the new module | one hwdep node, no PCM |

Give the control card a distinct id, for example `DigifaceControl`, so
discovery never confuses the two. The UI combines them into one visible
device: audio comes from the class card, the mixer model from the control
card.

If a one-card result is required later, the only clean route is to make the
module own interface 0 and the audio stream as well. That is a much larger
change and is not part of this plan.

## 3. Kernel module

Proposed files:

| File | Purpose |
|---|---|
| `kernel/ffdf.c` | USB driver, hwdep node, transfers |
| `kernel/ffdf.h` | private state |
| `kernel/ffdf-uapi.h` | userspace ABI |
| `kernel/Makefile` or `Kbuild` | module entry |
| `udev/` rule | hwdep node permissions, if needed |

### 3.1 Matching

- Match the RME vendor ID and the two Digiface product IDs. Take the values
  from the mainline quirk table; do not repeat them in documentation.
- Match interface class vendor-specific and interface number 1.
- Do not match interface 0. `snd-usb-audio` keeps it.

The match flags are the same shape as the mainline quirk: vendor, product,
interface class and interface number.

### 3.2 Probe

1. Claim interface 1.
2. Read the endpoints and confirm bulk OUT `0x0b` and bulk IN `0x84`.
3. Allocate state and a command buffer.
4. Enable the mixer: request 18 value 0 index `0x0100`, then request 16 value
   0 index `0x0400`.
5. Read the status block once.
6. Register the hwdep card and node.
7. Do not reset the matrix here. Userspace performs a full sync after it reads
   the workspace scene.

If the interface is already claimed, return `-EBUSY`. If the device answers
but the endpoints are missing, fail the probe with a clear log line.

### 3.3 Hwdep ABI

Mirror the style of `kernel/ffuc-uapi.h`. Proposed types:

```c
#define FFDF_ABI_VERSION 1

struct ffdf_hwdep_info {
    __u32 abi;
    __u32 inputs;         /* 32 at 1x */
    __u32 playback;       /* 34 at 1x */
    __u32 outputs;        /* 34 at 1x */
    __u32 nodes;          /* 2048 */
    __u32 meter_bytes;    /* 1024 */
    __u32 rate;           /* current rate from status */
    __u32 mixer_enabled;
    __u32 card;
    char  name[32];
    __u32 reserved[8];
};

struct ffdf_hwdep_vendor {
    __u8  request;        /* 16, 17, 18, 21, 22 */
    __u8  direction;      /* 0 out, 1 in */
    __u16 value;
    __u16 index;
    __u16 length;         /* 16 for request 17, else 0 */
    __u8  data[16];
};

struct ffdf_hwdep_words {
    __u32 count;
    __u32 reserved;
    __u32 words[512];     /* matrix command words */
};
```

Ioctls:

| Ioctl | Direction | Meaning |
|---|---|---|
| `FFDF_HWDEP_IOCTL_INFO` | read | Capabilities and current state |
| `FFDF_HWDEP_IOCTL_VENDOR` | read/write | One whitelisted control request |
| `FFDF_HWDEP_IOCTL_WORDS` | write | One list of matrix command words |
| `FFDF_HWDEP_IOCTL_SET_MIXER` | write | Enable or disable the mixer |

`read(2)` on the hwdep node returns one 1024-byte meter frame. A blocking read
waits up to 100 ms for a frame and returns `-EAGAIN` on a timeout. The frame
type is in the marker word, so the header is not needed.

Whitelist the vendor requests. Do not expose a generic vendor pipe: request
16 and 18 can change clock, rate, format and the mixer enable, and an
unvalidated value can leave the unit in an unusable state.

### 3.4 Transfers

- Control OUT: `usb_control_msg` with request type `0x40`.
- Control IN: request type `0xc0`, 16 bytes.
- Matrix OUT: `usb_bulk_msg` on `usb_sndbulkpipe(dev, 0x0b)`, 4-byte words,
  chunked at 4 KiB.
- Meters IN: `usb_bulk_msg` on `usb_rcvbulkpipe(dev, 0x84)`, 1024 bytes.

Use a mutex for control and bulk OUT. Do not hold it during a meter read.

The meter endpoint always has a frame ready according to prior art. Still
handle a short transfer and a timeout.

### 3.5 Validation

- Words: node < 2048, destination < 64, source < `0x200`, gain any 16-bit
  value.
- Vendor request: only the whitelist, only the documented length, index and
  value ranges per request.
- Rate writes: reject until the streaming question is answered. Expose the
  rate as read-only.
- Mixer enable: accept only 0 or 1.

### 3.6 Lifecycle

- Disconnect: mark the device dead, wake blocked readers, free the card with
  `snd_card_free_when_closed`.
- Suspend: keep the state; on resume re-enable the mixer and re-read status.
  Userspace re-syncs because the device may have lost its matrix.
- Module unload: release interface 1, free the card.
- Do not write the mixer on unload.

## 4. Userspace backend

Proposed files:

| File | Purpose |
|---|---|
| `totalcontrol-core/src/digiface/abi.rs` | Rust mirror of the UAPI |
| `totalcontrol-core/src/digiface/mod.rs` | `RmeDevice` implementation |
| `totalcontrol-core/src/digiface/protocol.rs` | Gain, status and meter codecs |
| `totalcontrol-core/src/profiles/digiface.rs` | Profile |
| `totalcontrol-core/src/profile.rs` | `ProfileId::Digiface`, `MeterLayout::Digiface` |
| `totalcontrol-gui/src/app.rs` | `DeviceKind::Digiface`, `DeviceHandle::Digiface` |
| `totalcontrol-gui/src/device_service.rs` | command dispatch |
| `docs/spec/DIGIFACE-USB-PROTOCOL-SPEC.md` | specification |

### 4.1 Opening

- Scan `/sys/class/sound/card*/id` for the control card id.
- Open `/dev/snd/hwC<card>D0`.
- Read `FFDF_HWDEP_IOCTL_INFO`.
- Build the channel model from the profile.
- Confirm the reported inputs, outputs, playback and meter size.
- If the control card is absent but an ALSA card with the Digiface product
  name exists, report "Digiface USB (no mixer interface)".

### 4.2 Read policy

The device has no full state readback. It reports status and meters only.
Therefore:

- volume, pan, mute, solo, routing and trim are `ReadPolicy::Host`;
- clock source, output format and rate are read from the status block;
- a full scene is written after connect;
- a device reset or reconnect triggers a full rewrite.

### 4.3 Write path

The core model has a source × output-pair matrix with a volume and pan per
cell, plus output faders.

Phase 1, pair model:

- `set_volume(channel, pair, volume)` and `set_pan(channel, pair, pan)`
  combine into two node gains, left and right.
- Recompute both nodes on either call, so the pair stays consistent.
- Extract the existing equal-power `pan_split` helper from
  `fireface/device.rs` into a shared module. Do not duplicate the law.
- `set_matrix_cell` is not exposed; `matrix_cells_supported()` stays false.

Phase 2, mono cells:

- Implement `matrix_cells_supported() -> true`.
- `matrix_blocks_for_pair` is the identity.
- Each cell is one node.
- Pair writes write the two natural cells.
- The core already carries the override and fold logic.

Node allocation:

```text
set cell (src, dst, gain):
    node = node_for(src, dst)
    if gain == 0:
        if node exists: send(node << 16); free(node)
    else:
        if node missing:
            node = alloc()
            send(0x40000000 | node << 16 | dst << 9 | src)
        send(node << 16 | encode_gain(gain))
```

Batch words and flush in chunks. A scene restore produces thousands of words;
one bulk transfer per cell would be slow. A command buffer that flushes at 4
KiB, or after a short timer, keeps both single fader moves and scene restores
responsive.

Output faders:

- `set_output_volume(pair, which, volume)` writes request 21 with
  `0x100 + channel`.
- `set_output_mute` writes the encoded zero gain.
- The output fader is independent of the matrix nodes.

Mute and solo:

- Input or playback mute: set every node of that source to zero and free the
  nodes. Keep the model gain so unmute can rebuild.
- Solo is resolved by the core model before the backend sees the levels; the
  backend writes the resulting gains.
- Output mute: request 21 with zero.

Trim:

- Trim multiplies the node gain. The core model already folds trim into the
  send level; do not add a second gain stage.

Loopback:

- Do not implement until the capture resolves request 22. Expose the control
  as unavailable, not as a host-side approximation.

Snapshots:

- Store TotalControl snapshots on the host. Do not claim device-stored
  snapshots.
- On recall, rewrite the whole matrix and output faders.

### 4.4 Meters

- `read()` returns one 1024-byte frame.
- Decode the marker to get the bus.
- Fill the 34 channels for that bus.
- Feed the existing meter pipeline.
- Read at the GUI meter rate, not in a busy loop.
- If a read times out, keep the previous values and retry next tick.

Add `MeterLayout::Digiface` with three buses and 34 channels. Add a decode
function with unit tests built from synthetic frames, not captured state.

### 4.5 Status polling

Poll the 16-byte status at a low rate, for example once per second, and on
demand after a clock or format change. Update:

- current rate;
- selected and current clock source;
- input lock, sync and format;
- output format;
- mixer-enabled state.

Do not poll faster than needed. Status is not a meter path.

## 5. Profile

```rust
pub const PROFILE: DeviceProfile = DeviceProfile {
    id: ProfileId::Digiface,
    model_name: "Digiface USB",
    channels: &CHANNELS,
    routing: RoutingMode::Pair,
    input_ops: INPUT_OPS,
    playback_ops: PLAYBACK_OPS,
    output_ops: OUTPUT_OPS,
    globals: GLOBALS,
    clock_sources: &["Internal", "Optical 1", "Optical 2", "Optical 3", "Optical 4"],
    preamps: &[],
    meters: MeterSpec {
        layout: MeterLayout::Digiface,
        hardware: true,
        playback: true,
    },
    layout: LayoutSpec { template: "no-fx" },
    requires: Requirements {
        min_inputs: 8,
        min_outputs: 10,
        min_playback: 10,
        meter_len: Some(1024),
    },
    phase_without_eq: false,
    channel_width: false,
    dsp: false,
};
```

Channel table:

- 32 inputs, all `ChannelType::ADAT`. The S/PDIF pair aliases the first two
  channels of each optical port.
- 17 output pairs: four ADAT octets and `PH L`/`PH R`.
- 17 playback pairs: `Playback 1/2` to `Playback 33/34`.
- Identity output order.

Operations:

| Row | Operations |
|---|---|
| Input | Send, Pan, Mute, Solo, Trim, Name |
| Playback | Send, Pan, Mute, Solo, Trim, Name |
| Output | Volume, Mute, Name |

Loopback is added to inputs only after request 22 is captured. Phase is added
only after a capture shows the write. There are no preamps and no FX.

Globals:

- Clock source.
- Sample rate, read-only at first.
- Four optical output format controls.
- Talkback, Dim, Speaker B, Mono.

The current `GlobalControl::OpticalFormat` is a single boolean. It cannot
represent four ports. Add one variant per port, for example
`Optical1Format` to `Optical4Format`, and map each to register 2. Do not
flatten four ports into one control.

## 6. GUI and device selection

- Add `DeviceKind::Digiface` and `DeviceHandle::Digiface`.
- Add the target to `available_targets()` when the control card exists.
- Show one device entry: the model name, the audio card and the control card.
- If only the audio card exists, show "Digiface USB (no mixer interface)" and
  do not open a backend.
- MIDI remotes: the Digiface has no device MIDI ports. The Mackie page shows
  no ports; the ARC page is independent and still works.

## 7. Tests

Unit tests:

- gain encode and decode at 0, 0.25, 0.5, 1.0 and 2.0;
- command word construction for gain, route, reset and silence;
- node allocation and reuse;
- status decode for every field;
- meter frame decode for all three types, using synthetic frames;
- profile requirements and channel counts;
- scene round-trip with host-authoritative values.

Hardware tests:

- open, enable the mixer, write one input-to-output route and hear it;
- move one output fader and measure the level;
- mute, solo, pan hard left and hard right;
- change one optical output to S/PDIF and back;
- change the clock source with a valid optical clock;
- recall a host snapshot and verify the matrix;
- read meters for each bus;
- unplug and reconnect, then verify the full rewrite.

Record firmware, speed mode and test conditions outside the repository.
Leave the unit on the user's scene.

## 8. Specification and documentation

Add `docs/spec/DIGIFACE-USB-PROTOCOL-SPEC.md`:

- terms;
- modes and host binding;
- the mixer interface and endpoints;
- control registers and status;
- matrix words and gain law;
- meters;
- channel topology per speed;
- clock and format;
- open items.

Update:

- `docs/CONTRIBUTING-NEW-DEVICE.md` only if the two-card model needs a policy
  sentence;
- `docs/spec/TOTALCONTROL-DEVICE-PROFILE-SPEC.md` for the profile;
- `docs/spec/TOTALCONTROL-STATE-SPEC.md` for any scene compatibility rule;
- `docs/spec/TOTALCONTROL-GUI-SPEC.md` for the device entry and the four
  format controls;
- `docs/TOTALCONTROL-Manual.md` for the user-facing how-to.

## 9. Milestones

1. Captures confirm the protocol.
2. Userspace prototype reads status and meters, writes one route and one
   fader.
3. Kernel module with info, vendor, words and meter read.
4. Rust backend with pair model and output faders.
5. Profile, GUI entry and scene write.
6. Clock, format and status.
7. Mono cells.
8. Loopback and snapshots, if the captures support them.

Do not skip from step 1 to step 3 without a capture. The prior art is a
starting point, not evidence.

## 10. Open decisions

| Status | Item |
|---|---|
| open | Control card name and id |
| open | Whether the hwdep read returns raw frames or a header plus frame |
| open | Whether rate writes are allowed while streaming |
| open | Pair model first, or mono cells from the start |
| open | How to show the audio card and the control card as one device |
| open | Whether the 1x channel table stays fixed at 2x and 4x |
