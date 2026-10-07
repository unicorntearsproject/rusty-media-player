//! The Theme dialog: paste a link to a Claude Design system (or its CSS), preview the look live, apply it, or go back to Unicorn Tears.
//!
//! The flow: the text in the box is either a link or CSS. A link goes to the host's [`Net`] capability (a page the user pointed at, fetched
//! only when *Preview* is pressed); an HTML answer is searched for its style sheets (`<style>` blocks, `<link rel="stylesheet">`,
//! `@import`), which are fetched in turn (a handful, never more); CSS pasted into the box skips the fetching. The sheets go to
//! [`rvp_ui::theming::from_css`], which maps and checks them, and the result is put on screen as a preview. *Apply* keeps it (saved
//! per user under `settings/theme`); closing the dialog without applying puts the previous look back.
use super::*;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_ui::theming::{self, THEME_KEY, Theme};

/// Style sheets fetched for one link, at most (the page's own and what it imports).
const MAX_SHEETS: usize = 10;
/// The box holds at most this many characters (a long style sheet fits; nothing absurd does).
const MAX_INPUT: usize = 400_000;

/// One sheet being fetched, or fetched.
#[derive(Debug, Clone)]
struct Job {
    ticket: u32,
    url: String,
    /// How deep the import chain is (the main page is 0).
    depth: usize,
}

/// What the dialog and the fetching are doing.
#[derive(Debug, Default)]
pub(crate) struct ThemeUi {
    loaded: bool,
    /// The theme in force (kept in storage); the preview goes over it.
    active: Option<Theme>,
    /// A preview on screen that has not been applied.
    candidate: Option<Theme>,
    /// What the box holds.
    pub input: String,
    /// Lines under the box: what is going on, what was found, what went wrong.
    status: Vec<String>,
    /// Fetches in flight.
    jobs: Vec<Job>,
    /// Sheets fetched so far, in the order they will be read (imports before what imports them).
    sheets: Vec<(usize, String)>,
    /// The page the link pointed at (its name for the theme).
    link: String,
    /// More sheets were found than are fetched.
    capped: bool,
    /// Pasted CSS that only imports other sheets: the text, and the absolute https addresses offered for fetching (pressing *Preview* again
    /// with the same text takes the offer).
    offer: Option<(String, Vec<String>)>,
}

impl ThemeUi {
    pub(crate) fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// The text the box shows: a link as it is, a long paste as its size.
    pub(crate) fn shown(&self) -> String {
        if self.input.chars().count() > 120 || self.input.contains('\n') {
            let lines = self.input.lines().count();
            alloc::format!("{} characters pasted ({} lines)", self.input.chars().count(), lines)
        } else {
            self.input.clone()
        }
    }

    /// The paragraphs of the dialog under the introduction.
    pub(crate) fn body(&self, current: &str) -> Vec<String> {
        let mut v = alloc::vec![alloc::format!("Now: {current}.")];
        v.push("Paste the link to a Claude Design system, or its CSS. Rusty Wave reads its colors and corner radius, checks that everything stays readable, and shows you the result before you keep it.".into());
        v.extend(self.status.iter().cloned());
        v
    }

    /// The dialog opens (fresh): nothing previewed, nothing fetching. What is in the box stays (a person may come back to it).
    pub(crate) fn theme_opened_state(&mut self) {
        self.status.clear();
        self.candidate = None;
        self.jobs.clear();
        self.sheets.clear();
        self.capped = false;
        self.offer = None;
    }

    pub(crate) fn has_candidate(&self) -> bool {
        self.candidate.is_some()
    }

    pub(crate) fn active_name(&self) -> String {
        self.active.as_ref().map_or_else(|| "Unicorn Tears".to_string(), |t| t.name.clone())
    }
}

/// True when the box holds a web address (and not CSS).
pub(crate) fn looks_like_link(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with("http://") || t.starts_with("https://"))
        && !t.contains(char::is_whitespace)
        && !t.contains('{')
}

