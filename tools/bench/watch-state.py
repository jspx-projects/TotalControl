#!/usr/bin/env python3
"""READ-ONLY state monitor. Writes nothing to the device/graph. Logs the facts
needed to model rate/pitch/mode bugs while the user drives the GUI.
Usage: watch-state.py [seconds]
Per line: t | force-rate/quantum | pcm rate/period | rate_meas | fill/state |
          out_underruns/in_overruns | tsched(IRQ?)
"""
import sys, time, subprocess, re, os
HERE = os.path.dirname(os.path.abspath(__file__))
HWDEP = os.path.join(HERE, "fireface-hwdep")
HP = "/proc/asound/card0/pcm0p/sub0/hw_params"
ST = "/proc/asound/card0/pcm0p/sub0/status"
dur = float(sys.argv[1]) if len(sys.argv) > 1 else 900.0

def sh(args, timeout=3):
    try:
        return subprocess.run(args, capture_output=True, text=True, timeout=timeout).stdout
    except Exception:
        return ""

def forces():
    out = sh(["pw-metadata", "-n", "settings", "0"])
    g = lambda k: (re.search(rf"key:'{k}' value:'(\d+)'", out) or [None, "?"])[1]
    return g("clock.force-rate"), g("clock.force-quantum")

def hwparams():
    try:
        t = open(HP).read()
        if t.strip() == "closed":
            return "closed", "-", "-"
        g = lambda k: (re.search(rf"^{k}:\s*(\d+)", t, re.M) or [None, "?"])[1]
        return g("rate"), g("period_size"), g("buffer_size")
    except Exception:
        return "?", "?", "?"

def fill_state():
    try:
        t = open(ST).read()
        if t.strip() == "closed":
            return 0, "CLOSED"
        s = re.search(r"^state:\s*(\S+)", t, re.M).group(1)
        hw = int(re.search(r"^hw_ptr\s*:\s*(\d+)", t, re.M).group(1))
        ap = int(re.search(r"^appl_ptr\s*:\s*(\d+)", t, re.M).group(1))
        return ap - hw, s
    except Exception:
        return 0, "?"

def counters():
    out = sh([HWDEP, "counters"])
    g = lambda k: (re.search(rf"^{k}=(\d+)", out, re.M) or [None, "?"])[1]
    return g("out_underruns"), g("in_overruns")

def rate_meas():
    out = sh([HWDEP, "clock"])
    m = re.search(r"rate_meas (\d+)", out)
    return m.group(1) if m else "?"

def out_node():
    out = sh(["pw-dump"], timeout=5)
    import json
    try:
        for o in json.loads(out):
            n = o.get("info", {}).get("props", {}).get("node.name", "")
            if "pro-output-0" in n and ".monitor" not in n:
                return o["id"]
    except Exception:
        pass
    return None

def tsched(nid):
    if nid is None:
        return "?"
    out = sh(["pw-cli", "enum-params", str(nid), "Props"])
    m = re.search(r'"api\.alsa\.disable-tsched"\s*\n\s*Bool (\w+)', out)
    if m:
        return "IRQ" if m.group(1) == "true" else "timer"
    return "?"

t0 = time.time()
nid = out_node()
last_slow = 0
fr = fq = ts = "?"
while time.time() - t0 < dur:
    now = time.time() - t0
    if now - last_slow >= 2.0:
        fr, fq = forces()
        nid = out_node()
        ts = tsched(nid)
        last_slow = now
    rate, per, buf = hwparams()
    rm = rate_meas()
    f, st = fill_state()
    u, o = counters()
    print(f"{now:7.1f}s force={fr}/{fq} pcm={rate}/{per} meas={rm} fill={f:>5} {st:8} ur={u} ov={o} {ts}", flush=True)
    time.sleep(1.0)