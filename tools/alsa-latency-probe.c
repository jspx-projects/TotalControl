// SPDX-License-Identifier: GPL-2.0-only
//
// Impulse-train latency probe for an ALSA PCM.
//
//   alsa-latency-probe <device> <rate> <period> <periods>
//                      [play_ch] [capture_ch] [capture_ch_b] [channels]
//                      [amplitude]
//
// Plays a full-scale impulse every 1024 frames on `play_ch`, captures one or
// two channels on the same PCM and reports the delay of the captured impulse
// train.  Any ALSA PCM with S32_LE samples and the requested channel count
// can be used.  The PCM streams are started separately; the capture trigger
// timestamp corrects the small start offset.  A second capture channel makes
// it possible to measure an internal loopback and a cable path in the same
// run, so the start phase cancels and only the difference remains.
//
// The caller owns the routing and the loopback:
//   - internal loopback: route playback to an output, enable the device's
//     loopback on that output, capture the same-numbered input;
//   - cable: route playback to the output, cable output -> input, capture the
//     input with the internal loopback disabled.
//
// Stop any server or application that owns the PCM first.
//
// The peak detector clusters the phase of the impulse train instead of
// averaging every local maximum, so ringing and reflections on an analog
// cable do not bias the result.

#include <alsa/asoundlib.h>
#include <math.h>
#include <stdint.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>

#define TRAIN 1024
#define PRE 512

/*
 * Delay of the impulse train on one capture channel.  Peaks are grouped by
 * phase modulo the train length; the densest cluster is the impulse train,
 * and reflections on the cable fall into other clusters.  Returns the
 * circular mean of the winning cluster and sets the peak count and the
 * largest absolute sample.
 */
static double measure_channel(const int32_t *in, snd_pcm_uframes_t rd, int ch,
                              int channels, double offset_frames, int *n_out,
                              long *amp_out) {
    enum { NB = 64 };
    int cnt[NB];
    double sum[NB];
    long maxv = 0;

    for (int b = 0; b < NB; b++) {
        cnt[b] = 0;
        sum[b] = 0.0;
    }
    for (snd_pcm_uframes_t i = 0; i < rd; i++) {
        long v = labs((long)in[(size_t)i * (size_t)channels + (size_t)ch]);
        if (v > maxv) maxv = v;
    }
    *amp_out = maxv;
    if (maxv < 1000000) {
        *n_out = 0;
        return 0.0;
    }
    const long thresh = maxv / 2;
    for (snd_pcm_uframes_t i = 1; i + 1 < rd; i++) {
        long v = labs((long)in[(size_t)i * (size_t)channels + (size_t)ch]);
        long vm = labs((long)in[(size_t)(i - 1) * (size_t)channels + (size_t)ch]);
        long vp = labs((long)in[(size_t)(i + 1) * (size_t)channels + (size_t)ch]);
        if (v < thresh || v < vm || v < vp) continue;
        double d = (double)i - (double)PRE - offset_frames;
        d = fmod(d, (double)TRAIN);
        if (d < 0) d += (double)TRAIN;
        int b = (int)(d * NB / TRAIN);
        if (b >= NB) b = NB - 1;
        cnt[b]++;
        sum[b] += d;
    }

    int best = 0;
    for (int b = 1; b < NB; b++)
        if (cnt[b] > cnt[best]) best = b;
    if (!cnt[best]) {
        *n_out = 0;
        return 0.0;
    }

    /* Circular mean of the winning bucket and its two neighbours. */
    const double center = ((double)best + 0.5) * TRAIN / NB;
    double sx = 0.0, sy = 0.0;
    int n = 0;
    for (int b = 0; b < NB; b++) {
        if (!cnt[b]) continue;
        double bc = ((double)b + 0.5) * TRAIN / NB;
        double diff = bc - center;
        if (diff > TRAIN / 2) diff -= TRAIN;
        if (diff < -TRAIN / 2) diff += TRAIN;
        if (fabs(diff) > 1.5 * TRAIN / NB) continue;
        double p = sum[b] / cnt[b];
        double ang = p * 2.0 * M_PI / TRAIN;
        sx += cnt[b] * cos(ang);
        sy += cnt[b] * sin(ang);
        n += cnt[b];
    }
    double ang = atan2(sy, sx);
    if (ang < 0) ang += 2.0 * M_PI;
    *n_out = n;
    return ang * TRAIN / (2.0 * M_PI);
}

