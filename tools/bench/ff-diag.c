// ff-diag — full hwdep counters + achievable-envelope dump.
//   ff-diag [hwdep-node]      (default /dev/snd/hwC0D0)
// Build: gcc -O2 -I<checkout>/kernel -o ff-diag ff-diag.c
#include <stdio.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <linux/types.h>
#include "ff-uapi.h"

int main(int argc, char **argv)
{
	const char *path = argc > 1 ? argv[1] : "/dev/snd/hwC0D0";
	int fd = open(path, O_RDONLY);
	if (fd < 0) { perror(path); return 1; }
	struct ff_hwdep_diagnostics c;
	if (ioctl(fd, FF_HWDEP_IOCTL_DIAGNOSTICS, &c) < 0) { perror("ioctl counters"); return 1; }
#define P(f) printf("%-18s %u\n", #f, c.f)
	P(out_urbs); P(out_gaps); P(out_shorts); P(out_status_errors);
	P(urb_errors); P(recoveries); P(events_dropped);
	P(in_urbs); P(in_gaps); P(in_shorts); P(in_status_errors);
	P(out_underruns); P(in_overruns);
	P(out_capacity_err); P(out_pkt_len_err); P(out_pkt_status);
	P(in_status_err); P(in_short); P(in_misaligned);
	P(in_bad_length); P(out_bad_length);
	P(in_pos_gap); P(in_frames_mismatch); P(rate_mismatch);
	P(out_fb_ahead); P(out_fb_stale);
	P(hw_ptr_mismatch); P(delta_zero);
#undef P
	struct ff_hwdep_envelope e;
	if (ioctl(fd, FF_HWDEP_IOCTL_ENVELOPE, &e) == 0) {
		printf("envelope: urb_frames=%u nurbs=%u iso=%u\n",
		       e.urb_frames, e.nurbs, e.iso);
		for (unsigned i = 0; i < e.count; i++)
			printf("  %6u Hz  floor %4u  bufmin %5u\n",
			       e.rates[i].rate, e.rates[i].period_floor,
			       e.rates[i].buffer_min);
	}
	close(fd);
	return 0;
}