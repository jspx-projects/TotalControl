// fireface-hwdep - unified access to the Fireface hwdep node.
//
// Build: gcc -O2 -I<checkout>/kernel -o fireface-hwdep fireface-hwdep.c
//
// Subcommands (node defaults to /dev/snd/hwC0D0; pass any /dev/snd/hwC*D*
// argument to override):
//   fireface-hwdep counters [node]            every counter field, key=value
//   fireface-hwdep envelope [node]            rate/quantum envelope, key=value
//   fireface-hwdep clock [node]               device timestamp + one-second delta
//   fireface-hwdep info [node]                ABI, model, geometry (no serial)
//   fireface-hwdep read <req> [node]          one vendor word (req hex, e.g. 0x19)
//   fireface-hwdep write <addr> <val> [...]   IF1 register words (hex)
//   fireface-hwdep probe <req> <secs> [node]  fast-sample a vendor word, print changes
//   fireface-hwdep dump [secs] [node]         full state dump (0x3f99), addr/value lines
//   fireface-hwdep loopback <dsp> <0|1> [node]  internal loopback flag (EP0 0x15)
//   fireface-hwdep loopback-setup <out> <src> [--cable] [node]
//                                             configure a loopback measurement (UCX)
//
// Loopback measurement recipe (UCX). `loopback-setup` does it in one call:
//   out  output pair: AN1/2 AN3/4 AN5/6 PH7/8 SPDIF ADAT1/2 ADAT3/4 ADAT5/6 ADAT7/8
//   src  source pair: pb1..pb17 (playback) or in1..in7 (analog input); N feeds L, N+1 R
//   --cable  external cable out->in (loopback flag OFF); default internal loopback ON
// It zeroes the output submix, writes unity matrix coefficients, sets the output
// volume to 0 dB, UNMUTES the same-numbered capture inputs, and sets the loopback
// flag. Capture the same-numbered input (AN5/6 -> in5/6). Reproducibility rules,
// each a verified failure cause:
//   - Close the TotalControl GUI first: one shared hwdep event FIFO (a second
//     reader steals dump/state events), and its scene reapply overwrites routing
//     and mutes while you measure.
//   - Free the PCM (card profile Off) so a direct-ALSA tool can open hw:, and so
//     IF1 writes land: they are bulk transfers with a 100 ms timeout that race
//     ISO audio traffic. Write while the PCM is idle; the tool retries on timeout.
//   - A muted capture input is the usual silent-capture cause (input word NN00,
//     bit 0 = mute). loopback-setup unmutes it; verify signal arrives before
//     trusting a silent capture (cable absent vs input muted look the same).
//   - Matrix coefficients (0x4000 + dsp*0x40 + src) are write-only live DSP; the
//     state dump's NNa0 words are fader-derived and do NOT reflect them. Verify
//     routing by audio, not by the dump.
//   - Internal loopback is a digital tap: sample-exact comparison is meaningful
//     (it reveals the device's Q23 internal quantization). A cable path goes
//     through the converters: fractional-sample group delay + noise make it
//     non-sample-exact; use alsa-latency-probe's impulse mode for cable latency.
//
// Device scope: counters/envelope/clock/info use the unified driver ABI and
// work on every supported family. read/probe use EP0 vendor requests (works
// where the device has EP0 control; request semantics are device-specific).
// write uses IF1 register words and needs the IF1 register sink: it works on
// IF1-control devices (UCX, Babyface Pro, UFX+) and returns ENODEV on the UC
// and Babyface-classic, which control the mixer through EP0 vendor requests.
//
// `write` changes the device's live mixer state. Read and restore the
// affected cells when the change is only for a measurement, so the user's
// scene is preserved. Do NOT assume an analog loopback cable is present:
// when routing playback to an output for a loopback capture, verify the
// signal actually arrives at the intended input first. A silent capture may
// mean the cable is absent or the input is muted, not that the write failed.

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <time.h>
#include <errno.h>
#include <sys/ioctl.h>
#include <linux/types.h>
#include "ffuc-uapi.h"

static const char *node = "/dev/snd/hwC0D0";