static int set_params(snd_pcm_t *p, unsigned rate, snd_pcm_uframes_t period,
                      unsigned channels, snd_pcm_uframes_t *aperiod,
                      snd_pcm_uframes_t *abuffer) {
    snd_pcm_hw_params_t *h;
    snd_pcm_hw_params_alloca(&h);
    int r;
    if ((r = snd_pcm_hw_params_any(p, h)) < 0) return r;
    if ((r = snd_pcm_hw_params_set_access(p, h, SND_PCM_ACCESS_RW_INTERLEAVED)) < 0)
        return r;
    if ((r = snd_pcm_hw_params_set_format(p, h, SND_PCM_FORMAT_S32_LE)) < 0)
        return r;
    if ((r = snd_pcm_hw_params_set_channels(p, h, channels)) < 0) return r;
    unsigned rrate = rate;
    if ((r = snd_pcm_hw_params_set_rate_near(p, h, &rrate, 0)) < 0) return r;
    snd_pcm_uframes_t per = period;
    int dir = 0;
    if ((r = snd_pcm_hw_params_set_period_size_near(p, h, &per, &dir)) < 0)
        return r;
    snd_pcm_uframes_t buf = per * 4;
    dir = 0;
    if ((r = snd_pcm_hw_params_set_buffer_size_near(p, h, &buf)) < 0) return r;
    if ((r = snd_pcm_hw_params(p, h)) < 0) return r;
    snd_pcm_hw_params_get_period_size(h, &per, &dir);
    snd_pcm_hw_params_get_buffer_size(h, &buf);
    *aperiod = per;
    *abuffer = buf;
    return 0;
}

static double timeval_ms(const snd_timestamp_t *t) {
    return (double)t->tv_sec * 1000.0 + (double)t->tv_usec / 1000.0;
}

