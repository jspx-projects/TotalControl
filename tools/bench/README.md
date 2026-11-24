# Fireface bench tooling

Reusable verification and measurement tools for the Fireface USB driver and the
host audio path. They reproduce the device/host clock interactions and measure
latency, fill, rate conduction and recovery without a GUI rebuild cycle. The
method: model the interaction with these tools, verify every prediction, then
align the product code.

Measurement **data** (captures, CSVs, logs) is not committed — keep it outside
the repository. The tools write their output to a directory you choose (default
the working directory).

## Design

- **Device-agnostic generic layer.** Graph control, stream playback,
  fill/state/xrun sampling and rate conduction are parameterized by card
  (`--card N`, or `CARD=N` for the shell scripts). PipeWire nodes are discovered
  by their `alsa.card` property, so the tools work with multiple devices.
- **Device-aware protocol layer.** The clock/register writes dispatch per model
  (`clock-write.c` detects the model from the card), so a new device extends in
  one place. Protocol layouts follow `docs/spec/`.

The default hwdep node is `/dev/snd/hwC0D0`; pass any `/dev/snd/hwC*D*` to target
another card.

## Tools

### PipeWire graph lab
| Tool | Purpose |
|---|---|
| `pw-lab.py` | `run` — init knobs + clock forces + optional capture re-prepare + one controlled stream + fill/xrun sampling; mid-run `--poke-*` (vendor word) and `--pitch-at/--pitch/--pitch0-at` (pitch clock) split the xrun counters per write. `matrix` — Auto-mode rate conduction, a new `pw-play` client per track, PASS/FAIL per step. `watch` — read-only fast event timeline (forces, live scheduling props, PCM rate/period, `rate_meas`, `run_seq`, fill/xruns). `run` is the default subcommand. |
| `pw-stream-rate.c` | one libpipewire playback stream that renegotiates its rate mid-run via `pw_stream_update_params` (the gapless same-client path). Demonstrates that a running PipeWire driver keeps its rate and resamples a same-client rate change rather than re-negotiating. |

### Device clock writes
| Tool | Purpose |
|---|---|
| `clock-write.c` | `vendor <req> <val> <idx>` — one vendor word (generic). `pitch <percent>` — the four-word DDS pitch quad (UC/UCX; Babyface-classic reports unsupported). `family <rate>` — the base-rate family word (`0x10`, `family<<4`, `0x30`). Each prints `rate_meas` before/after. |

### Device diagnostics (hwdep)
| Tool | Purpose |
|---|---|
| `ffuc-diag.c` | full hwdep counters + achievable-envelope dump. |
| `ffuc-0x19-probe.c` | read the readiness/status word (`0x19`) and `0x11`. |
| `rdopts.c` | read `0x19` / `0x11` / `0x10` words once. |
| `burst19.c`, `burst19r.c` | fast timestamped `0x19` sampling; `burst19r` reopens across a USB reset. |
| `ffuc-xrun-probe.c` | host-ring xrun + clock-diagnostic counters. |

### Host / ALSA measurement
| Tool | Purpose |
|---|---|
| `cpu-dma-latency.c` | hold a PM-QoS `CPU_DMA_LATENCY` constraint while running. |
| `alsa-fill-probe.c` | just-in-time ALSA playback filler holding a constant target ring fill. |
| `measure.py` | playback ring fill + PCM state + host-ring xrun sampler. |
| `watch-state.py` | read-only state monitor (forces, PCM, `rate_meas`, fill, xruns, live scheduling mode). |
| `proc-sampler.py` | tight `/proc/asound` playback status sampler to CSV. |
| `ringtrace-analyze.py` | reconstruct the ring-fill trajectory from a driver ringtrace dump. |

### Soak / load / recovery
| Tool | Purpose |
|---|---|
| `underrun-soak.sh` | per-second counters + refill-thread scheduling latency; dumps the ring trace after each underrun. |
| `io-load.sh` | baloo-like IO pressure generator (page cache + disk reads). |
| `xrun-test.sh` | freeze the PipeWire graph briefly, measure grace/XRUN behaviour and auto-recovery time. |

### RTL / loopback measurement
| Tool | Purpose |
|---|---|
| `fireface-rtl-probe.c` | physical round-trip frame accounting via raw ALSA linked duplex. |
| `fireface-rtl-measure.sh` | multi-device RTL measurement: stop the stack, save the scene, route, measure, restore. |
| `fireface-rtl-analyze.py` | correlate the captured stream against the chirp; derive the device span. |
| `fireface-scene-ctl/` | multi-device scene control (list / save / route / restore) used by the RTL measurement. |
| `loopback-measure.sh` | reproducible UCX loopback measurement using `fireface-hwdep` + `alsa-latency-probe` (in `tools/`). |

Read `RTL-MEASUREMENT.md` (method, operating point, pitfalls) and
`CHANNEL-NUMBERING.md` (routing) before running the RTL/loopback tools.

## Build

C tools build against the checkout's kernel headers. From this directory:

```sh
R=$(git rev-parse --show-toplevel)
gcc -O2 -I"$R/kernel" -o ffuc-diag        ffuc-diag.c
gcc -O2 -I"$R/kernel" -o ffuc-0x19-probe  ffuc-0x19-probe.c
gcc -O2 -I"$R/kernel" -o rdopts           rdopts.c
gcc -O2 -I"$R/kernel" -o burst19          burst19.c
gcc -O2 -I"$R/kernel" -o burst19r         burst19r.c
gcc -O2 -I"$R/kernel" -o ffuc-xrun-probe  ffuc-xrun-probe.c
gcc -O2 -I"$R/kernel" -o cpu-dma-latency  cpu-dma-latency.c
gcc -O2 -I"$R/kernel" -o clock-write      clock-write.c -lm
gcc -O2 -I"$R/kernel" -o alsa-fill-probe  alsa-fill-probe.c -lasound
gcc -O2 -I"$R/kernel" -o fireface-rtl-probe fireface-rtl-probe.c -lasound -lm
gcc -O2 -o pw-stream-rate pw-stream-rate.c \
    $(pkg-config --cflags --libs libpipewire-0.3 libspa-0.2)
(cd fireface-scene-ctl && cargo build --release)
```

Runtime dependencies: the PipeWire CLI tools (`pw-dump`, `pw-cli`,
`pw-metadata`, `pw-play`, `pw-record`), `alsa-lib` for the ALSA probes, and
`libpipewire-0.3` for `pw-stream-rate`. The shell scripts use the lab helpers
(`ffuc-lab`) and `pactl` where noted; see `docs/DEVELOPMENT-TOOLS.md`.

## Measurement trap

`pw-dump info.props` shows the configured node defaults, **not** the live
`Props` param override. Read the live scheduling mode with
`pw-cli enum-params <id> Props` or the PCM geometry (IRQ prepares
`period = quantum`; timer/batch prepares `period = quantum / 2`).