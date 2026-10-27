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
}
