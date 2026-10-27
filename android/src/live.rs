//! ZCB Live core (real-time clickbot). Rust decides WHAT to play, C++ plays it through GD's FMOD.
use bot::{Action, AudioSegment, Bot, Click, ClickType, Pitch, Player, Replay, ReplayType, Timings, VolumeSettings};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering::Relaxed},
        LazyLock, Mutex, MutexGuard,
    },
};

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct LiveConfig {
    pub enabled: bool,
    pub clickpack: Option<String>,
    pub timings: Timings,
    pub volume: VolumeSettings,
    pub platformer_volume: f32,
    pub pitch_from: f32,
    pub pitch_to: f32,
    pub speed: f64,
    pub ignore_hard: bool,
    pub ignore_regular: bool,
    pub ignore_soft: bool,
    pub ignore_micro: bool,
    pub force_p2: bool,
    pub noise: bool,
    pub noise_volume: f32,
    pub macro_play: bool,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            clickpack: None,
            timings: Timings::default(),
            volume: VolumeSettings::default(),
            platformer_volume: 1.0,
            pitch_from: 0.98,
            pitch_to: 1.02,
            speed: 1.0,
            ignore_hard: false,
            ignore_regular: false,
            ignore_soft: false,
            ignore_micro: false,
            force_p2: false,
            noise: true,
            noise_volume: 1.0,
            macro_play: true,
        }
    }
}

struct Pcm {
    data: Vec<i16>, // interleaved stereo
    rate: u32,
    looped: bool,
}

struct Loaded {
    slots: [[Vec<u32>; 8]; 6], // player1, player2, left1, left2, right1, right2 x click types
    total: usize,
    platformer: bool,
    noise: Option<u32>,
}

struct MacroState {
    actions: Vec<Action>,
    cursor: usize,
    last: f64,
}

struct State {
    mac: Option<MacroState>,
    mac_status: String,
    conf: LiveConfig,
    saved: String,
    save_dir: PathBuf,
    loaded: Option<Loaded>,
    status: String,
    loading: bool,
    in_level: bool,
    time: f64,
    prev: [(f64, bool); 6], // (time, was_click) per player x button
    plays: VecDeque<(u32, f32, f32)>,
    upload: VecDeque<(u32, Pcm)>,
    last_pcm: Option<Pcm>,
    reset: bool,
    main_clickpack: Option<PathBuf>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        mac: None,
        mac_status: "No macro loaded".into(),
        conf: LiveConfig::default(),
        saved: String::new(),
        save_dir: PathBuf::new(),
        loaded: None,
        status: "No clickpack loaded".into(),
        loading: false,
        in_level: false,
        time: 0.0,
        prev: [(0.0, false); 6],
        plays: VecDeque::new(),
        upload: VecDeque::new(),
        last_pcm: None,
        reset: false,
        main_clickpack: None,
    })
});
static OPEN: AtomicBool = AtomicBool::new(false);

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn toggle_window() {
    OPEN.store(!OPEN.load(Relaxed), Relaxed);
}

pub fn set_main_clickpack(p: Option<PathBuf>) {
    state().main_clickpack = p;
}

pub fn init(save_dir: &str) {
    let mut s = state();
    s.save_dir = PathBuf::from(save_dir);
    if let Ok(txt) = std::fs::read_to_string(s.save_dir.join("live_config.json")) {
        if let Ok(c) = serde_json::from_str::<LiveConfig>(&txt) {
            s.conf = c;
        }
    }
    s.saved = serde_json::to_string_pretty(&s.conf).unwrap_or_default();
    if let Some(p) = s.conf.clickpack.clone() {
        begin_load(&mut s, PathBuf::from(p));
    }
}

fn to_pcm(seg: &AudioSegment, looped: bool) -> Pcm {
    let mut data = Vec::with_capacity(seg.frames.len() * 2);
    for f in &seg.frames {
        data.push((f.left.clamp(-1.0, 1.0) * 32767.0) as i16);
        data.push((f.right.clamp(-1.0, 1.0) * 32767.0) as i16);
    }
    Pcm {
        data,
        rate: seg.sample_rate,
        looped,
    }
}

