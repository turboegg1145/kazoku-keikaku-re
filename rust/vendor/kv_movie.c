// pl_mpeg 的薄封装：只暴露扁平接口给 Rust，避免在 Rust 里镜像结构体。
//
// 用法：kv_open → kv_decode 每帧一次 → kv_rgba 取画面 / kv_audio 取声音
//       → kv_ended 判断结束 → kv_close。
//
// pl_mpeg 是 Dominic Szablewski 的 MIT 单文件解码器（vendor/pl_mpeg.h）。

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PL_MPEG_IMPLEMENTATION
#include "pl_mpeg.h"

#define KV_PCM_CAP (44100 * 2 * 4) // 约 4 秒，够一帧的解码量

typedef struct {
    plm_t *plm;
    uint8_t *rgba;   // src_w * src_h * 4
    int w, h;
    int fresh;       // 有新帧还没被 kv_rgba 取走
    int16_t *pcm;    // 交错立体声，环形
    int pcm_cap, pcm_len;
    int rate;
    int ended;
} kv_movie;

static void on_video(plm_t *plm, plm_frame_t *frame, void *user) {
    (void)plm;
    kv_movie *m = (kv_movie *)user;
    int w = (int)frame->width, h = (int)frame->height;
    if (w <= 0 || h <= 0) {
        return;
    }
    if (!m->rgba || m->w != w || m->h != h) {
        free(m->rgba);
        m->rgba = (uint8_t *)malloc((size_t)w * (size_t)h * 4);
        m->w = w;
        m->h = h;
    }
    plm_frame_to_rgba(frame, m->rgba, w * 4);
    // pl_mpeg 不写 alpha，这里补成不透明（画布是按 alpha 混合的）。
    for (int i = 3; i < w * h * 4; i += 4) {
        m->rgba[i] = 255;
    }
    m->fresh = 1;
}

static void on_audio(plm_t *plm, plm_samples_t *s, void *user) {
    (void)plm;
    kv_movie *m = (kv_movie *)user;
    int n = (int)s->count * 2;
    if (!m->pcm || m->pcm_len + n > m->pcm_cap) {
        return; // 塞不下就丢，宁可少几毫秒也不要卡住主线程
    }
    for (int i = 0; i < n; i++) {
        float v = s->interleaved[i];
        if (v > 1.0f) v = 1.0f;
        if (v < -1.0f) v = -1.0f;
        m->pcm[m->pcm_len + i] = (int16_t)(v * 32767.0f);
    }
    m->pcm_len += n;
}

void *kv_open(const char *path) {
    kv_movie *m = (kv_movie *)calloc(1, sizeof(kv_movie));
    if (!m) {
        return NULL;
    }
    m->plm = plm_create_with_filename(path);
    if (!m->plm) {
        free(m);
        return NULL;
    }
    plm_set_loop(m->plm, 0);
    plm_set_audio_enabled(m->plm, 1);
    plm_set_video_decode_callback(m->plm, on_video, m);
    plm_set_audio_decode_callback(m->plm, on_audio, m);
    m->rate = plm_get_samplerate(m->plm);
    if (m->rate <= 0) {
        m->rate = 44100;
    }
    m->pcm_cap = KV_PCM_CAP;
    m->pcm = (int16_t *)malloc((size_t)m->pcm_cap * sizeof(int16_t));
    return m;
}

int kv_info(void *ctx, int *w, int *h, double *fps, double *dur, int *rate) {
    kv_movie *m = (kv_movie *)ctx;
    if (!m) {
        return 0;
    }
    if (w) *w = plm_get_width(m->plm);
    if (h) *h = plm_get_height(m->plm);
    if (fps) *fps = plm_get_framerate(m->plm);
    if (dur) *dur = plm_get_duration(m->plm);
    if (rate) *rate = m->rate;
    return 1;
}

// 推进 dt 秒。返回 1 = 这一拍解出了新画面。
int kv_decode(void *ctx, double dt) {
    kv_movie *m = (kv_movie *)ctx;
    if (!m || m->ended) {
        return 0;
    }
    if (dt < 0.0) dt = 0.0;
    if (dt > 0.25) dt = 0.25; // 卡顿时不要一口气补太多帧
    plm_decode(m->plm, dt);
    if (plm_has_ended(m->plm)) {
        m->ended = 1;
    }
    int fresh = m->fresh;
    m->fresh = 0;
    return fresh;
}

// 取当前画面（RGBA）。返回 1 = 有画面可贴。
int kv_rgba(void *ctx, uint8_t *out, int stride) {
    kv_movie *m = (kv_movie *)ctx;
    if (!m || !m->rgba || !out) {
        return 0;
    }
    for (int y = 0; y < m->h; y++) {
        memcpy(out + (size_t)y * stride, m->rgba + (size_t)y * m->w * 4, (size_t)m->w * 4);
    }
    return m->w;
}

// 取解码好的 PCM（交错立体声 i16），最多 max_samples 个。返回实际个数。
int kv_audio(void *ctx, int16_t *out, int max_samples) {
    kv_movie *m = (kv_movie *)ctx;
    if (!m || !out || m->pcm_len <= 0) {
        return 0;
    }
    int n = m->pcm_len < max_samples ? m->pcm_len : max_samples;
    memcpy(out, m->pcm, (size_t)n * sizeof(int16_t));
    memmove(m->pcm, m->pcm + n, (size_t)(m->pcm_len - n) * sizeof(int16_t));
    m->pcm_len -= n;
    return n;
}

int kv_ended(void *ctx) {
    kv_movie *m = (kv_movie *)ctx;
    return m ? m->ended : 1;
}

void kv_close(void *ctx) {
    kv_movie *m = (kv_movie *)ctx;
    if (!m) {
        return;
    }
    plm_destroy(m->plm);
    free(m->rgba);
    free(m->pcm);
    free(m);
}
