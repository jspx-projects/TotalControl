// burst19 — fast-sample the device readiness/status word (request 0x19) via
// hwdep, printing a CLOCK_MONOTONIC timestamp and the word for each sample.
//   burst19 <hwdep-node> <seconds>
// Build: gcc -O2 -I<checkout>/kernel -o burst19 burst19.c
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <time.h>
#include <sys/ioctl.h>
#include <linux/types.h>
#include "ff-uapi.h"
static int fd;
static unsigned long long now_ns(void){struct timespec ts;clock_gettime(CLOCK_MONOTONIC,&ts);return (unsigned long long)ts.tv_sec*1000000000ull+ts.tv_nsec;}
int main(int argc,char**argv){
	fd=open(argv[1],O_RDONLY); if(fd<0){perror("open");return 1;}
	int secs=atoi(argv[2]);
	unsigned long long end=now_ns()+(unsigned long long)secs*1000000000ull;
	struct ff_hwdep_vendor v; unsigned int w;
	while(now_ns()<end){
		memset(&v,0,sizeof(v)); v.request=0x19; v.direction=1; v.length=4;
		if(ioctl(fd,FF_HWDEP_IOCTL_VENDOR,&v)<0){usleep(500);continue;}
		memcpy(&w,v.data,4);
		printf("%llu %08x\n",now_ns(),w);
	}
	return 0;
}
