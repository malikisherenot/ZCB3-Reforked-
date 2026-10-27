//! rfd-compatible FileDialog for Android: blocking calls wait for an in-game egui file browser.
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread::ThreadId;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    PickFile,
    SaveFile,
    PickFolder,
}

struct Request {
    kind: Kind,
    exts: Vec<String>,
    file_name: String,
    reply: mpsc::Sender<Option<PathBuf>>,
}

static PENDING: Mutex<Option<Request>> = Mutex::new(None);
static UI_THREAD: OnceLock<ThreadId> = OnceLock::new();
static SAVE_DIR: Mutex<String> = Mutex::new(String::new());

pub fn init(save_dir: &str) {
    let _ = UI_THREAD.set(std::thread::current().id());
    *SAVE_DIR.lock().unwrap() = save_dir.to_string();
}

fn save_dir() -> PathBuf {
    PathBuf::from(SAVE_DIR.lock().unwrap().clone())
}

#[derive(Default)]
pub struct FileDialog {
    exts: Vec<String>,
    file_name: String,
}

impl FileDialog {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add_filter(mut self, _name: impl Into<String>, exts: &[impl ToString]) -> Self {
        self.exts.extend(exts.iter().map(|e| e.to_string().to_lowercase()));
        self
    }
    #[allow(dead_code)]
    pub fn set_file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = name.into();
        self
    }
    pub fn pick_file(self) -> Option<PathBuf> {
        self.run(Kind::PickFile)
    }
    pub fn save_file(self) -> Option<PathBuf> {
        self.run(Kind::SaveFile)
    }
    pub fn pick_folder(self) -> Option<PathBuf> {
        self.run(Kind::PickFolder)
    }

    fn run(self, kind: Kind) -> Option<PathBuf> {
        // called from the UI thread (ClickpackDB "download to" callback): can't block it
        if UI_THREAD.get() == Some(&std::thread::current().id()) {
            let d = save_dir().join("clickpacks");
            let _ = std::fs::create_dir_all(&d);
            return (kind == Kind::PickFolder).then_some(d);
        }
        let (tx, rx) = mpsc::channel();
        *PENDING.lock().unwrap() = Some(Request {
            kind,
            exts: self.exts,
            file_name: self.file_name,
            reply: tx,
        });
        rx.recv().ok().flatten()
    }
}

#[derive(Default)]
pub struct Browser {
    req: Option<Request>,
    dir: PathBuf,
    loaded: PathBuf,
    entries: Vec<(String, bool)>,
    name: String,
}

fn start_dir() -> PathBuf {
    for p in ["/storage/emulated/0/Download", "/storage/emulated/0"] {
        if Path::new(p).is_dir() {
            return PathBuf::from(p);
        }
    }
    save_dir()
}

pub fn show(ctx: &egui::Context, st: &mut Browser) {
    if st.req.is_none() {
        if let Some(r) = PENDING.lock().unwrap().take() {
            st.name = r.file_name.clone();
            st.dir = start_dir();
            st.loaded = PathBuf::new();
            st.req = Some(r);
        }
    }
    let Some(req) = st.req.take() else { return };

    if st.loaded != st.dir {
        let mut v: Vec<(String, bool)> = std::fs::read_dir(&st.dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| (e.file_name().to_string_lossy().to_string(), e.path().is_dir()))
                    .filter(|(n, _)| !n.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        st.entries = v;
        st.loaded = st.dir.clone();
    }

    let mut result: Option<Option<PathBuf>> = None;
    let mut goto: Option<PathBuf> = None;
    let title = match req.kind {
        Kind::PickFile => "Select a file",
        Kind::SaveFile => "Save file",
        Kind::PickFolder => "Select a folder",
    };
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Mod folder").clicked() {
                    goto = Some(save_dir());
                }
                if ui.button("Downloads").clicked() {
                    goto = Some(PathBuf::from("/storage/emulated/0/Download"));
                }
                if ui.button("Storage").clicked() {
                    goto = Some(PathBuf::from("/storage/emulated/0"));
                }
                if ui.button("⬆ Up").clicked() {
                    goto = st.dir.parent().map(|p| p.to_path_buf());
                }
            });
            ui.label(st.dir.to_string_lossy());
            ui.separator();
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                for (name, is_dir) in &st.entries {
                    if *is_dir {
                        if ui.selectable_label(false, format!("📁 {name}")).clicked() {
                            goto = Some(st.dir.join(name));
                        }
                    } else if req.kind != Kind::PickFolder {
                        let ext = Path::new(name)
                            .extension()
                            .map(|e| e.to_string_lossy().to_lowercase())
                            .unwrap_or_default();
                        if !req.exts.is_empty() && !req.exts.contains(&ext) {
                            continue;
                        }
                        if ui.selectable_label(false, name).clicked() {
                            match req.kind {
                                Kind::PickFile => result = Some(Some(st.dir.join(name))),
                                _ => st.name = name.clone(),
                            }
                        }
                    }
                }
            });
            ui.separator();
            if req.kind == Kind::SaveFile {
                ui.text_edit_singleline(&mut st.name);
            }
            ui.horizontal(|ui| {
                match req.kind {
                    Kind::PickFolder => {
                        if ui.button("Select this folder").clicked() {
                            result = Some(Some(st.dir.clone()));
                        }
                    }
                    Kind::SaveFile => {
                        if ui.button("Save").clicked() && !st.name.trim().is_empty() {
                            let mut n = st.name.trim().to_string();
                            if let Some(e) = req.exts.first() {
                                if !n.to_lowercase().ends_with(&format!(".{e}")) {
                                    n = format!("{n}.{e}");
                                }
                            }
                            result = Some(Some(st.dir.join(n)));
                        }
                    }
                    Kind::PickFile => {}
                }
                if ui.button("Cancel").clicked() {
                    result = Some(None);
                }
            });
        });

    if let Some(d) = goto {
        st.dir = d;
    }
    match result {
        Some(r) => {
            let _ = req.reply.send(r);
        }
        None => st.req = Some(req),
    }
}
