#!/usr/bin/env python3
"""RTL analysis: correlate the captured input-5 stream against the chirp.

Companion to fireface-rtl-probe.c / fireface-rtl-measure.sh; see
RTL-MEASUREMENT.md. Measured invariants (linked duplex start, common device
clock):
  D = C - P: the chirp's capture-ring position minus its playback-ring
  position. The capture ring's first delivered frame was sampled up to
  one capture-pipeline earlier (the running gate discards whole URBs,
  not pre-trigger samples within the first running URB), so
  D = A + B +/- 1 URB, with A = playback consumption->DAC and
  B = ADC->capture delivery. D is exact (frame accounting).
  RTL = write->read host time = fill + A + B (+/- ~1 period of
  timestamp granularity).

The A/B split is model-dependent: the horizon split (A ~ bus delivery +
converter) is checked against the held-lead split (A = out_lead + URB/2),
which forces B below the capture in-flight + converter floor at 1x.

Usage: o1-analyze.py <outdir>
"""
import sys
import numpy as np

d = sys.argv[1] if len(sys.argv) > 1 else "."
meta = {}
Ps, tws, fills = [], [], []
for line in open(f"{d}/meta.txt"):
    t = line.split()
    if t[0] == "P":
        Ps.append(int(t[1])); tws.append(float(t[3])); fills.append(int(t[5]))
    else:
        meta[t[0]] = int(t[1])
rate, L = meta["rate"], meta["L"]
cap = np.fromfile(f"{d}/cap_ch4.f32", dtype=np.float32)
chirp = np.fromfile(f"{d}/chirp.f32", dtype=np.float32).reshape(-1, 2)[:, 0]
chunks = [tuple(map(float, l.split())) for l in open(f"{d}/capchunks.txt")]

if np.max(np.abs(cap)) < 1e-4:
    print("NO SIGNAL on input 5 — check the cable and the AN1/2 -> out5 routing.")
    sys.exit(1)

def t_at(pos):
    for t, p, n in chunks:
        if p <= pos < p + n:
            return t + (pos - p) / rate
    return None

print(f"rate={rate} L={L} cap={len(cap)} frames, peak={np.max(np.abs(cap)):.3f}")
print(f"{'rep':>3} {'P':>9} {'C':>9} {'D=C-P':>7} {'D ms':>7} "
      f"{'fill*':>6} {'RTL ms':>8} {'corr':>6}")
Ds, rtls, fills_l = [], [], []
for k, P in enumerate(Ps):
    lo = max(0, P - rate // 2)
    hi = min(len(cap) - L, P + rate)
    if hi <= lo:
        print(f"{k:>3} window out of range"); continue
    seg = cap[lo:hi + L].astype(np.float64)
    ref = chirp.astype(np.float64)
    # FFT cross-correlation of ref against seg
    nfft = 1 << (len(seg) + len(ref)).bit_length()
    corr = np.fft.irfft(np.fft.rfft(seg, nfft) * np.conj(np.fft.rfft(ref, nfft)), nfft)
    peak = int(np.argmax(corr[:hi - lo + 1]))
    C = lo + peak
    D = C - P
    norm = corr[peak] / (np.linalg.norm(ref) * np.linalg.norm(seg[peak:peak + L]) + 1e-12)
    td = t_at(C)
    rtl = (td - tws[k]) * 1000 if td else float("nan")
    Ds.append(D); rtls.append(rtl); fills_l.append(fills[k])
    print(f"{k:>3} {P:>9} {C:>9} {D:>7} {D/rate*1000:>7.2f} "
          f"{fills[k]:>6} {rtl:>8.2f} {norm:>6.3f}")

if Ds:
    da = np.array(Ds)
    # Model split (inference): playback A ~ bus delivery (mean 12.5
    # microframes) + converter; capture B = D - A (+/- 1 URB phase).
    a_model = 12.5 * rate / 8000 + 13
    b_model = da.mean() - a_model
    print(f"\nD = C-P (device round-trip span): mean {da.mean():.0f} frames = "
          f"{da.mean()/rate*1000:.2f} ms (min {da.min():.0f}, max {da.max():.0f}) — exact")
    print(f"RTL write->read: mean {np.nanmean(rtls):.2f} ms "
          f"(~1-period timestamp granularity)")
    print(f"*fill from snd_pcm_delay: reads low by the inflight bug (O9); "
          f"true JIT fill ~1-2 periods")
    print(f"Horizon split [inf]: A (hw_ptr->DAC) ~ {a_model:.0f} frames = "
          f"{a_model/rate*1000:.2f} ms; B (ADC->delivery) ~ {b_model:.0f} frames = "
          f"{b_model/rate*1000:.2f} ms (+/- 1 URB)")
    print(f"Held-lead split would need A = out_lead + URB/2 = "
          f"{round(24*rate/8000)+60 + rate/2000:.0f} frames; then B = D - A = "
          f"{da.mean() - (round(24*rate/8000)+60 + rate/2000):.0f} frames "
          f"(B < IN in-flight + converter ~ {12.5*rate/8000+13:.0f} refutes it at 1x)")
    with open(f"{d}/results.txt", "w") as f:
        f.write(f"D_mean_frames {da.mean():.1f}\nD_ms {da.mean()/rate*1000:.3f}\n"
                f"D_min {da.min():.0f}\nD_max {da.max():.0f}\n"
                f"RTL_ms {np.nanmean(rtls):.3f}\n"
                f"A_model_ms {a_model/rate*1000:.3f}\nB_model_ms {b_model/rate*1000:.3f}\n")