static unsigned long long now_ns(void)
{
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (unsigned long long)ts.tv_sec * 1000000000ull + ts.tv_nsec;
}

static int open_node(void)
{
	int fd = open(node, O_RDWR);
	if (fd < 0) { perror(node); exit(1); }
	return fd;
}

static int vread(int fd, unsigned char req, unsigned int *word)
{
	struct ffuc_hwdep_vendor v;
	memset(&v, 0, sizeof(v));
	v.request = req;
	v.direction = 1;
	v.length = 4;
	if (ioctl(fd, FFUC_HWDEP_IOCTL_VENDOR, &v) < 0)
		return -1;
	memcpy(word, v.data, 4);
	return 0;
}

static void cmd_counters(void)
{
	int fd = open_node();
	struct ffuc_hwdep_counters c;
	if (ioctl(fd, FFUC_HWDEP_IOCTL_COUNTERS, &c) < 0) { perror("ioctl counters"); exit(1); }
#define P(f) printf(#f "=%u\n", c.f)
	P(out_urbs); P(out_gaps); P(out_shorts); P(out_status_errors);
	P(urb_errors); P(recoveries); P(events_dropped);
	P(meter_urbs); P(state_urbs); P(status_urbs);
	P(in_urbs); P(in_gaps); P(in_shorts); P(in_status_errors);
	P(out_underruns); P(in_overruns);
	P(out_capacity_err); P(out_pkt_len_err); P(out_pkt_status);
	P(in_status_err); P(in_short); P(in_misaligned);
	P(in_bad_length); P(out_bad_length);
	P(in_pos_gap); P(in_frames_mismatch); P(rate_mismatch);
	P(out_fb_ahead); P(out_fb_stale);
	P(hw_ptr_mismatch); P(delta_zero); P(run_seq);
#undef P
	close(fd);
}

static void cmd_envelope(void)
{
	int fd = open_node();
	struct ffuc_hwdep_envelope e;
	if (ioctl(fd, FFUC_HWDEP_IOCTL_ENVELOPE, &e) < 0) { perror("ioctl envelope"); exit(1); }
	printf("iso=%u nurbs=%u urb_frames=%u\n", e.iso, e.nurbs, e.urb_frames);
	for (unsigned i = 0; i < e.count; i++) {
		printf("env_%u_floor=%u\n", e.rates[i].rate, e.rates[i].period_floor);
		printf("env_%u_bufmin=%u\n", e.rates[i].rate, e.rates[i].buffer_min);
	}
	close(fd);
}

static void cmd_clock(void)
{
	int fd = open_node();
	struct ffuc_hwdep_clock c1, c2;
	if (ioctl(fd, FFUC_HWDEP_IOCTL_CLOCK, &c1) < 0) { perror("ioctl clock"); exit(1); }
	usleep(1000000);
	if (ioctl(fd, FFUC_HWDEP_IOCTL_CLOCK, &c2) < 0) { perror("ioctl clock 2"); exit(1); }
	printf("ts_ns %llu pos %llu phase %u rate_meas %u wrap %u frames %llu\n",
	       (unsigned long long)c1.timestamp_ns, (unsigned long long)c1.timestamp_pos,
	       c1.timestamp_phase, c1.rate_meas, c1.wrap, (unsigned long long)c1.frames);
	printf("anchor_ns %llu\n", (unsigned long long)c1.anchor_ns);
	long long d_ns = (long long)c2.timestamp_ns - (long long)c1.timestamp_ns;
	long long d_pos = (long long)c2.timestamp_pos - (long long)c1.timestamp_pos;
	printf("delta: ns %lld pos %lld rate %.3f Hz\n", d_ns, d_pos,
	       d_ns ? (double)d_pos * 1e9 / (double)d_ns : 0.0);
	close(fd);
}

static void cmd_info(void)
{
	int fd = open_node();
	struct ffuc_hwdep_info i;
	if (ioctl(fd, FFUC_HWDEP_IOCTL_INFO, &i) < 0) { perror("ioctl info"); exit(1); }
	/* The device serial is intentionally not printed. */
	printf("abi=%u model=%u transport=%u iso=%u\n", i.abi, i.model, i.transport,
	       i.transport == FFUC_XPORT_ISO ? 1u : 0u);
	printf("channels=%u inputs=%u outputs=%u playback=%u matrix_sources=%u\n",
	       i.channels, i.inputs, i.outputs, i.playback, i.matrix_sources);
	printf("rate=%u firmware=%u card=%u pcm_device=%u name=%s\n",
	       i.rate, i.firmware, i.card, i.pcm_device, i.name);
	close(fd);
}

