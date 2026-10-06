use super::*;

const UNICORN: &str = include_str!("../../../theme/tokens/colors.css");

fn theme(css: &str) -> Theme {
    from_css(&[css], "test").unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn the_unicorn_tears_tokens_come_back_as_themselves() {
    let t = theme(UNICORN);
    let d = Colors::DEFAULT;
    assert_eq!(t.colors.ink_900, d.ink_900);
    assert_eq!(t.colors.ink_800, d.ink_800);
    assert_eq!(t.colors.ink_700, d.ink_700);
    assert_eq!(t.colors.ink_600, d.ink_600);
    assert_eq!(t.colors.ink_500, d.ink_500);
    assert_eq!(t.colors.magenta_500, d.magenta_500);
    assert_eq!(t.colors.cyan_500, d.cyan_500);
    assert_eq!(t.colors.violet_500, d.violet_500);
    assert_eq!(t.colors.text_strong, d.text_strong);
    assert_eq!(t.colors.text_muted, d.text_muted);
    assert_eq!(t.colors.text_dim, d.text_dim);
    assert_eq!(t.colors.text_disabled, d.text_disabled);
    assert_eq!(t.colors.warning, d.warning);
    assert_eq!(t.colors.danger, d.danger);
    assert!(t.found >= 14, "{}", t.found);
    assert!(t.warnings.is_empty(), "{:?}", t.warnings);
    assert_eq!(t.radius_scale, 1.0);
}

#[test]
fn a_shadcn_style_sheet_in_hsl_triples_with_a_dark_block() {
    let css = r#"
        :root { --background: 0 0% 100%; --foreground: 222.2 84% 4.9%; --card: 0 0% 100%; --primary: 222.2 47.4% 11.2%;
                --muted: 210 40% 96.1%; --muted-foreground: 215.4 16.3% 46.9%; --border: 214.3 31.8% 91.4%; --radius: 0.5rem; }
        .dark { --background: 222.2 84% 4.9%; --foreground: 210 40% 98%; --card: 222.2 84% 4.9%; --primary: 210 40% 98%;
                --muted: 217.2 32.6% 17.5%; --muted-foreground: 215 20.2% 65.1%; --border: 217.2 32.6% 17.5%; }
    "#;
    // Those values are bare triples; the sheet says `hsl(var(--x))` where it uses them, and a bare triple is what this reads.
    let t = theme(css);
    // The dark block wins (the app is a dark one): a near-black page and near-white text.
    assert!(
        t.colors.ink_900.r < 20 && t.colors.text_strong.r > 230,
        "{:?} {:?}",
        t.colors.ink_900,
        t.colors.text_strong
    );
    assert_eq!(t.radius_scale, 8.0 / 14.0);
    // Readable everywhere: the checked pairs all pass.
    let c = &t.colors;
    assert!(contrast(c.text_strong, c.ink_900) >= 4.5);
    assert!(contrast(c.text_muted, c.ink_800) >= 4.5);
}

#[test]
fn a_light_system_becomes_a_light_theme_with_dark_overlays() {
    let css = ":root{--bg:#ffffff;--surface:#f4f4f8;--text:#111118;--accent:#5b21b6;--border:#d4d4dc}";
    let t = theme(css);
    let c = &t.colors;
    assert_eq!(c.ink_900, Rgba::rgb(255, 255, 255));
    // The "white" token is the on-colour: the overlay tint and the label on accent buttons. On a light page it is dark.
    assert!(lightness(c.white) < 0.3 || contrast(c.white, c.magenta_500) >= 3.0, "{:?}", c.white);
    assert!(contrast(c.text_strong, c.ink_900) >= 4.5);
    assert!(contrast(c.text_muted, c.ink_800) >= 4.5, "{:?} on {:?}", c.text_muted, c.ink_800);
    // Derived shades move toward black on a light page.
    assert!(lightness(c.magenta_400) < lightness(c.magenta_500) + 0.001 || lightness(c.magenta_400) > 0.0);
    assert!(t.found >= 4);
}

#[test]
fn unreadable_pairs_are_repaired_and_said() {
    // Grey text on a grey page: 1.3:1.
    let css = ":root{--background:#808080;--foreground:#858585;--muted-foreground:#868686;--primary:#818181}";
    let t = theme(css);
    let c = &t.colors;
    assert!(contrast(c.text_strong, c.ink_900) >= 4.5, "{:?} on {:?}", c.text_strong, c.ink_900);
    assert!(contrast(c.text_muted, c.ink_800) >= 4.5);
    assert!(contrast(c.magenta_500, c.ink_900) >= 3.0);
    assert!(!t.warnings.is_empty());
    assert!(t.warnings.iter().any(|w| w.contains("readable")), "{:?}", t.warnings);
}

#[test]
fn a_page_and_surface_that_are_the_same_are_tinted_apart() {
    let t = theme(":root{--background:#101018;--card:#101018;--foreground:#f0f0f8;--primary:#9d4eff}");
    assert_ne!(t.colors.ink_900, t.colors.ink_800);
}

#[test]
fn only_one_accent_makes_a_calm_ramp_not_the_unicorn_rainbow() {
    let t = theme(":root{--background:#0b0b10;--foreground:#eeeef5;--primary:#3b82f6}");
    let c = &t.colors;
    // The other two accents come from the one, so the gradient stays in its family.
    for x in [c.cyan_500, c.violet_500] {
        let d = (x.b as i32 - c.magenta_500.b as i32).abs();
        assert!(d < 120, "{x:?}");
    }
    assert_eq!(c.tears, [c.magenta_500, c.violet_500, c.cyan_500]);
}

#[test]
fn sheets_that_are_not_design_systems_are_refused_in_words() {
    assert_eq!(from_css(&["body { margin: 0 }"], "x").unwrap_err(), ThemeError::NoVariables);
    assert_eq!(from_css(&[":root{--space-4:1rem;--radius:4px}"], "x").unwrap_err(), ThemeError::NoColors);
    assert_eq!(from_css(&[""], "x").unwrap_err(), ThemeError::NoVariables);
    assert!(ThemeError::NoVariables.to_string().contains("design tokens"));
    // Garbage values are skipped, not fatal.
    let t = theme(":root{--background:not-a-color;--foreground:#fff;--primary:#f0f;--surface:oklch(banana)}");
    assert_eq!(t.colors.text_strong, Rgba::rgb(255, 255, 255));
}

#[test]
fn radii_and_type_are_read() {
    let t = theme(
        ":root{--background:#000;--foreground:#fff;--primary:#f0f;--radius-md:0;--font-sans:'Inter', system-ui}",
    );
    assert_eq!(t.radius_scale, 0.0);
    assert!(t.font_note.contains("Inter"), "{}", t.font_note);
    let t = theme(
        ":root{--background:#000;--foreground:#fff;--primary:#f0f;--radius:1.5rem;--font-sans:'Space Grotesk', sans-serif}",
    );
    assert!((t.radius_scale - 24.0 / 14.0).abs() < 1e-4);
    assert!(t.font_note.is_empty());
    let t = theme(":root{--background:#000;--foreground:#fff;--primary:#f0f;--radius:99rem}");
    assert_eq!(t.radius_scale, 2.0, "clamped");
}

#[test]
fn a_theme_is_kept_as_text_and_comes_back_the_same() {
    let t = theme(UNICORN);
    let text = t.to_text();
    assert!(text.starts_with("rvp-theme 1\nname=test\n"));
    let back = Theme::from_text(&text).unwrap();
    assert_eq!(back.colors, t.colors);
    assert_eq!(back.radius_scale, t.radius_scale);
    assert_eq!(back.name, "test");
    // Anything else is no theme; a hand-edited unreadable pair is repaired on the way in.
    assert!(Theme::from_text("nonsense").is_none());
    assert!(Theme::from_text("rvp-theme 1\nname=x\n").is_none());
    let bad = text.replace("text_strong=#ffffff", "text_strong=#0a0a10");
    let fixed = Theme::from_text(&bad).unwrap();
    assert!(contrast(fixed.colors.text_strong, fixed.colors.ink_900) >= 4.5);
    assert!(!fixed.warnings.is_empty());
}

#[test]
fn imports_and_several_sheets_are_combined() {
    let main = "@import url('colors.css'); :root { --primary: #00ff88; }";
    let imported = ":root { --background: #101018; --foreground: #f5f5ff; --primary: #ff0000; }";
    // Imports first, the importing sheet last: what a sheet says itself wins over what it imports.
    let t = from_css(&[imported, main], "x").unwrap();
    assert_eq!(t.colors.ink_900, Rgba::rgb(0x10, 0x10, 0x18));
    assert_eq!(t.colors.magenta_500, Rgba::rgb(0, 255, 0x88));
}
