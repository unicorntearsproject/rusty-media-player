//! Finding system fonts for characters the bundled fonts do not have (CJK, Arabic, Hebrew, Thai, ...).
//!
//! The UI asks once per character (see `rvp_ui::FontLoader`); the first request scans the installed fonts, preferring the families a
//! desktop usually has for the language the user speaks, then any font that has the character. Nothing is copied or shipped: the
//! file is read from where the system keeps it.
use fontdb::{Database, ID, Source, Stretch, Style};
use rvp_ui::{FontData, FontLoader};

/// Families to try first, by script. The order inside a script is the order of preference.
const PREFERRED: &[&str] = &[
    "Noto Sans CJK JP",
    "Noto Sans JP",
    "Noto Sans CJK SC",
    "Noto Sans SC",
    "Source Han Sans",
    "Yu Gothic UI",
    "Meiryo UI",
    "Microsoft YaHei UI",
    "Malgun Gothic",
    "Noto Sans CJK KR",
    "Noto Sans CJK TC",
    "Noto Sans",
    "Segoe UI",
    "Segoe UI Symbol",
    "DejaVu Sans",
    "Droid Sans Fallback",
    "WenQuanYi Micro Hei",
    "Arial Unicode MS",
    "Arial",
];

/// Locale-specific family to put first for CJK, from `LANG` and friends.
fn locale_first() -> Option<&'static str> {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))?;
    let l = lang.to_ascii_lowercase();
    if l.starts_with("ja") {
        Some("Noto Sans CJK JP")
    } else if l.starts_with("ko") {
        Some("Noto Sans CJK KR")
    } else if l.starts_with("zh_tw") || l.starts_with("zh_hk") {
        Some("Noto Sans CJK TC")
    } else if l.starts_with("zh") {
        Some("Noto Sans CJK SC")
    } else {
        None
    }
}

struct Finder {
    db: Database,
    order: Vec<ID>,
    /// Scans that found nothing; after a few the search gives up for the session (a name full of emoji must not stall the UI).
    misses: u32,
}

impl Finder {
    fn new() -> Self {
        let mut db = Database::new();
        db.load_system_fonts();
        // Flatpak mounts the host's fonts here.
        for dir in ["/run/host/fonts", "/run/host/user-fonts", "/usr/share/fonts", "/usr/local/share/fonts"] {
            if std::path::Path::new(dir).is_dir() {
                db.load_fonts_dir(dir);
            }
        }
        let mut order = Vec::new();
        let first = locale_first();
        for fam in first.into_iter().chain(PREFERRED.iter().copied()) {
            for f in db.faces() {
                let regular = f.style == Style::Normal && f.weight.0 == 400 && f.stretch == Stretch::Normal;
                if regular && f.families.iter().any(|(n, _)| n == fam) && !order.contains(&f.id) {
                    order.push(f.id);
                }
            }
        }
        let preferred = order.len();
        // Then everything else that is a plain, proportional face, up to a limit.
        for f in db.faces().filter(|f| f.style == Style::Normal && !f.monospaced && f.weight.0 <= 500) {
            if !order.contains(&f.id) {
                order.push(f.id);
            }
            if order.len() > preferred + 400 {
                break;
            }
        }
        Self { db, order, misses: 0 }
    }

    fn has(&self, id: ID, ch: char) -> bool {
        self.db
            .with_face_data(id, |data, index| {
                ttf_parser::Face::parse(data, index).ok().is_some_and(|f| f.glyph_index(ch).is_some())
            })
            .unwrap_or(false)
    }

    fn find(&mut self, ch: char) -> Option<FontData> {
        if self.misses >= 24 {
            return None;
        }
        let hit = self.order.iter().copied().find(|&id| self.has(id, ch));
        let Some(id) = hit else {
            self.misses += 1;
            return None;
        };
        let face = self.db.face(id)?;
        let index = face.index;
        let bytes = match &face.source {
            Source::File(p) => std::fs::read(p).ok()?,
            Source::SharedFile(p, _) => std::fs::read(p).ok()?,
            Source::Binary(b) => b.as_ref().as_ref().to_vec(),
        };
        Some(FontData { bytes, index })
    }
}

/// The loader to install in the UI. The font database is read on the first request, not at start-up.
pub fn system_font_loader() -> FontLoader {
    let mut finder: Option<Finder> = None;
    Box::new(move |ch| finder.get_or_insert_with(Finder::new).find(ch))
}
