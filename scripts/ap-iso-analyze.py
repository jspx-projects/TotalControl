#!/usr/bin/env python3
"""ap-iso-analyze.py - decode Fireface AP/ISO usbmon captures (linktype 220).

The UCX ISO stream (interface 0) at 1x rates:
  OUT (ep 0x0e): each frame is 76 bytes = one 4-byte lead word + 18 channel
      words (4 bytes each). The lead word is a raw ring position:
      (out_counter + f) % wrap, so the whole lead-word sequence advances by
      exactly 1 per frame, wrapping at the ring modulus.
  IN (ep 0x8f): each packet is one 4-byte header + frames*18*4 audio bytes.
      Header: bits 0-4 frame count, bits 5-17 position (13-bit, wraps 8192),
      bits 21-31 phase (11-bit). The position advances by the frame count.

This tool extracts those fields and reports continuity, so a capture can
confirm the ring modulus and the position-field width without relying on the
driver's own (possibly buggy) reduction.

usbmon binary layout (LINKTYPE_USB_LINUX_MMAPPED = 220), 64-byte header:
  0 u64 id; 8 type('S'/'C'/'E'); 9 xfer_type(0=ISO); 10 epnum(bit7=IN);
  11 devnum; 12 u16 busnum; 14 flag_setup; 15 flag_data; 16 s64 ts_sec;
  24 s32 ts_usec; 28 s32 status; 32 u32 length(URB); 36 u32 len_cap;
  40 iso union; 60 s32 number_of_packets.
After the header: number_of_packets * 16-byte iso descriptors
  (s32 status, u32 offset, u32 length, u32 reserved), then the payload.
Each descriptor's offset/length slice the payload into per-microframe packets.

Usage:
  ap-iso-analyze.py capture.pcap --inspect          # per-endpoint layout
  ap-iso-analyze.py capture.pcap [--mod 8192]       # continuity report
      [--ep-out 0x0e] [--ep-in 0x8f] [--stride 76] [--audio]
"""
import sys, struct, argparse, collections, math

def packets(path):
    f = open(path, 'rb')
    gh = f.read(24)
    magic, vmaj, vmin, tz, sig, snap, link = struct.unpack('<IHHIIII', gh)
    if link != 220:
        sys.stderr.write("warning: linktype %d (expected 220)\n" % link)
    while True:
        ph = f.read(16)
        if len(ph) < 16:
            break
        ts, tus, cap, orig = struct.unpack('<IIII', ph)
        data = f.read(cap)
        if len(data) < 64:
            continue
        yield ts + tus / 1e6, data

def iso_split(data):
    """Return (typ, epnum, [per-microframe payload bytes]) for ISO packets."""
    typ, xf, ep = data[8], data[9], data[10]
    if xf != 0:
        return None
    npkts, = struct.unpack_from('<i', data, 60)
    length, len_cap = struct.unpack_from('<II', data, 32)
    if npkts <= 0 or npkts > 1024:
        return None
    desc_end = 64 + npkts * 16
    if desc_end > len(data):
        return None
    payload = data[desc_end:64 + len_cap]
    out = []
    for i in range(npkts):
        st, off, ln, _ = struct.unpack_from('<iIII', data, 64 + i * 16)
        out.append(payload[off:off + ln] if ln else b'')
    return chr(typ), ep, out

def collect(path):
    by_ep = collections.defaultdict(list)
    for ts, data in packets(path):
        r = iso_split(data)
        if not r:
            continue
        typ, ep, pkts = r
        for p in pkts:
            if p:
                by_ep[(ep, typ)].append((ts, p))
    return by_ep

def inspect(by_ep):
    for (ep, typ), lst in sorted(by_ep.items()):
        sizes = collections.Counter(len(p) for _, p in lst)
        first = [p[:4].hex() for _, p in lst[:3]]
        print("ep %#04x %s  n=%d  sizes=%s  first4=%s" %
              (ep, typ, len(lst), dict(sizes.most_common(4)), first))