static int cmd_impulse(int argc, char **argv) {
    if (argc < 5) {
        fprintf(stderr,
                "usage: %s <device> <rate> <period> <periods> [play_ch] "
                "[capture_ch] [capture_ch_b] [channels]\n"
                "  device      ALSA PCM name, e.g. hw:2,0\n"
                "  channels    interleaved channel count (default 2)\n"
                "  amplitude   impulse level 0..1 (default 1.0)\n"
                "  format      fixed to S32_LE\n",
                argv[0]);
        return 2;
    }
    const char *dev = argv[1];
    unsigned rate = (unsigned)atoi(argv[2]);
    snd_pcm_uframes_t want_period = (snd_pcm_uframes_t)atoi(argv[3]);
    int periods = atoi(argv[4]);
    int play_ch = argc > 5 ? atoi(argv[5]) : 0;
    int cap_ch = argc > 6 ? atoi(argv[6]) : 2;
    int cap_ch_b = argc > 7 ? atoi(argv[7]) : -1;
    unsigned channels = argc > 8 ? (unsigned)atoi(argv[8]) : 2;
    double amplitude = argc > 9 ? atof(argv[9]) : 1.0;
    if (!channels || channels > 4096) {
        fprintf(stderr, "channels outside 1..4096\n");
        return 2;
    }
    if (amplitude <= 0.0 || amplitude > 1.0) {
        fprintf(stderr, "amplitude outside 0..1\n");
        return 2;
    }
    if (play_ch < 0 || (unsigned)play_ch >= channels || cap_ch < 0 ||
        (unsigned)cap_ch >= channels ||
        (cap_ch_b >= 0 && (unsigned)cap_ch_b >= channels)) {
        fprintf(stderr, "channel outside 0..%u\n", channels - 1);
        return 2;
    }

    snd_pcm_t *p = NULL, *c = NULL;
    snd_pcm_uframes_t pper, pbuf, cper, cbuf;
    int r;
    if ((r = snd_pcm_open(&p, dev, SND_PCM_STREAM_PLAYBACK, 0)) < 0) {
        fprintf(stderr, "open playback: %s\n", snd_strerror(r));
        return 1;
    }
    if ((r = snd_pcm_open(&c, dev, SND_PCM_STREAM_CAPTURE, 0)) < 0) {
        fprintf(stderr, "open capture: %s\n", snd_strerror(r));
        return 1;
    }
    if ((r = set_params(p, rate, want_period, channels, &pper, &pbuf)) < 0) {
        fprintf(stderr, "playback params: %s\n", snd_strerror(r));
        return 1;
    }
    if ((r = set_params(c, rate, want_period, channels, &cper, &cbuf)) < 0) {
        fprintf(stderr, "capture params: %s\n", snd_strerror(r));
        return 1;
    }

    snd_pcm_uframes_t total = pper * (snd_pcm_uframes_t)periods;
    int32_t *out = calloc((size_t)total * channels, sizeof(int32_t));
    int32_t *in = calloc((size_t)total * channels, sizeof(int32_t));
    if (!out || !in) {
        fprintf(stderr, "out of memory\n");
        return 1;
    }
    if (total <= PRE + TRAIN * 2) {
        fprintf(stderr, "capture too short: need more periods (total %lu)\n",
                (unsigned long)total);
        return 2;
    }
    for (snd_pcm_uframes_t i = PRE; i < total; i += TRAIN) {
        out[(size_t)i * (size_t)channels + (size_t)play_ch] =
            (int32_t)(1000000000.0 * amplitude);
    }

    if ((r = snd_pcm_prepare(p)) < 0) {
        fprintf(stderr, "prepare playback: %s\n", snd_strerror(r));
        return 1;
    }
    if ((r = snd_pcm_prepare(c)) < 0) {
        fprintf(stderr, "prepare capture: %s\n", snd_strerror(r));
        return 1;
    }
    r = (int)snd_pcm_writei(p, out, pper);
    if (r < 0) {
        fprintf(stderr, "prefill playback: %s\n", snd_strerror(r));
        return 1;
    }

    r = snd_pcm_start(c);
    if (r < 0 && r != -EBADFD) {
        fprintf(stderr, "start capture: %s\n", snd_strerror(r));
        return 1;
    }
    snd_timestamp_t tc = {0, 0};
    {
        snd_pcm_status_t *st;
        snd_pcm_status_alloca(&st);
        if (snd_pcm_status(c, st) == 0)
            snd_pcm_status_get_trigger_tstamp(st, &tc);
    }
    r = snd_pcm_start(p);
    if (r < 0 && r != -EBADFD) {
        fprintf(stderr, "start playback: %s\n", snd_strerror(r));
        return 1;
    }
    snd_timestamp_t tp = {0, 0};
    {
        snd_pcm_status_t *st;
        snd_pcm_status_alloca(&st);
        if (snd_pcm_status(p, st) == 0)
            snd_pcm_status_get_trigger_tstamp(st, &tp);
    }
    double start_offset_ms = timeval_ms(&tp) - timeval_ms(&tc);

    snd_pcm_uframes_t w = pper, rd = 0;
    while (rd < total) {
        if (w < total) {
            snd_pcm_uframes_t n = total - w;
            if (n > pper) n = pper;
            r = (int)snd_pcm_writei(p, out + (size_t)w * channels, n);
            if (r < 0) {
                r = (int)snd_pcm_recover(p, r, 1);
                if (r < 0) break;
            } else {
                w += (snd_pcm_uframes_t)r;
            }
        }
        r = (int)snd_pcm_readi(c, in + (size_t)rd * channels, pper);
        if (r < 0) {
            r = (int)snd_pcm_recover(c, r, 1);
            if (r < 0) break;
        } else {
            rd += (snd_pcm_uframes_t)r;
        }
    }
    snd_pcm_drop(p);
    snd_pcm_drop(c);

    const char *dump = getenv("DUMP");
    if (dump && *dump) {
        FILE *f = fopen(dump, "wb");
        if (f) {
            fwrite(in, sizeof(int32_t), (size_t)rd * channels, f);
            fclose(f);
        }
    }

    double offset_frames = start_offset_ms * (double)rate / 1000.0;
    int na = 0, nb = 0;
    long ampa = 0, ampb = 0;
    double fa = measure_channel(in, rd, cap_ch, (int)channels, offset_frames, &na, &ampa);
    double fb = 0.0;
    if (cap_ch_b >= 0) {
        fb = measure_channel(in, rd, cap_ch_b, (int)channels, offset_frames, &nb, &ampb);
    }
    /* rate,period,buffer,nA,framesA,msA,nB,framesB,msB,delta_ms,
       start_off_ms,ampA,ampB */
    if (cap_ch_b >= 0) {
        printf("%u,%lu,%lu,%d,%.1f,%.3f,%d,%.1f,%.3f,%.3f,%.3f,%ld,%ld\n",
               rate, (unsigned long)pper, (unsigned long)pbuf, na, fa,
               fa * 1000.0 / (double)rate, nb, fb,
               fb * 1000.0 / (double)rate,
               (fa - fb) * 1000.0 / (double)rate, start_offset_ms, ampa,
               ampb);
    } else {
        printf("%u,%lu,%lu,%d,%.1f,%.3f,-1,0,0,0,%.3f,%ld,0\n", rate,
               (unsigned long)pper, (unsigned long)pbuf, na, fa,
               fa * 1000.0 / (double)rate, start_offset_ms, ampa);
    }
    snd_pcm_close(p);
    snd_pcm_close(c);
    free(out);
    free(in);
    return na ? 0 : 3;
}

