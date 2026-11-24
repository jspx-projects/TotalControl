#!/usr/bin/env python3
"""pw-lab: the PipeWire graph lab for timer/IRQ init and rate-follow verification.

One tool, three subcommands (the consolidated graph-side bench set):

  run     Init experiment: set node props + clock forces, optional capture
          re-prepare, start one controlled stream, sample fill/state/xruns.
          Optional mid-run pokes (a vendor word or a pitch-clock write) split
          the xrun counters so a clock disturbance is attributable to a write.
  matrix  Auto-mode rate conduction with a NEW pw-play client per track (the
          re-activation path). Per step: target vs hw_params vs rate_meas,
          time-to-conduct, xrun delta, PASS/FAIL.
  watch   Read-only fast event timeline: forces, live scheduling props, PCM
          rate/period, rate_meas, run_seq/recoveries, fill/xruns. Prints on
          change. Correlates a GUI action with its audio effect.

`run` is the default subcommand, so the legacy `pw-lab.py --mode timer ...`
invocation still works.

Measurement trap: pw-dump info.props shows configured defaults, NOT the live
Props param. `watch` reads the live mode via `pw-cli enum-params <id> Props`.

Read-only on the device except the graph knobs and the optional run pokes.
"""
import argparse, subprocess, time, os, re, sys, wave, array, math, json

HERE = os.path.dirname(os.path.abspath(__file__))
HWDEP = os.path.join(HERE, "fireface-hwdep")
CLOCK_WRITE = os.path.join(HERE, "clock-write")
# Device context (device-agnostic generic layer). resolve_device(card) sets these
# so every generic function works on any card; node discovery matches alsa.card.
CARD = 0
HWDEP_NODE = "/dev/snd/hwC0D0"
HP = "/proc/asound/card0/pcm0p/sub0/hw_params"
ST = "/proc/asound/card0/pcm0p/sub0/status"
# Regenerable tone cache (persistent across reboots, unlike /tmp).
TONE_DIR = os.path.join(os.environ.get("XDG_CACHE_HOME",
                                       os.path.join(os.path.expanduser("~"), ".cache")),
                        "pw-lab")

def resolve_device(card):
    global CARD, HWDEP_NODE, HP, ST
    CARD = card
    HWDEP_NODE = f"/dev/snd/hwC{card}D0"
    HP = f"/proc/asound/card{card}/pcm0p/sub0/hw_params"
    ST = f"/proc/asound/card{card}/pcm0p/sub0/status"

def sh(args, timeout=8):
    return subprocess.run(args, capture_output=True, text=True, timeout=timeout)

def _card_of(props):
    for k in ("alsa.card", "api.alsa.pcm.card", "api.alsa.card"):
        v = props.get(k)
        if v is not None:
            try: return int(v)
            except (ValueError, TypeError): pass
    return None

def nodes():
    out = {}
    for o in json.loads(sh(["pw-dump"]).stdout):
        p = o.get("info", {}).get("props", {})
        n = p.get("node.name", "")
        if "pro-" in n and ".monitor" not in n and _card_of(p) == CARD:
            if "pro-output-0" in n: out["out"] = (o["id"], n)
            if "pro-input-0" in n: out["inp"] = (o["id"], n)
    return out

def out_node():
    for o in json.loads(sh(["pw-dump"], timeout=5).stdout):
        p = o.get("info", {}).get("props", {})
        n = p.get("node.name", "")
        if "pro-output-0" in n and ".monitor" not in n and _card_of(p) == CARD:
            return o["id"]
    return None

def set_policy(nid, mode, headroom, start_delay, period_num):
    tsched = "true" if mode == "irq" else "false"
    pod = ('{ "params": [ "api.alsa.disable-tsched", %s, "api.alsa.headroom", %d, '
           '"api.alsa.start-delay", %d, "api.alsa.period-num", %d ] }'
           % (tsched, headroom, start_delay, period_num))
    sh(["pw-cli", "set-param", str(nid), "Props", pod])

def forces(rate, quantum):
    sh(["pw-metadata", "-n", "settings", "0", "clock.force-rate", str(rate)])
    sh(["pw-metadata", "-n", "settings", "0", "clock.force-quantum", str(quantum)])

def capture_reprepare(source, rate, quantum):
    p = subprocess.Popen(["pw-record", "--rate", str(rate), "--target", source, "/dev/null"],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.2)
    sh(["pw-metadata", "-n", "settings", "0", "clock.force-quantum", str(quantum - 1)])
    time.sleep(0.5)
    sh(["pw-metadata", "-n", "settings", "0", "clock.force-quantum", str(quantum)])
    time.sleep(1.5)
    p.terminate(); p.wait()

