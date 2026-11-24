/* clock-write.c — consolidated device-clock write tool (bench, local).
 *
 * Combines the former vendor-req.c and pitch-write.c: one place to write any
 * RME clock word via hwdep and read the device clock (rate_meas) before/after,
 * so a clock disturbance is attributable to the write.
 *
 *   clock-write vendor <req-hex> <val-hex> <idx-hex>   one vendor request
 *   clock-write pitch <percent>                        the 4-word DDS pitch quad
 *   clock-write family <rate>                          the base-family word for rate
 *
 * family: 0 = 32/64/128k, 1 = 44.1/88.2/176.4k, 2 = 48/96/192k. The family word
 * is 0x10, wValue family<<4, wIndex 0x30. pitch writes req 0x1b per base family
 * exactly as totalcontrol-core write_pitch_clock: dds = 0x5f5e10000000 /
 * (base * (1 + pitch/100)).
 *
 * Build: gcc -O2 -I<checkout>/kernel -o clock-write clock-write.c -lm
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <math.h>
#include <sys/ioctl.h>
#include <linux/types.h>
#include "ffuc-uapi.h"

static int fd;
static int vwr(unsigned char req, unsigned short val, unsigned short idx)
{
	struct ffuc_hwdep_vendor v; memset(&v, 0, sizeof(v));
	v.request = req; v.direction = 0; v.value = val; v.index = idx; v.length = 0;
	return ioctl(fd, FFUC_HWDEP_IOCTL_VENDOR, &v);
}
static double rate_meas(void)
{
	struct ffuc_hwdep_clock c;
	if (ioctl(fd, FFUC_HWDEP_IOCTL_CLOCK, &c) < 0) return -1;
	return (double)c.rate_meas;
}
static unsigned family_of(unsigned rate)
{
	switch (rate) {
	case 32000: case 64000: case 128000: return 0;
	case 44100: case 88200: case 176400: return 1;
	default: return 2;
	}
}
/* Device-aware layer: resolve the model from the hwdep node's ALSA card so the
 * protocol-specific writes dispatch per model and new models extend here. The
 * generic `vendor` subcommand is device-agnostic. */
static int card_of_node(const char *node)
{
	const char *p = strstr(node, "hwC");
	return p ? atoi(p + 3) : -1;
}
static const char *model_of_node(const char *node)
{
	static char id[64];
	int card = card_of_node(node);
	if (card < 0) return "unknown";
	char path[128];
	snprintf(path, sizeof(path), "/proc/asound/card%d/id", card);
	FILE *f = fopen(path, "r");
	if (!f) return "unknown";
	if (fgets(id, sizeof(id), f)) id[strcspn(id, "\n")] = 0;
	fclose(f);
	return id;
}
static int model_has_pitch(const char *id)
{
	/* UCX/UC share the four-word DDS pitch quad. Babyface-classic has no
	 * pitch control (its backend returns unsupported). */
	return strstr(id, "Babyface") == NULL;
}
int main(int argc, char **argv)
{
	if (argc < 3) {
		fprintf(stderr, "usage: clock-write <node> vendor <req> <val> <idx> (hex)\n"
				"       clock-write <node> pitch <percent>\n"
				"       clock-write <node> family <rate>\n");
		return 2;
	}
	fd = open(argv[1], O_RDWR);
	if (fd < 0) { perror("open"); return 1; }
	const char *cmd = argv[2];
	double before = rate_meas();
	if (!strcmp(cmd, "vendor") && argc >= 6) {
		unsigned req = strtoul(argv[3], 0, 16), val = strtoul(argv[4], 0, 16),
			 idx = strtoul(argv[5], 0, 16);
		if (vwr(req, val, idx) < 0) { perror("ioctl vendor"); return 1; }
		usleep(300000);
		printf("vendor req=0x%02x val=0x%04x idx=0x%04x: rate_meas %.1f -> %.1f Hz\n",
		       req, val, idx, before, rate_meas());
	} else if (!strcmp(cmd, "pitch") && argc >= 4) {
		const char *model = model_of_node(argv[1]);
		if (!model_has_pitch(model)) {
			fprintf(stderr, "pitch unsupported on model '%s'\n", model);
			return 1;
		}
		double pitch = atof(argv[3]);
		if (pitch < -5.0) pitch = -5.0;
		if (pitch > 5.0) pitch = 5.0;
		const unsigned bases[4] = { 32000, 44100, 48000, 50000 };
		for (int fam = 0; fam < 4; fam++) {
			double d = 0x5f5e10000000ULL / ((double)bases[fam] * (1.0 + pitch / 100.0));
			unsigned long long dds = (unsigned long long)llround(d);
			if (vwr(0x1b, (unsigned short)(dds >> 16),
				(unsigned short)((dds & 0xff00) | fam)) < 0) {
				perror("ioctl vendor DDS"); return 1;
			}
		}
		usleep(300000);
		printf("pitch %.3f%% [%s]: rate_meas %.1f -> %.1f Hz\n",
		       pitch, model, before, rate_meas());
	} else if (!strcmp(cmd, "family") && argc >= 4) {
		unsigned rate = strtoul(argv[3], 0, 10);
		unsigned fam = family_of(rate);
		if (vwr(0x10, fam << 4, 0x30) < 0) { perror("ioctl vendor family"); return 1; }
		usleep(300000);
		printf("family %u (rate %u): rate_meas %.1f -> %.1f Hz\n",
		       fam, rate, before, rate_meas());
	} else {
		fprintf(stderr, "bad subcommand or args\n"); return 2;
	}
	close(fd);
	return 0;
}