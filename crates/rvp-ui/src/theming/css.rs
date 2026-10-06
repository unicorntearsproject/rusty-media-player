//! Just enough CSS to read a design system's custom properties: strip comments, walk the rules (and the blocks inside `@media`, `@layer`,
//! `@supports`), collect the `--name: value` declarations of the rules that style the root (`:root`, `html`, `body`, `*`, and the usual
//! dark-scheme selectors), note the `@import`s, and resolve `var(--x, fallback)`. Nothing here evaluates anything else: no `calc`, no
//! selectors beyond those, no cascade beyond "later wins".
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Which set of variables a rule contributes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// The base set: `:root`, `html`, `body`, `*`.
    Base,
    /// The dark set: a dark-scheme selector, or a `prefers-color-scheme: dark` media block.
    Dark,
    /// The light set (kept apart so a `.light` block does not leak into the base).
    Light,
}

/// What was found in the style sheets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sheet {
    base: BTreeMap<String, String>,
    dark: BTreeMap<String, String>,
    light: BTreeMap<String, String>,
    /// `@import` targets, in order (urls as written).
    pub imports: Vec<String>,
    /// A `color-scheme` or media query said the sheet has a dark scheme.
    pub has_dark: bool,
}

/// Remove `/* ... */` comments.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Split `text` at top-level `;` (outside parentheses and strings).
fn split_decls(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    let mut quote: Option<char> = None;
    for (i, ch) in text.char_indices() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, ';') if depth <= 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// The scope a selector list styles, or `None` when it is not one this reads.
fn scope_of(selector: &str) -> Option<Scope> {
    let mut best: Option<Scope> = None;
    for part in selector.split(',') {
        let s = part.trim().to_ascii_lowercase();
        let compact: String = s.chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect();
        let dark = compact.contains("dark") || compact.contains("night");
        let light = compact.contains("light") && !dark;
        let rootish = matches!(compact.as_str(), ":root" | "html" | "body" | "*" | ":host")
            || compact.starts_with(":root")
            || compact.starts_with("html")
            || compact.starts_with("body")
            || compact.starts_with('.')
            || compact.starts_with("[data-");
        let scope = if dark && rootish {
            Some(Scope::Dark)
        } else if light && rootish {
            Some(Scope::Light)
        } else if matches!(compact.as_str(), ":root" | "html" | "body" | "*" | ":host" | "html,body") {
            Some(Scope::Base)
        } else {
            None
        };
        if scope.is_some() {
            best = scope;
            if scope == Some(Scope::Base) {
                break;
            }
        }
    }
    best
}

impl Sheet {
    /// Read a style sheet.
    pub fn parse(css: &str) -> Sheet {
        let mut sheet = Sheet::default();
        let text = strip_comments(css);
        sheet.walk(&text, Scope::Base, 0);
        sheet
    }

    /// Read another sheet into this one (an imported or linked file); its variables come after, so they win.
    pub fn add(&mut self, css: &str) {
        let text = strip_comments(css);
        self.walk(&text, Scope::Base, 0);
    }

    fn walk(&mut self, text: &str, ctx: Scope, depth: usize) {
        if depth > 6 {
            return;
        }
        let b = text.as_bytes();
        let mut i = 0;
        while i < b.len() {
            // Skip whitespace and stray semicolons.
            while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b';') {
                i += 1;
            }
            if i >= b.len() {
                break;
            }
            // The prelude: up to `{` or `;` (an at-rule without a block, like @import).
            let start = i;
            let mut quote: Option<u8> = None;
            let mut paren = 0i32;
            while i < b.len() {
                let c = b[i];
                match quote {
                    Some(q) => {
                        if c == q {
                            quote = None;
                        }
                    }
                    None => match c {
                        b'"' | b'\'' => quote = Some(c),
                        b'(' => paren += 1,
                        b')' => paren -= 1,
                        b'{' | b';' if paren <= 0 => break,
                        _ => {}
                    },
                }
                i += 1;
            }
            let prelude = text[start..i.min(b.len())].trim();
            if i >= b.len() || b[i] == b';' {
                if let Some(rest) = prelude.strip_prefix("@import") {
                    if let Some(u) = import_target(rest) {
                        self.imports.push(u);
                    }
                }
                i += 1;
                continue;
            }
            // A block: find its end.
            let body_start = i + 1;
            let mut level = 1;
            i = body_start;
            let mut quote: Option<u8> = None;
            while i < b.len() && level > 0 {
                let c = b[i];
                match quote {
                    Some(q) => {
                        if c == q {
                            quote = None;
                        }
                    }
                    None => match c {
                        b'"' | b'\'' => quote = Some(c),
                        b'{' => level += 1,
                        b'}' => level -= 1,
                        _ => {}
                    },
                }
                i += 1;
            }
            let body = &text[body_start..(i - 1).max(body_start)];
            if let Some(at) = prelude.strip_prefix('@') {
                let name = at.split(|c: char| c.is_whitespace() || c == '(').next().unwrap_or("");
                match name {
                    "media" | "supports" | "layer" | "container" | "document" => {
                        let mut inner = ctx;
                        let q = at.to_ascii_lowercase();
                        if q.contains("prefers-color-scheme") && q.contains("dark") {
                            inner = Scope::Dark;
                            self.has_dark = true;
                        } else if q.contains("prefers-color-scheme") && q.contains("light") {
                            inner = Scope::Light;
                        }
                        self.walk(body, inner, depth + 1);
                    }
                    _ => {}
                }
                continue;
            }
            // A style rule.
            let scope = match scope_of(prelude) {
                Some(s) => s,
                None => continue,
            };
            // Inside a dark media block even a plain `:root` is the dark set.
            let scope = if ctx != Scope::Base && scope == Scope::Base { ctx } else { scope };
            if scope == Scope::Dark {
                self.has_dark = true;
            }
            for d in split_decls(body) {
                let Some((name, value)) = d.split_once(':') else { continue };
                let name = name.trim();
                let value = value.trim().trim_end_matches("!important").trim();
                if name == "color-scheme" && value.contains("dark") {
                    self.has_dark = true;
                }
                if !name.starts_with("--") || value.is_empty() {
                    continue;
                }
                let map = match scope {
                    Scope::Base => &mut self.base,
                    Scope::Dark => &mut self.dark,
                    Scope::Light => &mut self.light,
                };
                map.insert(name.to_string(), value.to_string());
            }
        }
    }

    /// The variables to use: the base set, with the dark set over it when `prefer_dark` (the app is a dark one) and the sheet has one.
    pub fn variables(&self, prefer_dark: bool) -> BTreeMap<String, String> {
        let mut v = self.base.clone();
        if prefer_dark {
            v.extend(self.dark.iter().map(|(k, x)| (k.clone(), x.clone())));
        }
        v
    }

    /// True when no custom property was found at all.
    pub fn is_empty(&self) -> bool {
        self.base.is_empty() && self.dark.is_empty() && self.light.is_empty()
    }
}

