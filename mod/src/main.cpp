#include <Geode/Geode.hpp>
#include <Geode/ui/OverlayManager.hpp>
#include <Geode/modify/CCDirector.hpp>
#include <Geode/modify/CCTouchDispatcher.hpp>
#include <Geode/modify/CCIMEDispatcher.hpp>
#include <Geode/modify/GJBaseGameLayer.hpp>
#include <Geode/modify/PlayLayer.hpp>
#include <GLES2/gl2.h>
#include <set>
#include <string>
#include "zcb_ffi.h"

using namespace geode::prelude;

static CCNode* g_button = nullptr;
static bool g_kb = false;

// cocos view coords (design res, y down) -> framebuffer pixels
static CCPoint toPixels(CCPoint p) {
    auto win = CCDirector::get()->getWinSize();
    auto fr = CCEGLView::get()->getFrameSize();
    return {p.x * fr.width / win.width, p.y * fr.height / win.height};
}

static bool overButton(CCPoint px) {
    if (!g_button || !g_button->isVisible()) return false;
    auto win = CCDirector::get()->getWinSize();
    auto fr = CCEGLView::get()->getFrameSize();
    auto r = g_button->boundingBox();
    float x = r.origin.x * fr.width / win.width, w = r.size.width * fr.width / win.width;
    float y = fr.height - (r.origin.y + r.size.height) * fr.height / win.height,
          h = r.size.height * fr.height / win.height;
    return px.x >= x && px.x <= x + w && px.y >= y && px.y <= y + h;
}

class ZcbButton : public CCNode, public CCTouchDelegate {
    bool m_down = false, m_drag = false;
    CCPoint m_start, m_startPos;

public:
    static ZcbButton* create() {
        auto r = new ZcbButton();
        if (r->init()) { r->autorelease(); return r; }
        delete r;
        return nullptr;
    }
    bool init() override {
        if (!CCNode::init()) return false;
        this->setContentSize({30.f, 30.f});
        this->setAnchorPoint({0.5f, 0.5f});
        this->ignoreAnchorPointForPosition(false);
        auto c = this->getContentSize() / 2;
        auto dot = CCDrawNode::create();
        dot->drawDot(c, 15.f, {0.30f, 0.30f, 0.30f, 1.f});          // thin outline
        dot->drawDot(c, 14.f, {27 / 255.f, 27 / 255.f, 27 / 255.f, 1.f}); // egui dark panel_fill
        this->addChild(dot);
        auto lbl = CCLabelBMFont::create("Z", "bigFont.fnt");
        lbl->setScale(0.5f);
        lbl->setPosition(c);
        this->addChild(lbl);
        auto win = CCDirector::get()->getWinSize();
        this->setPosition({Mod::get()->getSavedValue<float>("btn-x", 40.f),
                           Mod::get()->getSavedValue<float>("btn-y", win.height - 40.f)});
        CCDirector::get()->getTouchDispatcher()->addTargetedDelegate(this, -9999, true);
        return true;
    }
    ~ZcbButton() override {
        CCDirector::get()->getTouchDispatcher()->removeDelegate(this);
    }
    bool ccTouchBegan(CCTouch* t, CCEvent*) override {
        if (!this->isVisible()) return false;
        auto local = this->getParent()->convertToNodeSpace(t->getLocation());
        if (ccpDistance(local, this->getPosition()) > 15.f) return false;
        m_down = true; m_drag = false;
        m_start = t->getLocation(); m_startPos = this->getPosition();
        return true;
    }
    void ccTouchMoved(CCTouch* t, CCEvent*) override {
        if (!m_down) return;
        auto d = t->getLocation() - m_start;
        if (!m_drag && ccpLength(d) > 8.f) m_drag = true;
        if (m_drag) {
            auto win = CCDirector::get()->getWinSize();
            auto p = m_startPos + d;
            p.x = std::clamp(p.x, 15.f, win.width - 15.f);
            p.y = std::clamp(p.y, 15.f, win.height - 15.f);
            this->setPosition(p);
        }
    }
    void ccTouchEnded(CCTouch*, CCEvent*) override {
        if (!m_down) return;
        m_down = false;
        if (m_drag) {
            Mod::get()->setSavedValue("btn-x", this->getPositionX());
            Mod::get()->setSavedValue("btn-y", this->getPositionY());
        } else {
            zcb_set_visible(!zcb_is_visible());
        }
    }
    void ccTouchCancelled(CCTouch*, CCEvent*) override { m_down = false; }
};

$on_mod(Loaded) {
    zcb_init(Mod::get()->getSaveDir().string().c_str());
    Loader::get()->queueInMainThread([] {
        auto btn = ZcbButton::create();
        if (!btn) return;
        g_button = btn;
        OverlayManager::get()->addChild(btn);
    });
}


// ===================================================================== live clickbot
static std::vector<FMOD::Sound*> g_sounds;
static FMOD::Channel* g_noiseCh = nullptr;