/// Resolve `href` against the page at `base` (absolute links, `//host/x`, `/x` and relative ones).
pub(crate) fn resolve_url(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.starts_with("data:") || href.starts_with('#') {
        return None;
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_string());
    }
    let (scheme, rest) = base.split_once("://")?;
    let (host, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
    if let Some(r) = href.strip_prefix("//") {
        return Some(alloc::format!("{scheme}://{r}"));
    }
    if let Some(r) = href.strip_prefix('/') {
        return Some(alloc::format!("{scheme}://{host}/{r}"));
    }
    // Relative to the directory of the page (query and fragment dropped).
    let path = path.split(['?', '#']).next().unwrap_or("");
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for seg in href.split(['?', '#']).next().unwrap_or("").split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    Some(alloc::format!("{scheme}://{host}/{}", parts.join("/")))
}

/// What a fetched page names: its `<style>` blocks (as CSS text) and the style sheets it links.
pub(crate) fn scan_html(html: &str) -> (Vec<String>, Vec<String>) {
    let lower = html.to_ascii_lowercase();
    let mut styles = Vec::new();
    let mut links = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<style") {
        let open = at + i;
        let Some(gt) = lower[open..].find('>') else { break };
        let start = open + gt + 1;
        let Some(end) = lower[start..].find("</style") else { break };
        styles.push(html[start..start + end].to_string());
        at = start + end;
    }
    let mut at = 0;
    while let Some(i) = lower[at..].find("<link") {
        let open = at + i;
        let Some(gt) = lower[open..].find('>') else { break };
        let tag = &html[open..open + gt];
        let tag_l = &lower[open..open + gt];
        at = open + gt;
        if !tag_l.contains("stylesheet") {
            continue;
        }
        if let Some(h) = attr(tag, tag_l, "href") {
            links.push(h);
        }
    }
    (styles, links)
}

/// The value of attribute `name` in a tag (quoted or not).
fn attr(tag: &str, tag_l: &str, name: &str) -> Option<String> {
    let key = alloc::format!("{name}=");
    let mut from = 0;
    while let Some(i) = tag_l[from..].find(&key) {
        let at = from + i;
        // A whole attribute name: preceded by whitespace.
        if at > 0 && !tag_l.as_bytes()[at - 1].is_ascii_whitespace() {
            from = at + key.len();
            continue;
        }
        let v = &tag[at + key.len()..];
        let v = match v.chars().next()? {
            q @ ('"' | '\'') => v[1..].split(q).next()?,
            _ => v.split(|c: char| c.is_whitespace() || c == '>').next()?,
        };
        return Some(v.to_string());
    }
    None
}

/// Does this text look like a style sheet (rather than a page)?
pub(crate) fn looks_like_css(text: &str) -> bool {
    let head = text.trim_start();
    !head.starts_with('<') && (text.contains(":root") || text.contains("--") || text.contains('{'))
}

impl App {
    /// Load the saved theme once and put it on screen; poll the fetches of the Theme dialog.
    pub(crate) fn theme_tick<H>(&mut self, host: &mut H)
    where
        H: Host<Video = FrameSink>,
    {
        if !self.themeui.loaded {
            self.themeui.loaded = true;
            let stored = rvp_core::task::block_on(host.storage().load(THEME_KEY));
            if let Some(t) =
                stored.as_deref().and_then(|b| core::str::from_utf8(b).ok()).and_then(Theme::from_text)
            {
                t.apply();
                self.themeui.active = Some(t);
                self.base_dirty = true;
                self.force_draw = true;
            }
        }
        if self.themeui.jobs.is_empty() {
            return;
        }
        // Collect what arrived.
        let mut arrived: Vec<(Job, Result<String, String>)> = Vec::new();
        if let Some(net) = host.net() {
            let jobs = core::mem::take(&mut self.themeui.jobs);
            for j in jobs {
                match net.poll_fetch(j.ticket) {
                    Some(r) => arrived.push((j, r)),
                    None => self.themeui.jobs.push(j),
                }
            }
        } else {
            self.themeui.jobs.clear();
            self.themeui.status = alloc::vec!["This host cannot fetch links. Paste the CSS instead.".into()];
        }
        for (job, result) in arrived {
            self.theme_arrived(host, job, result);
        }
        if self.themeui.jobs.is_empty() && !self.themeui.sheets.is_empty() && self.themeui.candidate.is_none()
        {
            self.theme_build();
        }
        self.base_dirty = true;
        self.force_draw = true;
    }

