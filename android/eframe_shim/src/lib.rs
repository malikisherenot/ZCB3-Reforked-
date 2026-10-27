//! Tiny stand-in for `eframe` so gui.rs compiles on Android without winit/android-activity.
pub use egui;
pub use egui::{emath, epaint};

pub struct Frame;

pub trait App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame);
}
