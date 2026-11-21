/* Write IF1 register words via the Fireface hwdep node.
 *
 *   wrword <hwdep-node> <addr-hex> <value-hex> [<addr-hex> <value-hex> ...]
 *
 * Example: route software playback 1 to analog output 5 at unity on the UCX
 * (matrix cell 0x4120 = dsp_output 4 * 0x40 + 0x4000 + playback source 0x20,
 * unity coefficient 0x9000):
 *
 *   wrword /dev/snd/hwC0D0 0x4120 0x9000
 *
 * Build:  gcc -O2 -I<checkout>/kernel -o wrword wrword.c
 *
 * NOTE: a register write changes the device's live mixer state. Read and
 * restore the affected cells when the change is only for a measurement, so
 * the user's scene is preserved.
 *
 * NOTE: do NOT assume an analog loopback cable is present. When using this
 * to set up a playback->output submix for a loopback capture, first verify
 * the signal actually arrives at the intended input (play a tone and check
 * the captured level). A silent capture may mean the cable is absent, the
 * input is muted or its gain is down, not that the submix write failed.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <linux/types.h>
#include "ffuc-uapi.h"

int main(int argc, char **argv)
{
	if (argc < 4 || (argc - 2) % 2) {
		fprintf(stderr, "usage: wrword node addr value [addr value ...]\n");
		return 2;
	}
	int fd = open(argv[1], O_RDWR);
	if (fd < 0) { perror("open"); return 1; }
	struct ffuc_hwdep_words w;
	memset(&w, 0, sizeof(w));
	w.count = (argc - 2) / 2;
	if (w.count > FFUC_HWDEP_MAX_WORDS) {
		fprintf(stderr, "too many words (max %d)\n", FFUC_HWDEP_MAX_WORDS);
		return 2;
	}
	for (__u32 i = 0; i < w.count; i++) {
		w.words[i].addr = (__u16)strtoul(argv[2 + 2 * i], 0, 16);
		w.words[i].value = (__u16)strtoul(argv[3 + 2 * i], 0, 16);
	}
	if (ioctl(fd, FFUC_HWDEP_IOCTL_WRITE_WORDS, &w) < 0) {
		perror("ioctl");
		close(fd);
		return 1;
	}
	for (__u32 i = 0; i < w.count; i++)
		printf("wrote %04x = %04x\n", w.words[i].addr, w.words[i].value);
	close(fd);
	return 0;
}
