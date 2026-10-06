//! The colours the UI draws with. By default the Unicorn Tears tokens; a theme the user applied (see [`crate::theming`]) replaces them for as
//! long as it is on. Every colour lives in an atomic slot, so reading one costs a load and the drawing code needs no theme parameter; the
//! gradients are made from the accent colours on the way out.
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use theme::{GradientStop, Rgba, tokens as t};

fn pack(c: Rgba) -> u32 {
    c.to_u32()
}

fn unpack(v: u32) -> Rgba {
    Rgba::new((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)
}

macro_rules! colors {
    ($($name:ident: $slot:ident = $default:expr),* $(,)?) => {
        $(static $slot: AtomicU32 = AtomicU32::new($default.to_u32());)*

        /// Every colour token the UI draws with.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Colors {
            $(#[allow(missing_docs)] pub $name: Rgba,)*
            /// The three stops of the signature gradient (left to right, 120 degrees).
            pub tears: [Rgba; 3],
            /// The three stops of its vertical form (top to bottom).
            pub tears_v: [Rgba; 3],
            /// The three stops of the page's radial glow (centre to edge).
            pub night: [Rgba; 3],
        }

        impl Colors {
            /// The Unicorn Tears tokens.
            pub const DEFAULT: Colors = Colors {
                $($name: $default,)*
                tears: [t::MAGENTA_500, t::VIOLET_500, t::CYAN_500],
                tears_v: [t::CYAN_500, t::VIOLET_500, t::MAGENTA_500],
                night: [Rgba::rgb(42, 15, 74), Rgba::rgb(18, 12, 31), t::INK_900],
            };
        }

        impl Colors {
            /// Every token as `(name, colour)`: the plain tokens, then the nine gradient stops as `tears0`..`night2`.
            pub fn entries(&self) -> alloc::vec::Vec<(alloc::string::String, Rgba)> {
                let mut v = alloc::vec::Vec::new();
                $(v.push((alloc::string::String::from(stringify!($name)), self.$name));)*
                for (group, stops) in [("tears", &self.tears), ("tears_v", &self.tears_v), ("night", &self.night)] {
                    for (i, c) in stops.iter().enumerate() {
                        v.push((alloc::format!("{group}{i}"), *c));
                    }
                }
                v
            }

            /// Set the token (or gradient stop) called `name`; `false` for a name that is not one.
            pub fn set_named(&mut self, name: &str, c: Rgba) -> bool {
                $(if name == stringify!($name) {
                    self.$name = c;
                    return true;
                })*
                for (group, stops) in [("tears", &mut self.tears), ("tears_v", &mut self.tears_v), ("night", &mut self.night)] {
                    if let Some(i) = name.strip_prefix(group).and_then(|n| n.parse::<usize>().ok()).filter(|i| *i < 3) {
                        stops[i] = c;
                        return true;
                    }
                }
                false
            }
        }

        $(
            #[doc = concat!("The `", stringify!($name), "` colour.")]
            #[inline]
            pub fn $name() -> Rgba {
                unpack($slot.load(Relaxed))
            }
        )*

        /// Make `c` the colours in use.
        pub fn set(c: &Colors) {
            $($slot.store(pack(c.$name), Relaxed);)*
            for (i, v) in c.tears.iter().enumerate() {
                TEARS[i].store(pack(*v), Relaxed);
            }
            for (i, v) in c.tears_v.iter().enumerate() {
                TEARS_V[i].store(pack(*v), Relaxed);
            }
            for (i, v) in c.night.iter().enumerate() {
                NIGHT[i].store(pack(*v), Relaxed);
            }
        }

        /// The colours in use.
        pub fn get() -> Colors {
            Colors {
                $($name: $name(),)*
                tears: core::array::from_fn(|i| unpack(TEARS[i].load(Relaxed))),
                tears_v: core::array::from_fn(|i| unpack(TEARS_V[i].load(Relaxed))),
                night: core::array::from_fn(|i| unpack(NIGHT[i].load(Relaxed))),
            }
        }
    };
}

const fn slots(c: [Rgba; 3]) -> [AtomicU32; 3] {
    [AtomicU32::new(c[0].to_u32()), AtomicU32::new(c[1].to_u32()), AtomicU32::new(c[2].to_u32())]
}

static TEARS: [AtomicU32; 3] = slots([t::MAGENTA_500, t::VIOLET_500, t::CYAN_500]);
static TEARS_V: [AtomicU32; 3] = slots([t::CYAN_500, t::VIOLET_500, t::MAGENTA_500]);
static NIGHT: [AtomicU32; 3] = slots([Rgba::rgb(42, 15, 74), Rgba::rgb(18, 12, 31), t::INK_900]);

colors! {
    ink_900: INK_900_SLOT = t::INK_900,
    ink_850: INK_850_SLOT = t::INK_850,
    ink_800: INK_800_SLOT = t::INK_800,
    ink_700: INK_700_SLOT = t::INK_700,
    ink_600: INK_600_SLOT = t::INK_600,
    ink_500: INK_500_SLOT = t::INK_500,
    magenta_700: MAGENTA_700_SLOT = t::MAGENTA_700,
    magenta_500: MAGENTA_500_SLOT = t::MAGENTA_500,
    magenta_400: MAGENTA_400_SLOT = t::MAGENTA_400,
    cyan_600: CYAN_600_SLOT = t::CYAN_600,
    cyan_500: CYAN_500_SLOT = t::CYAN_500,
    cyan_400: CYAN_400_SLOT = t::CYAN_400,
    violet_600: VIOLET_600_SLOT = t::VIOLET_600,
    violet_500: VIOLET_500_SLOT = t::VIOLET_500,
    violet_400: VIOLET_400_SLOT = t::VIOLET_400,
    lime_500: LIME_500_SLOT = t::LIME_500,
    white: WHITE_SLOT = t::WHITE,
    pink_white: PINK_WHITE_SLOT = t::PINK_WHITE,
    text_strong: TEXT_STRONG_SLOT = t::TEXT_STRONG,
    text_body: TEXT_BODY_SLOT = t::TEXT_BODY,
    text_muted: TEXT_MUTED_SLOT = t::TEXT_MUTED,
    text_dim: TEXT_DIM_SLOT = t::TEXT_DIM,
    text_disabled: TEXT_DISABLED_SLOT = t::TEXT_DISABLED,
    border_subtle: BORDER_SUBTLE_SLOT = t::BORDER_SUBTLE,
    focus_ring: FOCUS_RING_SLOT = t::FOCUS_RING,
    warning: WARNING_SLOT = t::WARNING,
    danger: DANGER_SLOT = t::DANGER,
    bg_page: BG_PAGE_SLOT = t::BG_PAGE,
}

/// Back to the Unicorn Tears tokens.
pub fn reset() {
    set(&Colors::DEFAULT);
}

/// A linear gradient made of three colours (what the brand gradient is).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grad {
    /// CSS angle, degrees.
    pub angle_deg: f32,
    /// The stops.
    pub stops: [GradientStop; 3],
}

