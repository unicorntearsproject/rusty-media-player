//! A JSON description of the library screen for tests and tooling: the view, what is on screen and where.
use super::rows::EntKind;
use super::{Detail, LibCtx, Mode, View};
use crate::gfx::RectF;
use crate::model::UiModel;
use crate::ui::Ui;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

fn esc(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn rect(r: RectF) -> String {
    format!("{{\"x\":{:.1},\"y\":{:.1},\"w\":{:.1},\"h\":{:.1}}}", r.x, r.y, r.w, r.h)
}

fn view_name(v: View) -> &'static str {
    match v {
        View::NowPlaying => "nowplaying",
        View::Albums => "albums",
        View::Artists => "artists",
        View::Tracks => "tracks",
        View::Playlists => "playlists",
        View::Queue => "queue",
        View::Search => "search",
        View::Visualizer => "visualizer",
    }
}

impl Ui {
    /// The library face as a JSON object: view, detail, search text, what is selected and the rectangles of what was drawn
    /// last (rail entries, bar buttons, the header's controls, and the rows and cards on screen with their labels).
    pub fn lib_snapshot_json(&self, ctx: &LibCtx<'_>, model: &UiModel) -> String {
        let lib = ctx.lib;
        let l = &self.lib;
        let mut j = String::from("{");
        j += &format!(
            "\"mode\":\"{}\",\"view\":\"{}\",\"query\":{},\"typing\":{},\"selected\":{},\"scroll\":{:.1},",
            if l.mode == Mode::Library { "library" } else { "player" },
            view_name(l.view),
            esc(&l.query),
            self.lib_state().typing(),
            l.sel.map_or("null".into(), |s| s.to_string()),
            l.scroll
        );
        j += &match l.detail {
            Some(Detail::Album(id)) => format!("\"detail\":{{\"kind\":\"album\",\"id\":{id}}},"),
            Some(Detail::Artist(id)) => format!("\"detail\":{{\"kind\":\"artist\",\"id\":{id}}},"),
            Some(Detail::Playlist(id)) => format!("\"detail\":{{\"kind\":\"playlist\",\"id\":{id}}},"),
            None => "\"detail\":null,".into(),
        };
        j += &format!(
            "\"tracks\":{},\"albums\":{},\"artists\":{},\"playlists\":{},\"queue\":{},\"viz_on\":{},\"viz_info\":{},",
            lib.track_count(),
            lib.albums().len(),
            lib.artists().len(),
            lib.playlists().len(),
            model.playlist.len(),
            l.viz_on,
            l.viz_info
        );
        let roots: Vec<String> = lib
            .roots()
            .iter()
            .map(|r| {
                format!("{{\"id\":{},\"name\":{},\"connected\":{}}}", esc(&r.id), esc(&r.name), r.connected)
            })
            .collect();
        j += &format!("\"roots\":[{}],", roots.join(","));
        let pls: Vec<String> = lib
            .playlists()
            .iter()
            .map(|p| {
                format!(
                    "{{\"id\":{},\"name\":{},\"tracks\":{},\"missing\":{}}}",
                    p.id,
                    esc(&p.name),
                    p.entries.len(),
                    p.missing()
                )
            })
            .collect();
        j += &format!("\"playlist_list\":[{}],", pls.join(","));
        j += &match ctx.scan {
            Some(s) => {
                format!("\"scan\":{{\"root\":{},\"done\":{},\"total\":{}}},", esc(&s.root), s.done, s.total)
            }
            None => "\"scan\":null,".into(),
        };
        if let Some(g) = &l.last_geom {
            let nav: Vec<String> =
                g.nav.iter().map(|(v, r)| format!("\"{}\":{}", view_name(*v), rect(*r))).collect();
            j += &format!("\"rail\":{{{}}},", nav.join(","));
            j += &format!(
                "\"mode_switch\":{{\"library\":{},\"player\":{}}},",
                rect(g.mode[0]),
                rect(g.mode[1])
            );
            j += &format!("\"add_folder\":{},\"search\":{},", rect(g.add_folder), rect(g.search));
            let bar: Vec<String> =
                g.bar_btns.iter().map(|(b, r)| format!("\"{b:?}\":{}", rect(*r))).collect();
            j += &format!("\"bar\":{{{}}},\"seek\":{},", bar.join(","), rect(g.seek_hit));
            let hb: Vec<String> = g
                .header_btns
                .iter()
                .map(|b| format!("{{\"id\":{},\"label\":{},\"rect\":{}}}", b.id, esc(&b.label), rect(b.rect)))
                .collect();
            j += &format!("\"header_buttons\":[{}],", hb.join(","));
            if let Some(s) = g.sort {
                j += &format!("\"sort\":{},", rect(s));
            }
            let vz: Vec<String> =
                g.viz_btns.iter().map(|(i, r)| format!("{{\"id\":{i},\"rect\":{}}}", rect(*r))).collect();
            j += &format!("\"viz_buttons\":[{}],", vz.join(","));
            j += &format!("\"body\":{},", rect(g.m.body));
        }
        let hero: Vec<String> = l
            .hero
            .iter()
            .map(|(id, r, label)| format!("{{\"id\":{id},\"label\":{},\"rect\":{}}}", esc(label), rect(*r)))
            .collect();
        j += &format!("\"hero_buttons\":[{}],", hero.join(","));
        let ents: Vec<String> = l
            .visible
            .iter()
            .filter_map(|(i, r)| {
                let e = l.rows.as_ref()?.ents.get(*i)?;
                let (kind, label, id) = match e.kind {
                    EntKind::Album(ai) => {
                        let a = lib.albums().get(ai)?;
                        ("album", a.title.clone(), a.id as i64)
                    }
                    EntKind::Artist(ai) => {
                        let a = lib.artists().get(ai)?;
                        ("artist", a.name.clone(), a.id as i64)
                    }
                    EntKind::Track { id, .. } => ("track", lib.track(id)?.display_title().into(), id as i64),
                    EntKind::Queue(id) => {
                        ("queue", model.playlist.iter().find(|q| q.id == id)?.label.clone(), id as i64)
                    }
                    EntKind::Playlist(id) => ("playlist", lib.playlist(id)?.name.clone(), id as i64),
                    EntKind::PlEntry { pl, idx } => {
                        let e = lib.playlist(pl)?.entries.get(idx)?;
                        ("plentry", e.title.clone().unwrap_or_else(|| e.path.clone()), idx as i64)
                    }
                };
                Some(format!(
                    "{{\"i\":{i},\"kind\":\"{kind}\",\"id\":{id},\"label\":{},\"rect\":{}}}",
                    esc(&label),
                    rect(*r)
                ))
            })
            .collect();
        j += &format!("\"ents\":[{}],", ents.join(","));
        j += &format!("\"ent_count\":{},", l.rows.as_ref().map_or(0, |r| r.ents.len()));
        j += &format!("\"menu_open\":{}}}", self.menu_open());
        j
    }
}
