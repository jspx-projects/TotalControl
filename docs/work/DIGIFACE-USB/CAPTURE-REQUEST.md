# Digiface USB capture request

Working note, not part of the specification. This is the evidence needed
before a Digiface mixer backend can be specified. It is written so it can be
shared with a device owner as-is.

## 1. What we need and why

The Digiface USB has two modes:

- normal mode, with a vendor-specific audio interface and a separate vendor
  interface that carries the TotalMix mixer;
- CC mode, which is class compliant.

The internal mixer is hardware of the unit and runs in both modes. CC mode has
no documented mixer-control interface for a third-party host, while normal mode
has one that public reverse engineering describes.

Mainline Linux supports audio in normal mode and exposes clock and format
controls. The mixer protocol below comes from public reverse engineering. It is
a starting point, not proof. Two captures decide the backend: the CC-mode
interface map, and the normal-mode mixer traffic.

## 2. Before capturing

1. Save the current TotalMix workspace and the interface's current scene.
2. Capture CC mode first, then normal mode. The firmware update tool shows the
   mode; §3.1 is the CC-mode capture.
3. Use a Linux host. TotalMix FX runs on Windows or macOS while its traffic is
   captured with `usbmon` where the unit is attached, or through a USB analysis
   tool on that host.
4. Record the firmware version and the mode.
5. Use a simple test scene, not a production scene. One input routed to one
   output is enough.

Keep these out of shared files:

- serial numbers;
- USB vendor and product IDs, if the file is published;
- hostnames, user names and home-directory paths;
- any real programme material.

The protocol needs the traffic, not the identity of the unit.

## 3. Descriptors

### 3.1 CC mode: the interface map

Flash CC mode, attach the unit to the Linux host, and capture the full
descriptor tree:

```sh
lsusb -v > digiface-cc-descriptors.txt
```

The question this answers: which interfaces and endpoints exist in CC mode, and
whether any of them can carry mixer control. Record every interface class,
alternate setting and endpoint, and whether `snd-usb-audio` bound it. This
separates a control interface that a host can drive from one that only RME's
own application drives.

Also record whether the unit exposes a USB MIDI port in CC mode. RME's iPad
application is the only known control path there, and RME states that it reaches
the mixer over USB. A MIDI port would be its transport.

### 3.1.1 CC mode: control traffic

When the unit exposes a MIDI port in CC mode, attach an iPad with TotalMix FX for
iPad and capture the traffic while the mixer changes:

1. Record the port count and the port names.
2. Idle for ten seconds.
3. Move one output fader, mute one input, route one input, recall one setup.
4. Save the raw bytes per port.

A SysEx-heavy stream on one port identifies a vendor application protocol.
Such a protocol is captured traffic, not an interface a host can claim. Its
meaning stays unknown until the messages are decoded.

### 3.2 Normal mode: the mixer interface

Flash normal mode and capture the descriptor tree:

```sh
lsusb -v > digiface-descriptors.txt
```

The interesting part is interface 1: its class, alternate settings, endpoints,
packet sizes and interval. Interface 0 is the audio interface.

Redact the serial string and the product string if they contain one.

## 4. USB traffic

### 4.1 Setup

```sh
sudo modprobe usbmon
cat /sys/kernel/debug/usb/devices
```

Find the bus and device number of the Digiface. The capture file is
`/sys/kernel/debug/usb/usbmon/<bus>u` for the whole bus, or the per-device
variant if the kernel provides it. Wireshark can read `usbmon` directly and
labels the endpoints.

Start the capture before TotalMix performs any write. Keep it running for the
whole session.

### 4.2 Session script

Wait two seconds between actions. Say the action out loud or keep a written
log with timestamps, so the capture can be aligned.

1. Idle for ten seconds with TotalMix open and no changes.
2. Move output 1's fader down by about 6 dB, then back to unity.
3. Mute output 1, then unmute it.
4. Route input 1 to output 1 at unity, then remove the route.
5. Pan input 1 hard left, then hard right, then centre.
6. Mute input 1, then unmute.
7. Solo input 1, then unsolo.
8. Change output 1's optical format from ADAT to S/PDIF, then back.
9. Change the clock source from Internal to Optical 1 and back, with a valid
   clock present on Optical 1.
10. Change the sample rate from 48 kHz to 44.1 kHz and back.
11. Enable loopback on input 1, then disable it.
12. Store snapshot 1, change two faders, recall snapshot 1.
13. Leave the unit idle for ten seconds with meters active.

If a step cannot be performed on the unit, skip it and say so.

### 4.3 Meters

Capture at least five seconds of idle meter traffic with a signal present on
one input and one output. Note the signal: sine, noise or silence, and the
level. The frame rate and the idle behaviour are both needed.

### 4.4 Control writes only

If the full capture is too large, the most valuable records are:

- every vendor control write (requests 16, 18, 21, 22) with value and index;
- every bulk OUT packet on the mixer endpoint, decoded as 32-bit
  little-endian words;
- a few bulk IN meter frames;
- the status read reply.

## 5. What each capture decides

| Capture | Question |
|---|---|
| CC-mode descriptors | Interfaces, endpoints and any control channel in CC mode |
| Normal-mode descriptors | Interface 1 endpoints, packet sizes, alternate settings |
| Output fader | Request 21 value and index, and the gain scale |
| Route add/remove | Bulk word layout, node allocation, reset |
| Pan | Whether pan is two node gains and which law |
| Mute and solo | Node zeroing versus output fader |
| Output format | Register 2 bits per port |
| Clock source | Register 1 bits and status reply |
| Sample rate | Register 1 bits, and whether a write is allowed while streaming |
| Loopback | Request 22 value and index, resolving the conflict |
| Snapshot | Whether the device stores state, and through which request |
| Meters | Frame rate, marker words, RMS and peak scaling |

## 6. What to send

- the descriptor text of CC mode and of normal mode, redacted;
- the capture file, or a trimmed capture around the control writes;
- the action log with timestamps;
- firmware version and mode;
- the sample rate and speed mode used;
- anything that behaved differently from the steps above.

Keep the capture outside any repository. A link to a file share is enough.

## 7. Questions for the device owner

1. Which application controls the Digiface in your setup?
2. Does it use normal mode, direct USB access, hwdep or an ALSA mixer?
3. Can you share its source or its protocol notes?
4. Does it keep the mixer state in the device or on the host?
5. Which firmware version is installed?
6. In CC mode, does the unit expose a MIDI port or any second interface?
