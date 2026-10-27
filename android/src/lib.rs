//! ZCB3 embedded in Geometry Dash (Android) as an egui overlay. Everything runs on the GL thread.
use std::{
    cell::RefCell,
    ffi::{c_char, c_void, CStr},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::Relaxed},
        Arc, Mutex,
    },
    time::Instant,
};

mod android_dialog;
#[path = "../../src/gui.rs"]
mod gui;
#[path = "../../src/utils.rs"]
#[allow(dead_code)]
mod utils;
pub mod built_info {
    pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
}

use egui_glow::{glow, Painter};

extern "C" {
    fn eglGetProcAddress(name: *const c_char) -> *const c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

static VISIBLE: AtomicBool = AtomicBool::new(false);
static WANTS_KB: AtomicBool = AtomicBool::new(false);
static FAILED: AtomicBool = AtomicBool::new(false);
static PPP: AtomicU32 = AtomicU32::new(0);
static PRIMARY: AtomicI32 = AtomicI32::new(-1);
static EVENTS: Mutex<Vec<egui::Event>> = Mutex::new(Vec::new());
static URL: Mutex<Option<String>> = Mutex::new(None);
static SAVE_DIR: Mutex<String> = Mutex::new(String::new());

struct Runtime {
    ctx: egui::Context,
    painter: Painter,
    app: gui::App,
    browser: android_dialog::Browser,
    start: Instant,
}

thread_local! {
    static RT: RefCell<Option<Runtime>> = const { RefCell::new(None) };
}

fn ppp() -> f32 {
    f32::from_bits(PPP.load(Relaxed)).max(0.5)
}

impl Runtime {
    fn new() -> Option<Self> {
        let dir = SAVE_DIR.lock().unwrap().clone();
        // std::env::temp_dir() is unusable on Android
        let tmp = format!("{dir}/tmp");
        let _ = std::fs::create_dir_all(&tmp);
        std::env::set_var("TMPDIR", &tmp);
        let _ = std::env::set_current_dir(&dir);

        let rt: &'static tokio::runtime::Runtime = Box::leak(Box::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .build()
                .ok()?,
        ));
        std::mem::forget(rt.enter());

        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|s: &CStr| {
                let p = eglGetProcAddress(s.as_ptr());
                if p.is_null() {
                    dlsym(std::ptr::null_mut(), s.as_ptr()) as *const c_void
                } else {
                    p
                }
            })
        };
        let painter = Painter::new(Arc::new(gl), "", None, false)
            .map_err(|e| log::error!("painter: {e}"))
            .ok()?;
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        android_dialog::init(&dir);
        Some(Self {
            ctx,
            painter,
            app: gui::App::default(),
            browser: Default::default(),
            start: Instant::now(),
        })
    }

    fn frame(&mut self, w: f32, h: f32) {
        let p = (h / 540.0).clamp(1.25, 4.0);
        PPP.store(p.to_bits(), Relaxed);

        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(w / p, h / p),
            )),
            time: Some(self.start.elapsed().as_secs_f64()),
            max_texture_side: Some(self.painter.max_texture_side()),
            events: std::mem::take(&mut *EVENTS.lock().unwrap()),
            ..Default::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(p);

        let (app, browser) = (&mut self.app, &mut self.browser);
        let mut out = self.ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx().clone();
            let screen = ctx.content_rect();
            let size = egui::vec2(460.0, 500.0).min(screen.size() - egui::vec2(12.0, 12.0));
            let pos = screen.center() - size / 2.0;
            egui::Area::new(egui::Id::new("zcb_main"))
                .order(egui::Order::Middle)
                .fixed_pos(pos)
                .show(&ctx, |ui| {
                    let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter()
                        .rect_filled(r, 6.0, ui.visuals().panel_fill);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
                    child.set_clip_rect(r);
                    app.show(&mut child);
                });
            android_dialog::show(&ctx, browser);
        });

        WANTS_KB.store(self.ctx.egui_wants_keyboard_input(), Relaxed);
        for cmd in &out.platform_output.commands {
            if let egui::OutputCommand::OpenUrl(u) = cmd {
                *URL.lock().unwrap() = Some(u.url.clone());
            }
        }
        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        self.painter.paint_and_update_textures(
            [w as u32, h as u32],
            out.pixels_per_point,
            &prims,
            &mut out.textures_delta,
        );
    }
}

