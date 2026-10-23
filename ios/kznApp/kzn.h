// C ABI（rust/src/capi.rs）的声明。Swift 通过桥接头看到这些函数。
#ifndef KZN_H
#define KZN_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
  KZN_RUN = 0,
  KZN_TEXT = 1,
  KZN_INPUT = 2,
  KZN_CHOICE = 3,
  KZN_HALT = 4,
  KZN_FAILED = 5
};

typedef struct Kzn Kzn;

Kzn *kzn_open(const char *dir, int audio, const char *font);
void kzn_free(Kzn *k);
int  kzn_start(Kzn *k, const char *script);
void kzn_tick(Kzn *k, double dt_ms);
void kzn_pump(Kzn *k, uint32_t budget);
int  kzn_state(Kzn *k);
int  kzn_frame(Kzn *k, uint8_t *out);
void kzn_mouse(Kzn *k, int x, int y);
int  kzn_click(Kzn *k, int x, int y);
int  kzn_audio(Kzn *k, int16_t *out, uint32_t frames, uint32_t rate);
int  kzn_text(Kzn *k, char *out, int cap);
int  kzn_text_total(Kzn *k);
int  kzn_choice_count(Kzn *k, int w);
int  kzn_choice_text(Kzn *k, int w, int i, char *out, int cap);
int  kzn_choice_rect(Kzn *k, int w, int i, int32_t *out);
int  kzn_error(Kzn *k, char *out, int cap);

// 存读档界面（原版是 Win32 对话框，这里按同一套几何重画）
int  kzn_save_open(Kzn *k, int mode);   // mode: 0 = 读档, 1 = 存档
void kzn_save_close(Kzn *k);
int  kzn_save_active(Kzn *k);
void kzn_save_move(Kzn *k, int x, int y);
void kzn_save_page(Kzn *k, int delta);

#ifdef __cplusplus
}
#endif

#endif