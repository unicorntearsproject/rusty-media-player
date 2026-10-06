//! Drawing the player chrome with the Unicorn Tears tokens. Glow is reserved for focus, hover on the
//! primary actions and the seek handle (u Studio's rule: no glowing static chrome).
use crate::font::Face;
use crate::gfx::{FrameBuffer, Paint, RectF, fade};
use crate::icon::Icon;
use crate::model::{MediaState, UiModel, format_time};
use crate::tk as t;
use crate::ui::{Btn, Layout, Panel, Target, Ui};
use alloc::string::String;
use alloc::vec::Vec;
use theme::Rgba;

const CLEAR: Rgba = Rgba::new(7, 6, 13, 0);

impl Ui {
    /// Draw the layer under the chrome: the night backdrop on the empty screen, or the letterboxed picture.
    /// `video` is the latest frame as `(rgba, width, height)`.
    pub fn draw_base(&mut self, fb: &mut FrameBuffer, model: &UiModel, video: Option<(&[u8], u32, u32)>) {
        match video {
            Some((rgba, w, h)) if model.has_media() => {
                fb.clear(t::bg_page());
                let r = self.video_rect(w, h);
                fb.blit_scaled(r, rgba, w, h);
            }
            // Nothing to show yet (empty screen, opening, audio only, error): the night sky.
            _ => fb.fill_radial(&t::gradient_night()),
        }
    }

    /// Draw the chrome over the base layer.
    pub fn draw_overlay(&mut self, fb: &mut FrameBuffer, model: &UiModel) {
        let l = self.layout(model);
        if !model.has_media() {
            self.draw_welcome(fb, &l);
        } else {
            if !model.has_video
                && matches!(
                    model.state,
                    MediaState::Paused | MediaState::Playing | MediaState::Buffering | MediaState::Ended
                )
            {
                self.draw_audio_hero(fb, &l, model);
            }
            match model.state {
                MediaState::Failed => self.draw_error(fb, &l, model),
                MediaState::Paused | MediaState::Ended if model.has_video => self.draw_big_play(fb, &l),
                MediaState::Buffering | MediaState::Opening => self.draw_spinner(fb, &l),
                _ => {}
            }
            if (model.subtitle.is_some() || Self::has_styled_cues(model)) && model.state != MediaState::Failed
            {
                self.draw_subtitle(fb, &l, model);
            }
            if self.controls_alpha > 0.005 && model.state != MediaState::Failed {
                self.draw_top(fb, &l, model);
                self.draw_bar(fb, &l, model);
            }
        }
        self.draw_toast(fb, &l);
        self.draw_tooltip(fb, &l, model);
        self.draw_audio_panel(fb, model);
        self.draw_app_dialog(fb, model);
        if self.drag_over {
            self.draw_drop_outline(fb, &l);
        }
        let panels = core::mem::take(&mut self.menu);
        for p in &panels {
            self.draw_panel(fb, &l, p);
        }
        self.menu = panels;
    }

    // ---- small helpers ---------------------------------------------------------------------------------

    pub(crate) fn text(
        &mut self,
        fb: &mut FrameBuffer,
        face: Face,
        size: f32,
        x: f32,
        cy: f32,
        text: &str,
        c: Rgba,
        a: f32,
        tracking: f32,
    ) -> f32 {
        let s = self.scale;
        self.fonts.draw(fb, face, size * s, x, cy + size * s * 0.36, text, c, a, tracking * s)
    }

    pub(crate) fn text_w(&mut self, face: Face, size: f32, text: &str, tracking: f32) -> f32 {
        let s = self.scale;
        self.fonts.measure(face, size * s, text, tracking * s)
    }

    pub(crate) fn icon(
        &mut self,
        fb: &mut FrameBuffer,
        icon: Icon,
        cx: f32,
        cy: f32,
        size: f32,
        c: Rgba,
        a: f32,
        filled: bool,
    ) {
        let s = self.scale;
        self.icons.draw(fb, icon, cx, cy, size * s, c, a, filled);
    }

    pub(crate) fn focus_ring(&mut self, fb: &mut FrameBuffer, r: RectF, radius: f32, a: f32) {
        let s = self.scale;
        fb.glow_rrect(r.inflate(1.0 * s), radius, 12.0 * s, t::cyan_500(), 0.35 * a);
        fb.stroke_rrect(r.inflate(2.0 * s), radius + 2.0 * s, 2.0 * s, t::focus_ring(), a);
    }

    // ---- empty screen ----------------------------------------------------------------------------------

