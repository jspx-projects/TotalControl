// SPDX-License-Identifier: GPL-2.0-only
// Direct ALSA duplex cushion probe. Controls the playback RING FILL
// (appl_ptr - hw_ptr), not snd_pcm_delay, so the measured margin is the same
// quantity as the PipeWire api.alsa.headroom cushion. Runs a capture stream
// too, so the driver's duplex gate is exercised with equal periods.
//
//   alsa-cushion-probe <card> <rate> <play_period> <cap_period> <seconds>
//                      <target_fill> <stall_at_ms> <stall_ms> <phase>
//
// phase 0 = start the stall at the requested time
// phase 1 = start it immediately after the next playback write (fill at max)
// phase 2 = start it just before the next playback write (fill at min)
//
// Prints the ring fill at the stall start. The driver counters
// (out_underruns, ring_starved, ring_min_filled) are read externally.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <time.h>
#include <poll.h>
#include <stdbool.h>
#include <alsa/asoundlib.h>

struct pcm {
	snd_pcm_t *h;
	snd_pcm_uframes_t period;
	snd_pcm_uframes_t buffer;
	unsigned channels;
	float *buf;
};

static int setup(struct pcm *p, const char *dev, snd_pcm_stream_t stream,
		 unsigned rate, snd_pcm_uframes_t period,
		 snd_pcm_uframes_t buffer, unsigned channels)
{
	p->channels = channels;
	p->period = period;
	p->buffer = buffer;
	int err = snd_pcm_open(&p->h, dev, stream, SND_PCM_NONBLOCK);
	if (err < 0) {
		fprintf(stderr, "open %s: %s\n", dev, snd_strerror(err));
		return -1;
	}
	snd_pcm_hw_params_t *hw;
	snd_pcm_hw_params_alloca(&hw);
	snd_pcm_hw_params_any(p->h, hw);
	snd_pcm_hw_params_set_access(p->h, hw, SND_PCM_ACCESS_RW_INTERLEAVED);
	snd_pcm_hw_params_set_format(p->h, hw, SND_PCM_FORMAT_FLOAT_LE);
	snd_pcm_hw_params_set_channels(p->h, hw, channels);
	snd_pcm_hw_params_set_rate(p->h, hw, rate, 0);
	int dir = 0;
	snd_pcm_hw_params_set_period_size_near(p->h, hw, &p->period, &dir);
	snd_pcm_hw_params_set_buffer_size_near(p->h, hw, &p->buffer);
	err = snd_pcm_hw_params(p->h, hw);
	if (err < 0) {
		fprintf(stderr, "hw_params: %s\n", snd_strerror(err));
		return -1;
	}
	snd_pcm_hw_params_get_period_size(hw, &p->period, &dir);
	snd_pcm_hw_params_get_buffer_size(hw, &p->buffer);
	fprintf(stderr, "%s: period=%lu buffer=%lu\n",
		stream == SND_PCM_STREAM_PLAYBACK ? "play" : "capture",
		(unsigned long)p->period, (unsigned long)p->buffer);
	snd_pcm_sw_params_t *sw;
	snd_pcm_sw_params_alloca(&sw);
	snd_pcm_sw_params_current(p->h, sw);
	snd_pcm_sw_params_set_avail_min(p->h, sw, p->period);
	snd_pcm_sw_params_set_start_threshold(p->h, sw, p->buffer);
	snd_pcm_sw_params(p->h, sw);
	snd_pcm_prepare(p->h);
	p->buf = calloc(p->period * channels, sizeof(float));
	return p->buf ? 0 : -1;
}

/* Ring fill (appl_ptr - hw_ptr), in frames. */
static long ring_fill(struct pcm *p)
{
	snd_pcm_sframes_t avail = snd_pcm_avail_update(p->h);
	if (avail < 0)
		return -1;
	return (long)p->buffer - (long)avail;
}

