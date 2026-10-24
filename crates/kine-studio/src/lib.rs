//! `kine-studio` — Native Rust **Xilem** visual studio UI over [`kine_session`].
//!
//! All headless state (`StudioState`), NLE timeline logic (`timeline`), property
//! inspector (`inspector`), scene catalog (`catalog`), live file watcher, and
//! external CLI agent TCP bridge (`bridge` / `agent_tools`) live in the
//! UI-independent [`kine_session`] crate. `kine-studio` contains only the
//! Xilem/Masonry desktop rendering layer.

#[cfg(feature = "xilem-ui")]
pub mod custom_widgets;
#[cfg(feature = "xilem-ui")]
pub mod local_portal;
#[cfg(feature = "xilem-ui")]
pub mod preview_canvas;
#[cfg(feature = "xilem-ui")]
pub mod settings_panel;
pub mod theme;
#[cfg(feature = "xilem-ui")]
pub mod ui;

pub use kine_session::*;
#[cfg(feature = "xilem-ui")]
pub use theme::{pixmap_to_xilem_image, StudioPalette, ICON_FONT, MONO_FONT, UI_FONT};
#[cfg(feature = "xilem-ui")]
pub use ui::{app_logic, run_studio};
