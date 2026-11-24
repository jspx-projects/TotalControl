/* Fireface physical round-trip frame accounting via raw ALSA linked duplex.
 * Multi-device (UC / UCX): auto-detects the Fireface ALSA card.
 *
 * Both directions on hw:<card>, linked start (one device clock, common t0).
 * A chirp is written at a known playback ring position P; offline
 * correlation finds its capture ring position C. Because both rings
 * count device frames from the common trigger:
 *
 *   D = C - P  =  the device round-trip span (playback hw_ptr -> DAC, plus
 *                 ADC -> capture delivery), in frames, exact.
 *
 * The write->read time (host timestamps) additionally gives the host fill.
 *
 * PipeWire must be stopped. Routing playback AN1/2 -> out5 must be live
 * (fireface-scene-ctl route "AN5"), cable out5 -> in5. Capture channel =
 * input 5 (AN5), index 4 on UC and UCX.
 *
 * Build: gcc -O2 -o fireface-rtl-probe fireface-rtl-probe.c -lasound -lm
 * Usage: fireface-rtl-probe [rate=48000] [period=48] [reps=5] [outdir=.] [capch=4]
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <time.h>
#include <unistd.h>
#include <alsa/asoundlib.h>

static double now_s(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec * 1e-9;
}

/* Find the ALSA card whose id contains "Fireface" (UC or UCX). */
static int find_fireface_card(void)
{
    for (int c = 0; c < 8; c++) {
        char path[64];
        snprintf(path, sizeof path, "/proc/asound/card%d/id", c);
        FILE *f = fopen(path, "r");
        if (!f) continue;
        char id[64] = {0};
        char *got = fgets(id, sizeof id, f);
        fclose(f);
        if (got && strstr(id, "Fireface")) return c;
    }
    return -1;
}

static void die(snd_pcm_t *p, const char *what, int err)
{
    fprintf(stderr, "%s: %s\n", what, snd_strerror(err));
    if (p) snd_pcm_close(p);
    exit(1);
}

static void setup(snd_pcm_t *pcm, unsigned rate, unsigned ch,
                  unsigned period, unsigned buffer_req, unsigned *buffer_out)
{
    snd_pcm_hw_params_t *hw;
    snd_pcm_sw_params_t *sw;
    int err;
    snd_pcm_hw_params_alloca(&hw);
    snd_pcm_hw_params_any(pcm, hw);
    snd_pcm_hw_params_set_access(pcm, hw, SND_PCM_ACCESS_MMAP_INTERLEAVED);
    snd_pcm_hw_params_set_format(pcm, hw, SND_PCM_FORMAT_FLOAT_LE);
    snd_pcm_hw_params_set_channels(pcm, hw, ch);
    snd_pcm_hw_params_set_rate(pcm, hw, rate, 0);
    snd_pcm_hw_params_set_period_size(pcm, hw, period, 0);
    snd_pcm_hw_params_set_buffer_size(pcm, hw, buffer_req);
    if ((err = snd_pcm_hw_params(pcm, hw)) < 0) die(pcm, "hw_params", err);
    snd_pcm_uframes_t buf = 0, per = 0;
    snd_pcm_hw_params_get_buffer_size(hw, &buf);
    snd_pcm_hw_params_get_period_size(hw, &per, NULL);
    *buffer_out = buf;
    snd_pcm_sw_params_alloca(&sw);
    snd_pcm_sw_params_current(pcm, sw);
    snd_pcm_sw_params_set_start_threshold(pcm, sw, buf);
    snd_pcm_sw_params_set_avail_min(pcm, sw, per);
    if ((err = snd_pcm_sw_params(pcm, sw)) < 0) die(pcm, "sw_params", err);
}

/* Write `frames` from src (interleaved float, ch channels) at the current
 * mmap position; returns the ring offset written. */
static snd_pcm_uframes_t write_frames(snd_pcm_t *pcm, const float *src,
                                      unsigned ch, snd_pcm_uframes_t frames)
{
    const snd_pcm_channel_area_t *areas;
    snd_pcm_uframes_t off, n = frames;
    int err = snd_pcm_mmap_begin(pcm, &areas, &off, &n);
    if (err < 0) die(pcm, "mmap_begin(w)", err);
    for (snd_pcm_uframes_t i = 0; i < n; i++)
        for (unsigned c = 0; c < ch; c++) {
            float *dst = (float *)((char *)areas[c].addr +
                          (areas[c].first + (off + i) * areas[c].step) / 8);
            *dst = src ? src[i * ch + c] : 0.0f;
        }
    snd_pcm_mmap_commit(pcm, off, n);
    return n;
}