static FMOD::System* fmodSystem() {
    auto engine = FMODAudioEngine::get();
    return engine ? engine->m_system : nullptr;
}

// move decoded clickpack sounds from Rust into FMOD
static void liveUpload() {
    auto sys = fmodSystem();
    if (!sys) return;
    if (zcb_live_take_reset()) {
        if (g_noiseCh) { g_noiseCh->stop(); g_noiseCh = nullptr; }
        for (auto s : g_sounds) if (s) s->release();
        g_sounds.clear();
    }
    const short* data; unsigned frames, rate; bool looped;
    int id;
    while ((id = zcb_live_next_pcm(&data, &frames, &rate, &looped)) >= 0) {
        FMOD_CREATESOUNDEXINFO ex{};
        ex.cbsize = sizeof(ex);
        ex.length = frames * 2 * sizeof(short);
        ex.numchannels = 2;
        ex.defaultfrequency = (int)rate;
        ex.format = FMOD_SOUND_FORMAT_PCM16;
        FMOD_MODE mode = FMOD_OPENMEMORY | FMOD_OPENRAW | FMOD_CREATESAMPLE | FMOD_2D |
                         (looped ? FMOD_LOOP_NORMAL : FMOD_LOOP_OFF);
        FMOD::Sound* snd = nullptr;
        sys->createSound(reinterpret_cast<const char*>(data), mode, &ex, &snd);
        if ((size_t)id >= g_sounds.size()) g_sounds.resize(id + 1, nullptr);
        g_sounds[id] = snd;
    }
}

static void liveDrainPlays() {
    auto sys = fmodSystem();
    if (!sys) return;
    unsigned id; float vol, pitch;
    while (zcb_live_poll_play(&id, &vol, &pitch)) {
        if (id >= g_sounds.size() || !g_sounds[id]) continue;
        FMOD::Channel* ch = nullptr;
        sys->playSound(g_sounds[id], nullptr, true, &ch);
        if (ch) { ch->setVolume(vol); ch->setPitch(pitch); ch->setPaused(false); }
    }
}

static void liveNoise() {
    auto sys = fmodSystem();
    unsigned id; float vol;
    bool want = sys && zcb_live_noise(&id, &vol) && id < g_sounds.size() && g_sounds[id];
    if (want) {
        bool playing = false;
        if (g_noiseCh) g_noiseCh->isPlaying(&playing);
        if (!playing) {
            g_noiseCh = nullptr;
            sys->playSound(g_sounds[id], nullptr, true, &g_noiseCh);
            if (g_noiseCh) g_noiseCh->setPaused(false);
        }
        if (g_noiseCh) g_noiseCh->setVolume(vol);
    } else if (g_noiseCh) {
        g_noiseCh->stop();
        g_noiseCh = nullptr;
    }
}

class $modify(ZcbLiveGameLayer, GJBaseGameLayer) {
    void handleButton(bool push, int button, bool player1) {
        auto pl = PlayLayer::get();
        if (pl && pl->m_player1 && pl->m_player2) {
            zcb_live_set_level(true);
            zcb_live_set_time(pl->m_currentTime);
            bool invalid = (button == 2 || button == 3) &&
                           !(player1 && pl->m_player1->m_isPlatformer) &&
                           !(!player1 && pl->m_player2->m_isPlatformer);
            if (!invalid) {
                bool p2 = !player1 && (pl->m_levelSettings->m_twoPlayerMode || zcb_live_force_p2());
                zcb_live_action(button, p2, push);
                liveDrainPlays();
            }
        }
        GJBaseGameLayer::handleButton(push, button, player1);
    }
};

class $modify(ZcbLivePlayLayer, PlayLayer) {
    void resetLevel() {
        zcb_live_reset();
        PlayLayer::resetLevel();
    }
    void onQuit() {
        zcb_live_set_level(false);
        zcb_live_reset();
        PlayLayer::onQuit();
    }
};
// =====================================================================

