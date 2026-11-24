// ffuc-xrun-probe — read host-ring xrun + clock-diagnostic counters and the
// measured device rate, twice (2 s apart) to show deltas. Investigation tool.
// Build: gcc -O2 -I<checkout>/kernel -o ffuc-xrun-probe ffuc-xrun-probe.c
#include <stdio.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <linux/types.h>
#include "ffuc-uapi.h"

static void snap(int fd, struct ffuc_hwdep_counters *c, struct ffuc_hwdep_clock *k) {
    ioctl(fd, FFUC_HWDEP_IOCTL_COUNTERS, c);
    ioctl(fd, FFUC_HWDEP_IOCTL_CLOCK, k);
}
static void line(const char *tag, struct ffuc_hwdep_counters *c, struct ffuc_hwdep_clock *k) {
    printf("%s underruns=%u overruns=%u rate_mismatch=%u fb_ahead=%u fb_stale=%u hw_ptr_mismatch=%u delta_zero=%u recoveries=%u rate_meas=%u\n",
        tag, c->out_underruns, c->in_overruns, c->rate_mismatch, c->out_fb_ahead,
        c->out_fb_stale, c->hw_ptr_mismatch, c->delta_zero, c->recoveries, k->rate_meas);
}
int main(int argc, char **argv) {
    const char *path = argc > 1 ? argv[1] : "/dev/snd/hwC0D0";
    int fd = open(path, O_RDONLY);
    if (fd < 0) { perror(path); return 1; }
    struct ffuc_hwdep_counters a, b; struct ffuc_hwdep_clock ka, kb;
    snap(fd, &a, &ka); line("t0   ", &a, &ka);
    sleep(2);
    snap(fd, &b, &kb); line("t+2s ", &b, &kb);
    printf("delta underruns=%d overruns=%d rate_mismatch=%d fb_ahead=%d fb_stale=%d hw_ptr_mismatch=%d\n",
        (int)b.out_underruns-(int)a.out_underruns, (int)b.in_overruns-(int)a.in_overruns,
        (int)b.rate_mismatch-(int)a.rate_mismatch, (int)b.out_fb_ahead-(int)a.out_fb_ahead,
        (int)b.out_fb_stale-(int)a.out_fb_stale, (int)b.hw_ptr_mismatch-(int)a.hw_ptr_mismatch);
    close(fd);
    return 0;
}