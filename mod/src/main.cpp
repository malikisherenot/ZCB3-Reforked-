#include <Geode/Geode.hpp>
#include <Geode/ui/OverlayManager.hpp>
#include <Geode/modify/CCDirector.hpp>
#include <Geode/modify/CCTouchDispatcher.hpp>
#include <Geode/modify/CCIMEDispatcher.hpp>
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
    if (!g_button) return false;
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
        this->setContentSize({46.f, 26.f});
        this->setAnchorPoint({0.5f, 0.5f});
        auto bg = CCScale9Sprite::create("square02_001.png", {0, 0, 80, 80});
        bg->setContentSize(this->getContentSize());
        bg->setColor({22, 22, 30});
        bg->setOpacity(215);
        bg->setPosition(this->getContentSize() / 2);
        this->addChild(bg);
        auto lbl = CCLabelBMFont::create("ZCB", "bigFont.fnt");
        lbl->setScale(0.45f);
        lbl->setPosition(this->getContentSize() / 2);
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
        auto local = this->getParent()->convertToNodeSpace(t->getLocation());
        if (!this->boundingBox().containsPoint(local)) return false;
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
            p.x = std::clamp(p.x, 23.f, win.width - 23.f);
            p.y = std::clamp(p.y, 13.f, win.height - 13.f);
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
