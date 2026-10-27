#pragma once
extern "C" {
void zcb_init(const char* saveDir);
void zcb_set_visible(bool v);
bool zcb_is_visible();
bool zcb_wants_keyboard();
void zcb_frame(float w, float h);
bool zcb_hit(float x, float y);
void zcb_touch(int phase, int id, float x, float y);
void zcb_text(const char* s);
void zcb_key(int key); // 0 backspace, 1 enter
int zcb_poll_url(char* buf, unsigned long cap);

// --- live clickbot
void zcb_live_action(int button, bool player2, bool push);
void zcb_live_set_level(bool v);
void zcb_live_set_time(double t);
void zcb_live_reset();
void zcb_live_tick(double now);
bool zcb_live_force_p2();
bool zcb_live_take_reset();
int zcb_live_next_pcm(const short** data, unsigned* frames, unsigned* rate, bool* looped);
bool zcb_live_poll_play(unsigned* id, float* vol, float* pitch);
bool zcb_live_noise(unsigned* id, float* vol);
}
