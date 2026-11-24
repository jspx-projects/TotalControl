#!/usr/bin/env python3
"""Analyze ffuc ringtrace dumps: reconstruct the ring-fill trajectory.

Usage: ringtrace-analyze.py <trace-file> [--events N]
Records: ktime_ns appl hw status_hw filled total flags
  flags: 1 = dry vs status pointer (counted underrun), 2 = dry vs driver
  read position (starved), 4 = a full period passed without an appl_ptr
  advance (overdue write episode), 8 = appl_ptr behind the read position
  (inverted ring; filled clamped 0).
"""
import sys
import statistics as st

def load(path):
    recs = []
    with open(path) as f:
        for line in f:
            if line.startswith('#') or not line.strip():
                continue
            t, appl, hw, shw, filled, total, flags = map(int, line.split())
            recs.append(dict(t=t, appl=appl, hw=hw, shw=shw,
                             filled=filled, total=total, flags=flags))
    return recs

def main():
    path = sys.argv[1]
    nev = int(sys.argv[sys.argv.index('--events') + 1]) if '--events' in sys.argv else 10
    r = load(path)
    if len(r) < 3:
        print("too few records"); return
    # derived per-cycle columns
    for i in range(1, len(r)):
        a, b = r[i - 1], r[i]
        b['dt_us'] = (b['t'] - a['t']) / 1000.0
        b['d_appl'] = b['appl'] - a['appl']
        b['d_hw'] = b['hw'] - a['hw']
        b['lag'] = b['hw'] - b['shw']          # status-pointer masking
    body = r[1:]
    dts = [x['dt_us'] for x in body]
    das = [x['d_appl'] for x in body]
    dhs = [x['d_hw'] for x in body]
    lags = [x['lag'] for x in body]
    fills = [x['filled'] for x in body]
    print(f"records {len(body)}  span {(r[-1]['t']-r[0]['t'])/1e9:.2f}s")
    print(f"completion dt us: mean {st.mean(dts):.1f}  min {min(dts):.1f}  max {max(dts):.1f}")
    print(f"drain/URB: {sorted(set(dhs))}")
    from collections import Counter
    print(f"write/cycle histogram: {dict(Counter(das))}")
    print(f"filled at read: min {min(fills)}  median {st.median(fills)}  max {max(fills)}")
    print(f"status-pointer lag: min {min(lags)}  median {st.median(lags)}  max {max(lags)}")
    missed = [x for x in body if x['flags'] & 4]
    starved = [x for x in body if x['flags'] & 2]
    counted = [x for x in body if x['flags'] & 1]
    print(f"missed writes {len(missed)}  starved {len(starved)}  counted(status) {len(counted)}")
    # completion gaps > 1.5 nominal: USB-side scheduling holes
    nom = st.median(dts)
    holes = [x for x in body if x['dt_us'] > 1.5 * nom]
    print(f"completion holes (>1.5x nominal): {len(holes)}")
    for x in holes[:10]:
        print(f"  t={x['t']} dt={x['dt_us']:.0f}us d_appl={x['d_appl']} filled={x['filled']}")
    # event windows
    events = [i for i, x in enumerate(r) if x['flags'] & 6]
    print(f"\n=== {len(events)} event windows (last {nev}) ===")
    for i in events[-nev:]:
        print(f"--- event at {r[i]['t']} flags={r[i]['flags']} ---")
        for j in range(max(1, i - 8), min(len(r), i + 4)):
            x = r[j]
            mark = ' <<<' if j == i else ''
            print(f"  dt={x.get('dt_us',0):7.1f}us appl+{x.get('d_appl',0):4d} "
                  f"hw+{x.get('d_hw',0):4d} filled={x['filled']:4d} total={x['total']:3d} "
                  f"lag={x.get('lag',0):3d} f={x['flags']}{mark}")

if __name__ == '__main__':
    main()