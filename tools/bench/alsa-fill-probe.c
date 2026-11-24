// SPDX-License-Identifier: GPL-2.0-only
//
// Just-in-time ALSA playback filler with a constant target ring fill.
// Maintains a CONSTANT target ring fill (appl_ptr - hw_ptr) regardless of the
// ring capacity, so capacity can be varied without changing steady-state
// latency. This mirrors JACK's just-in-time fill; a naive blocking writer
// would fill the whole ring and falsely make fill track capacity.
//
//   alsa-fill-probe <rate> <period> <buffer> <target_fill> <seconds>
//
// Plays silence (the stream only needs to run). Sample the ring externally
// with proc-sampler.py; read XRUNs from fireface-hwdep counters.
// Build: gcc -O2 -o alsa-fill-probe alsa-fill-probe.c -lasound
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <alsa/asoundlib.h>

int main(int argc, char **argv)
{
	if (argc < 6) {
		fprintf(stderr,
			"usage: alsa-fill-probe <rate> <period> <buffer> <target_fill> <seconds>\n");
		return 2;
	}
	unsigned rate = atoi(argv[1]);
	snd_pcm_uframes_t period = atoi(argv[2]), buffer = atoi(argv[3]);
	snd_pcm_sframes_t target = atoi(argv[4]);
	int seconds = atoi(argv[5]);

	snd_pcm_t *pcm;
	int err = snd_pcm_open(&pcm, "hw:0,0", SND_PCM_STREAM_PLAYBACK, 0);
	if (err < 0) { fprintf(stderr, "open: %s\n", snd_strerror(err)); return 1; }
	snd_pcm_hw_params_t *hw;
	snd_pcm_hw_params_alloca(&hw);
	snd_pcm_hw_params_any(pcm, hw);
	snd_pcm_hw_params_set_access(pcm, hw, SND_PCM_ACCESS_RW_INTERLEAVED);
	snd_pcm_hw_params_set_format(pcm, hw, SND_PCM_FORMAT_FLOAT_LE);
	snd_pcm_hw_params_set_channels(pcm, hw, 2);
	unsigned r = rate;
	snd_pcm_hw_params_set_rate_near(pcm, hw, &r, 0);
	snd_pcm_uframes_t p = period, b = buffer;
	snd_pcm_hw_params_set_period_size_near(pcm, hw, &p, 0);
	snd_pcm_hw_params_set_buffer_size_near(pcm, hw, &b);
	err = snd_pcm_hw_params(pcm, hw);
	if (err < 0) { fprintf(stderr, "hw_params: %s\n", snd_strerror(err)); return 1; }
	snd_pcm_hw_params_get_period_size(hw, &p, 0);
	snd_pcm_hw_params_get_buffer_size(hw, &b);
	snd_pcm_prepare(pcm);
	fprintf(stderr, "rate=%u period=%lu buffer=%lu target_fill=%ld\n",
		r, (unsigned long)p, (unsigned long)b, (long)target);

	float *buf = calloc(p * 2, sizeof(float));
	snd_pcm_start(pcm);
	long long end_us = (long long)seconds * 1000000;
	long long t0 = 0;
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	t0 = (long long)ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
	long xruns = 0;
	for (;;) {
		clock_gettime(CLOCK_MONOTONIC, &ts);
		long long now = (long long)ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
		if (now - t0 > end_us) break;
		snd_pcm_sframes_t delay = 0;
		if (snd_pcm_delay(pcm, &delay) < 0) {
			snd_pcm_prepare(pcm);
			snd_pcm_start(pcm);
			xruns++;
			continue;
		}
		/* Write just enough to hold the target fill. */
		if (delay < target) {
			snd_pcm_uframes_t n = target - delay;
			if (n > p) n = p;
			snd_pcm_sframes_t w = snd_pcm_writei(pcm, buf, n);
			if (w == -EPIPE) {
				xruns++;
				snd_pcm_prepare(pcm);
				snd_pcm_start(pcm);
			}
		}
		usleep(200);
	}
	snd_pcm_drop(pcm);
	snd_pcm_close(pcm);
	free(buf);
	fprintf(stderr, "done, writer-side xruns=%ld\n", xruns);
	return 0;
}