def tone(path, rate, secs=30):
    os.makedirs(TONE_DIR, exist_ok=True)
    if os.path.exists(path): return
    n = rate * secs
    w = wave.open(path, "w"); w.setnchannels(2); w.setsampwidth(2); w.setframerate(rate)
    a = array.array("h", [int(12000 * math.sin(2 * math.pi * 440 * i / rate)) for i in range(n)])
    st = array.array("h", [0] * (2 * n))
    for i in range(n): st[2 * i] = a[i]; st[2 * i + 1] = a[i]
    w.writeframes(st.tobytes()); w.close()

def counters():
    out = sh([HWDEP, "counters"]).stdout
    g = lambda k: int(re.search(rf"^{k}=(\d+)", out, re.M).group(1))
    return g("out_underruns"), g("in_overruns")

def counters_full():
    out = sh([HWDEP, "counters"]).stdout
    g = lambda k: (re.search(rf"^{k}=(\d+)", out, re.M) or [None, "?"])[1]
    return g("out_underruns"), g("in_overruns"), g("recoveries"), g("run_seq")

def sample():
    try: t = open(ST).read()
    except Exception: return ("?", 0)
    if t.strip() == "closed": return ("CLOSED", 0)
    s = re.search(r"^state:\s*(\S+)", t, re.M).group(1)
    hw = int(re.search(r"^hw_ptr\s*:\s*(\d+)", t, re.M).group(1))
    ap = int(re.search(r"^appl_ptr\s*:\s*(\d+)", t, re.M).group(1))
    return (s, ap - hw)

def hwparams():
    try:
        t = open(HP).read()
        if t.strip() == "closed": return "closed", "-", "-"
        g = lambda k: (re.search(rf"^{k}:\s*(\d+)", t, re.M) or [None, "?"])[1]
        return g("rate"), g("period_size"), g("buffer_size")
    except Exception:
        return "?", "?", "?"

def hw_rate():
    r = hwparams()[0]
    try: return int(r)
    except Exception: return 0

def rate_meas():
    m = re.search(r"rate_meas (\d+)", sh([HWDEP, "clock"]).stdout)
    return int(m.group(1)) if m else 0

def live_props(nid):
    out = sh(["pw-cli", "enum-params", str(nid), "Props"]).stdout
    def gi(k):
        m = re.search(rf'"{k}"\s*\n\s*Int (-?\d+)', out); return m.group(1) if m else "?"
    def gb(k):
        m = re.search(rf'"{k}"\s*\n\s*Bool (\w+)', out); return m.group(1) if m else "?"
    return ("IRQ" if gb("api.alsa.disable-tsched") == "true" else "timer",
            gi("api.alsa.headroom"), gi("api.alsa.start-delay"), gi("api.alsa.period-num"))

def mode_defaults(a):
    if a.mode == "irq":
        headroom = a.headroom if a.headroom is not None else 2 * a.quantum
        start_delay = a.start_delay if a.start_delay is not None else 0
        period_num = a.period_num if a.period_num is not None else 32
    else:
        headroom = a.headroom if a.headroom is not None else 336
        start_delay = a.start_delay if a.start_delay is not None else 288
        period_num = a.period_num if a.period_num is not None else 16
    force_rate = a.force_rate if a.force_rate is not None else (a.rate if a.mode == "irq" else 0)
    return headroom, start_delay, period_num, force_rate

