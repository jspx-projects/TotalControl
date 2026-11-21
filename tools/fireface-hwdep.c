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

static void usage(void)
{
	fprintf(stderr,
		"usage: fireface-hwdep <counters|envelope|clock|info|read|write|probe> [args] [node]\n");
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
	else usage();
	return 0;
}