    fn draw_welcome(&mut self, fb: &mut FrameBuffer, l: &Layout) {
        let s = l.s;
        let card = l.card;
        let over = self.drag_over;
        fb.shadow_rrect(card, 28.0 * s, 24.0 * s, 64.0 * s, Rgba::new(5, 2, 15, 166), 1.0);
        fb.fill_rrect(card, 28.0 * s, Paint::Solid(t::ink_800()), 1.0);
        if over {
            fb.glow_rrect(card, 28.0 * s, 28.0 * s, t::magenta_500(), 0.45);
            fb.stroke_rrect(card, 28.0 * s, 2.0 * s, t::magenta_500(), 1.0);
        } else {
            fb.stroke_rrect(card, 28.0 * s, 1.0 * s, t::border_subtle(), 1.0);
        }
        // The tears gradient rule across the top edge (GlowDivider).
        let rule = RectF::new(card.x + 48.0 * s, card.y + 1.0 * s, card.w - 96.0 * s, 2.0 * s);
        fb.fill_rrect(rule, 1.0 * s, Paint::Gradient(t::gradient_tears()), 0.9);

        let cx = card.cx();
        let mut y = card.y + 36.0 * s;
        let kicker = "RUSTY WAVE";
        let kw = self.text_w(Face::SansBold, 11.0, kicker, 3.0);
        self.text(fb, Face::SansBold, 11.0, cx - kw * 0.5, y, kicker, t::violet_400(), 1.0, 3.0);
        y += 54.0 * s;
        // The logo.
        crate::logo::draw(fb, RectF::new(cx - 44.0 * s, y - 44.0 * s, 88.0 * s, 88.0 * s), 1.0);
        y += 60.0 * s;
        let head = if over { "Let go to play" } else { "Drop a video here" };
        let hw = self.text_w(Face::SansBold, 26.0, head, -0.3);
        self.text(fb, Face::SansBold, 26.0, cx - hw * 0.5, y, head, t::text_strong(), 1.0, -0.3);
        y += 30.0 * s;
        let sub = "MP4, MKV and WebM. Everything plays right here on your device.";
        let sub = self.fonts.fit(Face::Sans, 14.0 * s, sub, card.w - 48.0 * s);
        let sw = self.text_w(Face::Sans, 14.0, &sub, 0.0);
        self.text(fb, Face::Sans, 14.0, cx - sw * 0.5, y, &sub, t::text_muted(), 1.0, 0.0);

        // Primary button.
        if let Some(r) = l.rect_of(Btn::Welcome) {
            let hot = self.hover == Target::Btn(Btn::Welcome);
            let pressed = hot && self.pressed == Some(Target::Btn(Btn::Welcome));
            let rr = if pressed { r.scaled(0.97) } else { r };
            if hot || self.focus == Some(Btn::Welcome) {
                fb.glow_rrect(rr, rr.h * 0.5, 24.0 * s, t::magenta_500(), 0.55);
            }
            fb.fill_rrect(rr, rr.h * 0.5, Paint::Gradient(t::gradient_tears()), 1.0);
            if hot {
                fb.fill_rrect(rr, rr.h * 0.5, Paint::Solid(t::white()), 0.12);
            }
            self.icon(fb, Icon::FolderOpen, rr.x + 30.0 * s, rr.cy(), 18.0, t::white(), 1.0, false);
            self.text(fb, Face::SansBold, 14.0, rr.x + 48.0 * s, rr.cy(), "OPEN FILE", t::white(), 1.0, 1.1);
            if self.focus == Some(Btn::Welcome) {
                self.focus_ring(fb, rr, rr.h * 0.5, 1.0);
            }
        }
        // Hints.
        let hint_y = card.bottom() - 36.0 * s;
        let hints = "Space play   F fullscreen   M mute   O open";
        let hw = self.text_w(Face::Mono, 11.5, hints, 0.0);
        self.text(fb, Face::Mono, 11.5, cx - hw * 0.5, hint_y, hints, t::text_dim(), 1.0, 0.0);
    }

    fn draw_audio_hero(&mut self, fb: &mut FrameBuffer, l: &Layout, model: &UiModel) {
        let s = l.s;
        let (cx, cy) = (l.w * 0.5, l.h * 0.5 - 20.0 * s);
        let ring = RectF::new(cx - 56.0 * s, cy - 56.0 * s, 112.0 * s, 112.0 * s);
        fb.fill_rrect(ring, 56.0 * s, Paint::Solid(fade(t::ink_700(), 0.9)), 1.0);
        fb.stroke_rrect(ring, 56.0 * s, 1.5 * s, fade(t::violet_500(), 0.7), 1.0);
        self.icon(fb, Icon::AudioLines, cx, cy, 48.0, t::violet_400(), 1.0, false);
        let name = self.fonts.fit(Face::SansMedium, 18.0 * s, &model.title, l.w - 80.0 * s);
        let w = self.text_w(Face::SansMedium, 18.0, &name, 0.0);
        self.text(fb, Face::SansMedium, 18.0, cx - w * 0.5, cy + 86.0 * s, &name, t::text_body(), 1.0, 0.0);
    }

