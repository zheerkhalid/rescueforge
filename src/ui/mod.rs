//! The egui user interface.
mod app;
pub mod bidi;
mod home;
mod other;
mod previewer;
mod recover_ui;
mod results;
mod state;
mod theme;
mod widgets;

pub use app::{run, Options};
