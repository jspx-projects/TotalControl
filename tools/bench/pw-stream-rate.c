/* pw-stream-rate.c — one playback stream that renegotiates its rate mid-run.
 *
 * Reproduces a gapless player switching tracks of different rates WITHOUT
 * closing the PipeWire stream: the same client calls pw_stream_update_params
 * with a new rate, so the graph must re-prepare a RUNNING stream. This is the
 * path pw-play-per-track (a new client each time) does not exercise.
 *
 * Local bench tool; not part of the product.
 * Build:
 *   gcc -O2 -o pw-stream-rate pw-stream-rate.c \
 *       $(pkg-config --cflags --libs libpipewire-0.3 libspa-0.2)
 * Usage:
 *   pw-stream-rate <target-node-name> <dwell_ms> <rate1,rate2,...>
 * Plays silence; every dwell_ms renegotiates to the next rate (cycling).
 * Logs each renegotiation and the negotiated rate so conduction can be checked
 * against the device clock (fireface-hwdep clock) externally.
 */
#include <pipewire/pipewire.h>
#include <spa/param/audio/format-utils.h>
#include <spa/param/param.h>
#include <spa/utils/result.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

struct app {
	struct pw_main_loop *loop;
	struct pw_context *context;
	struct pw_core *core;
	struct pw_stream *stream;
	struct spa_hook stream_listener;
	int *rates;
	int nrates;
	int idx;
	int dwell_ms;
	uint64_t next_change;
	uint64_t next_measure;
	int measuring;
	uint64_t frames;
};

static int hw_rate(void)
{
	FILE *f = fopen("/proc/asound/card0/pcm0p/sub0/hw_params", "r");
	if (!f) return -1;
	char line[256]; int rate = -1;
	while (fgets(line, sizeof(line), f))
		if (sscanf(line, "rate: %d", &rate) == 1) break;
	fclose(f);
	return rate;
}

static uint64_t now_us(void)
{
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (uint64_t)ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
}

static void on_process(void *data)
{
	struct app *a = data;
	struct pw_buffer *b;
	struct spa_buffer *buf;
	if ((b = pw_stream_dequeue_buffer(a->stream)) == NULL)
		return;
	buf = b->buffer;
	if (buf->datas[0].data) {
		/* silence */
		memset(buf->datas[0].data, 0, buf->datas[0].maxsize);
		buf->datas[0].chunk->offset = 0;
		buf->datas[0].chunk->stride = 8; /* 2ch float */
		buf->datas[0].chunk->size = buf->datas[0].maxsize;
	}
	pw_stream_queue_buffer(a->stream, b);
	a->frames++;

	uint64_t now = now_us();
	if (a->measuring && now >= a->next_measure) {
		int hr = hw_rate();
		int target = a->rates[a->idx];
		printf("  step target=%d hw=%d %s\n", target, hr,
		       hr == target ? "PASS" : "FAIL");
		fflush(stdout);
		a->measuring = 0;
	}
	if (!a->measuring && now >= a->next_change) {
		a->idx = (a->idx + 1) % a->nrates;
		int rate = a->rates[a->idx];
		uint8_t buffer[1024];
		struct spa_pod_builder bld = SPA_POD_BUILDER_INIT(buffer, sizeof(buffer));
		struct spa_pod_frame f;
		spa_pod_builder_push_object(&bld, &f, SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat);
		spa_pod_builder_add(&bld,
			SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_audio),
			SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
			SPA_FORMAT_AUDIO_format, SPA_POD_Id(SPA_AUDIO_FORMAT_F32),
			SPA_FORMAT_AUDIO_rate, SPA_POD_Int(rate),
			SPA_FORMAT_AUDIO_channels, SPA_POD_Int(2),
			0);
		const struct spa_pod *params[1] = { spa_pod_builder_pop(&bld, &f) };
		printf("[%.3f] renegotiate -> %d Hz\n", now/1e6, rate);
		fflush(stdout);
		pw_stream_update_params(a->stream, params, 1);
		/* measure conduction 2.5 s into the dwell, then renegotiate at dwell end */
		a->measuring = 1;
		a->next_measure = now + 2500000;
		a->next_change = now + (uint64_t)a->dwell_ms * 1000;
	}
}

static void on_state_changed(void *data, enum pw_stream_state old,
			     enum pw_stream_state state, const char *error)
{
	printf("[%.3f] stream state %s -> %s%s%s\n", now_us()/1e6,
	       pw_stream_state_as_string(old), pw_stream_state_as_string(state),
	       error ? " error=" : "", error ? error : "");
	fflush(stdout);
}

static const struct pw_stream_events stream_events = {
	PW_VERSION_STREAM_EVENTS,
	.state_changed = on_state_changed,
	.process = on_process,
};

int main(int argc, char **argv)
{
	if (argc < 4) {
		fprintf(stderr, "usage: pw-stream-rate <target> <dwell_ms> <r1,r2,...>\n");
		return 2;
	}
	pw_init(&argc, &argv);
	struct app a = {0};
	a.dwell_ms = atoi(argv[2]);
	/* parse rates */
	char *s = strdup(argv[3]);
	int cap = 16; a.rates = malloc(cap * sizeof(int));
	char *tok = strtok(s, ",");
	while (tok) {
		if (a.nrates == cap) { cap *= 2; a.rates = realloc(a.rates, cap*sizeof(int)); }
		a.rates[a.nrates++] = atoi(tok);
		tok = strtok(NULL, ",");
	}
	free(s);
	if (a.nrates == 0) return 2;

	a.loop = pw_main_loop_new(NULL);
	a.context = pw_context_new(pw_main_loop_get_loop(a.loop), NULL, 0);
	a.core = pw_context_connect(a.context, NULL, 0);
	if (!a.core) { fprintf(stderr, "connect failed\n"); return 1; }

	a.stream = pw_stream_new(a.core, "pw-stream-rate",
		pw_properties_new(PW_KEY_MEDIA_TYPE, "Audio",
				  PW_KEY_MEDIA_CATEGORY, "Playback",
				  PW_KEY_MEDIA_ROLE, "Music",
				  PW_KEY_TARGET_OBJECT, argv[1], NULL));
	pw_stream_add_listener(a.stream, &a.stream_listener, &stream_events, &a);

	uint8_t buffer[1024];
	struct spa_pod_builder bld = SPA_POD_BUILDER_INIT(buffer, sizeof(buffer));
	struct spa_pod_frame f;
	spa_pod_builder_push_object(&bld, &f, SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat);
	spa_pod_builder_add(&bld,
		SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_audio),
		SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
		SPA_FORMAT_AUDIO_format, SPA_POD_Id(SPA_AUDIO_FORMAT_F32),
		SPA_FORMAT_AUDIO_rate, SPA_POD_Int(a.rates[0]),
		SPA_FORMAT_AUDIO_channels, SPA_POD_Int(2),
		0);
	const struct spa_pod *params[1] = { spa_pod_builder_pop(&bld, &f) };

	printf("[%.3f] connect -> %d Hz target=%s\n", now_us()/1e6, a.rates[0], argv[1]);
	fflush(stdout);
	pw_stream_connect(a.stream, PW_DIRECTION_OUTPUT, PW_ID_ANY,
		PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS,
		params, 1);
	a.next_change = now_us() + (uint64_t)a.dwell_ms * 1000;
	pw_main_loop_run(a.loop);
	return 0;
}