/* ---- pattern mode: sample-exact transfer via internal loopback ---------- */

#include <pthread.h>
#include <unistd.h>

#define PCH_MAX 64
static unsigned int p_rate = 48000;
static int p_seconds = 2, p_channels = 18;
static int p_play_a, p_play_b = 1, p_cap_a, p_cap_b = 1;
static float *p_cap;
static snd_pcm_uframes_t p_cap_frames;
static int p_cap_done;

static uint32_t p_rng(uint32_t *s) {
    uint32_t x = *s;
    x ^= x << 13; x ^= x >> 17; x ^= x << 5;
    return *s = x;
}
static float p_sample(uint32_t seed, snd_pcm_uframes_t i) {
    uint32_t s = seed ^ (uint32_t)(i * 2654435761u);
    p_rng(&s);
    return ((float)(s >> 8) / 16777216.0f) - 0.5f;
}
static uint32_t p_bits(float v) {
    uint32_t u; memcpy(&u, &v, 4); return u;
}

static int p_open(snd_pcm_t **pcm, snd_pcm_stream_t dir) {
    int err = snd_pcm_open(pcm, "hw:0,0", dir, 0);
    if (err < 0) { fprintf(stderr, "open: %s\n", snd_strerror(err)); return err; }
    snd_pcm_hw_params_t *hw;
    snd_pcm_hw_params_alloca(&hw);
    snd_pcm_hw_params_any(*pcm, hw);
    snd_pcm_hw_params_set_access(*pcm, hw, SND_PCM_ACCESS_RW_INTERLEAVED);
    snd_pcm_hw_params_set_format(*pcm, hw, SND_PCM_FORMAT_FLOAT_LE);
    snd_pcm_hw_params_set_channels(*pcm, hw, p_channels);
    unsigned int r = p_rate;
    snd_pcm_hw_params_set_rate_near(*pcm, hw, &r, 0);
    snd_pcm_uframes_t per = 1024, buf = 32768;
    snd_pcm_hw_params_set_period_size_near(*pcm, hw, &per, 0);
    snd_pcm_hw_params_set_buffer_size_near(*pcm, hw, &buf);
    err = snd_pcm_hw_params(*pcm, hw);
    if (err < 0) { fprintf(stderr, "hw_params: %s\n", snd_strerror(err)); return err; }
    return snd_pcm_prepare(*pcm);
}

