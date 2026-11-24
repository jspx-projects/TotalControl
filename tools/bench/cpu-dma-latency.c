/* Hold a PM-QoS CPU_DMA_LATENCY constraint while running.
 * Writing a latency in microseconds to /dev/cpu_dma_latency forbids
 * deeper idle states for as long as the fd stays open.  Group realtime
 * has write access, so no root is needed on this desk.
 *
 *   cpu-dma-latency <us> <seconds>
 */
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(int argc, char **argv)
{
	int us = argc > 1 ? atoi(argv[1]) : 50;
	int secs = argc > 2 ? atoi(argv[2]) : 60;
	int fd = open("/dev/cpu_dma_latency", O_WRONLY);
	if (fd < 0) { perror("/dev/cpu_dma_latency"); return 1; }
	if (write(fd, &us, sizeof(us)) != sizeof(us)) { perror("write"); return 1; }
	printf("latency %d us held for %d s\n", us, secs);
	fflush(stdout);
	sleep(secs);
	close(fd);
	return 0;
}
