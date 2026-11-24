// rdopts — read the device status/options words once via hwdep vendor reads.
// Prints request 0x19 (rate/status), 0x11 (idle/watch mode) and 0x10 (options).
//   rdopts <hwdep-node>            (e.g. /dev/snd/hwC0D0)
// Build: gcc -O2 -I<checkout>/kernel -o rdopts rdopts.c
#include <stdio.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <linux/types.h>
#include "ffuc-uapi.h"
int main(int argc,char**argv){
	int fd=open(argv[1],O_RDONLY); if(fd<0)return 1;
	struct ffuc_hwdep_vendor v; unsigned int w;
	unsigned char reqs[]={0x19,0x11,0x10};
	for(int i=0;i<3;i++){
		memset(&v,0,sizeof(v)); v.request=reqs[i]; v.direction=1; v.length=4;
		if(ioctl(fd,FFUC_HWDEP_IOCTL_VENDOR,&v)==0){memcpy(&w,v.data,4);printf("req %02x = %08x\n",reqs[i],w);}
	}
	return 0;
}