struct GLState {
    GLint prog, abuf, ebuf, unit, tex0, texU, vp[4], bsrc, bdst, bsrca, bdsta, attr[8];
    GLboolean blend, depth, cull, scissor, mask[4];
    void save() {
        glGetIntegerv(GL_CURRENT_PROGRAM, &prog);
        glGetIntegerv(GL_ARRAY_BUFFER_BINDING, &abuf);
        glGetIntegerv(GL_ELEMENT_ARRAY_BUFFER_BINDING, &ebuf);
        glGetIntegerv(GL_ACTIVE_TEXTURE, &unit);
        glActiveTexture(GL_TEXTURE0);
        glGetIntegerv(GL_TEXTURE_BINDING_2D, &tex0);
        glActiveTexture(unit);
        glGetIntegerv(GL_VIEWPORT, vp);
        glGetIntegerv(GL_BLEND_SRC_RGB, &bsrc);
        glGetIntegerv(GL_BLEND_DST_RGB, &bdst);
        glGetIntegerv(GL_BLEND_SRC_ALPHA, &bsrca);
        glGetIntegerv(GL_BLEND_DST_ALPHA, &bdsta);
        for (int i = 0; i < 8; i++) glGetVertexAttribiv(i, GL_VERTEX_ATTRIB_ARRAY_ENABLED, &attr[i]);
        blend = glIsEnabled(GL_BLEND); depth = glIsEnabled(GL_DEPTH_TEST);
        cull = glIsEnabled(GL_CULL_FACE); scissor = glIsEnabled(GL_SCISSOR_TEST);
        glGetBooleanv(GL_COLOR_WRITEMASK, mask);
    }
    void restore() {
        glUseProgram(prog);
        glBindBuffer(GL_ARRAY_BUFFER, abuf);
        glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, ebuf);
        glActiveTexture(GL_TEXTURE0);
        glBindTexture(GL_TEXTURE_2D, tex0);
        glActiveTexture(unit);
        glViewport(vp[0], vp[1], vp[2], vp[3]);
        glBlendFuncSeparate(bsrc, bdst, bsrca, bdsta);
        for (int i = 0; i < 8; i++) attr[i] ? glEnableVertexAttribArray(i) : glDisableVertexAttribArray(i);
        auto set = [](GLenum c, GLboolean on) { on ? glEnable(c) : glDisable(c); };
        set(GL_BLEND, blend); set(GL_DEPTH_TEST, depth); set(GL_CULL_FACE, cull); set(GL_SCISSOR_TEST, scissor);
        glColorMask(mask[0], mask[1], mask[2], mask[3]);
    }
};

// draw the egui overlay after the game finished drawing the frame (Android swaps after native render)
class $modify(ZcbDirector, CCDirector) {
    void drawScene() {
        CCDirector::drawScene();
        liveUpload();
        if (auto pl = PlayLayer::get()) {
            zcb_live_set_level(true);
            zcb_live_set_time(pl->m_currentTime);
            zcb_live_tick(pl->m_currentTime);
        } else {
            zcb_live_set_level(false);
        }
        liveDrainPlays();
        liveNoise();
        // hide the button (and close the window) while actually playing a level; paused is fine
        auto scene = CCDirector::get()->getRunningScene();
        bool playing = PlayLayer::get() && !(scene && scene->getChildByType<PauseLayer>(0));
        if (g_button) g_button->setVisible(!playing);
        if (playing) zcb_set_visible(false);
        if (!zcb_is_visible()) {
            if (g_kb) { g_kb = false; CCEGLView::get()->setIMEKeyboardState(false); }
            return;
        }
        auto fr = CCEGLView::get()->getFrameSize();
        GLState st; st.save();
        zcb_frame(fr.width, fr.height);
        st.restore();

        bool want = zcb_wants_keyboard();
        if (want != g_kb) { g_kb = want; CCEGLView::get()->setIMEKeyboardState(want); }
        char url[1024];
        if (zcb_poll_url(url, sizeof url)) web::openLinkInBrowser(url);
    }
};

// send touches that land on ZCB windows to egui instead of the game
class $modify(ZcbTouch, CCTouchDispatcher) {
    void touches(CCSet* set, CCEvent* ev, unsigned int type) {
        static std::set<int> captured;
        if (!set || !zcb_is_visible()) return CCTouchDispatcher::touches(set, ev, type);
        std::vector<CCTouch*> mine;
        for (auto it = set->begin(); it != set->end(); ++it) {
            auto t = static_cast<CCTouch*>(*it);
            auto px = toPixels(t->getLocationInView());
            if (type == CCTOUCHBEGAN && !overButton(px) && zcb_hit(px.x, px.y)) captured.insert(t->getID());
            if (captured.count(t->getID())) mine.push_back(t);
        }
        for (auto t : mine) {
            auto px = toPixels(t->getLocationInView());
            zcb_touch(type, t->getID(), px.x, px.y);
            if (type == CCTOUCHENDED || type == CCTOUCHCANCELLED) captured.erase(t->getID());
            set->removeObject(t);
        }
        if (set->count() == 0) return;
        CCTouchDispatcher::touches(set, ev, type);
    }
};

// soft keyboard -> egui
class $modify(ZcbIME, CCIMEDispatcher) {
    void dispatchInsertText(const char* text, int len, enumKeyCodes key) {
        if (zcb_is_visible() && zcb_wants_keyboard()) {
            std::string s(text ? text : "", text ? len : 0);
            if (s == "\n") zcb_key(1); else if (!s.empty()) zcb_text(s.c_str());
            return;
        }
        CCIMEDispatcher::dispatchInsertText(text, len, key);
    }
    void dispatchDeleteBackward() {
        if (zcb_is_visible() && zcb_wants_keyboard()) return zcb_key(0);
        CCIMEDispatcher::dispatchDeleteBackward();
    }
};
