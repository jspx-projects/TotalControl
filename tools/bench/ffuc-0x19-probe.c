// ffuc-0x19-probe: read the device readiness register (0x19)
// and the status word (0x11) through the hwdep VENDOR ioctl.
//   ffuc-0x19-probe <hwdep> idle <n>        n reads, 100 ms apart
//   ffuc-0x19-probe <hwdep> watch <secs>    fast sampling, print changes
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <time.h>
#include <sys/ioctl.h>
#include <linux/types.h>
#include "ffuc-uapi.h"

static int fd;
static unsigned long long now_ns(void)
{
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (unsigned long long)ts.tv_sec * 1000000000ull + ts.tv_nsec;
}
static int vread(unsigned char req, unsigned int *word)
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
int main(int argc, char **argv)
{
	if (argc < 3) { fprintf(stderr, "usage: %s <hwdep> idle|watch [n|secs]\n", argv[0]); return 2; }
	fd = open(argv[1], O_RDONLY);
	if (fd < 0) { perror(argv[1]); return 1; }
	int n = argc > 3 ? atoi(argv[3]) : 10;
	unsigned int w19 = 0, w11 = 0, p19 = ~0u, p11 = ~0u;
	if (!strcmp(argv[2], "idle")) {
		for (int i = 0; i < n; i++) {
			if (vread(0x19, &w19) || vread(0x11, &w11)) { perror("vread"); return 1; }
			printf("%llu 0x19=%08x (bits0-1=%u) 0x11=%08x\n",
			       now_ns(), w19, w19 & 3, w11);
			usleep(100000);
		}
	} else {
		unsigned long long end = now_ns() + (unsigned long long)n * 1000000000ull;
		unsigned long long last = 0;
		while (now_ns() < end) {
			if (vread(0x19, &w19) || vread(0x11, &w11)) { usleep(1000); continue; }
			unsigned long long t = now_ns();
			if (w19 != p19 || w11 != p11 || t - last > 1000000000ull) {
				printf("%llu 0x19=%08x (bits0-1=%u) 0x11=%08x%s\n",
				       t, w19, w19 & 3, w11,
				       (w19 != p19 || w11 != p11) ? " *" : "");
				p19 = w19; p11 = w11; last = t;
			}
			usleep(200);
		}
	}
	close(fd);
	return 0;
}