fn load(dir: &Path) -> anyhow::Result<(Loaded, Vec<(u32, Pcm)>)> {
    let mut b = Bot::new(44100);
    b.load_clickpack(dir, Pitch::NO_PITCH)?;
    let mut slots: [[Vec<u32>; 8]; 6] = Default::default();
    let mut pcms: Vec<(u32, Pcm)> = Vec::new();
    for s in 0..6 {
        for t in 0..8 {
            for f in b.clickpack[s][t].iter() {
                if f.segment.frames.is_empty() {
                    continue;
                }
                let id = pcms.len() as u32;
                pcms.push((id, to_pcm(&f.segment, false)));
                slots[s][t].push(id);
            }
        }
    }
    let mut noise = None;
    if let Some(n) = &b.noise {
        if !n.frames.is_empty() {
            let id = pcms.len() as u32;
            pcms.push((id, to_pcm(n, true)));
            noise = Some(id);
        }
    }
    let total = pcms.len() - noise.is_some() as usize;
    let platformer = (2..6).any(|s| slots[s].iter().any(|v| !v.is_empty()));
    Ok((
        Loaded {
            slots,
            total,
            platformer,
            noise,
        },
        pcms,
    ))
}

fn begin_load(s: &mut State, dir: PathBuf) {
    s.conf.clickpack = Some(dir.to_string_lossy().to_string());
    s.loading = true;
    s.status = format!("Loading {} ...", dir.display());
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(|| load(&dir));
        let mut s = state();
        s.loading = false;
        match r {
            Ok(Ok((loaded, pcms))) => {
                s.status = format!("Loaded {} sounds", loaded.total);
                s.reset = true;
                s.upload = pcms.into();
                s.loaded = Some(loaded);
                s.prev = [(0.0, false); 6];
            }
            Ok(Err(e)) => s.status = format!("Failed: {e}"),
            Err(_) => s.status = "Failed: clickpack loader crashed".into(),
        }
    });
}

fn tidx(t: ClickType) -> Option<usize> {
    use ClickType::*;
    Some(match t {
        HardClick => 0,
        HardRelease => 1,
        Click => 2,
        Release => 3,
        SoftClick => 4,
        SoftRelease => 5,
        MicroClick => 6,
        MicroRelease => 7,
        None => return Option::None,
    })
}

fn pick(slot: &[Vec<u32>; 8], typ: ClickType) -> Option<u32> {
    for t in typ.preferred() {
        if let Some(i) = tidx(t) {
            let v = &slot[i];
            if !v.is_empty() {
                return Some(v[fastrand::usize(..v.len())]);
            }
        }
    }
    None
}

fn rand_range(lo: f32, hi: f32) -> f32 {
    lo + fastrand::f32() * (hi - lo)
}