fn push(e: egui::Event) {
    EVENTS.lock().unwrap().push(e);
}

#[no_mangle]
pub extern "C" fn zcb_init(save_dir: *const c_char) {
    let dir = unsafe { CStr::from_ptr(save_dir) }.to_string_lossy().to_string();
    *SAVE_DIR.lock().unwrap() = dir;
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("ZCB"),
    );
}

#[no_mangle]
pub extern "C" fn zcb_set_visible(v: bool) {
    VISIBLE.store(v, Relaxed);
    if !v {
        WANTS_KB.store(false, Relaxed);
    }
}

#[no_mangle]
pub extern "C" fn zcb_is_visible() -> bool {
    VISIBLE.load(Relaxed) && !FAILED.load(Relaxed)
}

#[no_mangle]
pub extern "C" fn zcb_wants_keyboard() -> bool {
    WANTS_KB.load(Relaxed)
}

/// Call once per frame (GL thread, after the game drew) with the framebuffer size in pixels.
#[no_mangle]
pub extern "C" fn zcb_frame(w: f32, h: f32) {
    if !zcb_is_visible() {
        return;
    }
    let r = catch_unwind(AssertUnwindSafe(|| {
        RT.with(|c| {
            let mut slot = c.borrow_mut();
            if slot.is_none() {
                *slot = Runtime::new();
                if slot.is_none() {
                    FAILED.store(true, Relaxed);
                }
            }
            if let Some(rt) = slot.as_mut() {
                rt.frame(w, h);
            }
        })
    }));
    if r.is_err() {
        log::error!("ZCB panicked, overlay disabled");
        FAILED.store(true, Relaxed);
    }
}

/// Is this pixel over any ZCB window? (used to swallow touches)
#[no_mangle]
pub extern "C" fn zcb_hit(x: f32, y: f32) -> bool {
    if !VISIBLE.load(Relaxed) {
        return false;
    }
    let p = ppp();
    RT.with(|c| {
        c.try_borrow().ok().and_then(|s| {
            s.as_ref().and_then(|rt| {
                rt.ctx
                    .layer_id_at(egui::pos2(x / p, y / p))
                    .map(|l| l.order != egui::Order::Background)
            })
        })
    })
    .unwrap_or(false)
}

/// phase: 0 begin, 1 move, 2 end, 3 cancel. Pixel coords, y down.
#[no_mangle]
pub extern "C" fn zcb_touch(phase: i32, id: i32, x: f32, y: f32) {
    let p = ppp();
    let pos = egui::pos2(x / p, y / p);
    let prim = PRIMARY.load(Relaxed);
    if phase == 0 {
        if prim != -1 {
            return;
        }
        PRIMARY.store(id, Relaxed);
    } else if prim != id {
        return;
    }
    let m = egui::Modifiers::NONE;
    match phase {
        0 => {
            push(egui::Event::PointerMoved(pos));
            push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: m,
            });
        }
        1 => push(egui::Event::PointerMoved(pos)),
        _ => {
            push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: m,
            });
            push(egui::Event::PointerGone);
            PRIMARY.store(-1, Relaxed);
        }
    }
}

#[no_mangle]
pub extern "C" fn zcb_text(s: *const c_char) {
    let s = unsafe { CStr::from_ptr(s) }.to_string_lossy().to_string();
    push(egui::Event::Text(s));
}

/// key: 0 backspace, 1 enter
#[no_mangle]
pub extern "C" fn zcb_key(key: i32) {
    let key = if key == 0 { egui::Key::Backspace } else { egui::Key::Enter };
    for pressed in [true, false] {
        push(egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
    }
}

/// Returns 1 and fills buf if a URL should be opened, else 0.
#[no_mangle]
pub extern "C" fn zcb_poll_url(buf: *mut c_char, cap: usize) -> i32 {
    let Some(u) = URL.lock().unwrap().take() else { return 0 };
    let b = u.as_bytes();
    let n = b.len().min(cap.saturating_sub(1));
    unsafe {
        std::ptr::copy_nonoverlapping(b.as_ptr(), buf as *mut u8, n);
        *buf.add(n) = 0;
    }
    1
}
