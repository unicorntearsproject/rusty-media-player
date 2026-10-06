//! The player UI, drawn by us into an RGBA framebuffer with Unicorn Tears tokens so it looks the same
//! in a browser and in Rusty Bucket's Canvas surface.
//!
//! * [`Ui`] is the state machine: layout, hit testing, pointer and keyboard handling, menus, auto-hide. It
//!   turns [`rvp_host::InputEvent`]s into [`Action`]s and never touches the player.
//! * [`UiModel`] is the read-only snapshot it draws.
//! * Drawing (`draw_base`, `draw_overlay`) goes into a [`FrameBuffer`] with software primitives; text uses
//!   bundled OFL fonts and icons are Lucide path data.
#![no_std]
#![forbid(unsafe_code)]
// Drawing helpers take a framebuffer, a face, geometry and a colour: many arguments by nature.
#![allow(clippy::too_many_arguments)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod actions;
pub mod audio_panel;
pub mod dialog;
mod draw;
pub mod font;
pub mod gfx;
pub mod icon;
mod icon_data;
pub mod lib_ui;
mod logo;
pub mod model;
pub mod theming;
pub mod tk;
mod subs;
pub mod ui;

pub use actions::{Action, MenuItem, SHORTCUTS, SPEEDS, Shortcut};
pub use audio_panel::{AudioControl, AudioPanelGeom};
pub use dialog::{DialogButton, DialogControl, DialogGeom, DialogSpec, DialogToggle};
pub use font::{FontData, FontLoader};
pub use gfx::{FrameBuffer, Paint, RectF};
pub use lib_ui::{Detail, Enqueue, LibAction, LibCtx, LibHit, LibUi, Mode, Scope, UiCommand, View};
pub use model::{AppModel, ChapterItem, MediaState, PlaylistEntry, TrackItem, UiModel, format_time};
pub use ui::{Cursor, HIDE_AFTER_US, Layout, Target, Ui, UiConfig};