    fn theme_arrived<H: Host<Video = FrameSink>>(
        &mut self,
        host: &mut H,
        job: Job,
        result: Result<String, String>,
    ) {
        let text = match result {
            Ok(t) => t,
            Err(e) => {
                if job.depth == 0 {
                    self.themeui.status = alloc::vec![
                        alloc::format!("Couldn't read that link ({e})."),
                        "Open it in your browser, copy the style sheet's text and paste it here instead."
                            .into(),
                    ];
                    self.themeui.sheets.clear();
                    self.themeui.jobs.clear();
                } else {
                    // A sheet it imports that does not load: the rest may still be enough.
                    self.themeui.status.push(alloc::format!("Skipped {}: {e}", job.url));
                }
                return;
            }
        };
        let mut next: Vec<String> = Vec::new();
        if looks_like_css(&text) {
            let sheet = rvp_ui::theming::css::Sheet::parse(&text);
            for i in &sheet.imports {
                if let Some(u) = resolve_url(&job.url, i) {
                    next.push(u);
                }
            }
            self.themeui.sheets.push((job.depth, text));
        } else {
            let (styles, links) = scan_html(&text);
            for s in styles {
                let sheet = rvp_ui::theming::css::Sheet::parse(&s);
                for i in &sheet.imports {
                    if let Some(u) = resolve_url(&job.url, i) {
                        next.push(u);
                    }
                }
                self.themeui.sheets.push((job.depth, s));
            }
            for l in links {
                if let Some(u) = resolve_url(&job.url, &l) {
                    next.push(u);
                }
            }
        }
        if job.depth >= 2 {
            return;
        }
        for u in next {
            let already = self.themeui.sheets.len() + self.themeui.jobs.len();
            if already >= MAX_SHEETS {
                self.themeui.capped = true;
                break;
            }
            if self.themeui.jobs.iter().any(|j| j.url == u) {
                continue;
            }
            if let Some(net) = host.net() {
                let ticket = net.fetch_text(&u);
                self.themeui.jobs.push(Job { ticket, url: u, depth: job.depth + 1 });
            }
        }
    }

    /// Every sheet is in: make the theme and put it on screen as a preview.
    fn theme_build(&mut self) {
        // Imports (deeper) come first, the page's own sheets last: what a sheet says itself wins over what it imports.
        let mut sheets = core::mem::take(&mut self.themeui.sheets);
        sheets.sort_by_key(|(d, _)| core::cmp::Reverse(*d));
        let texts: Vec<&str> = sheets.iter().map(|(_, s)| s.as_str()).collect();
        let name = host_of(&self.themeui.link);
        self.theme_make(&texts, &name);
        self.themeui.sheets = sheets;
        self.themeui.sheets.clear();
    }

    fn theme_make(&mut self, texts: &[&str], name: &str) {
        match theming::from_css(texts, name) {
            Ok(t) => {
                t.apply();
                let mut status = alloc::vec![alloc::format!(
                    "Previewing {}: {} colors found, the rest worked out from them.",
                    t.name,
                    t.found
                )];
                if t.radius_scale != 1.0 {
                    status.push(alloc::format!(
                        "Corners at {}% of the usual roundness.",
                        (t.radius_scale * 100.0) as i32
                    ));
                }
                status.extend(t.warnings.iter().take(3).cloned());
                if t.warnings.len() > 3 {
                    status.push(alloc::format!("{} more adjustments for readability.", t.warnings.len() - 3));
                }
                if !t.font_note.is_empty() {
                    status.push(t.font_note.clone());
                }
                if self.themeui.capped {
                    status.push(
                        "The page names more style sheets than were read; the first ones were used.".into(),
                    );
                }
                self.themeui.status = status;
                self.themeui.candidate = Some(t);
            }
            Err(e) => {
                // Put the look that was on screen back.
                self.theme_restore();
                self.themeui.status = alloc::vec![e.to_string()];
                self.themeui.candidate = None;
            }
        }
    }

