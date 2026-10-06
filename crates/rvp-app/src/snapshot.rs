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
        j += &format!("\"toast\":{},", ui.toast_text().map_or("null".to_string(), esc));
        j += &format!("\"menu_open\":{},\"has_video\":{},", ui.menu_open(), m.has_video);
        let tracks = |v: &[rvp_ui::TrackItem]| -> String {
            let items: alloc::vec::Vec<String> =
                v.iter().map(|t| format!("{{\"id\":{},\"label\":{}}}", t.id, esc(&t.label))).collect();
            format!("[{}]", items.join(","))
        };
        let pl: alloc::vec::Vec<String> = m
            .playlist
            .iter()
            .map(|e| format!("{{\"id\":{},\"label\":{},\"current\":{}}}", e.id, esc(&e.label), e.current))
            .collect();
        let ch: alloc::vec::Vec<String> = m
            .chapters
            .iter()
            .map(|c| format!("{{\"start_us\":{},\"title\":{}}}", c.start_us, esc(&c.title)))
            .collect();
        j += &format!("\"chapters\":[{}],", ch.join(","));
        j += &format!(
            "\"playlist\":[{}],\"repeat\":{},\"shuffle\":{},\"loop_a\":{},\"loop_b\":{},",
            pl.join(","),
            m.repeat,
            m.shuffle,
            m.loop_a.map_or("null".to_string(), |v| v.to_string()),
            m.loop_b.map_or("null".to_string(), |v| v.to_string()),
        );
        // The accessible state of the two toggles, the same text on every face, tooltip and menu.
        j += &format!(
            "\"transport\":{{\"shuffle\":{{\"pressed\":{},\"label\":{}}},\"repeat\":{{\"state\":\"{}\",\"label\":{}}}}},",
            m.shuffle,
            esc(m.shuffle_label()),
            ["off", "all", "one"][m.repeat.min(2) as usize],
            esc(m.repeat_label()),
        );
        j += &format!(
            "\"audio_tracks\":{},\"selected_audio\":{},\"subtitle_tracks\":{},\"selected_subtitle\":{},\"subtitle\":{},",
            tracks(&m.audio_tracks),
            m.selected_audio.map_or("null".to_string(), |i| i.to_string()),
            tracks(&m.subtitle_tracks),
            m.selected_subtitle.map_or("null".to_string(), |i| i.to_string()),
            m.subtitle.as_deref().map_or("null".to_string(), esc),
        );
        let cues: alloc::vec::Vec<String> = m
            .subtitle_cues
            .iter()
            .map(|c| {
                format!(
                    "{{\"text\":{},\"styled\":{},\"image\":{}}}",
                    esc(&c.text),
                    c.rich.is_some(),
                    c.image.is_some()
                )
            })
            .collect();
        j += &format!("\"subtitle_cues\":[{}],", cues.join(","));
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
        let ctx = Self::lib_ctx(&self.lib, None);
        j += &format!("\"lib\":{},", ui.lib_snapshot_json(&ctx, m));
        j += &format!(
            "\"now\":{{\"title\":{},\"artist\":{},\"album\":{},\"has_art\":{},\"track\":{}}},",
            esc(&m.title),
            esc(&m.artist),
            esc(&m.album),
            self.lib.now.art.is_some(),
            m.now_track.map_or("null".to_string(), |t| t.to_string()),
        );
        let a = self.audio_settings();
        j += &format!(
            "\"audio\":{{\"crossfade\":{},\"crossfade_secs\":{},\"auto_level\":{},\"target_lufs\":{},\"level_mode\":\"{}\",\"gain_db\":{},\"crossfading\":{},\"library_measured\":{},\"library_unmeasured\":{}}},",
            a.crossfade,
            a.crossfade_secs,
            a.auto_level,
            a.target_lufs,
            if a.level_mode == rvp_core::LevelMode::Album { "album" } else { "track" },
            m.level_gain_db.map_or("null".to_string(), |g| format!("{g:.2}")),
            self.session().is_some_and(|s| s.crossfading()),
            self.library().all_tracks().iter().filter(|t| t.loudness.lufs.is_some()).count(),
            self.library().pending_loudness().len(),
        );
        j += &match ui.audio_settings_open() {
            true => {
                let g = ui.audio_panel_geom();
                let controls: alloc::vec::Vec<String> =
                    g.controls.iter().map(|(c, hit, _)| format!("\"{}\":{}", c.name(), rect(*hit))).collect();
                format!(
                    "\"audio_panel\":{{\"card\":{},\"close\":{},\"focus\":{},\"controls\":{{{}}}}},",
                    rect(g.card),
                    rect(g.close),
                    ui.audio_panel_focus().map_or("null".to_string(), |c| format!("\"{}\"", c.name())),
                    controls.join(",")
                )
            }
            false => "\"audio_panel\":null,".to_string(),
        };
        j += &format!(
            "\"app\":{{\"updates\":{},\"integration\":{}}},",
            m.app.updates,
            m.app.integration.map_or("null".to_string(), |b| b.to_string())
        );
        j += &match &m.dialog {
            Some(d) => {
                let join = |v: alloc::vec::Vec<String>| v.join(",");
                format!(
                    "\"dialog\":{{\"title\":{},\"body\":[{}],\"progress\":{},\"toggles\":[{}],\"buttons\":[{}]}},",
                    esc(&d.title),
                    join(d.body.iter().map(|b| esc(b)).collect()),
                    d.progress.map_or("null".to_string(), |p| p.to_string()),
                    join(
                        d.toggles
                            .iter()
                            .map(|t| format!("{{\"label\":{},\"on\":{}}}", esc(&t.label), t.on))
                            .collect()
                    ),
                    join(
                        d.buttons
                            .iter()
                            .map(|b| format!(
                                "{{\"label\":{},\"primary\":{},\"enabled\":{}}}",
                                esc(&b.label),
                                b.primary,
                                b.enabled
                            ))
                            .collect()
                    ),
                )
            }
            None => "\"dialog\":null,".to_string(),
        };
        let v = self.viz();
        j += &format!(
            "\"viz\":{{\"effect\":{},\"palette\":{},\"frames\":{}}},",
            esc(v.effect.name()),
            esc(v.palette.name()),
            v.frames()
        );
        j += &format!("\"frames_drawn\":{}}}", self.frames_drawn());
        Snapshot { json: j }
    }
}
