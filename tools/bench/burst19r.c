// burst19r — like burst19, but reopens the hwdep node when a read fails so the
// sampling survives a USB reset / driver reattach (prints a REOPEN marker).
//   burst19r <hwdep-node> <seconds>
// Build: gcc -O2 -I<checkout>/kernel -o burst19r burst19r.c
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <time.h>
#include <sys/ioctl.h>
#include <linux/types.h>
#include "ffuc-uapi.h"
static unsigned long long now_ns(void){struct timespec ts;clock_gettime(CLOCK_MONOTONIC,&ts);return (unsigned long long)ts.tv_sec*1000000000ull+ts.tv_nsec;}
int main(int argc,char**argv){
	const char*path=argv[1]; int secs=atoi(argv[2]);
	int fd=open(path,O_RDONLY);
	unsigned long long end=now_ns()+(unsigned long long)secs*1000000000ull;
	struct ffuc_hwdep_vendor v; unsigned int w;
	while(now_ns()<end){
		if(fd<0){fd=open(path,O_RDONLY); if(fd<0){usleep(10000);continue;}
			printf("%llu REOPEN\n",now_ns());}
		memset(&v,0,sizeof(v)); v.request=0x19; v.direction=1; v.length=4;
		if(ioctl(fd,FFUC_HWDEP_IOCTL_VENDOR,&v)<0){close(fd);fd=-1;usleep(5000);continue;}
		memcpy(&w,v.data,4);
		printf("%llu %08x\n",now_ns(),w);
	}
	return 0;
}
