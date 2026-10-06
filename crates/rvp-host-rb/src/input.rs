//! Key, button and modifier numbers of the App API as the host-neutral input types.
use bucket_v0_sys::{self as sys, key};
use rvp_host::{Key, Modifiers, PointerButton};

/// The player's key for an API key code, or `None` for keys it has no use for.
pub fn map_key(k: u32) -> Option<Key> {
    if k < key::NAMED_BASE {
        // A character: Space is U+0020 (our `Key::Space`), control characters are not keys.
        return match char::from_u32(k)? {
            ' ' => Some(Key::Space),
            c if c.is_control() => None,
            c => Some(Key::Char(c)),
        };
    }
    let n = k - key::NAMED_BASE;
    Some(match k {
        key::ENTER => Key::Enter,
        key::ESCAPE => Key::Escape,
        key::LEFT => Key::Left,
        key::RIGHT => Key::Right,
        key::UP => Key::Up,
        key::DOWN => Key::Down,
        key::HOME => Key::Home,
        key::END => Key::End,
        key::TAB => Key::Other("Tab".into()),
        key::BACKSPACE => Key::Other("Backspace".into()),
        key::DELETE => Key::Other("Delete".into()),
        key::INSERT => Key::Other("Insert".into()),
        key::PAGE_UP => Key::Other("PageUp".into()),
        key::PAGE_DOWN => Key::Other("PageDown".into()),
        key::MENU => Key::Other("ContextMenu".into()),
        // Full screen on the key every desktop player uses (as the desktop host does).
        k if k == key::F1 + 10 => Key::Char('f'),
        k if (key::F1..=key::F24).contains(&k) => Key::Other(format!("F{}", k - key::F1 + 1)),
        key::MEDIA_PLAY_PAUSE => Key::Other("MediaPlayPause".into()),
        key::MEDIA_NEXT => Key::Other("MediaNext".into()),
        key::MEDIA_PREV => Key::Other("MediaPrev".into()),
        key::MEDIA_STOP => Key::Other("MediaStop".into()),
        key::VOLUME_UP => Key::Other("VolumeUp".into()),
        key::VOLUME_DOWN => Key::Other("VolumeDown".into()),
        key::VOLUME_MUTE => Key::Other("VolumeMute".into()),
        key::BROWSER_BACK => Key::Other("BrowserBack".into()),
        key::BROWSER_FORWARD => Key::Other("BrowserForward".into()),
        _ => {
            let _ = n;
            return None;
        }
    })
}

/// Modifier bits as [`Modifiers`].
pub fn map_mods(m: u32) -> Modifiers {
    Modifiers {
        shift: m & sys::mods::SHIFT != 0,
        ctrl: m & sys::mods::CTRL != 0,
        alt: m & sys::mods::ALT != 0,
        logo: m & sys::mods::SUPER != 0,
    }
}

/// The pointer button for an API button number.
pub fn map_button(b: u32) -> Option<PointerButton> {
    Some(match b {
        sys::button::PRIMARY => PointerButton::Primary,
        sys::button::SECONDARY => PointerButton::Secondary,
        sys::button::MIDDLE => PointerButton::Middle,
        sys::button::BACK => PointerButton::Back,
        sys::button::FORWARD => PointerButton::Forward,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map() {
        assert_eq!(map_key(0x20), Some(Key::Space));
        assert_eq!(map_key('k' as u32), Some(Key::Char('k')));
        assert_eq!(map_key('é' as u32), Some(Key::Char('é')));
        assert_eq!(map_key(0x07), None);
        assert_eq!(map_key(0xD800), None);
        assert_eq!(map_key(key::ENTER), Some(Key::Enter));
        assert_eq!(map_key(key::RIGHT), Some(Key::Right));
        assert_eq!(map_key(key::PAGE_DOWN), Some(Key::Other("PageDown".into())));
        assert_eq!(map_key(key::F1), Some(Key::Other("F1".into())));
        assert_eq!(map_key(key::F1 + 10), Some(Key::Char('f')));
        assert_eq!(map_key(key::F24), Some(Key::Other("F24".into())));
        assert_eq!(map_key(key::MEDIA_NEXT), Some(Key::Other("MediaNext".into())));
        assert_eq!(map_key(key::NAMED_BASE + 99), None);
        assert_eq!(map_button(3), Some(PointerButton::Back));
        assert_eq!(map_button(9), None);
        let m = map_mods(sys::mods::SHIFT | sys::mods::SUPER);
        assert!(m.shift && m.logo && !m.ctrl && !m.alt);
    }
}
