//! Input events: keyboard and pointer, matching Rusty Bucket's keyboard/mouse parity model.
use alloc::string::String;

/// A rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    /// Left.
    pub x: i32,
    /// Top.
    pub y: i32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
}

/// Modifier keys held during an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Shift.
    pub shift: bool,
    /// Control.
    pub ctrl: bool,
    /// Alt / Option.
    pub alt: bool,
    /// Super / Meta.
    pub logo: bool,
}

/// Logical key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// A printable character (already case/layout resolved).
    Char(char),
    /// Space bar.
    Space,
    /// Enter / Return.
    Enter,
    /// Escape.
    Escape,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Home.
    Home,
    /// End.
    End,
    /// Any other key, by host-reported name.
    Other(String),
}

/// Pointer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    /// Primary (left).
    Primary,
    /// Secondary (right): context menu.
    Secondary,
    /// Middle.
    Middle,
    /// Back (browser-style).
    Back,
    /// Forward.
    Forward,
}

/// One input event.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// Key pressed (`repeat` is true for auto-repeat).
    KeyDown {
        /// The key.
        key: Key,
        /// Modifiers.
        mods: Modifiers,
        /// Auto-repeat.
        repeat: bool,
    },
    /// Key released.
    KeyUp {
        /// The key.
        key: Key,
        /// Modifiers.
        mods: Modifiers,
    },
    /// Pointer moved.
    PointerMove {
        /// X in physical pixels.
        x: f32,
        /// Y in physical pixels.
        y: f32,
    },
    /// Button pressed.
    PointerDown {
        /// X.
        x: f32,
        /// Y.
        y: f32,
        /// Button.
        button: PointerButton,
    },
    /// Button released.
    PointerUp {
        /// X.
        x: f32,
        /// Y.
        y: f32,
        /// Button.
        button: PointerButton,
    },
    /// Wheel or trackpad scroll (positive `dy` scrolls down; `dx` covers tilt wheels).
    Wheel {
        /// Horizontal delta.
        dx: f32,
        /// Vertical delta.
        dy: f32,
    },
    /// Surface resized (physical pixels, device pixel ratio).
    Resize {
        /// Width.
        w: u32,
        /// Height.
        h: u32,
        /// Device pixel ratio.
        dpr: f32,
    },
    /// A file was dropped on the window (host-defined identifier for `OpenRequest::Id`).
    Drop {
        /// Identifier.
        id: String,
    },
    /// A file is being dragged over the window (`true`) or left it (`false`); lets the UI show a drop target.
    DragOver(bool),
    /// Window focus changed.
    Focus(bool),
}
