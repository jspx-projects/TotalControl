#!/usr/bin/env python3
"""Tight /proc/asound playback status sampler.
Usage: proc-sampler.py OUT.csv SECONDS INTERVAL
Columns: t (CLOCK_MONOTONIC), appl, hw, fill, delay, state, rate.
"""
import sys, time

out, secs, iv = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
STATUS = '/proc/asound/card0/pcm0p/sub0/status'
HWPARAMS = '/proc/asound/card0/pcm0p/sub0/hw_params'
rate = 48000
try:
    for l in open(HWPARAMS):
        if l.startswith('rate:'):
            rate = int(l.split()[1])
except OSError:
    pass
t0 = time.clock_gettime(time.CLOCK_MONOTONIC)
with open(out, 'w') as f:
    f.write('t,appl,hw,fill,delay,state,rate\n')
    while True:
        t = time.clock_gettime(time.CLOCK_MONOTONIC)
        if t - t0 > secs:
            break
        d = {}
        state = 'closed'
        try:
            for l in open(STATUS):
                if l.startswith('state:'):
                    state = l.split()[1]
                for k in ('appl_ptr', 'hw_ptr', 'delay'):
                    if l.startswith(k):
                        d[k] = int(l.split(':')[1])
        except OSError:
            pass
        if 'appl_ptr' in d:
            fill = d['appl_ptr'] - d['hw_ptr']
            f.write(f'{t:.6f},{d["appl_ptr"]},{d["hw_ptr"]},{fill},'
                    f'{d.get("delay",0)},{state},{rate}\n')
        time.sleep(iv)