    /// The look that was in force before a preview.
    fn theme_restore(&mut self) {
        match &self.themeui.active {
            Some(t) => t.apply(),
            None => theming::reset(),
        }
    }

    /// *Preview* was pressed.
    pub(crate) fn theme_preview<H: Host<Video = FrameSink>>(&mut self, host: &mut H) {
        let text = self.themeui.input.trim().to_string();
        self.themeui.candidate = None;
        self.themeui.jobs.clear();
        self.themeui.sheets.clear();
        self.themeui.capped = false;
        if text.is_empty() {
            self.themeui.status = alloc::vec!["Paste a link or some CSS first.".into()];
            return;
        }
        if looks_like_link(&text) {
            let Some(net) = host.net() else {
                self.themeui.status = alloc::vec!["This host cannot fetch links. Open the link in your browser, copy the style sheet's text and paste it here.".into()];
                return;
            };
            self.themeui.link = text.clone();
            let ticket = net.fetch_text(&text);
            self.themeui.jobs.push(Job { ticket, url: text, depth: 0 });
            self.themeui.status = alloc::vec!["Fetching\u{2026}".into()];
        } else if let Some(urls) = self.themeui.offer.take().filter(|(t, _)| *t == text).map(|(_, u)| u) {
            // The offer was taken: fetch the files the pasted CSS imports; it is read last, so what it says itself wins.
            let Some(net) = host.net() else { return };
            self.themeui.link = urls.first().cloned().unwrap_or_default();
            self.themeui.sheets.push((0, text));
            for u in urls.into_iter().take(MAX_SHEETS - 1) {
                let ticket = net.fetch_text(&u);
                self.themeui.jobs.push(Job { ticket, url: u, depth: 1 });
            }
            self.themeui.status = alloc::vec!["Fetching the imported files\u{2026}".into()];
        } else {
            let t = text.clone();
            self.theme_make(&[&t], "Pasted theme");
            if self.themeui.candidate.is_none() {
                self.theme_explain_imports(host, &t);
            }
        }
    }

    /// Pasted CSS that gave no theme but imports other sheets (a design system's `styles.css` is often only `@import` lines): say so, and
    /// offer to fetch the imports that are absolute https addresses.
    fn theme_explain_imports<H: Host<Video = FrameSink>>(&mut self, host: &mut H, text: &str) {
        let imports = rvp_ui::theming::css::Sheet::parse(text).imports;
        if imports.is_empty() {
            return;
        }
        let https: Vec<String> =
            imports.iter().filter(|i| i.trim().starts_with("https://")).fold(Vec::new(), |mut v, i| {
                if !v.contains(i) {
                    v.push(i.trim().to_string());
                }
                v
            });
        let n = imports.len();
        let mut status = alloc::vec![alloc::format!(
            "That CSS only imports {} style sheet{} (@import) and has no design tokens of its own, so there is nothing to read yet.",
            n,
            if n == 1 { "" } else { "s" }
        )];
        if !https.is_empty() && host.net().is_some() {
            status.push(alloc::format!(
                "Press Preview again to fetch the {} imported file{} from the web: {}.",
                https.len(),
                if https.len() == 1 { "" } else { "s" },
                https.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            ));
            status.push("Or paste the link to the design system, or the CSS of the imported files.".into());
            self.themeui.offer = Some((text.trim().to_string(), https));
        } else {
            status.push("Paste the link to the design system, or the CSS of the imported files (copy their text from the browser).".into());
        }
        self.themeui.status = status;
    }

    /// *Apply* was pressed: keep the preview.
    pub(crate) fn theme_apply<H: Host<Video = FrameSink>>(&mut self, host: &mut H, now: Timestamp) {
        let Some(t) = self.themeui.candidate.take() else { return };
        rvp_core::task::block_on(host.storage().store(THEME_KEY, t.to_text().as_bytes()));
        self.themeui.status = alloc::vec![alloc::format!("{} is now your theme.", t.name)];
        self.ui.show_toast(&alloc::format!("Theme: {}", t.name), now);
        self.themeui.active = Some(t);
    }