def out_leads(lst, stride):
    """Every frame's lead word, in order."""
    leads = []
    for ts, p in lst:
        if len(p) % stride:
            continue
        for off in range(0, len(p), stride):
            leads.append(struct.unpack_from('<I', p, off)[0])
    return leads

def continuity(vals, mod):
    """Count steps whose forward difference mod `mod` is not 1."""
    jumps = 0
    examples = []
    for a, b in zip(vals, vals[1:]):
        d = (b - a) % mod
        if d != 1:
            jumps += 1
            if len(examples) < 5:
                examples.append((a, b, d))
    return jumps, examples

def report(args):
    by_ep = collect(args.pcap)
    if args.inspect:
        inspect(by_ep)
        return
    mod = args.mod
    out = by_ep.get((args.ep_out, 'S'), [])
    stride = args.stride or detect_stride(out)
    if not args.stride:
        print("(auto-detected OUT frame stride = %d bytes, %d channels)" %
              (stride, stride // 4 - 1))
    leads = out_leads(out, stride)
    print("== OUT ep %#04x: %d packets, %d frame lead words ==" %
          (args.ep_out, len(out), len(leads)))
    if leads:
        print("  lead min=%d max=%d" % (min(leads), max(leads)))
        for m in (8192, 3072, mod):
            j, ex = continuity(leads, m)
            print("  steps not +1 mod %-5d : %d / %d   e.g. %s" %
                  (m, j, len(leads) - 1, ex))
        print("  first leads:", leads[:6], " last:", leads[-6:])

    inn = by_ep.get((args.ep_in, 'C'), [])
    print("== IN ep %#04x: %d packets ==" % (args.ep_in, len(inn)))
    hdrs = []
    for ts, p in inn:
        if len(p) >= 4:
            h = struct.unpack_from('<I', p, 0)[0]
            hdrs.append((h & 0x1f, (h >> 5) & 0x1fff, h >> 21, h))
    if hdrs:
        gaps = 0
        ex = []
        for (c0, p0, _, _), (c1, p1, _, _) in zip(hdrs, hdrs[1:]):
            exp = (p0 + c0) % 8192
            if p1 != exp:
                gaps += 1
                if len(ex) < 5:
                    ex.append((p0, c0, p1, (p1 - p0) % 8192))
        print("  max position field = %d  (13-bit reaches ~8191)" %
              max(p for _, p, _, _ in hdrs))
        print("  position steps != prev+count mod 8192 : %d / %d" %
              (gaps, len(hdrs) - 1))
        print("  e.g. (prev_pos,count,new_pos,delta):", ex)
        print("  first (cnt,pos,phase):", [(c, p, ph) for c, p, ph, _ in hdrs[:5]])
        phases = [ph for _, _, ph, _ in hdrs]
        print("  phase min=%d max=%d (11-bit <2048)" % (min(phases), max(phases)))

    if args.audio:
        audio_check(out, stride, args.fmt)

def detect_stride(out):
    """Auto-detect the OUT frame stride (1 lead word + nch samples, 4 bytes
    each). Try plausible channel counts; the right stride makes the lead
    words advance by exactly 1. Scored on a sample of packets for speed."""
    sample = [p for _, p in out[:4000] if p]
    best, best_score = 76, -1.0
    for nch in (18, 14, 12, 10, 8, 6, 24, 2):
        stride = (1 + nch) * 4
        leads = []
        ok = 0
        for p in sample:
            if len(p) % stride:
                continue
            ok += 1
            for off in range(0, len(p), stride):
                leads.append(struct.unpack_from('<I', p, off)[0])
        if ok < len(sample) * 0.5 or len(leads) < 16:
            continue
        if max(leads) >= 8192:
            continue
        good = sum(1 for a, b in zip(leads, leads[1:]) if (b - a) % 8192 == 1)
        score = good / (len(leads) - 1)
        if score > best_score:
            best, best_score = stride, score
    return best

def detect_format(out, stride):
    """Auto-detect the sample format: float32 (ISO) vs int32 (interrupt).
    The correct interpretation is a smooth waveform with few full-scale
    jumps. The wrong one reads bit patterns as values that flip sign at every
    zero crossing / exponent change, producing many |step| > peak jumps (and
    possibly non-finite floats). Score = non-finite + full-scale-jump count;
    the lower score wins."""
    nch = stride // 4 - 1
    buf = [[] for _ in range(nch)]
    cnt = 0
    for _, p in out:
        if len(p) % stride:
            continue
        for off in range(0, len(p), stride):
            for c in range(nch):
                buf[c].append(p[off + 4 + c * 4:off + 8 + c * 4])
        cnt += 1
        if cnt >= 4000:
            break
    act = []
    for c in range(nch):
        vals = [struct.unpack('<i', b)[0] for b in buf[c]]
        act.append(max((abs(v) for v in vals), default=0))
    c = act.index(max(act)) if act else 0
    raws = buf[c]
    score = {}
    for fmt in ('f', 'i'):
        vals = [struct.unpack('<' + fmt, b)[0] for b in raws]
        bad = sum(1 for v in vals if not math.isfinite(v))
        fin = [v for v in vals if math.isfinite(v)]
        peak = max((abs(v) for v in fin), default=0)
        if peak == 0:
            score[fmt] = float('inf')
            continue
        big = sum(1 for a, b in zip(fin, fin[1:]) if abs(b - a) > peak)
        score[fmt] = bad + big
    return 'f' if score['f'] <= score['i'] else 'i'

def audio_check(out, stride, fmt='auto'):
    """Detect clicks/dropouts in the OUT audio (skip the lead word of each
    frame). ISO carries float32 samples ('f'); the interrupt path is int32
    ('i'). A clean tone has a bounded, smooth sample-to-sample change; a
    dropout is a run of identical samples; a click is an isolated large jump
    well above the normal per-sample slew."""
    if fmt == 'auto':
        fmt = detect_format(out, stride)
    nch = stride // 4 - 1
    chans = [[] for _ in range(nch)]
    for ts, p in out:
        if len(p) % stride:
            continue
        for off in range(0, len(p), stride):
            for c in range(nch):
                v = struct.unpack_from('<' + fmt, p, off + 4 + c * 4)[0]
                chans[c].append(v)
    print("== OUT audio (%s): %d channels, %d frames ==" %
          (fmt, nch, len(chans[0]) if chans else 0))
    for c, seq in enumerate(chans):
        if not seq:
            continue
        peak = max(abs(v) for v in seq)
        if peak < 1e-4:
            continue  # silent channel
        jumps = [abs(b - a) for a, b in zip(seq, seq[1:])]
        # normal slew for a sine of amplitude A at freq f, rate sr:
        # max |dx| ~ A*2*pi*f/sr. Flag steps far above the median slew.
        js = sorted(jumps)
        med = js[len(js) // 2] if js else 0
        thresh = max(med * 8, peak * 0.5)
        big = [(i, j) for i, j in enumerate(jumps) if j > thresh]
        # dropout: >=8 consecutive identical samples
        drop = 0
        run = 1
        for a, b in zip(seq, seq[1:]):
            run = run + 1 if a == b else 1
            if run == 8:
                drop += 1
        print("  ch%d peak=%.4f median_slew=%.5f big_jumps=%d dropouts=%d" %
              (c, peak, med, len(big), drop))
        for i, j in big[:5]:
            print("     jump @frame %d: %.4f -> %.4f (|d|=%.4f)" %
                  (i, seq[i], seq[i + 1], j))

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('pcap')
    ap.add_argument('--mod', type=int, default=8192)
    ap.add_argument('--ep-out', type=lambda x: int(x, 0), default=0x0e)
    ap.add_argument('--ep-in', type=lambda x: int(x, 0), default=0x8f)
    ap.add_argument('--stride', type=int, default=0,
                    help='OUT frame stride in bytes (1 lead + nch*4); 0=auto')
    ap.add_argument('--fmt', choices=['auto', 'f', 'i'], default='auto',
                    help="sample format: f=float32 (ISO), i=int32 (interrupt), auto")
    ap.add_argument('--inspect', action='store_true')
    ap.add_argument('--audio', action='store_true')
    args = ap.parse_args()
    report(args)

if __name__ == '__main__':
    main()