    fn draw_error(&mut self, fb: &mut FrameBuffer, l: &Layout, model: &UiModel) {
        let s = l.s;
        let card = RectF::new(l.w * 0.5 - 220.0 * s, l.h * 0.5 - 90.0 * s, 440.0 * s, 180.0 * s);
        fb.shadow_rrect(card, 20.0 * s, 16.0 * s, 48.0 * s, Rgba::new(5, 2, 15, 166), 1.0);
        fb.fill_rrect(card, 20.0 * s, Paint::Solid(fade(t::ink_800(), 0.96)), 1.0);
        fb.stroke_rrect(card, 20.0 * s, 1.0 * s, fade(t::danger(), 0.6), 1.0);
        self.icon(fb, Icon::CircleAlert, card.x + 44.0 * s, card.y + 48.0 * s, 28.0, t::danger(), 1.0, false);
        self.text(
            fb,
            Face::SansBold,
            20.0,
            card.x + 72.0 * s,
            card.y + 48.0 * s,
            "Can't play this one.",
            t::text_strong(),
            1.0,
            -0.2,
        );
        let msg = model.error.clone().unwrap_or_else(|| "Something went sideways.".into());
        let lines = self.wrap(Face::Sans, 14.0, &msg, card.w - 48.0 * s, 3);
        for (i, ln) in lines.iter().enumerate() {
            self.text(
                fb,
                Face::Sans,
                14.0,
                card.x + 24.0 * s,
                card.y + 88.0 * s + i as f32 * 22.0 * s,
                ln,
                t::text_muted(),
                1.0,
                0.0,
            );
        }
        let hint = "Press O to pick another file.";
        self.text(
            fb,
            Face::Mono,
            11.5,
            card.x + 24.0 * s,
            card.bottom() - 24.0 * s,
            hint,
            t::text_dim(),
            1.0,
            0.0,
        );
    }

