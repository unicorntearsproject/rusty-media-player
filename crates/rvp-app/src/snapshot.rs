//! A JSON snapshot of the app for tests and tooling (the browser exposes it as `window.rvp.snapshot()`).
use crate::App;
use alloc::format;
use alloc::string::{String, ToString};
use rvp_ui::{MediaState, RectF};

/// Plain data describing the app right now.
#[derive(Debug, Clone)]
pub struct Snapshot {
    json: String,
}

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

impl Snapshot {
    /// The snapshot as a JSON object.
    pub fn json(&self) -> &str {
        &self.json
    }
}

impl App {
    /// Describe the app: playback state, geometry of the controls, presentation counters.
    pub fn snapshot(&self) -> Snapshot {
        let m = &self.model;
        let ui = self.ui();
        let l = ui.layout(m);
        let state = match m.state {
            MediaState::Idle => "idle",
            MediaState::Opening => "opening",
            MediaState::Paused => "paused",
            MediaState::Buffering => "buffering",
            MediaState::Playing => "playing",
            MediaState::Ended => "ended",
            MediaState::Failed => "failed",
        };
        let mut j = String::from("{");
        j += &format!("\"state\":\"{state}\",\"position_us\":{},", m.position_us);
        j += &match m.duration_us {
            Some(d) => format!("\"duration_us\":{d},"),
            None => "\"duration_us\":null,".to_string(),
        };
        j += &format!(
            "\"volume\":{:.3},\"muted\":{},\"rate\":{},\"fullscreen\":{},\"title\":{},",
            m.volume,
            m.muted,
            m.rate,
            m.fullscreen,
            esc(&m.title)
        );
        j += &format!(
            "\"controls_visible\":{},\"controls_opacity\":{:.2},",
            ui.controls_visible(),
            ui.controls_opacity()
        );
        j += &format!("\"menu_open\":{},\"has_video\":{},", ui.menu_open(), m.has_video);
        j += &match &m.error {
            Some(e) => format!("\"error\":{},", esc(e)),
            None => "\"error\":null,".to_string(),
        };
        let (w, h) = ui.size();
        j += &format!("\"surface\":{{\"w\":{w},\"h\":{h},\"dpr\":{:.2}}},", ui.scale());
        j += &format!("\"seek\":{},", rect(l.seek_track));
        j += &format!("\"seek_hit\":{},", rect(l.seek_hit));
        if let Some(v) = l.vol_hit {
            j += &format!("\"volume_slider\":{},", rect(v));
        }
        j += "\"buttons\":{";
        for (i, (b, r)) in l.buttons.iter().enumerate() {
            if i > 0 {
                j += ",";
            }
            j += &format!("\"{b:?}\":{}", rect(*r));
        }
        j += "},\"menu\":[";
        for (i, (label, r, enabled)) in ui.menu_rows().iter().enumerate() {
            if i > 0 {
                j += ",";
            }
            j += &format!("{{\"label\":{},\"enabled\":{enabled},\"rect\":{}}}", esc(label), rect(*r));
        }
        j += "],";
        if let Some(s) = self.session() {
            // The picture's own size wins over the container's: it can change mid-stream.
            let size = if self.drawn_size.0 > 0 {
                Some(self.drawn_size)
            } else {
                s.streams()
                    .iter()
                    .find(|i| i.kind == rvp_core::StreamKind::Video)
                    .and_then(|i| i.video)
                    .map(|f| (f.width, f.height))
            };
            if let Some((w, h)) = size {
                let v = ui.video_rect(w, h);
                j += &format!("\"video_rect\":{},\"frame_size\":[{w},{h}],", rect(v));
            }
            let st = s.video_stats();
            j += &format!(
                "\"video\":{{\"presented\":{},\"dropped\":{},\"max_drift_us\":{}}},",
                st.presented, st.dropped, st.max_drift_us
            );
            j += "\"warnings\":[";
            for (i, w) in s.warnings().iter().enumerate() {
                if i > 0 {
                    j += ",";
                }
                j += &esc(w);
            }
            j += "],";
        }
        let p = self.perf();
        j += &format!(
            "\"perf\":{{\"ticks\":{},\"session_ms\":{:.1},\"render_ms\":{:.1},\"base_ms\":{:.1},\"overlay_ms\":{:.1},\"present_ms\":{:.1},\"max_tick_ms\":{:.1}}},",
            p.ticks,
            p.session_us as f64 / 1000.0,
            p.render_us as f64 / 1000.0,
            p.base_us as f64 / 1000.0,
            p.overlay_us as f64 / 1000.0,
            p.present_us as f64 / 1000.0,
            p.max_tick_us as f64 / 1000.0
        );
        j += &format!("\"frames_drawn\":{}}}", self.frames_drawn());
        Snapshot { json: j }
    }
}