fn on_action(s: &mut State, button: i32, player2: bool, push: bool) {
    let State {
        conf,
        loaded,
        prev,
        plays,
        time,
        in_level,
        ..
    } = s;
    if !conf.enabled || !*in_level {
        return;
    }
    let Some(loaded) = loaded.as_ref() else { return };
    let btn = match button {
        2 => 1usize,
        3 => 2,
        _ => 0,
    };
    if btn != 0 && !loaded.platformer {
        return;
    }
    let now = *time;
    if now == 0.0 {
        return;
    }
    let key = player2 as usize * 3 + btn;
    let (pt, pclick) = prev[key];
    if pclick == push {
        return;
    }
    let dt = (now - pt).abs();
    let (typ, _) = ClickType::from_time(dt, conf.timings, push, VolumeSettings::default());
    use ClickType::*;
    let ignored = match typ {
        HardClick | HardRelease => conf.ignore_hard,
        Click | Release => conf.ignore_regular,
        SoftClick | SoftRelease => conf.ignore_soft,
        MicroClick | MicroRelease => conf.ignore_micro,
        None => false,
    };
    if ignored {
        return;
    }
    // slots: 0 p1, 1 p2, 2 left1, 3 left2, 4 right1, 5 right2
    let mut slot = match btn {
        0 => player2 as usize,
        1 => 2 + player2 as usize,
        _ => 4 + player2 as usize,
    };
    if player2 && loaded.slots[slot].iter().all(|v| v.is_empty()) {
        slot -= 1; // no player 2 sounds: use player 1's
    }
    let Some(id) = pick(&loaded.slots[slot], typ) else { return };

    let vol = &conf.volume;
    let mut volume = 1.0f32;
    if vol.volume_var != 0.0 {
        volume += rand_range(-vol.volume_var, vol.volume_var);
    }
    if (push || vol.change_releases_volume) && dt < vol.spam_time && vol.enabled {
        let offset = (vol.spam_time - dt) as f32 * vol.spam_vol_offset_factor;
        volume -= offset.min(vol.max_spam_vol_offset);
    }
    if btn != 0 {
        volume *= conf.platformer_volume;
    }
    volume *= vol.global_volume;
    let (lo, hi) = (conf.pitch_from.min(conf.pitch_to), conf.pitch_from.max(conf.pitch_to));
    let pitch = rand_range(lo, hi) * conf.speed as f32;
    plays.push_back((id, volume.max(0.0), pitch.max(0.01)));
    if plays.len() > 64 {
        plays.pop_front();
    }
    prev[key] = (now, push);
}

fn macro_trigger(s: &mut State, a: &Action) {
    let (btn, typ) = match a.click {
        Click::Regular(t) => (1, t),
        Click::Left(t) => (2, t),
        Click::Right(t) => (3, t),
    };
    let push = matches!(
        typ,
        ClickType::HardClick | ClickType::Click | ClickType::SoftClick | ClickType::MicroClick
    );
    if matches!(typ, ClickType::None) {
        return;
    }
    let saved = s.time;
    s.time = a.time;
    on_action(s, btn, matches!(a.player, Player::Two), push);
    s.time = saved;
}

fn macro_tick(s: &mut State, now: f64) {
    if !s.conf.enabled || !s.conf.macro_play || !s.in_level {
        return;
    }
    let Some(m) = s.mac.as_mut() else { return };
    let rewound = now < m.last - 0.05;
    if rewound {
        m.cursor = m.actions.partition_point(|a| a.time < now);
    }
    m.last = now;
    let mut due: Vec<Action> = Vec::new();
    while m.cursor < m.actions.len() && m.actions[m.cursor].time <= now {
        let a = m.actions[m.cursor];
        m.cursor += 1;
        if now - a.time <= 0.15 {
            due.push(a);
        }
    }
    if rewound {
        s.prev = [(0.0, false); 6];
    }
    for a in due {
        macro_trigger(s, &a);
    }
}

fn load_macro(path: PathBuf) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<Replay> {
        let typ = ReplayType::guess_format(&name)?;
        let f = std::fs::File::open(&path)?;
        Replay::build()
            .with_sort_actions(true)
            .parse(typ, std::io::BufReader::new(f))
    }));
    let mut s = state();
    match r {
        Ok(Ok(replay)) => {
            s.mac_status = format!("{name}: {} actions, {:.1}s", replay.actions.len(), replay.duration);
            s.mac = Some(MacroState {
                actions: replay.actions,
                cursor: 0,
                last: 0.0,
            });
        }
        Ok(Err(e)) => s.mac_status = format!("Failed to load macro: {e}"),
        Err(_) => s.mac_status = "Failed to load macro (crashed)".into(),
    }
}

// ---------------------------------------------------------------- FFI (GL thread)

/// Called every frame while a level is open; drives macro playback.
#[no_mangle]
pub extern "C" fn zcb_live_tick(now: f64) {
    macro_tick(&mut state(), now);
}

#[no_mangle]
pub extern "C" fn zcb_live_action(button: i32, player2: bool, push: bool) {
    let mut s = state();
    if s.conf.macro_play && s.mac.is_some() {
        return; // clicks come from the macro instead
    }
    on_action(&mut s, button, player2, push);
}

