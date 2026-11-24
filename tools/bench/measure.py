#!/usr/bin/env python3
"""Sample the Fireface playback ring fill + PCM state + host-ring xrun counters.
Local bench tool. Usage: measure.py <label> <seconds> [interval_ms]
Writes <label>.csv (t,state,delay,fill) and prints a summary + xrun deltas.
"""
import sys, time, subprocess, re, os

HERE = os.path.dirname(os.path.abspath(__file__))
HWDEP = os.path.join(HERE, "fireface-hwdep")
STATUS = "/proc/asound/card0/pcm0p/sub0/status"

def counters():
    try:
        out = subprocess.run([HWDEP, "counters"], capture_output=True, text=True, timeout=3).stdout
        d = {}
        for k in ("out_underruns", "in_overruns", "recoveries", "out_gaps", "in_gaps"):
            m = re.search(rf"^{k}=(\d+)", out, re.M)
            d[k] = int(m.group(1)) if m else None
        return d
    except Exception:
        return {}

def sample():
    try:
        txt = open(STATUS).read()
    except Exception:
        return None
    if txt.strip() == "closed":
        return ("CLOSED", 0, 0)
    st = re.search(r"^state:\s*(\S+)", txt, re.M)
    de = re.search(r"^delay\s*:\s*(\d+)", txt, re.M)
    hw = re.search(r"^hw_ptr\s*:\s*(\d+)", txt, re.M)
    ap = re.search(r"^appl_ptr\s*:\s*(\d+)", txt, re.M)
    state = st.group(1) if st else "?"
    delay = int(de.group(1)) if de else 0
    fill = (int(ap.group(1)) - int(hw.group(1))) if (hw and ap) else 0
    return (state, delay, fill)

def main():
    label = sys.argv[1]; dur = float(sys.argv[2])
    iv = (float(sys.argv[3]) / 1000.0) if len(sys.argv) > 3 else 0.02
    c0 = counters()
    t0 = time.time()
    rows = []
    while time.time() - t0 < dur:
        s = sample()
        if s:
            rows.append((time.time() - t0, *s))
        time.sleep(iv)
    c1 = counters()
    with open(label + ".csv", "w") as f:
        f.write("t,state,delay,fill\n")
        for r in rows:
            f.write(f"{r[0]:.3f},{r[1]},{r[2]},{r[3]}\n")
    fills = [r[3] for r in rows if r[1] == "RUNNING"]
    states = {}
    for r in rows:
        states[r[1]] = states.get(r[1], 0) + 1
    print(f"== {label}: {len(rows)} samples, states={states}")
    if fills:
        print(f"   fill RUNNING: min={min(fills)} max={max(fills)} mean={sum(fills)/len(fills):.0f}")
    for k in c0:
        if c0[k] is not None and c1.get(k) is not None:
            print(f"   {k}: {c0[k]} -> {c1[k]}  (delta {c1[k]-c0[k]})")

if __name__ == "__main__":
    main()