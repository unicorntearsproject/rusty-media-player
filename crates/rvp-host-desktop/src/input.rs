//! Window-system events as the host-neutral input events of `rvp-host`.
use rvp_host::{Key, PointerButton};
use winit::event::MouseButton;
use winit::keyboard::{Key as WKey, NamedKey};

/// Pixels a wheel "line" scrolls (the web host gets pixels from the browser). One detent is one line, 40 px: the notch the UI's
/// volume, seek and list steps consume, and the line of Rusty Bucket's App API.
pub const WHEEL_LINE_PX: f32 = 40.0;

/// A logical key as the player's key, or `None` for keys it has no use for (modifiers on their own, dead keys).
pub fn map_key(k: &WKey) -> Option<Key> {
    Some(match k {
        WKey::Named(n) => match n {
            NamedKey::Space => Key::Space,
            NamedKey::Enter => Key::Enter,
            NamedKey::Escape => Key::Escape,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::Other("PageUp".into()),
            NamedKey::PageDown => Key::Other("PageDown".into()),
            NamedKey::Delete => Key::Other("Delete".into()),
            NamedKey::Backspace => Key::Other("Backspace".into()),
            NamedKey::Tab => Key::Other("Tab".into()),
            NamedKey::ContextMenu => Key::Other("ContextMenu".into()),
            // Full screen on the key every desktop player uses.
            NamedKey::F11 => Key::Char('f'),
            NamedKey::F1 => Key::Other("F1".into()),
            NamedKey::F10 => Key::Other("F10".into()),
            NamedKey::Shift
            | NamedKey::Control
            | NamedKey::Alt
            | NamedKey::Super
            | NamedKey::Meta
            | NamedKey::AltGraph => {
                return None;
            }
            _ => return None,
        },
        WKey::Character(s) => {
            let mut it = s.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => Key::Char(c),
                _ => Key::Other(s.to_string()),
            }
        }
        WKey::Dead(_) | WKey::Unidentified(_) => return None,
    })
}

/// A mouse button as the player's pointer button.
pub fn map_button(b: MouseButton) -> Option<PointerButton> {
    Some(match b {
        MouseButton::Left => PointerButton::Primary,
        MouseButton::Right => PointerButton::Secondary,
        MouseButton::Middle => PointerButton::Middle,
        MouseButton::Back => PointerButton::Back,
        MouseButton::Forward => PointerButton::Forward,
        MouseButton::Other(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map() {
        assert_eq!(map_key(&WKey::Named(NamedKey::Space)), Some(Key::Space));
        assert_eq!(map_key(&WKey::Character("k".into())), Some(Key::Char('k')));
        assert_eq!(map_key(&WKey::Named(NamedKey::F11)), Some(Key::Char('f')));
        assert_eq!(map_key(&WKey::Named(NamedKey::PageDown)), Some(Key::Other("PageDown".into())));
        assert_eq!(map_key(&WKey::Named(NamedKey::Shift)), None);
        assert_eq!(map_button(MouseButton::Back), Some(PointerButton::Back));
    }
}
