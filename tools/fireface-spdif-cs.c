// Read and describe the S/PDIF input channel status.
//
//   fireface-spdif-cs <card>
//
// The driver exposes the 192-bit C block as the standard ALSA
// "IEC958 Capture Default" control.  It is populated from the capture
// stream, so a PCM must be open and the S/PDIF input locked.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <sys/ioctl.h>
#include <sound/asound.h>

static const char *consumer_rate(unsigned int code)
{
	switch (code) {
	case 0: return "44.1 kHz";
	case 2: return "48 kHz";
	case 3: return "32 kHz";
	case 8: return "88.2 kHz";
	case 10: return "96 kHz";
	case 12: return "176.4 kHz";
	case 14: return "192 kHz";
	default: return "rate not indicated";
	}
}

static void describe(const unsigned char *cs)
{
	if (cs[0] & 1) {
		static const char *rate[4] = {
			"rate not indicated", "44.1 kHz", "48 kHz", "32 kHz"
		};
		printf("professional, %s, %s",
		       cs[0] & 2 ? "non-audio" : "audio",
		       rate[(cs[0] >> 6) & 3]);
		if (cs[0] & 0x20)
			printf(", source unlocked");
		if ((cs[2] & 0x3f) == 0x2c)
			printf(", 24 bit");
		if (cs[6] || cs[7] || cs[8] || cs[9]) {
			char origin[5] = { cs[6], cs[7], cs[8], cs[9], 0 };
			printf(", origin \"%s\"", origin);
		}
	} else {
		printf("consumer, %s, %s, %s",
		       cs[0] & 2 ? "non-audio" : "audio",
		       cs[0] & 4 ? "copy permitted" : "copy protected",
		       consumer_rate(cs[3] & 0x0f));
		if ((cs[4] & 0x0f) == 0x0b)
			printf(", 24 bit");
		else if ((cs[4] & 0x0f) == 0x02)
			printf(", 16 bit");
	}
	printf("\n");
}

int main(int argc, char **argv)
{
	struct snd_ctl_elem_value *value;
	const unsigned char *cs;
	char path[64];
	int card, fd, i;

	if (argc < 2) {
		fprintf(stderr, "usage: %s <card>\n", argv[0]);
		return 2;
	}
	card = atoi(argv[1]);
	snprintf(path, sizeof(path), "/dev/snd/controlC%d", card);
	fd = open(path, O_RDONLY);
	if (fd < 0) {
		perror(path);
		return 1;
	}

	value = calloc(1, sizeof(*value));
	if (!value) {
		perror("calloc");
		close(fd);
		return 1;
	}
	value->id.iface = SNDRV_CTL_ELEM_IFACE_PCM;
	strncpy((char *)value->id.name, "IEC958 Capture Default",
		sizeof(value->id.name) - 1);
	if (ioctl(fd, SNDRV_CTL_IOCTL_ELEM_READ, value) < 0) {
		perror("IEC958 Capture Default");
		free(value);
		close(fd);
		return 1;
	}
	cs = value->value.iec958.status;
	printf("S/PDIF in channel status:");
	for (i = 0; i < 24; i++)
		printf(" %02x", cs[i]);
	printf("\n");
	describe(cs);

	free(value);
	close(fd);
	return 0;
}
