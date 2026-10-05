//! Render the UI states to PNG files for eyeballing: `cargo run -p rvp-ui --example preview -- <outdir>`.
use rvp_host::{InputEvent, PointerButton};
use rvp_ui::{FrameBuffer, MediaState, TrackItem, Ui, UiConfig, UiModel};

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn png(fb: &FrameBuffer) -> Vec<u8> {
    let mut raw = Vec::new();
    for y in 0..fb.height as usize {
        raw.push(0);
        raw.extend_from_slice(&fb.pixels[y * fb.width as usize * 4..(y + 1) * fb.width as usize * 4]);
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(if chunks.peek().is_none() { 1 } else { 0 });
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&fb.width.to_be_bytes());
    ihdr.extend_from_slice(&fb.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

/// A stand-in picture: 4:3 colour bars with a gradient.
fn picture() -> (Vec<u8>, u32, u32) {
    let (w, h) = (320u32, 240u32);
    let mut v = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let bar = (x * 7 / w) as usize;
            let cols = [
                [235, 235, 235],
                [235, 235, 16],
                [16, 235, 235],
                [16, 235, 16],
                [235, 16, 235],
                [235, 16, 16],
                [16, 16, 235],
            ];
            let c = cols[bar];
            let k = 1.0 - (y as f32 / h as f32) * 0.6;
            v.extend_from_slice(&[
                (c[0] as f32 * k) as u8,
                (c[1] as f32 * k) as u8,
                (c[2] as f32 * k) as u8,
                255,
            ]);
        }
    }
    (v, w, h)
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let (w, h, dpr) = (1280u32, 720u32, 1.0f32);
    let mut base = FrameBuffer::new(w, h);
    let mut fb = FrameBuffer::new(w, h);
    let (pic, pw, ph) = picture();
    let mut model = UiModel { state: MediaState::Idle, rate: 1.0, volume: 0.8, ..UiModel::default() };
    let mut ui = Ui::new(UiConfig { reduce_motion: true });
    ui.set_size(w, h, dpr);
    let mut t = 1_000_000i64;
    let mut shot = |name: &str, ui: &mut Ui, model: &UiModel, with_video: bool, t: i64| {
        ui.update(t, model);
        ui.draw_base(&mut base, model, if with_video { Some((&pic, pw, ph)) } else { None });
        fb.copy_from(&base);
        ui.draw_overlay(&mut fb, model);
        std::fs::write(format!("{dir}/{name}.png"), png(&fb)).unwrap();
    };
    shot("idle", &mut ui, &model, false, t);
    ui.handle(&InputEvent::DragOver(true), t, &model);
    shot("idle-dragover", &mut ui, &model, false, t);
    ui.handle(&InputEvent::DragOver(false), t, &model);

    model = UiModel {
        state: MediaState::Paused,
        title: "av1_opus_60s.webm".into(),
        position_us: 23_000_000,
        duration_us: Some(60_000_000),
        volume: 0.8,
        rate: 1.0,
        has_video: true,
        audio_tracks: vec![TrackItem { id: 2, label: "Audio 1 (opus, stereo)".into() }],
        selected_audio: Some(2),
        ..UiModel::default()
    };
    t += 100_000;
    ui.handle(&InputEvent::PointerMove { x: 640.0, y: 300.0 }, t, &model);
    shot("paused", &mut ui, &model, true, t);

    model.state = MediaState::Playing;
    t += 100_000;
    ui.handle(&InputEvent::PointerMove { x: 640.0, y: 700.0 }, t, &model);
    ui.handle(&InputEvent::PointerMove { x: 400.0, y: 646.0 }, t + 10, &model);
    t += 700_000;
    shot("playing-seek-hover", &mut ui, &model, true, t);

    let l = ui.layout(&model);
    let p = l.rect_of(rvp_ui::ui::Btn::Play).unwrap();
    ui.handle(&InputEvent::PointerMove { x: p.cx(), y: p.cy() }, t, &model);
    t += 800_000;
    shot("playing-play-hover", &mut ui, &model, true, t);

    let r = l.rect_of(rvp_ui::ui::Btn::Speed).unwrap();
    ui.handle(&InputEvent::PointerMove { x: r.cx(), y: r.cy() }, t, &model);
    ui.handle(&InputEvent::PointerDown { x: r.cx(), y: r.cy(), button: PointerButton::Primary }, t, &model);
    ui.handle(&InputEvent::PointerUp { x: r.cx(), y: r.cy(), button: PointerButton::Primary }, t, &model);
    shot("speed-menu", &mut ui, &model, true, t);
    ui.handle(
        &InputEvent::KeyDown { key: rvp_host::Key::Escape, mods: Default::default(), repeat: false },
        t,
        &model,
    );

    ui.handle(&InputEvent::PointerDown { x: 700.0, y: 250.0, button: PointerButton::Secondary }, t, &model);
    ui.handle(&InputEvent::PointerMove { x: 760.0, y: 330.0 }, t, &model);
    shot("context-menu", &mut ui, &model, true, t);
    ui.handle(
        &InputEvent::KeyDown { key: rvp_host::Key::Escape, mods: Default::default(), repeat: false },
        t,
        &model,
    );

    ui.show_toast("Speed 1.5\u{d7}", t);
    model.rate = 1.5;
    shot("toast", &mut ui, &model, true, t);

    model.has_video = false;
    model.state = MediaState::Paused;
    shot("audio-only", &mut ui, &model, false, t);

    model.state = MediaState::Failed;
    model.error = Some("Video codec `hevc` isn't supported. HEVC isn't on the guest list.".into());
    shot("error", &mut ui, &model, false, t);
}