# ── run ──────────────────────────────────────────────────────────────
def cmd_run(a):
    headroom, start_delay, period_num, force_rate = mode_defaults(a)
    subprocess.run(["pkill", "-x", "pw-play"], capture_output=True)
    time.sleep(1.0)
    nd = nodes()
    for k in ("out", "inp"):
        set_policy(nd[k][0], a.mode, headroom, start_delay, period_num)
    forces(force_rate, a.quantum)
    time.sleep(1.0)
    if a.reprepare:
        capture_reprepare(nd["inp"][1], force_rate or a.rate, a.quantum)
        time.sleep(1.0)
    wav = os.path.join(TONE_DIR, f"tone{a.rate}.wav"); tone(wav, a.rate)
    u0, o0 = counters()
    t0 = time.time(); rows = []
    events = []
    if a.poke_at is not None:
        events.append((a.poke_at, "vendor", [a.poke_req, a.poke_val, a.poke_idx]))
    if a.pitch_at is not None:
        events.append((a.pitch_at, "pitch", [str(a.pitch)]))
    if a.pitch0_at is not None:
        events.append((a.pitch0_at, "pitch", ["0"]))
    events.sort()
    marks = []
    player = subprocess.Popen(["pw-play", "--target", nd["out"][1], wav],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    while time.time() - t0 < a.secs:
        t = time.time() - t0
        while events and t >= events[0][0]:
            _, kind, args = events.pop(0)
            up, op = counters()
            if kind == "vendor":
                r = sh([CLOCK_WRITE, HWDEP_NODE, "vendor"] + args)
            else:
                r = sh([CLOCK_WRITE, HWDEP_NODE, "pitch"] + args)
            marks.append((t, f"{kind} {' '.join(args)}", up, op))
            print(f"   poke {kind} at {t:.2f}s: {r.stdout.strip()}")
        s, f = sample(); rows.append((t, s, f)); time.sleep(0.02)
    player.terminate(); player.wait()
    u1, o1 = counters()
    with open(a.label + ".csv", "w") as fh:
        fh.write("t,state,fill\n")
        for t, s, f in rows: fh.write(f"{t:.3f},{s},{f}\n")
    fills = [f for _, s, f in rows if s == "RUNNING"]
    print(f"[{a.label}] mode={a.mode} rate={a.rate} q={a.quantum} sd={start_delay} "
          f"hr={headroom} pn={period_num} force_rate={force_rate} reprepare={a.reprepare}")
    if fills:
        print(f"   fill min={min(fills)} max={max(fills)} mean={sum(fills)//len(fills)}")
    if marks:
        bounds = [0.0] + [m[0] for m in marks] + [a.secs]
        for i, (t, label, up, op) in enumerate(marks):
            seg = [f for tt, s, f in rows if s == "RUNNING" and bounds[i] <= tt < bounds[i+1]]
            nxt_u = marks[i+1][2] if i+1 < len(marks) else u1
            nxt_o = marks[i+1][3] if i+1 < len(marks) else o1
            fmin = min(seg) if seg else "-"
            print(f"   after [{label}] @{t:.1f}s: underruns +{nxt_u-up} overruns +{nxt_o-op} fill_min={fmin}")
        print(f"   TOTAL underruns +{u1-u0}  overruns +{o1-o0}")
    else:
        print(f"   underruns +{u1-u0}  overruns +{o1-o0}")

# ── matrix ───────────────────────────────────────────────────────────
def cmd_matrix(a):
    rates = [int(r) for r in a.rates.split(",")]
    subprocess.run(["pkill", "-x", "pw-play"], capture_output=True)
    time.sleep(1.0)
    nd = nodes()
    headroom, start_delay, period_num, _ = mode_defaults(a)
    for k in ("out", "inp"):
        if k in nd: set_policy(nd[k][0], a.mode, headroom, start_delay, period_num)
    forces(0, a.quantum)  # Auto: the graph conducts the stream rate
    time.sleep(1.0)
    rows = []; fails = 0
    print(f"# {a.label}: mode={a.mode} quantum={a.quantum} rates={rates}")
    for idx, rate in enumerate(rates):
        wav = os.path.join(TONE_DIR, f"tone{rate}.wav"); tone(wav, rate)
        u0, o0 = counters()
        t0 = time.time()
        player = subprocess.Popen(["pw-play", "--target", nd["out"][1], wav],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        conducted = None; deadline = t0 + 8.0
        while time.time() < deadline:
            hr = hw_rate(); rm = rate_meas()
            if hr == rate and abs(rm - rate) <= rate * 0.01:
                conducted = time.time() - t0; break
            time.sleep(0.05)
        time.sleep(a.dwell)
        hr = hw_rate(); rm = rate_meas(); u1, o1 = counters()
        player.terminate(); player.wait()
        ok = conducted is not None and hr == rate and abs(rm - rate) <= rate * 0.01
        if not ok: fails += 1
        ct = f"{conducted:.2f}s" if conducted is not None else "NEVER"
        print(f"  step {idx+1}: target={rate:6d} hw={hr:6d} meas={rm:6d} "
              f"conduct={ct:>7} ur+{u1-u0} ov+{o1-o0} {'PASS' if ok else 'FAIL'}")
        rows.append((idx+1, rate, hr, rm, conducted if conducted else -1, u1-u0, o1-o0, int(ok)))
        time.sleep(a.gap)
    with open(a.label + ".csv", "w") as fh:
        fh.write("step,target,hw_rate,rate_meas,conduct_s,underruns,overruns,pass\n")
        for r in rows: fh.write(",".join(str(x) for x in r) + "\n")
    print(f"# {a.label}: {len(rates)-fails}/{len(rates)} conducted, {fails} FAIL")
    return 1 if fails else 0

# ── watch ────────────────────────────────────────────────────────────
def cmd_watch(a):
    t0 = time.time(); nid = out_node()
    last_slow = -10; last_fill = -10; prev = {}
    def emit(now, tag, val):
        if prev.get(tag) != val:
            print(f"{now:7.2f}s {tag:10} = {val}", flush=True); prev[tag] = val
    print(f"# t0 epoch {t0:.3f}  ({time.strftime('%H:%M:%S', time.localtime(t0))})", flush=True)
    while time.time() - t0 < a.secs:
        now = time.time() - t0
        if now - last_slow >= 0.4:
            out = sh(["pw-metadata", "-n", "settings", "0"]).stdout
            g = lambda k: (re.search(rf"key:'{k}' value:'(\d+)'", out) or [None, "?"])[1]
            emit(now, "force", f"{g('clock.force-rate')}/{g('clock.force-quantum')}")
            if nid:
                ts, hr, sd, pn = live_props(nid)
                emit(now, "sched", f"{ts} hr={hr} sd={sd} pn={pn}")
            nid2 = out_node()
            if nid2 != nid: nid = nid2
            last_slow = now
        rate, per, buf = hwparams(); emit(now, "pcm", f"{rate}/{per}/{buf}")
        emit(now, "rate_meas", rate_meas())
        u, o, rec, seq = counters_full()
        emit(now, "run_seq", seq); emit(now, "recoveries", rec)
        if now - last_fill >= a.fill_interval:
            st, f = sample()
            print(f"{now:7.2f}s fill={f:>5} {st:8} ur={u} ov={o}", flush=True)
            last_fill = now
        time.sleep(0.05)
    return 0

def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd")
    # common device selector (device-agnostic generic layer)
    dev = argparse.ArgumentParser(add_help=False)
    dev.add_argument("--card", type=int, default=0, help="ALSA card number (default 0)")
    # run
    r = sub.add_parser("run", help="init experiment + controlled stream + pokes", parents=[dev])
    r.add_argument("--mode", choices=["timer", "irq"], default="timer")
    r.add_argument("--rate", type=int, default=96000)
    r.add_argument("--quantum", type=int, default=256)
    r.add_argument("--start-delay", type=int, default=None)
    r.add_argument("--headroom", type=int, default=None)
    r.add_argument("--period-num", type=int, default=None)
    r.add_argument("--force-rate", type=int, default=None)
    r.add_argument("--reprepare", action="store_true")
    r.add_argument("--secs", type=float, default=8)
    r.add_argument("--label", default="pwl")
    r.add_argument("--poke-at", type=float, default=None)
    r.add_argument("--poke-req", default="0x10")
    r.add_argument("--poke-val", default="0x0020")
    r.add_argument("--poke-idx", default="0x30")
    r.add_argument("--pitch-at", type=float, default=None)
    r.add_argument("--pitch", type=float, default=5.0)
    r.add_argument("--pitch0-at", type=float, default=None)
    # matrix
    m = sub.add_parser("matrix", help="Auto rate conduction, new client per track", parents=[dev])
    m.add_argument("--rates", required=True)
    m.add_argument("--dwell", type=float, default=6.0)
    m.add_argument("--gap", type=float, default=0.5)
    m.add_argument("--mode", choices=["timer", "irq"], default="timer")
    m.add_argument("--quantum", type=int, default=256)
    m.add_argument("--headroom", type=int, default=None)
    m.add_argument("--start-delay", type=int, default=None)
    m.add_argument("--period-num", type=int, default=None)
    m.add_argument("--force-rate", type=int, default=None)
    m.add_argument("--label", default="ratematrix")
    # watch
    w = sub.add_parser("watch", help="read-only fast event timeline", parents=[dev])
    w.add_argument("--secs", type=float, default=30.0)
    w.add_argument("--fill-interval", type=float, default=0.2)
    w.add_argument("--label", default="watch")

    argv = sys.argv[1:]
    if argv and argv[0] not in ("run", "matrix", "watch", "-h", "--help"):
        argv = ["run"] + argv  # legacy default
    a = ap.parse_args(argv)
    resolve_device(getattr(a, "card", 0))
    if a.cmd == "matrix": sys.exit(cmd_matrix(a))
    elif a.cmd == "watch": sys.exit(cmd_watch(a))
    else: cmd_run(a)

if __name__ == "__main__":
    main()