int main(int argc, char **argv)
{
	if (argc < 10) {
		fprintf(stderr,
			"usage: %s <card> <rate> <play_period> <cap_period> <seconds> "
			"<target_fill> <stall_at_ms> <stall_ms> <phase 0|1|2>\n",
			argv[0]);
		return 2;
	}
	char dev[32];
	snprintf(dev, sizeof(dev), "hw:%s,0", argv[1]);
	unsigned rate = strtoul(argv[2], NULL, 0);
	snd_pcm_uframes_t play_period = strtoul(argv[3], NULL, 0);
	snd_pcm_uframes_t cap_period = strtoul(argv[4], NULL, 0);
	int seconds = atoi(argv[5]);
	snd_pcm_uframes_t target = strtoul(argv[6], NULL, 0);
	long stall_at_ms = strtol(argv[7], NULL, 0);
	long stall_ms = strtol(argv[8], NULL, 0);
	int phase = atoi(argv[9]);
	unsigned channels = 2;

	struct pcm pb = {0}, cap = {0};
	if (setup(&pb, dev, SND_PCM_STREAM_PLAYBACK, rate, play_period,
		  play_period * 32, channels) < 0)
		return 1;
	if (setup(&cap, dev, SND_PCM_STREAM_CAPTURE, rate, cap_period,
		  cap_period * 32, channels) < 0)
		return 1;

	/* Prefill the playback ring to the target ring fill. */
	snd_pcm_sframes_t w = 0;
	for (snd_pcm_uframes_t done = 0; done < target;) {
		w = snd_pcm_writei(pb.h, pb.buf, target - done);
		if (w < 0) {
			fprintf(stderr, "prefill: %s\n", snd_strerror((int)w));
			return 1;
		}
		done += w;
	}
	if (snd_pcm_start(pb.h) < 0 || snd_pcm_start(cap.h) < 0) {
		fprintf(stderr, "start failed\n");
		return 1;
	}

	struct pollfd pfd[2];
	snd_pcm_poll_descriptors(pb.h, &pfd[0], 1);
	snd_pcm_poll_descriptors(cap.h, &pfd[1], 1);

	struct timespec t0, now;
	clock_gettime(CLOCK_MONOTONIC, &t0);
	long xruns = 0;
	long long stall_until_ms = -1;
	long stall_fill = -1;
	bool armed = false;
	bool stall_done = false;
	while (1) {
		clock_gettime(CLOCK_MONOTONIC, &now);
		long long elapsed_ms = (now.tv_sec - t0.tv_sec) * 1000LL +
				       (now.tv_nsec - t0.tv_nsec) / 1000000;
		if (elapsed_ms >= seconds * 1000LL)
			break;

		long fill = ring_fill(&pb);
		if (fill < 0) {
			xruns++;
			break;
		}
		if (stall_ms > 0 && !stall_done) {
			bool start = false;
			if (phase == 0 && elapsed_ms >= stall_at_ms)
				start = true;
			else if (phase == 1 && elapsed_ms >= stall_at_ms) {
				/* after the next successful write */
				if (armed)
					start = true;
			} else if (phase == 2 && elapsed_ms >= stall_at_ms) {
				/* just before the next write: fill at its minimum */
				if (fill <= (long)target - (long)pb.period + 48)
					start = true;
				else if (elapsed_ms > stall_at_ms + 500)
					start = true;
			}
			if (start) {
				stall_fill = fill;
				stall_until_ms = elapsed_ms + stall_ms;
				stall_done = true;
				fprintf(stderr,
					"stall %ld ms at %lld ms, ring fill %ld frames\n",
					stall_ms, elapsed_ms, fill);
			}
		}
		if (stall_until_ms >= 0 && elapsed_ms >= stall_until_ms)
			stall_until_ms = -1;

		poll(pfd, 2, 5);

		if (pfd[1].revents & (POLLIN | POLLERR)) {
			snd_pcm_sframes_t avail = snd_pcm_avail_update(cap.h);
			if (avail < 0) {
				xruns++;
				break;
			}
			if (avail > 0) {
				snd_pcm_uframes_t n = (snd_pcm_uframes_t)avail;
				if (n > cap.period)
					n = cap.period;
				if (snd_pcm_readi(cap.h, cap.buf, n) == -EPIPE) {
					xruns++;
					break;
				}
			}
		}
		if ((pfd[0].revents & (POLLOUT | POLLERR)) && stall_until_ms < 0) {
			fill = ring_fill(&pb);
			if (fill < 0) {
				xruns++;
				break;
			}
			if (fill < (long)target) {
				snd_pcm_uframes_t n = target - fill;
				if (n > pb.period)
					n = pb.period;
				w = snd_pcm_writei(pb.h, pb.buf, n);
				if (w == -EPIPE) {
					xruns++;
					break;
				}
				if (w > 0 && phase == 1)
					armed = true;
			}
		}
	}
	fprintf(stderr, "done: app_xruns=%ld stall_fill=%ld\n", xruns, stall_fill);
	snd_pcm_drop(pb.h);
	snd_pcm_drop(cap.h);
	snd_pcm_close(pb.h);
	snd_pcm_close(cap.h);
	free(pb.buf);
	free(cap.buf);
	return xruns ? 1 : 0;
}
