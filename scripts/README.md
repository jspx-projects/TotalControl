# Fireface transport lab scripts

Development tools for capturing and decoding the Fireface AP/ISO USB
transport, and for watching the driver's transport counters. They are used
to verify the streaming protocol against the device and to diagnose
low-latency behaviour. They need root (usbmon / debugfs) and a connected
Fireface; run them with `sudo`.

Captures and logs are written outside the repository (under
`~/Documents/fb-telemetry/` by default). Do not commit captured device
state.

## ap-iso-capture.sh — record AP/ISO USB traffic

Captures the Fireface's USB bus with `tcpdump -i usbmonN` while audio plays,
producing a pcap that contains the ISO IN headers and OUT lead words.

```sh
sudo bash scripts/ap-iso-capture.sh [seconds] [outfile.pcap]
```

- `seconds` — capture window (default 15). Play audio during the window.
- `outfile.pcap` — output path (default `~/Documents/fb-telemetry/ap-iso-<ts>.pcap`).

The script finds the Fireface card by its ALSA id and derives the bus number
(no hardcoded bus/port). Route the audio to the Fireface sink first if it is
not the default output.

## ap-iso-analyze.py — decode a capture

Decodes a usbmon pcap (linktype 220) and reports the streaming fields. It
auto-detects the OUT frame stride (channel count) and the sample format
(float32 for ISO, int32 for the interrupt transport), so it works across
models and rates without flags.

```sh
python3 scripts/ap-iso-analyze.py capture.pcap --inspect        # per-endpoint layout
python3 scripts/ap-iso-analyze.py capture.pcap [--audio]        # continuity + audio purity
```

Options:

- `--ep-out`, `--ep-in` — the audio endpoints. Defaults are the UCX ISO
  endpoints (`0x0e` OUT, `0x8f` IN). UC Mac ISO is `0x01` / `0x82`;
  Babyface-classic is `0x03` / `0x84`. Use `--inspect` first to confirm.
- `--stride` — OUT frame stride in bytes (0 = auto).
- `--fmt` — sample format: `f` (float32), `i` (int32), `auto` (default).
- `--audio` — also check the OUT audio for clicks/dropouts.

What it reports:

- **OUT lead words** — the per-frame ring position. A healthy stream advances
  by exactly 1 per frame modulo the ring; the tool counts steps that are not
  `+1` under mod 8192 and mod 3072, which identifies the ring modulus.
- **IN position** — the device's free-running counter. It checks that each
  position equals the previous position plus the frame count (mod 8192) and
  reports the maximum value reached (a 13-bit field reaches 8191).
- **IN phase** — the fractional-sample accumulator range.
- **OUT audio** (`--audio`) — full-scale jumps (clicks) and runs of identical
  samples (dropouts). A clean tone has zero of both.

## fb-telemetry-monitor.sh — watch transport counters

Samples the driver's debugfs `stats` counters and appends recent driver
kernel lines, so audible ticks can be correlated with counter deltas.

```sh
sudo bash scripts/fb-telemetry-monitor.sh [seconds] [outfile]
```

Enable the driver's dynamic debug first so underruns carry the appl/hw
pointers:

```sh
sudo sh -c 'echo "module snd_fireface_uc +p" > /sys/kernel/debug/dynamic_debug/control'
```

Read the deltas afterwards: `out_underruns` = the host ring ran dry (the
tick); `in_pos_gap` = a feedback position discontinuity; `frames_mismatch` =
header count differs from the packet count. Kernel lines are timestamped;
correlate by time.

## Typical workflow

```sh
# 1. play a tone routed to the Fireface, then capture:
sudo bash scripts/ap-iso-capture.sh 12
# 2. confirm the endpoints, then decode:
python3 scripts/ap-iso-analyze.py ~/Documents/fb-telemetry/ap-iso-<ts>.pcap --inspect
python3 scripts/ap-iso-analyze.py ~/Documents/fb-telemetry/ap-iso-<ts>.pcap --audio
```