    /// *Reset* was pressed: Unicorn Tears again.
    pub(crate) fn theme_reset<H: Host<Video = FrameSink>>(&mut self, host: &mut H) {
        theming::reset();
        self.themeui.active = None;
        self.themeui.candidate = None;
        self.themeui.jobs.clear();
        self.themeui.status = alloc::vec!["Back to Unicorn Tears.".into()];
        rvp_core::task::block_on(host.storage().store(THEME_KEY, &[]));
    }

    /// The dialog closes: an unapplied preview goes away.
    pub(crate) fn theme_closed(&mut self) {
        if self.themeui.candidate.take().is_some() || !self.themeui.jobs.is_empty() {
            self.theme_restore();
        }
        self.themeui.jobs.clear();
        self.themeui.sheets.clear();
    }

    /// A character typed into the box.
    pub(crate) fn theme_char(&mut self, c: char) {
        if self.themeui.input.chars().count() < MAX_INPUT && !c.is_control() {
            self.themeui.input.push(c);
        }
    }

    pub(crate) fn theme_backspace(&mut self) {
        self.themeui.input.pop();
    }

    /// Pasted text into the box.
    pub(crate) fn theme_paste(&mut self, text: &str) {
        let room = MAX_INPUT.saturating_sub(self.themeui.input.chars().count());
        // A pasted link replaces what was in the box; pasted CSS is added (a person may paste it in pieces).
        if looks_like_link(text) {
            self.themeui.input.clear();
        }
        let t: String =
            text.chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).take(room).collect();
        self.themeui.input.push_str(&t);
    }

    /// The theme in force (for the snapshot and tests).
    pub fn theme_name(&self) -> String {
        self.themeui.active_name()
    }
}

/// `https://host/path` as `host` (a name for the theme).
fn host_of(link: &str) -> String {
    link.split_once("://")
        .map(|(_, r)| r.split('/').next().unwrap_or(r))
        .filter(|h| !h.is_empty())
        .map_or_else(|| "Linked theme".to_string(), |h| h.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_told_from_css() {
        assert!(looks_like_link("https://claude.ai/artifact/abc?x=1"));
        assert!(looks_like_link("  http://localhost:8080/styles.css  "));
        assert!(!looks_like_link(":root { --a: #fff; }"));
        assert!(!looks_like_link("https://x.y/a b"));
        assert!(!looks_like_link("ftp://x"));
        assert!(looks_like_css(":root{--a:#fff}"));
        assert!(!looks_like_css("<!doctype html><style>:root{}</style>"));
    }

    #[test]
    fn urls_resolve_the_way_a_browser_does() {
        let base = "https://ex.com/a/b/page.html?q=1";
        assert_eq!(resolve_url(base, "x.css").as_deref(), Some("https://ex.com/a/b/x.css"));
        assert_eq!(resolve_url(base, "./tokens/c.css").as_deref(), Some("https://ex.com/a/b/tokens/c.css"));
        assert_eq!(resolve_url(base, "../c.css").as_deref(), Some("https://ex.com/a/c.css"));
        assert_eq!(resolve_url(base, "/root.css").as_deref(), Some("https://ex.com/root.css"));
        assert_eq!(resolve_url(base, "//cdn.x/y.css").as_deref(), Some("https://cdn.x/y.css"));
        assert_eq!(resolve_url(base, "https://o.org/z.css").as_deref(), Some("https://o.org/z.css"));
        assert_eq!(resolve_url(base, "data:text/css,x"), None);
        assert_eq!(resolve_url("https://ex.com", "a.css").as_deref(), Some("https://ex.com/a.css"));
    }

    #[test]
    fn a_page_is_scanned_for_its_style_sheets() {
        let html = r#"<html><head>
            <link rel="preload" href="font.woff2">
            <link rel='stylesheet' href='styles.css'>
            <LINK REL=stylesheet HREF=other.css>
            <style>:root{--a:#111}</style>
            <style type="text/css">body{}</style></head></html>"#;
        let (styles, links) = scan_html(html);
        assert_eq!(links, ["styles.css", "other.css"]);
        assert_eq!(styles.len(), 2);
        assert!(styles[0].contains("--a"));
    }
}