static void *p_capture(void *arg) {
    (void)arg;
    snd_pcm_t *pcm;
    if (p_open(&pcm, SND_PCM_STREAM_CAPTURE) < 0) { p_cap_done = 1; return NULL; }
    p_cap_frames = (snd_pcm_uframes_t)p_rate * (p_seconds + 2);
    p_cap = malloc(p_cap_frames * p_channels * sizeof(float));
    snd_pcm_start(pcm);
    snd_pcm_uframes_t got = 0;
    while (got < p_cap_frames) {
        snd_pcm_sframes_t n = snd_pcm_readi(pcm, p_cap + got * p_channels,
                                            p_cap_frames - got);
        if (n == -EPIPE) { snd_pcm_prepare(pcm); snd_pcm_start(pcm); continue; }
        if (n < 0) { fprintf(stderr, "read: %s\n", snd_strerror(n)); break; }
        got += n;
    }
    p_cap_frames = got;
    snd_pcm_drop(pcm);
    snd_pcm_close(pcm);
    p_cap_done = 1;
    return NULL;
}

/* Classify one channel: exact, 1-mantissa-bit truncation (device Q23), zero,
 * other. Returns the slip (captured index - pattern index) via best offset. */
static void p_analyze(int ch, const float *pat, long total, long align,
                      long *exact, long *trunc, long *zero, long *other) {
    *exact = *trunc = *zero = *other = 0;
    long i0 = p_rate / 2;           /* skip the startup-gate window (<=256 ms) */
    for (long i = i0; i < total; i++) {
        long ci = align + i;
        if (ci < 0 || ci >= (long)p_cap_frames) break;
        float c = p_cap[ci * p_channels + ch];
        float p = pat[i];
        if (c == p) { (*exact)++; continue; }
        if (c == 0.0f) { (*zero)++; continue; }
        int32_t d = (int32_t)p_bits(c) - (int32_t)p_bits(p);
        uint32_t u = (uint32_t)(d < 0 ? -d : d);
        if (d < 0 && (u & (u - 1)) == 0) (*trunc)++;
        else (*other)++;
    }
}

