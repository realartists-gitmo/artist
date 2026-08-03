//! The native GPUI frontend for Artist.

mod app;
#[cfg(feature = "embedded-canvas")]
mod canvas_accessibility;
mod canvas_security;
#[cfg(target_os = "linux")]
mod host_controller;
#[cfg(target_os = "linux")]
mod stage_surface;

pub use app::run;
pub use gpui;