#[no_mangle]
pub extern "C" fn zcb_live_set_level(v: bool) {
    let mut s = state();
    s.in_level = v;
}

#[no_mangle]
pub extern "C" fn zcb_live_set_time(t: f64) {
    state().time = t;
}

#[no_mangle]
pub extern "C" fn zcb_live_reset() {
    let mut s = state();
    s.prev = [(0.0, false); 6];
    s.plays.clear();
}

#[no_mangle]
pub extern "C" fn zcb_live_force_p2() -> bool {
    state().conf.force_p2
}

#[no_mangle]
pub extern "C" fn zcb_live_take_reset() -> bool {
    std::mem::take(&mut state().reset)
}

/// Pops the next sound to upload. Returns its id, or -1 if none. Pointer stays valid until the next call.
#[no_mangle]
pub extern "C" fn zcb_live_next_pcm(
    data: *mut *const i16,
    frames: *mut u32,
    rate: *mut u32,
    looped: *mut bool,
) -> i32 {
    let mut s = state();
    let Some((id, pcm)) = s.upload.pop_front() else { return -1 };
    s.last_pcm = Some(pcm);
    let p = s.last_pcm.as_ref().unwrap();
    unsafe {
        *data = p.data.as_ptr();
        *frames = (p.data.len() / 2) as u32;
        *rate = p.rate;
        *looped = p.looped;
    }
    id as i32
}

#[no_mangle]
pub extern "C" fn zcb_live_poll_play(id: *mut u32, vol: *mut f32, pitch: *mut f32) -> bool {
    let Some((i, v, p)) = state().plays.pop_front() else { return false };
    unsafe {
        *id = i;
        *vol = v;
        *pitch = p;
    }
    true
}

/// True if the background noise should be playing right now.
#[no_mangle]
pub extern "C" fn zcb_live_noise(id: *mut u32, vol: *mut f32) -> bool {
    let s = state();
    let Some(n) = s.loaded.as_ref().and_then(|l| l.noise) else { return false };
    if !(s.conf.enabled && s.conf.noise && s.in_level) {
        return false;
    }
    unsafe {
        *id = n;
        *vol = s.conf.noise_volume * s.conf.volume.global_volume;
    }
    true
}

// ---------------------------------------------------------------- UI

fn slider<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    v: &mut N,
    r: std::ops::RangeInclusive<N>,
    text: &str,
) {
    ui.add(egui::Slider::new(v, r).text(text));
}