static void cmd_read(int argc, char **argv)
{
	if (argc < 1) { fprintf(stderr, "usage: fireface-hwdep read <req> [node]\n"); exit(2); }
	unsigned char req = (unsigned char)strtoul(argv[0], 0, 16);
	int fd = open_node();
	unsigned int w = 0;
	if (vread(fd, req, &w) < 0) { perror("vread"); exit(1); }
	printf("req %02x = %08x\n", req, w);
	close(fd);
}

static void cmd_write(int argc, char **argv)
{
	if (argc < 2 || argc % 2) {
		fprintf(stderr, "usage: fireface-hwdep write <addr> <val> [addr val ...] [node]\n");
		exit(2);
	}
	int fd = open_node();
	struct ffuc_hwdep_words w;
	memset(&w, 0, sizeof(w));
	w.count = argc / 2;
	if (w.count > FFUC_HWDEP_MAX_WORDS) { fprintf(stderr, "too many words\n"); exit(2); }
	for (__u32 i = 0; i < w.count; i++) {
		w.words[i].addr = (__u16)strtoul(argv[2 * i], 0, 16);
		w.words[i].value = (__u16)strtoul(argv[2 * i + 1], 0, 16);
	}
	if (ioctl(fd, FFUC_HWDEP_IOCTL_WRITE_WORDS, &w) < 0) { perror("ioctl write"); exit(1); }
	for (__u32 i = 0; i < w.count; i++)
		printf("wrote %04x = %04x\n", w.words[i].addr, w.words[i].value);
	close(fd);
}

static void cmd_probe(int argc, char **argv)
{
	if (argc < 2) { fprintf(stderr, "usage: fireface-hwdep probe <req> <secs> [node]\n"); exit(2); }
	unsigned char req = (unsigned char)strtoul(argv[0], 0, 16);
	int secs = atoi(argv[1]);
	int fd = open_node();
	unsigned int w = 0, prev = ~0u;
	unsigned long long end = now_ns() + (unsigned long long)secs * 1000000000ull;
	unsigned long long last = 0;
	while (now_ns() < end) {
		if (vread(fd, req, &w) < 0) { usleep(1000); continue; }
		unsigned long long t = now_ns();
		if (w != prev || t - last > 1000000000ull) {
			printf("%llu req %02x = %08x (bits0-1=%u)%s\n",
			       t, req, w, w & 3, (w != prev) ? " *" : "");
			prev = w; last = t;
		}
		usleep(200);
	}
	close(fd);
}

/* Send IF1 words in device-sized chunks, retrying on the bulk-transfer timeout
 * that races ISO audio traffic. Writes land reliably while the PCM is idle. */
static int write_words_retry(int fd, const struct ffuc_hwdep_word *w,
			     unsigned int count)
{
	unsigned int done = 0;
	while (done < count) {
		struct ffuc_hwdep_words b;
		unsigned int n = count - done;
		if (n > FFUC_HWDEP_MAX_WORDS)
			n = FFUC_HWDEP_MAX_WORDS;
		memset(&b, 0, sizeof(b));
		b.count = n;
		memcpy(b.words, w + done, n * sizeof(*w));
		int ret = -1;
		for (int try = 0; try < 5; try++) {
			ret = ioctl(fd, FFUC_HWDEP_IOCTL_WRITE_WORDS, &b);
			if (ret >= 0)
				break;
			if (errno != ETIMEDOUT && errno != EAGAIN)
				break;
			usleep(50000);
		}
		if (ret < 0)
			return -1;
		done += n;
	}
	return 0;
}

static int vendor_out(int fd, unsigned char req, unsigned int value,
		      unsigned int index)
{
	struct ffuc_hwdep_vendor v;
	memset(&v, 0, sizeof(v));
	v.request = req;
	v.direction = 0;
	v.value = value;
	v.index = index;
	return ioctl(fd, FFUC_HWDEP_IOCTL_VENDOR, &v);
}