int main(int argc, char **argv)
{
    unsigned rate = argc > 1 ? atoi(argv[1]) : 48000;
    unsigned period = argc > 2 ? atoi(argv[2]) : 48;
    unsigned reps = argc > 3 ? atoi(argv[3]) : 5;
    const char *outdir = argc > 4 ? argv[4] : ".";
    unsigned capch = argc > 5 ? (unsigned)atoi(argv[5]) : 4;
    /* Held fill / prefill in periods (env FILL_PERIODS, default 2). Used to
     * isolate how the startup fill shifts the measured span. */
    unsigned fillp = 2;
    const char *fe = getenv("FILL_PERIODS");
    if (fe && atoi(fe) > 0) fillp = (unsigned)atoi(fe);
    const unsigned pch = 2, cch = 18;
    unsigned pbuf = 0, cbuf = 0;
    snd_pcm_t *pb, *cap;
    int err;

    int card = find_fireface_card();
    if (card < 0) { fprintf(stderr, "no Fireface ALSA card found\n"); return 1; }
    char devname[32];
    snprintf(devname, sizeof devname, "hw:%d,0", card);
    fprintf(stderr, "Fireface on %s, capture channel %u\n", devname, capch);
    if (capch >= cch) { fprintf(stderr, "capch out of range\n"); return 1; }

    if ((err = snd_pcm_open(&pb, devname, SND_PCM_STREAM_PLAYBACK,
                            SND_PCM_NONBLOCK)) < 0) die(NULL, "open pb", err);
    if ((err = snd_pcm_open(&cap, devname, SND_PCM_STREAM_CAPTURE,
                            SND_PCM_NONBLOCK)) < 0) die(pb, "open cap", err);
    setup(pb, rate, pch, period, period * 8, &pbuf);
    setup(cap, rate, cch, period, period * 32, &cbuf);
    if ((err = snd_pcm_link(pb, cap)) < 0) die(pb, "link", err);
    if ((err = snd_pcm_prepare(pb)) < 0) die(pb, "prepare pb", err);
    if ((err = snd_pcm_prepare(cap)) < 0) die(pb, "prepare cap", err);

    /* Chirp template: linear 200 -> 16000 Hz, 0.25 s, 5 ms fades. */
    unsigned L = rate / 4;
    float *chirp = malloc(L * pch * sizeof(float));
    double phase = 0;
    for (unsigned i = 0; i < L; i++) {
        double f = 200.0 + (16000.0 - 200.0) * i / L;
        phase += 2 * M_PI * f / rate;
        double env = 1.0;
        unsigned fade = rate / 200;
        if (i < fade) env = 0.5 - 0.5 * cos(M_PI * i / fade);
        if (i >= L - fade) env = 0.5 - 0.5 * cos(M_PI * (L - 1 - i) / fade);
        float s = 0.5 * env * sin(phase);
        chirp[i * pch] = s;
        chirp[i * pch + 1] = s;
    }

    /* Schedule: 2 s silence (let the device queue/lead settle), then
     * reps x (chirp + 0.75 s silence). */
    unsigned long *P = malloc(reps * sizeof(unsigned long));
    double *tw = malloc(reps * sizeof(double));
    unsigned long *fillw = malloc(reps * sizeof(unsigned long));
    for (unsigned k = 0; k < reps; k++)
        P[k] = period + (unsigned long)rate * 2 + k * (unsigned long)rate;
    unsigned long done_write = P[reps - 1] + L + rate / 2;
    unsigned long done_cap = P[reps - 1] + rate;

    /* Prefill two periods of silence, then linked start. The write loop
     * holds ~2 periods of fill. Keep the buffer and fill at this operating
     * point: a larger buffer/fill shifts the UC's feedback/lead alignment and
     * moves the measured span. Poll fast enough to avoid underruns instead. */
    write_frames(pb, NULL, pch, fillp * period);
    double t0 = now_s();
    if ((err = snd_pcm_start(pb)) < 0) die(pb, "start", err);

    char path[512];
    snprintf(path, sizeof path, "%s/cap_ch4.f32", outdir);
    FILE *fcap = fopen(path, "wb");
    snprintf(path, sizeof path, "%s/capchunks.txt", outdir);
    FILE *fchunks = fopen(path, "w");
    snprintf(path, sizeof path, "%s/chirp.f32", outdir);
    FILE *fchirp = fopen(path, "wb");
    fwrite(chirp, sizeof(float), L * pch, fchirp);
    fclose(fchirp);

    unsigned long total_w = fillp * period, total_c = 0;
    unsigned next_rep = 0;
    while (total_w < done_write || total_c < done_cap) {
        if (now_s() - t0 > 30.0) {
            fprintf(stderr, "timeout: w=%lu c=%lu\n", total_w, total_c);
            break;
        }
        /* playback: hold ~2 periods of fill, write the scheduled source */
        snd_pcm_sframes_t pavail = snd_pcm_avail_update(pb);
        if (pavail < 0 && pavail != -EAGAIN) die(pb, "pb xrun", pavail);
        if (pavail >= (snd_pcm_sframes_t)(pbuf - fillp * period)) {
            snd_pcm_uframes_t n = period;
            float *tmp = malloc(n * pch * sizeof(float));
            for (snd_pcm_uframes_t i = 0; i < n; i++) {
                unsigned long g = total_w + i;
                float s = 0.0f;
                for (unsigned k = 0; k < reps; k++)
                    if (g >= P[k] && g < P[k] + L) {
                        s = chirp[(g - P[k]) * pch];
                        break;
                    }
                tmp[i * pch] = s;
                tmp[i * pch + 1] = s;
            }
            if (next_rep < reps && total_w <= P[next_rep] &&
                total_w + n > P[next_rep]) {
                snd_pcm_sframes_t delay = 0;
                snd_pcm_delay(pb, &delay);
                tw[next_rep] = now_s();
                fillw[next_rep] = delay > 0 ? (unsigned long)delay : 0;
                next_rep++;
            }
            write_frames(pb, tmp, pch, n);
            free(tmp);
            total_w += n;
        }
        /* capture: drain everything available, keep channel 4 */
        snd_pcm_sframes_t cavail = snd_pcm_avail_update(cap);
        if (cavail < 0 && cavail != -EAGAIN) die(pb, "cap xrun", cavail);
        while (cavail >= (snd_pcm_sframes_t)period) {
            const snd_pcm_channel_area_t *areas;
            snd_pcm_uframes_t off, n = cavail;
            err = snd_pcm_mmap_begin(cap, &areas, &off, &n);
            if (err < 0) die(pb, "mmap_begin(r)", err);
            if (off != total_c % cbuf) {
                fprintf(stderr, "capture gap: off=%lu expected=%lu\n",
                        (unsigned long)off, total_c % cbuf);
                exit(1);
            }
            double tc = now_s();
            for (snd_pcm_uframes_t i = 0; i < n; i++) {
                float *src = (float *)((char *)areas[capch].addr +
                              (areas[capch].first + (off + i) * areas[capch].step) / 8);
                fwrite(src, sizeof(float), 1, fcap);
            }
            fprintf(fchunks, "%.6f %lu %lu\n", tc, total_c, (unsigned long)n);
            snd_pcm_mmap_commit(cap, off, n);
            total_c += n;
            cavail -= n;
        }
        usleep(50);
    }
    fclose(fcap);
    fclose(fchunks);

    snprintf(path, sizeof path, "%s/meta.txt", outdir);
    FILE *fm = fopen(path, "w");
    fprintf(fm, "rate %u\nperiod %u\npbuf %u\ncbuf %u\nL %u\nreps %u\n",
            rate, period, pbuf, cbuf, L, reps);
    for (unsigned k = 0; k < reps; k++)
        fprintf(fm, "P %lu tw %.6f fill %lu\n", P[k], tw[k], fillw[k]);
    fclose(fm);
    printf("captured %lu frames, wrote %lu; P:", total_c, total_w);
    for (unsigned k = 0; k < reps; k++) printf(" %lu", P[k]);
    printf("\nfiles in %s; run o1-analyze.py\n", outdir);
    snd_pcm_drop(pb);
    snd_pcm_drop(cap);
    snd_pcm_close(pb);
    snd_pcm_close(cap);
    return 0;
}