fn contents(ui: &mut egui::Ui, s: &mut State) {
    ui.checkbox(&mut s.conf.enabled, "Enable live clickbot");
    ui.label(
        egui::RichText::new("Plays click sounds while you play a level (Android audio has some latency).")
            .weak()
            .small(),
    );
    ui.separator();
    ui.label(egui::RichText::new("Clickpack").strong());
    ui.label(&s.status);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!s.loading, egui::Button::new("Select folder"))
            .clicked()
        {
            std::thread::spawn(|| {
                if let Some(p) = crate::android_dialog::FileDialog::new().pick_folder() {
                    begin_load(&mut state(), p);
                }
            });
        }
        if let Some(p) = s.main_clickpack.clone() {
            if ui
                .add_enabled(!s.loading, egui::Button::new("Use main menu clickpack"))
                .clicked()
            {
                begin_load(s, p);
            }
        }
    });
    if ui.button("Play test click").clicked() {
        if let Some(l) = &s.loaded {
            if let Some(id) = pick(&l.slots[0], ClickType::Click) {
                s.plays.push_back((id, s.conf.volume.global_volume, 1.0));
            }
        }
    }

    egui::CollapsingHeader::new("Macro (real-time clicks)")
        .default_open(true)
        .show(ui, |ui| {
            ui.checkbox(&mut s.conf.macro_play, "Play macro clicks in real time");
            ui.label(&s.mac_status);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Select macro").clicked() {
                    std::thread::spawn(|| {
                        if let Some(p) = crate::android_dialog::FileDialog::new()
                            .add_filter("Replay", Replay::SUPPORTED_EXTENSIONS)
                            .pick_file()
                        {
                            load_macro(p);
                        }
                    });
                }
                if s.mac.is_some() && ui.button("Unload macro").clicked() {
                    s.mac = None;
                    s.mac_status = "No macro loaded".into();
                }
            });
            ui.label(
                egui::RichText::new("Start the level and the clicks follow the level time. Real button presses are ignored while a macro is loaded.")
                    .weak()
                    .small(),
            );
        });

    egui::CollapsingHeader::new("Timings").show(ui, |ui| {
        slider(ui, &mut s.conf.timings.hard, 0.1..=5.0, "Hard timing");
        slider(ui, &mut s.conf.timings.regular, 0.01..=1.0, "Regular timing");
        slider(ui, &mut s.conf.timings.soft, 0.001..=0.2, "Soft timing");
    });
    egui::CollapsingHeader::new("Volume").show(ui, |ui| {
        slider(ui, &mut s.conf.volume.global_volume, 0.0..=3.0, "Global volume");
        slider(ui, &mut s.conf.volume.volume_var, 0.0..=1.0, "Volume variation");
        slider(ui, &mut s.conf.platformer_volume, 0.0..=2.0, "Platformer volume");
        ui.checkbox(&mut s.conf.volume.enabled, "Lower volume when spamming");
        slider(ui, &mut s.conf.volume.spam_time, 0.0..=1.0, "Spam time");
        slider(ui, &mut s.conf.volume.spam_vol_offset_factor, 0.0..=3.0, "Spam factor");
        slider(ui, &mut s.conf.volume.max_spam_vol_offset, 0.0..=1.0, "Max spam reduction");
        ui.checkbox(&mut s.conf.volume.change_releases_volume, "Also lower releases");
    });
    egui::CollapsingHeader::new("Pitch").show(ui, |ui| {
        slider(ui, &mut s.conf.pitch_from, 0.5..=1.5, "Pitch from");
        slider(ui, &mut s.conf.pitch_to, 0.5..=1.5, "Pitch to");
        slider(ui, &mut s.conf.speed, 0.25..=3.0, "Click speed");
    });
    egui::CollapsingHeader::new("Ignore click types").show(ui, |ui| {
        ui.checkbox(&mut s.conf.ignore_hard, "Hard clicks");
        ui.checkbox(&mut s.conf.ignore_regular, "Regular clicks");
        ui.checkbox(&mut s.conf.ignore_soft, "Soft clicks");
        ui.checkbox(&mut s.conf.ignore_micro, "Micro clicks");
    });
    egui::CollapsingHeader::new("Other").show(ui, |ui| {
        ui.checkbox(&mut s.conf.force_p2, "Always use player 2 sounds for player 2");
        ui.checkbox(&mut s.conf.noise, "Play background noise");
        slider(ui, &mut s.conf.noise_volume, 0.0..=2.0, "Noise volume");
        if ui.button("Reset settings").clicked() {
            let cp = s.conf.clickpack.take();
            s.conf = LiveConfig {
                clickpack: cp,
                ..Default::default()
            };
        }
    });
}

fn save_if_changed(s: &mut State) {
    let j = serde_json::to_string_pretty(&s.conf).unwrap_or_default();
    if j != s.saved {
        let _ = std::fs::write(s.save_dir.join("live_config.json"), &j);
        s.saved = j;
    }
}

pub fn show_window(ctx: &egui::Context) {
    if !OPEN.load(Relaxed) {
        return;
    }
    let mut open = true;
    let max_h = (ctx.content_rect().height() - 90.0).max(120.0);
    egui::Window::new("ZCB Live")
        .id(egui::Id::new("zcb_live_win"))
        .open(&mut open)
        .collapsible(false)
        .default_width(340.0)
        .show(ctx, |ui| {
            let mut g = state();
            egui::ScrollArea::vertical()
                .max_height(max_h)
                .show(ui, |ui| contents(ui, &mut g));
            save_if_changed(&mut g);
        });
    if !open {
        OPEN.store(false, Relaxed);
    }
}