static void cmd_dump(int argc, char **argv)
{
	int secs = argc > 0 ? atoi(argv[0]) : 3;
	int fd = open_node();
	struct ffuc_hwdep_word trig = { 0x3f99, 0x0812 };
	if (write_words_retry(fd, &trig, 1) < 0) {
		perror("dump trigger"); exit(1);
	}
	static __u16 val[0x8000];
	static char seen[0x8000];
	unsigned char buf[4096];
	unsigned long long end = now_ns() + (unsigned long long)secs * 1000000000ull;
	unsigned int nwords = 0;
	while (now_ns() < end) {
		ssize_t r = read(fd, buf, sizeof(buf));
		if (r < (ssize_t)sizeof(struct ffuc_hwdep_event_hdr)) {
			usleep(2000);
			continue;
		}
		struct ffuc_hwdep_event_hdr *h = (void *)buf;
		if (h->type != FFUC_EV_STATE)
			continue;
		unsigned char *p = buf + sizeof(*h);
		for (unsigned int o = 0; o + 4 <= h->size; o += 4) {
			__u32 raw = p[o] | (p[o+1]<<8) | (p[o+2]<<16) | ((__u32)p[o+3]<<24);
			__u16 a = (raw >> 16) & 0x7fff;
			if (!seen[a]) { seen[a] = 1; nwords++; }
			val[a] = raw & 0xffff;
		}
	}
	for (unsigned int a = 0; a < 0x8000; a++)
		if (seen[a])
			printf("%04x %04x\n", a, val[a]);
	fprintf(stderr, "%u words\n", nwords);
	close(fd);
}

static void cmd_loopback(int argc, char **argv)
{
	if (argc < 2) {
		fprintf(stderr, "usage: fireface-hwdep loopback <dsp-idx> <0|1> [node]\n");
		exit(2);
	}
	unsigned int idx = strtoul(argv[0], 0, 0);
	unsigned int on = strtoul(argv[1], 0, 0);
	int fd = open_node();
	if (vendor_out(fd, 0x15, on, idx) < 0) { perror("loopback"); exit(1); }
	printf("loopback dsp=%u on=%u\n", idx, on);
	close(fd);
}

/* UCX output-pair name -> first DSP output channel. */
static int ucx_out_dsp(const char *name)
{
	static const struct { const char *n; int dsp; } t[] = {
		{"AN1/2",0},{"AN3/4",2},{"AN5/6",4},{"PH7/8",6},{"SPDIF",8},
		{"ADAT1/2",10},{"ADAT3/4",12},{"ADAT5/6",14},{"ADAT7/8",16},
	};
	for (unsigned i = 0; i < sizeof(t)/sizeof(t[0]); i++)
		if (!strcasecmp(name, t[i].n))
			return t[i].dsp;
	return -1;
}