    /// Greedy word wrap into at most `max_lines` lines (the last is ellipsised if text remains).
    pub(crate) fn wrap(
        &mut self,
        face: Face,
        size: f32,
        text: &str,
        max_w: f32,
        max_lines: usize,
    ) -> Vec<String> {
        let s = self.scale;
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        for word in text.split_whitespace() {
            let trial = if cur.is_empty() { word.into() } else { alloc::format!("{cur} {word}") };
            if self.fonts.measure(face, size * s, &trial, 0.0) <= max_w || cur.is_empty() {
                cur = trial;
            } else {
                lines.push(core::mem::take(&mut cur));
                cur = word.into();
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        if lines.len() > max_lines {
            lines.truncate(max_lines);
            if let Some(last) = lines.last_mut() {
                last.push('\u{2026}');
            }
        }
        for ln in &mut lines {
            *ln = self.fonts.fit(face, size * s, ln, max_w);
        }
        lines
    }

    // ---- over the picture ------------------------------------------------------------------------------

    fn draw_big_play(&mut self, fb: &mut FrameBuffer, l: &Layout) {
        let s = l.s;
        let r = RectF::new(l.w * 0.5 - 42.0 * s, l.h * 0.5 - 42.0 * s - 12.0 * s, 84.0 * s, 84.0 * s);
        fb.fill_rrect(r, 42.0 * s, Paint::Solid(fade(t::ink_900(), 0.58)), 1.0);
        fb.stroke_rrect(r, 42.0 * s, 1.5 * s, fade(t::white(), 0.3), 1.0);
        self.icon(fb, Icon::Play, r.cx() + 2.0 * s, r.cy(), 34.0, t::white(), 0.95, true);
    }

    fn draw_spinner(&mut self, fb: &mut FrameBuffer, l: &Layout) {
        let s = l.s;
        let (cx, cy) = (l.w * 0.5, l.h * 0.5 - 12.0 * s);
        let head = if self.config.reduce_motion { 0 } else { (self.now / 90_000) as usize % 8 };
        for i in 0..8usize {
            let ang = i as f32 * core::f32::consts::FRAC_PI_4;
            let (px, py) = (cx + libm::sinf(ang) * 20.0 * s, cy - libm::cosf(ang) * 20.0 * s);
            let age = (i + 8 - head) % 8; // 0 = brightest
            let a = if self.config.reduce_motion { 0.7 } else { 1.0 - age as f32 * 0.11 };
            let d = RectF::new(px - 3.0 * s, py - 3.0 * s, 6.0 * s, 6.0 * s);
            fb.fill_rrect(d, 3.0 * s, Paint::Solid(t::cyan_500()), a.max(0.15));
        }
    }

    fn draw_top(&mut self, fb: &mut FrameBuffer, l: &Layout, model: &UiModel) {
        let s = l.s;
        let a = self.controls_alpha;
        fb.fill_rect_paint(
            RectF::new(0.0, 0.0, l.w, 96.0 * s),
            Paint::Vertical(fade(t::ink_900(), 0.8), CLEAR),
            a,
        );
        let name = self.fonts.fit(Face::SansMedium, 15.0 * s, &model.title, l.w - 48.0 * s);
        self.text(fb, Face::SansMedium, 15.0, 24.0 * s, 32.0 * s, &name, t::text_body(), a, 0.0);
    }

    fn draw_bar(&mut self, fb: &mut FrameBuffer, l: &Layout, model: &UiModel) {
        let s = l.s;
        let a = self.controls_alpha;
        fb.fill_rect_paint(
            RectF::new(0.0, l.bar_top, l.w, l.h - l.bar_top),
            Paint::Vertical(CLEAR, fade(t::ink_900(), 0.92)),
            a,
        );

        // Seek bar.
        let live = self.scrub.is_some();
        let hot = self.hover == Target::Seek || live;
        let tr = l.seek_track;
        let th = if hot { 6.0 * s } else { 4.0 * s };
        let track = RectF::new(tr.x, tr.cy() - th * 0.5, tr.w, th);
        fb.fill_rrect(track, th * 0.5, Paint::Solid(fade(t::white(), 0.2)), a);
        let frac = match (self.scrub, model.duration_us) {
            (Some(f), _) => f,
            (None, Some(d)) if d > 0 => (model.position_us as f32 / d as f32).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let px = track.x + track.w * frac;
        if frac > 0.0 {
            let played = RectF::new(track.x, track.y, (px - track.x).max(th), th);
            fb.fill_rrect(played, th * 0.5, Paint::Horizontal(t::magenta_500(), t::violet_400()), a);
        }
        // Chapter marks: small gaps in the track.
        if let Some(d) = model.duration_us.filter(|d| *d > 0) {
            for c in model.chapters.iter().filter(|c| c.start_us > 0) {
                let x = track.x + track.w * (c.start_us as f32 / d as f32).clamp(0.0, 1.0);
                fb.fill_rect_paint(
                    RectF::new(x - 1.0 * s, track.y, 2.0 * s, th),
                    Paint::Solid(t::ink_900()),
                    a,
                );
            }
        }
        // The A-B loop: a cyan band between the marks, and a tick at each mark.
        if let Some(d) = model.duration_us.filter(|d| *d > 0) {
            let at = |us: i64| track.x + track.w * (us as f32 / d as f32).clamp(0.0, 1.0);
            match (model.loop_a, model.loop_b) {
                (Some(la), Some(lb)) => {
                    let (xa, xb) = (at(la), at(lb));
                    fb.fill_rect_paint(
                        RectF::new(xa, track.y, (xb - xa).max(1.0), th),
                        Paint::Solid(fade(t::cyan_500(), 0.55)),
                        a,
                    );
                    for x in [xa, xb] {
                        fb.fill_rrect(
                            RectF::new(x - 1.0 * s, tr.cy() - 9.0 * s, 2.0 * s, 18.0 * s),
                            1.0 * s,
                            Paint::Solid(t::cyan_500()),
                            a,
                        );
                    }
                }
                (Some(la), None) => {
                    let x = at(la);
                    fb.fill_rrect(
                        RectF::new(x - 1.0 * s, tr.cy() - 9.0 * s, 2.0 * s, 18.0 * s),
                        1.0 * s,
                        Paint::Solid(t::cyan_500()),
                        a,
                    );
                }
                _ => {}
            }
        }
        if hot {
            // The handle: a cyan knob with the only glow on the bar.
            let k = RectF::new(px - 7.0 * s, tr.cy() - 7.0 * s, 14.0 * s, 14.0 * s);
            fb.glow_rrect(k, 7.0 * s, 14.0 * s, t::cyan_500(), 0.55 * a);
            fb.fill_rrect(k, 7.0 * s, Paint::Solid(t::cyan_500()), a);
        } else {
            // The playhead line (u Studio): a thin cyan tick.
            let k = RectF::new(px - 1.0 * s, tr.cy() - 7.0 * s, 2.0 * s, 14.0 * s);
            fb.fill_rrect(k, 1.0 * s, Paint::Solid(t::cyan_500()), a);
        }
        // Hover time.
        if let (Some(f), Some(d)) = (self.scrub.or(self.hover_seek), model.duration_us) {
            if hot {
                let hx = track.x + track.w * f;
                let at = (f as f64 * d as f64) as i64;
                let mut label = format_time(at);
                if let Some(c) = model.chapters.iter().rev().find(|c| c.start_us <= at) {
                    if !c.title.is_empty() {
                        label = alloc::format!("{label}  {}", c.title);
                    }
                }
                let tw = self.text_w(Face::MonoBold, 12.0, &label, 0.0);
                let pill = RectF::new(
                    (hx - tw * 0.5 - 10.0 * s).clamp(8.0 * s, l.w - tw - 28.0 * s),
                    tr.cy() - 44.0 * s,
                    tw + 20.0 * s,
                    26.0 * s,
                );
                fb.shadow_rrect(pill, 8.0 * s, 4.0 * s, 12.0 * s, Rgba::new(5, 2, 15, 140), a);
                fb.fill_rrect(pill, 8.0 * s, Paint::Solid(t::ink_700()), a);
                fb.stroke_rrect(pill, 8.0 * s, 1.0 * s, t::ink_500(), a);
                self.text(
                    fb,
                    Face::MonoBold,
                    12.0,
                    pill.x + 10.0 * s,
                    pill.cy(),
                    &label,
                    t::text_strong(),
                    a,
                    0.0,
                );
            }
        }

        // Buttons.
        let buttons = l.buttons.clone();
        for (b, r) in buttons {
            self.draw_button(fb, b, r, model, a);
        }

        // Volume slider.
        if let Some(vh) = l.vol_hit {
            let vt = l.vol_track;
            let vol = if model.muted { 0.0 } else { model.volume };
            let dragging = self.dragging();
            let hot = self.hover == Target::Volume || dragging;
            fb.fill_rrect(vt, vt.h * 0.5, Paint::Solid(fade(t::white(), 0.22)), a);
            if vol > 0.0 {
                fb.fill_rrect(
                    RectF::new(vt.x, vt.y, (vt.w * vol).max(vt.h), vt.h),
                    vt.h * 0.5,
                    Paint::Solid(fade(t::pink_white(), 0.92)),
                    a,
                );
            }
            let kx = vt.x + vt.w * vol;
            let kr = if hot { 6.5 * s } else { 5.0 * s };
            fb.fill_rrect(
                RectF::new(kx - kr, vh.cy() - kr, kr * 2.0, kr * 2.0),
                kr,
                Paint::Solid(t::text_strong()),
                a,
            );
        }

        // Time readout.
        let cur = format_time(if let (Some(f), Some(d)) = (self.scrub, model.duration_us) {
            (f as f64 * d as f64) as i64
        } else {
            model.position_us
        });
        let mut x = l.time_x;
        x = self.text(fb, Face::Mono, 13.0, x, l.row_cy, &cur, t::cyan_500(), a, 0.0);
        if let Some(d) = model.duration_us {
            x = self.text(fb, Face::Mono, 13.0, x, l.row_cy, " / ", t::text_dim(), a, 0.0);
            self.text(fb, Face::Mono, 13.0, x, l.row_cy, &format_time(d), t::text_muted(), a, 0.0);
        }
    }

    fn draw_button(&mut self, fb: &mut FrameBuffer, b: Btn, r: RectF, model: &UiModel, a: f32) {
        let s = self.scale;
        let hot = self.hover == Target::Btn(b);
        let pressed = hot && self.pressed == Some(Target::Btn(b));
        let focused = self.focus == Some(b);
        let rr = if pressed { r.scaled(0.94) } else { r };
        match b {
            Btn::Play => {
                if hot || focused {
                    fb.glow_rrect(rr, rr.h * 0.5, 22.0 * s, t::magenta_500(), 0.6 * a);
                }
                fb.fill_rrect(rr, rr.h * 0.5, Paint::Gradient(t::gradient_tears()), a);
                if hot {
                    fb.fill_rrect(rr, rr.h * 0.5, Paint::Solid(t::white()), 0.14 * a);
                }
                let icon = if model.state.is_active() { Icon::Pause } else { Icon::Play };
                let nudge = if icon == Icon::Play { 1.5 * s } else { 0.0 };
                self.icon(fb, icon, rr.cx() + nudge, rr.cy(), 20.0, t::white(), a, true);
                if focused {
                    self.focus_ring(fb, rr, rr.h * 0.5, a);
                }
            }
            Btn::Speed => {
                let off = (model.rate - 1.0).abs() > 0.001;
                if hot || pressed {
                    fb.fill_rrect(rr, rr.h * 0.5, Paint::Solid(fade(t::white(), 0.12)), a);
                }
                let border = if off { fade(t::cyan_500(), 0.7) } else { fade(t::white(), 0.24) };
                fb.stroke_rrect(rr, rr.h * 0.5, 1.0 * s, border, a);
                let label = crate::actions::speed_label(model.rate);
                let col = if off { t::cyan_500() } else { t::text_body() };
                let w = self.text_w(Face::MonoBold, 12.5, &label, 0.0);
                self.text(fb, Face::MonoBold, 12.5, rr.cx() - w * 0.5, rr.cy(), &label, col, a, 0.0);
                if focused {
                    self.focus_ring(fb, rr, rr.h * 0.5, a);
                }
            }
            _ => {
                if hot || pressed {
                    fb.fill_rrect(rr, 10.0 * s, Paint::Solid(fade(t::white(), 0.12)), a);
                }
                let icon = match b {
                    Btn::Back => Icon::Rewind,
                    Btn::Fwd => Icon::FastForward,
                    Btn::Mute => {
                        if model.muted || model.volume <= 0.0 {
                            Icon::VolumeX
                        } else if model.volume < 0.5 {
                            Icon::Volume1
                        } else {
                            Icon::Volume2
                        }
                    }
                    Btn::Tracks => Icon::Subtitles,
                    Btn::Playlist => Icon::List,
                    Btn::Open => Icon::FolderOpen,
                    Btn::ModeSwitch => Icon::Music,
                    Btn::Shuffle => Icon::Shuffle,
                    Btn::Repeat => Icon::for_repeat(model.repeat),
                    Btn::Fullscreen => {
                        if model.fullscreen {
                            Icon::Minimize
                        } else {
                            Icon::Maximize
                        }
                    }
                    _ => Icon::Play,
                };
                // Shuffle and repeat light up when they are on; repeat's icon also says which mode.
                let on = match b {
                    Btn::Shuffle => model.shuffle,
                    Btn::Repeat => model.repeat != 0,
                    _ => false,
                };
                let col = if on {
                    t::cyan_500()
                } else if hot {
                    t::white()
                } else {
                    t::text_body()
                };
                let tint =
                    if b == Btn::Mute && (model.muted || model.volume <= 0.0) { t::text_dim() } else { col };
                self.icon(fb, icon, rr.cx(), rr.cy(), 20.0, tint, a, false);
                if on {
                    fb.fill_rrect(
                        RectF::new(rr.cx() - 2.0 * s, rr.bottom() - 4.0 * s, 4.0 * s, 4.0 * s),
                        2.0 * s,
                        Paint::Solid(t::cyan_500()),
                        a,
                    );
                }
                if focused {
                    self.focus_ring(fb, rr, 10.0 * s, a);
                }
            }
        }
    }

    /// Subtitles: centred lines on dark pills, above the bar while it is showing and near the bottom otherwise.
    fn draw_subtitle(&mut self, fb: &mut FrameBuffer, l: &Layout, model: &UiModel) {
        if Self::has_styled_cues(model) {
            self.draw_styled_cues(fb, l, model);
            return;
        }
        let Some(text) = model.subtitle.as_deref() else { return };
        let s = l.s;
        let size = (l.h / s * 0.042).clamp(15.0, 34.0);
        let max_w = l.w * 0.86;
        // Word-wrap each line to the window.
        let mut lines: Vec<String> = Vec::new();
        for raw in text.split('\n') {
            let mut cur = String::new();
            for word in raw.split_whitespace() {
                let candidate =
                    if cur.is_empty() { String::from(word) } else { alloc::format!("{cur} {word}") };
                if cur.is_empty() || self.text_w(Face::SansMedium, size, &candidate, 0.0) <= max_w {
                    cur = candidate;
                } else {
                    lines.push(core::mem::take(&mut cur));
                    cur = String::from(word);
                }
            }
            if !cur.is_empty() {
                lines.push(cur);
            }
        }
        if lines.is_empty() {
            return;
        }
        let line_h = size * 1.5 * s;
        let bottom = if self.controls_alpha > 0.05 { l.bar_top + 40.0 * s } else { l.h - 36.0 * s };
        let bottom = bottom.min(l.h - 24.0 * s);
        let top = bottom - line_h * lines.len() as f32;
        for (i, line) in lines.iter().enumerate() {
            let tw = self.text_w(Face::SansMedium, size, line, 0.0);
            let cy = top + line_h * (i as f32 + 0.5);
            let r = RectF::new(
                l.w * 0.5 - tw * 0.5 - 12.0 * s,
                cy - line_h * 0.5 + 2.0 * s,
                tw + 24.0 * s,
                line_h - 4.0 * s,
            );
            fb.fill_rrect(r, 8.0 * s, Paint::Solid(fade(Rgba::new(7, 6, 13, 255), 0.78)), 1.0);
            self.text(fb, Face::SansMedium, size, r.x + 12.0 * s, cy, line, t::text_strong(), 1.0, 0.0);
        }
    }

    pub(crate) fn draw_toast(&mut self, fb: &mut FrameBuffer, l: &Layout) {
        let Some((text, until)) = self.toast.clone() else { return };
        let s = l.s;
        let left = until - self.now;
        let a = if self.config.reduce_motion { 1.0 } else { (left as f32 / 300_000.0).clamp(0.0, 1.0) };
        let tw = self.text_w(Face::SansMedium, 14.0, &text, 0.0);
        let y = if l.toast_y > 0.0 { l.toast_y } else { 64.0 * s };
        let r = RectF::new(l.w * 0.5 - tw * 0.5 - 18.0 * s, y, tw + 36.0 * s, 36.0 * s);
        fb.shadow_rrect(r, r.h * 0.5, 6.0 * s, 20.0 * s, Rgba::new(5, 2, 15, 160), a);
        fb.fill_rrect(r, r.h * 0.5, Paint::Solid(fade(t::ink_800(), 0.94)), a);
        fb.stroke_rrect(r, r.h * 0.5, 1.0 * s, t::ink_500(), a);
        self.text(fb, Face::SansMedium, 14.0, r.x + 18.0 * s, r.cy(), &text, t::text_strong(), a, 0.0);
    }

    pub(crate) fn tooltip_text(&self, target: Target, model: &UiModel) -> Option<(String, &'static str)> {
        let b = match target {
            Target::Btn(b) => b,
            Target::Volume => {
                return Some((
                    alloc::format!("Volume {}%", (model.volume * 100.0 + 0.5) as i32),
                    "Up / Down",
                ));
            }
            _ => return None,
        };
        let (label, key) = match b {
            Btn::Play => (if model.state.is_active() { "Pause" } else { "Play" }, "Space"),
            Btn::Back => ("Back 10 s", "J"),
            Btn::Fwd => ("Forward 10 s", "L"),
            Btn::Mute => (if model.muted { "Unmute" } else { "Mute" }, "M"),
            Btn::Speed => ("Playback speed", "[ ]"),
            Btn::Tracks => ("Audio and subtitles", "A / S"),
            Btn::Playlist => ("Playlist", "Q"),
            Btn::Open => ("Open file", "O"),
            Btn::Fullscreen => (if model.fullscreen { "Leave fullscreen" } else { "Fullscreen" }, "F"),
            Btn::ModeSwitch => ("Library", "B"),
            Btn::Shuffle => (model.shuffle_label(), "Z"),
            Btn::Repeat => (model.repeat_label(), "R"),
            Btn::Welcome | Btn::Prev | Btn::Next | Btn::QueueView | Btn::VizView | Btn::Favorite => {
                return None;
            }
        };
        Some((label.into(), key))
    }

    fn draw_tooltip(&mut self, fb: &mut FrameBuffer, l: &Layout, model: &UiModel) {
        if !self.tooltip_ready() || self.controls_alpha < 0.5 {
            return;
        }
        let Some((label, key)) = self.tooltip_text(self.hover, model) else { return };
        let anchor = match self.hover {
            Target::Btn(b) => match l.rect_of(b) {
                Some(r) => r,
                None => return,
            },
            Target::Volume => match l.vol_hit {
                Some(r) => r,
                None => return,
            },
            _ => return,
        };
        let s = l.s;
        let lw = self.text_w(Face::SansMedium, 12.5, &label, 0.0);
        let kw = self.text_w(Face::MonoBold, 11.0, key, 0.0);
        let w = lw + kw + 40.0 * s;
        let r = RectF::new(
            (anchor.cx() - w * 0.5).clamp(8.0 * s, l.w - w - 8.0 * s),
            anchor.y - 12.0 * s - 30.0 * s,
            w,
            30.0 * s,
        );
        fb.shadow_rrect(r, 8.0 * s, 4.0 * s, 12.0 * s, Rgba::new(5, 2, 15, 140), 1.0);
        fb.fill_rrect(r, 8.0 * s, Paint::Solid(t::ink_700()), 1.0);
        fb.stroke_rrect(r, 8.0 * s, 1.0 * s, t::ink_500(), 1.0);
        let x =
            self.text(fb, Face::SansMedium, 12.5, r.x + 12.0 * s, r.cy(), &label, t::text_body(), 1.0, 0.0);
        let chip = RectF::new(x + 10.0 * s, r.cy() - 9.0 * s, kw + 12.0 * s, 18.0 * s);
        fb.fill_rrect(chip, 5.0 * s, Paint::Solid(fade(t::cyan_500(), 0.14)), 1.0);
        self.text(fb, Face::MonoBold, 11.0, chip.x + 6.0 * s, chip.cy(), key, t::cyan_400(), 1.0, 0.0);
    }

    pub(crate) fn draw_drop_outline(&mut self, fb: &mut FrameBuffer, l: &Layout) {
        // On the empty screen the card itself lights up; over a playing picture, outline the window.
        if self.pointer.is_none() && l.card.w <= 0.0 {
            return;
        }
        let s = l.s;
        let r = RectF::new(10.0 * s, 10.0 * s, l.w - 20.0 * s, l.h - 20.0 * s);
        fb.stroke_rrect(r, 24.0 * s, 2.0 * s, t::magenta_500(), 0.9);
        fb.stroke_rrect(r.inflate(-3.0 * s), 21.0 * s, 8.0 * s, t::magenta_500(), 0.12);
    }

    pub(crate) fn draw_panel(&mut self, fb: &mut FrameBuffer, l: &Layout, p: &Panel) {
        let s = l.s;
        fb.shadow_rrect(p.rect, 14.0 * s, 8.0 * s, 28.0 * s, Rgba::new(5, 2, 15, 150), 1.0);
        fb.fill_rrect(p.rect, 14.0 * s, Paint::Solid(t::ink_800()), 1.0);
        fb.stroke_rrect(p.rect, 14.0 * s, 1.0 * s, t::ink_500(), 1.0);
        fb.fill_rect_paint(
            RectF::new(p.rect.x + 14.0 * s, p.rect.y + 1.0 * s, p.rect.w - 28.0 * s, 1.0 * s),
            Paint::Solid(fade(t::white(), 0.07)),
            1.0,
        );
        for (i, it) in p.items.iter().enumerate() {
            let r = p.rows[i];
            if it.separator && i > 0 {
                fb.fill_rect_paint(
                    RectF::new(r.x + 12.0 * s, r.y - 4.5 * s, r.w - 24.0 * s, 1.0 * s),
                    Paint::Solid(fade(t::white(), 0.09)),
                    1.0,
                );
            }
            if it.heading {
                let label: String = it.label.chars().flat_map(char::to_uppercase).collect();
                self.text(
                    fb,
                    Face::SansBold,
                    10.5,
                    r.x + 16.0 * s,
                    r.cy() + 2.0 * s,
                    &label,
                    t::violet_400(),
                    1.0,
                    1.6,
                );
                continue;
            }
            let hot = p.hover == Some(i) && it.enabled;
            if hot {
                let hr = RectF::new(r.x + 4.0 * s, r.y, r.w - 8.0 * s, r.h);
                fb.fill_rrect(hr, 8.0 * s, Paint::Solid(t::ink_600()), 1.0);
                fb.fill_rrect(
                    RectF::new(hr.x + 1.0 * s, r.cy() - 8.0 * s, 3.0 * s, 16.0 * s),
                    1.5 * s,
                    Paint::Solid(t::violet_500()),
                    1.0,
                );
            }
            let col = if it.enabled { t::text_body() } else { t::text_disabled() };
            let gx = r.x + if p.gutter { 36.0 * s } else { 16.0 * s };
            if it.checked {
                self.icon(fb, Icon::Check, r.x + 22.0 * s, r.cy(), 14.0, t::cyan_500(), 1.0, false);
            }
            self.text(fb, Face::Sans, 14.0, gx, r.cy(), &it.label, col, 1.0, 0.0);
            if !it.sub.is_empty() {
                self.icon(
                    fb,
                    Icon::ChevronRight,
                    r.right() - 18.0 * s,
                    r.cy(),
                    14.0,
                    if it.enabled { t::text_muted() } else { t::text_disabled() },
                    1.0,
                    false,
                );
            } else if !it.hint.is_empty() {
                let w = self.text_w(Face::Mono, 11.0, &it.hint, 0.0);
                self.text(
                    fb,
                    Face::Mono,
                    11.0,
                    r.right() - 16.0 * s - w,
                    r.cy(),
                    &it.hint,
                    if it.enabled { t::text_dim() } else { t::text_disabled() },
                    1.0,
                    0.0,
                );
            }
        }
    }
}