fn grad(angle_deg: f32, mid: f32, c: [Rgba; 3]) -> Grad {
    Grad {
        angle_deg,
        stops: [
            GradientStop { pos: 0.0, color: c[0] },
            GradientStop { pos: mid, color: c[1] },
            GradientStop { pos: 1.0, color: c[2] },
        ],
    }
}

/// The signature gradient, 120 degrees (magenta, violet, cyan by default).
pub fn gradient_tears() -> Grad {
    grad(120.0, 0.5, core::array::from_fn(|i| unpack(TEARS[i].load(Relaxed))))
}

/// Its vertical form (cyan, violet, magenta by default).
pub fn gradient_tears_v() -> Grad {
    grad(180.0, 0.55, core::array::from_fn(|i| unpack(TEARS_V[i].load(Relaxed))))
}

/// The radial glow behind the page: an ellipse of 120 by 90 percent centred at 50 and -10 percent.
pub fn gradient_night() -> RGrad {
    RGrad {
        rx_pct: 120.0,
        ry_pct: 90.0,
        cx_pct: 50.0,
        cy_pct: -10.0,
        stops: [
            GradientStop { pos: 0.0, color: unpack(NIGHT[0].load(Relaxed)) },
            GradientStop { pos: 0.45, color: unpack(NIGHT[1].load(Relaxed)) },
            GradientStop { pos: 1.0, color: unpack(NIGHT[2].load(Relaxed)) },
        ],
    }
}

/// A radial gradient made of three colours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RGrad {
    /// Horizontal radius, percent of the width.
    pub rx_pct: f32,
    /// Vertical radius, percent of the height.
    pub ry_pct: f32,
    /// Centre x, percent.
    pub cx_pct: f32,
    /// Centre y, percent.
    pub cy_pct: f32,
    /// The stops.
    pub stops: [GradientStop; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_token_set_and_set_replaces_all_of_it() {
        // One test owns the slots (the others only read the default, which this restores).
        reset();
        assert_eq!(get(), Colors::DEFAULT);
        assert_eq!(ink_900(), t::INK_900);
        assert_eq!(gradient_tears().stops[1].color, t::VIOLET_500);
        assert_eq!(gradient_night().stops[0].color, Rgba::rgb(42, 15, 74));
        let mut c = Colors::DEFAULT;
        c.ink_900 = Rgba::rgb(1, 2, 3);
        c.tears[0] = Rgba::rgb(9, 8, 7);
        set(&c);
        assert_eq!(ink_900(), Rgba::rgb(1, 2, 3));
        assert_eq!(gradient_tears().stops[0].color, Rgba::rgb(9, 8, 7));
        reset();
        assert_eq!(get(), Colors::DEFAULT);
    }
}