static void cmd_loopback_setup(int argc, char **argv)
{
	if (argc < 2) {
		fprintf(stderr,
			"usage: fireface-hwdep loopback-setup <out-pair> <pbN|inN> [--cable] [node]\n");
		exit(2);
	}
	int cable = 0;
	for (int i = 2; i < argc; i++)
		if (!strcmp(argv[i], "--cable"))
			cable = 1;
	int dsp = ucx_out_dsp(argv[0]);
	if (dsp < 0) { fprintf(stderr, "unknown output pair '%s'\n", argv[0]); exit(2); }
	int src_base, src_is_pb;
	if (!strncasecmp(argv[1], "pb", 2)) {
		src_is_pb = 1; src_base = atoi(argv[1] + 2) - 1;
	} else if (!strncasecmp(argv[1], "in", 2)) {
		src_is_pb = 0; src_base = atoi(argv[1] + 2) - 1;
	} else { fprintf(stderr, "source must be pbN or inN\n"); exit(2); }
	if (src_base < 0) { fprintf(stderr, "source index out of range\n"); exit(2); }

	int fd = open_node();
	struct ffuc_hwdep_info info;
	if (ioctl(fd, FFUC_HWDEP_IOCTL_INFO, &info) < 0) { perror("info"); exit(1); }
	if (info.model != FFUC_MODEL_UCX) {
		fprintf(stderr, "loopback-setup is UCX-only (model %u)\n", info.model);
		exit(2);
	}

	struct ffuc_hwdep_word w[256];
	unsigned int n = 0;
	/* Zero both output sides' submix, then unity coefficients for the
	 * source pair (L->side0, R->side1). Unity coarse = 0x9000. */
	for (int side = 0; side < 2; side++) {
		unsigned int base = 0x4000 + (dsp + side) * 0x40;
		for (int s = 0; s < 0x40; s++)
			w[n++] = (struct ffuc_hwdep_word){ base + s, 0 };
		unsigned int src = src_is_pb ? 0x20 + src_base + side
					     : (unsigned int)src_base + side;
		w[n++] = (struct ffuc_hwdep_word){ base + src, 0x9000 };
		/* Output volume 0 dB (NN00). */
		w[n++] = (struct ffuc_hwdep_word){ (0x12 + dsp + side) << 8, 0 };
	}
	/* Unmute the same-numbered capture inputs (input word NN00 bit0=mute).
	 * AN5/6 output loops to in5/6; input index = dsp/2 pair * 2 + side. */
	for (int side = 0; side < 2; side++) {
		int in_idx = dsp + side;   /* UCX: input N mirrors output DSP N */
		w[n++] = (struct ffuc_hwdep_word){ in_idx << 8, 0 };
	}
	if (write_words_retry(fd, w, n) < 0) {
		perror("loopback-setup writes (is the PCM idle?)"); exit(1);
	}
	/* Internal loopback flag per DSP output channel; off for a cable. */
	for (int side = 0; side < 2; side++)
		if (vendor_out(fd, 0x15, cable ? 0 : 1, dsp + side) < 0)
			perror("loopback flag");
	printf("loopback-setup %s <- %s%d/%d (%s)\n", argv[0],
	       src_is_pb ? "pb" : "in", src_base + 1, src_base + 2,
	       cable ? "cable, loopback OFF" : "internal loopback ON");
	/* The loopback capture input mirrors the DSP channel only for the
	 * analog outputs (AN1/2..AN5/6, dsp 0..5), verified by measurement.
	 * Phones/SPDIF/ADAT loopback capture channels are not verified. */
	if (dsp <= 5)
		printf("  capture input %d/%d (0-based %d/%d); output volume 0 dB, inputs unmuted\n",
		       dsp + 1, dsp + 2, dsp, dsp + 1);
	else
		printf("  output volume 0 dB; capture input for this pair is UNVERIFIED — scan the capture channels\n");
	close(fd);
}

static void usage(void)
{
	fprintf(stderr,
		"usage: fireface-hwdep <counters|envelope|clock|info|read|write|probe|dump|loopback|loopback-setup> [args] [node]\n");
	exit(2);
}

int main(int argc, char **argv)
{
	if (argc < 2) usage();
	const char *cmd = argv[1];
	/* Collect positional args; any /dev/ argument overrides the node. */
	char *pos[64];
	int npos = 0;
	for (int i = 2; i < argc; i++) {
		if (!strncmp(argv[i], "/dev/", 5)) node = argv[i];
		else if (npos < 64) pos[npos++] = argv[i];
	}
	if (!strcmp(cmd, "counters")) cmd_counters();
	else if (!strcmp(cmd, "envelope")) cmd_envelope();
	else if (!strcmp(cmd, "clock")) cmd_clock();
	else if (!strcmp(cmd, "info")) cmd_info();
	else if (!strcmp(cmd, "read")) cmd_read(npos, pos);
	else if (!strcmp(cmd, "write")) cmd_write(npos, pos);
	else if (!strcmp(cmd, "probe")) cmd_probe(npos, pos);
	else if (!strcmp(cmd, "dump")) cmd_dump(npos, pos);
	else if (!strcmp(cmd, "loopback")) cmd_loopback(npos, pos);
	else if (!strcmp(cmd, "loopback-setup")) cmd_loopback_setup(npos, pos);
	else usage();
	return 0;
}