static int cmd_pattern(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr,
            "usage: alsa-latency-probe pattern <device> <rate> <seconds>\n"
            "         [play_a] [play_b] [cap_a] [cap_b] [channels]\n"
            "  Sample-exact transfer test (FLOAT_LE). Configure routing first:\n"
            "    fireface-hwdep loopback-setup <out-pair> pb1   (internal loopback)\n"
            "  Internal loopback is digital: bit-exact comparison is meaningful.\n"
            "  A cable path is not sample-exact; use the impulse mode for it.\n");
        return 2;
    }
    /* argv[1] is the device (accepted for symmetry; hw:0,0 is used). */
    p_rate = (unsigned)atoi(argv[2]);
    p_seconds = argc > 3 ? atoi(argv[3]) : 2;
    p_play_a = argc > 4 ? atoi(argv[4]) - 1 : 0;
    p_play_b = argc > 5 ? atoi(argv[5]) - 1 : 1;
    p_cap_a = argc > 6 ? atoi(argv[6]) - 1 : 0;
    p_cap_b = argc > 7 ? atoi(argv[7]) - 1 : 1;
    p_channels = argc > 8 ? atoi(argv[8]) : 18;
    if (p_channels > PCH_MAX) { fprintf(stderr, "channels > %d\n", PCH_MAX); return 2; }

    pthread_t th;
    pthread_create(&th, NULL, p_capture, NULL);
    usleep(300000);
    snd_pcm_t *pcm;
    if (p_open(&pcm, SND_PCM_STREAM_PLAYBACK) < 0) return 1;
    snd_pcm_uframes_t total = (snd_pcm_uframes_t)p_rate * p_seconds;
    float *buf = malloc(1024 * p_channels * sizeof(float));
    snd_pcm_start(pcm);
    snd_pcm_uframes_t sent = 0;
    while (sent < total) {
        snd_pcm_uframes_t n = total - sent;
        if (n > 1024) n = 1024;
        memset(buf, 0, n * p_channels * sizeof(float));
        for (snd_pcm_uframes_t i = 0; i < n; i++) {
            buf[i * p_channels + p_play_a] = p_sample(0x12345678, sent + i);
            if (p_play_b != p_play_a)
                buf[i * p_channels + p_play_b] = p_sample(0x9abcdef0, sent + i);
        }
        snd_pcm_sframes_t w = snd_pcm_writei(pcm, buf, n);
        if (w == -EPIPE) { snd_pcm_prepare(pcm); snd_pcm_start(pcm); continue; }
        if (w < 0) { fprintf(stderr, "write: %s\n", snd_strerror(w)); return 1; }
        sent += w;
    }
    snd_pcm_drain(pcm);             /* play the whole ring; no tail truncation */
    snd_pcm_drop(pcm);
    snd_pcm_close(pcm);
    while (!p_cap_done) usleep(50000);
    pthread_join(th, NULL);
    if (!p_cap) { fprintf(stderr, "capture failed\n"); return 1; }

    const char *dump = getenv("DUMP");
    if (dump && *dump) {
        FILE *f = fopen(dump, "wb");
        if (f) { fwrite(p_cap, sizeof(float), p_cap_frames * p_channels, f); fclose(f); }
    }

    /* Alignment: search several anchor frames (the gate may mute the start). */
    float *pa = malloc(total * sizeof(float));
    for (snd_pcm_uframes_t i = 0; i < total; i++) pa[i] = p_sample(0x12345678, i);
    long align = -1;
    const int anchors[] = {0, 100, 500, 1000, 2000, 4000, 16000, 24000, 40000};
    for (int k = 0; k < 9 && align < 0; k++) {
        int kf = anchors[k];
        if ((snd_pcm_uframes_t)kf >= total) continue;
        for (snd_pcm_uframes_t o = 0; o + 64 < p_cap_frames; o++)
            if (p_cap[o * p_channels + p_cap_a] == pa[kf]) { align = (long)o - kf; break; }
    }
    if (align < 0) {
        printf("NO ALIGNMENT: pattern not found bit-exact (cable path, muted input, or wrong routing?)\n");
        free(pa); free(buf); free(p_cap);
        return 2;
    }
    printf("alignment %ld frames (%.3f ms; includes the <=256 ms startup gate)\n",
           align, (double)align * 1000.0 / p_rate);
    long n = (long)p_cap_frames - align;
    if (n > (long)total) n = (long)total;
    int rc = 0;
    int chans[2] = {p_cap_a, p_cap_b};
    uint32_t seeds[2] = {0x12345678, 0x9abcdef0};
    for (int c = 0; c < (p_play_b == p_play_a ? 1 : 2); c++) {
        float *pat = pa;
        if (c == 1) {
            pat = malloc(total * sizeof(float));
            for (snd_pcm_uframes_t i = 0; i < total; i++) pat[i] = p_sample(seeds[1], i);
        }
        long ex, tr, ze, ot;
        p_analyze(chans[c], pat, n, align, &ex, &tr, &ze, &ot);
        long den = n - p_rate / 2;
        printf("ch%d: exact %ld (%.1f%%), Q23-trunc %ld (%.1f%%), zero %ld, other %ld\n",
               chans[c] + 1, ex, den ? ex * 100.0 / den : 0,
               tr, den ? tr * 100.0 / den : 0, ze, ot);
        if (ze || ot) rc = 3;
        if (c == 1) free(pat);
    }
    printf(rc ? "NOT sample-exact (drops/other)\n" :
           "sample-exact up to device Q23 truncation (no drops)\n");
    free(pa); free(buf); free(p_cap);
    return rc;
}

int main(int argc, char **argv) {
    if (argc >= 2 && !strcmp(argv[1], "pattern"))
        return cmd_pattern(argc - 1, argv + 1);
    return cmd_impulse(argc, argv);
}