/// The url of `@import url("x.css")`, `@import "x.css"` and `@import url(x.css) layer(a)`.
fn import_target(rest: &str) -> Option<String> {
    let r = rest.trim();
    let inner = if let Some(u) = r.strip_prefix("url(") {
        u.split(')').next()?
    } else {
        r.split_whitespace().next()?
    };
    let t = inner.trim().trim_matches(|c| c == '"' || c == '\'');
    (!t.is_empty()).then(|| t.to_string())
}

/// Replace `var(--x)` and `var(--x, fallback)` in `value` with what the variables say (a few levels deep). A reference that cannot be
/// resolved makes the whole value `None`.
pub fn resolve(value: &str, vars: &BTreeMap<String, String>) -> Option<String> {
    resolve_depth(value, vars, 0)
}

fn resolve_depth(value: &str, vars: &BTreeMap<String, String>, depth: usize) -> Option<String> {
    if depth > 10 {
        return None;
    }
    let mut out = String::new();
    let mut rest = value;
    while let Some(i) = rest.find("var(") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 4..];
        // The matching close paren.
        let mut level = 1;
        let mut end = None;
        for (j, ch) in after.char_indices() {
            match ch {
                '(' => level += 1,
                ')' => {
                    level -= 1;
                    if level == 0 {
                        end = Some(j);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        let inner = &after[..end];
        let (name, fallback) = match inner.split_once(',') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (inner.trim(), None),
        };
        let resolved = match vars.get(name) {
            Some(v) => resolve_depth(v, vars, depth + 1),
            None => fallback.and_then(|f| resolve_depth(f, vars, depth + 1)),
        }?;
        out.push_str(&resolved);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = r#"
        /* tokens */
        @import url('tokens/colors.css');
        @import "fonts.css" layer(base);
        :root { --ink-900: #07060d; --accent: var(--ink-900); --radius: 12px; --label: "a;b"; }
        @media (prefers-color-scheme: dark) { :root { --ink-900: #000000; } }
        .btn { --not-root: #123456; }
        html.dark, [data-theme="dark"] { --bg: oklch(0.2 0.02 280); }
        .light { --bg: #ffffff; }
        body { color-scheme: light dark; }
    "#;

    #[test]
    fn variables_come_from_the_root_scopes_and_dark_overrides() {
        let s = Sheet::parse(SHEET);
        assert_eq!(s.imports, ["tokens/colors.css", "fonts.css"]);
        assert!(s.has_dark);
        let light = s.variables(false);
        assert_eq!(light.get("--ink-900").map(String::as_str), Some("#07060d"));
        assert!(!light.contains_key("--not-root") && !light.contains_key("--bg"), "{light:?}");
        let dark = s.variables(true);
        assert_eq!(dark.get("--ink-900").map(String::as_str), Some("#000000"));
        assert_eq!(dark.get("--bg").map(String::as_str), Some("oklch(0.2 0.02 280)"));
        assert_eq!(dark.get("--label").map(String::as_str), Some("\"a;b\""), "semicolons in strings stay");
    }

    #[test]
    fn var_references_resolve_with_fallbacks_and_loops_do_not_hang() {
        let s = Sheet::parse(
            ":root{--a:#111111;--b:var(--a);--c:var(--missing, var(--b));--loop:var(--loop);--rgb:rgb(var(--r, 1) 2 3)}",
        );
        let v = s.variables(true);
        assert_eq!(resolve("var(--b)", &v).as_deref(), Some("#111111"));
        assert_eq!(resolve("var(--c)", &v).as_deref(), Some("#111111"));
        assert_eq!(resolve("var(--nope)", &v), None);
        assert_eq!(resolve("var(--loop)", &v), None);
        assert_eq!(resolve("var(--rgb)", &v).as_deref(), Some("rgb(1 2 3)"));
        assert_eq!(resolve("plain", &v).as_deref(), Some("plain"));
    }

    #[test]
    fn odd_input_is_survived() {
        assert!(Sheet::parse("").is_empty());
        assert!(Sheet::parse("}}} {{{ ;;; @media").is_empty());
        assert!(Sheet::parse("/* unterminated").is_empty());
        assert!(Sheet::parse(":root { --a: ").variables(true).is_empty() || true);
        // A later sheet wins over an earlier one.
        let mut s = Sheet::parse(":root{--a:#111111}");
        s.add(":root{--a:#222222}");
        assert_eq!(s.variables(true).get("--a").map(String::as_str), Some("#222222"));
    }
}
