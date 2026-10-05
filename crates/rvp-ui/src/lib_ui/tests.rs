use super::*;
use crate::actions::{Action, menu_actions};
use crate::gfx::FrameBuffer;
use crate::model::{MediaState, PlaylistEntry};
use crate::ui::{Ui, UiConfig};
use alloc::vec;
use rvp_host::{FileEntry, InputEvent, Key, Modifiers, PointerButton};
use rvp_library::TrackTags;

/// 5 artists, 12 albums, 60 tracks.
fn library() -> Library {
    let mut l = Library::new();
    let mut files = Vec::new();
    for a in 0..12 {
        for t in 0..5 {
            let path = alloc::format!("Artist {}/Album {a:02}/{t:02} Track {t}.mp3", a % 5);
            files.push(FileEntry { id: alloc::format!("id{a}-{t}"), path, size: 100 + t, mtime_ms: 1 });
        }
    }
    let plan = l.begin_scan("r", "Music", &files);
    for e in &plan.read {
        let parts: Vec<&str> = e.path.split('/').collect();
        let (artist, album) = (parts[0], parts[1]);
        let no: u16 = parts[2][..2].parse::<u16>().unwrap() + 1;
        l.apply_tags(
            plan.root,
            e,
            Ok(TrackTags {
                title: alloc::format!("Song {no} of {album}"),
                artist: artist.into(),
                album: album.into(),
                track_no: no,
                year: 2000 + (album.len() as i32),
                duration_us: 180_000_000 + no as i64 * 1_000_000,
                codec: "mp3".into(),
                ..Default::default()
            }),
        );
    }
    l.finish_scan();
    l
}

fn model() -> UiModel {
    UiModel { state: MediaState::Idle, volume: 1.0, rate: 1.0, ..UiModel::default() }
}

fn playing_model(lib: &Library) -> UiModel {
    let t = lib.albums()[2].tracks[1];
    let tr = lib.track(t).unwrap();
    let q: Vec<PlaylistEntry> = lib.albums()[2]
        .tracks
        .iter()
        .enumerate()
        .map(|(i, &id)| PlaylistEntry {
            id: i as u32 + 1,
            label: lib.track(id).unwrap().display_title().into(),
            current: id == t,
            subtitle: "Artist \u{b7} Album".into(),
            duration_us: 200_000_000,
            track: Some(id),
            art: 0,
        })
        .collect();
    UiModel {
        state: MediaState::Playing,
        title: tr.display_title().into(),
        artist: tr.display_artist().into(),
        album: tr.display_album().into(),
        now_track: Some(t),
        position_us: 30_000_000,
        duration_us: Some(200_000_000),
        volume: 0.8,
        rate: 1.0,
        playlist: alloc::rc::Rc::new(q),
        queue_rev: 1,
        ..UiModel::default()
    }
}

struct Rig {
    ui: Ui,
    lib: Library,
    m: UiModel,
    now: i64,
}

impl Rig {
    fn new() -> Self {
        let mut ui = Ui::new(UiConfig { reduce_motion: true });
        ui.set_size(1280, 720, 1.0);
        ui.set_mode(Mode::Library);
        Rig { ui, lib: library(), m: model(), now: 1_000_000 }
    }

    fn handle(&mut self, ev: InputEvent) -> Vec<Action> {
        self.now += 20_000;
        let ctx = LibCtx { lib: &self.lib, now_art: None, scan: None, viz: None, video: None };
        self.ui.handle_lib(&ev, self.now, &self.m, &ctx)
    }

    fn key(&mut self, k: Key) -> Vec<Action> {
        self.handle(InputEvent::KeyDown { key: k, mods: Modifiers::default(), repeat: false })
    }

    fn key_mod(&mut self, k: Key, mods: Modifiers) -> Vec<Action> {
        self.handle(InputEvent::KeyDown { key: k, mods, repeat: false })
    }

    fn typed(&mut self, text: &str) {
        for c in text.chars() {
            self.key(if c == ' ' { Key::Space } else { Key::Char(c) });
        }
    }

    fn click(&mut self, x: f32, y: f32) -> Vec<Action> {
        self.handle(InputEvent::PointerMove { x, y });
        let mut out = self.handle(InputEvent::PointerDown { x, y, button: PointerButton::Primary });
        out.extend(self.handle(InputEvent::PointerUp { x, y, button: PointerButton::Primary }));
        out
    }

    fn right_click(&mut self, x: f32, y: f32) {
        self.handle(InputEvent::PointerMove { x, y });
        self.handle(InputEvent::PointerDown { x, y, button: PointerButton::Secondary });
    }

    /// Lay the rows out for the current state and return the screen rectangle of entity `i`.
    fn ent_rect(&mut self, i: usize) -> crate::gfx::RectF {
        let ctx = LibCtx { lib: &self.lib, now_art: None, scan: None, viz: None, video: None };
        let g = self.ui.lib_geom(&self.m, &ctx);
        self.ui.ensure_rows(&self.m, &ctx, &g);
        let rows = self.ui.lib.rows.as_ref().unwrap();
        let r = rows.ent_rect(i, &g.m);
        crate::gfx::RectF::new(g.m.body.x + r.x, g.m.body.y + r.y - self.ui.lib.scroll, r.w, r.h)
    }

    /// The entities of the current view (rows are laid out first).
    fn ents(&mut self) -> Vec<super::Ent> {
        let ctx = LibCtx { lib: &self.lib, now_art: None, scan: None, viz: None, video: None };
        let g = self.ui.lib_geom(&self.m, &ctx);
        self.ui.ensure_rows(&self.m, &ctx, &g);
        self.ui.lib_state().entities().to_vec()
    }

    fn rail_item(&mut self, v: View) -> crate::gfx::RectF {
        let ctx = LibCtx { lib: &self.lib, now_art: None, scan: None, viz: None, video: None };
        let g = self.ui.lib_geom(&self.m, &ctx);
        g.nav.iter().find(|(x, _)| *x == v).unwrap().1
    }

    fn draw(&mut self) -> FrameBuffer {
        let ctx = LibCtx { lib: &self.lib, now_art: None, scan: None, viz: None, video: None };
        let mut fb = FrameBuffer::new(self.ui.size().0, self.ui.size().1);
        self.ui.update(self.now, &self.m);
        self.ui.draw_base_lib(&mut fb, &self.m, &ctx);
        self.ui.draw_overlay_lib(&mut fb, &self.m, &ctx);
        fb
    }
}

fn center(r: crate::gfx::RectF) -> (f32, f32) {
    (r.cx(), r.cy())
}

#[test]
fn albums_are_a_grid_a_click_opens_one_and_back_returns() {
    let mut r = Rig::new();
    r.ui.show_view(View::Albums);
    let first = r.ent_rect(0);
    let ents = {
        let ctx = LibCtx { lib: &r.lib, now_art: None, scan: None, viz: None, video: None };
        let g = r.ui.lib_geom(&r.m, &ctx);
        r.ui.ensure_rows(&r.m, &ctx, &g);
        r.ui.lib.rows.as_ref().unwrap().ents.len()
    };
    assert_eq!(ents, 12);
    assert!(r.ent_rect(1).x > first.right(), "cards sit side by side");
    let (x, y) = center(first);
    r.click(x, y - 20.0);
    assert!(matches!(r.ui.lib_state().detail(), Some(Detail::Album(_))));
    // The detail has the album's tracks.
    let n = r.ents().len();
    assert_eq!(n, 5);
    r.key(Key::Escape);
    assert_eq!(r.ui.lib_state().detail(), None);
    assert_eq!(r.ui.lib_state().view(), View::Albums);
}

#[test]
fn the_play_button_of_a_card_plays_the_album() {
    let mut r = Rig::new();
    r.ui.show_view(View::Albums);
    let c = r.ent_rect(3);
    let cw = c.w;
    // Its play button is at the lower right of the cover.
    let out = r.click(c.x + cw - 30.0, c.y + cw - 30.0);
    let id = r.lib.albums()[3].id;
    assert_eq!(out, [Action::Lib(LibAction::Play(Scope::Album(id), Enqueue::Now))]);
}

#[test]
fn arrows_enter_and_the_context_key_drive_the_track_list() {
    let mut r = Rig::new();
    r.ui.show_view(View::Tracks);
    assert!(r.key(Key::Down).is_empty());
    assert_eq!(r.ui.lib_state().selected(), Some(0));
    r.key(Key::Down);
    r.key(Key::Down);
    assert_eq!(r.ui.lib_state().selected(), Some(2));
    r.key(Key::Up);
    assert_eq!(r.ui.lib_state().selected(), Some(1));
    // Enter plays from the selected row; Shift+Enter adds to the queue; Ctrl+Enter plays next.
    let out = r.key(Key::Enter);
    assert_eq!(out, [Action::Lib(LibAction::Play(Scope::ListFrom(1), Enqueue::Now))]);
    let id = match r.ui.lib_state().entities()[1].kind {
        EntKind::Track { id, .. } => id,
        k => panic!("{k:?}"),
    };
    assert_eq!(
        r.key_mod(Key::Enter, Modifiers { shift: true, ..Default::default() }),
        [Action::Lib(LibAction::Play(Scope::Track(id), Enqueue::Append))]
    );
    assert_eq!(
        r.key_mod(Key::Enter, Modifiers { ctrl: true, ..Default::default() }),
        [Action::Lib(LibAction::Play(Scope::Track(id), Enqueue::Next))]
    );
    // End goes to the last row and scrolls it into view; Home comes back.
    r.key(Key::End);
    assert_eq!(r.ui.lib_state().selected(), Some(59));
    assert!(r.ui.lib.scroll > 0.0);
    r.key(Key::Home);
    assert_eq!(r.ui.lib_state().selected(), Some(0));
    assert_eq!(r.ui.lib.scroll, 0.0);
    // The menu key opens the row's menu and its entries run.
    assert!(r.key(Key::Other("ContextMenu".into())).is_empty());
    assert!(r.ui.menu_open());
    let labels: Vec<String> = r.ui.menu_rows().into_iter().map(|(l, _, _)| l).collect();
    for want in ["Play", "Play next", "Add to queue", "Add to playlist", "Go to album", "Go to artist"] {
        assert!(labels.iter().any(|l| l == want), "{want} in {labels:?}");
    }
    r.key(Key::Down); // from Play to Play next
    let out = r.key(Key::Enter);
    let first = match r.ui.lib_state().entities()[0].kind {
        EntKind::Track { id, .. } => id,
        _ => unreachable!(),
    };
    assert_eq!(out, [Action::Lib(LibAction::Play(Scope::Track(first), Enqueue::Next))]);
    assert!(!r.ui.menu_open());
}

#[test]
fn clicking_a_header_sorts_and_the_wheel_scrolls() {
    let mut r = Rig::new();
    r.ui.show_view(View::Tracks);
    let ctx = LibCtx { lib: &r.lib, now_art: None, scan: None, viz: None, video: None };
    let g = r.ui.lib_geom(&r.m, &ctx);
    let (_, col) = g.table_cols.iter().find(|(c, _)| *c == 1).copied().unwrap();
    r.click(col.cx(), col.cy());
    assert_eq!(r.ui.lib.track_sort, TrackSort::Artist);
    r.click(col.cx(), col.cy());
    assert!(!r.ui.lib.track_asc, "a second click reverses it");
    r.handle(InputEvent::Wheel { dx: 0.0, dy: 300.0 });
    assert_eq!(r.ui.lib.scroll, 300.0);
    r.handle(InputEvent::Wheel { dx: 0.0, dy: -1000.0 });
    assert_eq!(r.ui.lib.scroll, 0.0);
}

#[test]
fn typing_in_the_search_box_shows_results_and_escape_goes_back() {
    let mut r = Rig::new();
    r.ui.show_view(View::Artists);
    r.key(Key::Char('/'));
    assert_eq!(r.ui.lib_state().view(), View::Search);
    assert!(r.ui.lib_state().typing(), "the page must not treat letters as shortcuts now");
    r.typed("album 03");
    assert_eq!(r.ui.lib_state().query(), "album 03");
    // The album, and its five tracks.
    let kinds: Vec<EntKind> = r.ui.lib_state().entities().iter().map(|e| e.kind).collect();
    let _ = kinds;
    let ctx = LibCtx { lib: &r.lib, now_art: None, scan: None, viz: None, video: None };
    let g = r.ui.lib_geom(&r.m, &ctx);
    r.ui.ensure_rows(&r.m, &ctx, &g);
    let ents = r.ui.lib_state().entities().to_vec();
    assert_eq!(ents.iter().filter(|e| matches!(e.kind, EntKind::Album(_))).count(), 1);
    assert_eq!(ents.iter().filter(|e| matches!(e.kind, EntKind::Track { .. })).count(), 5);
    // Backspace shortens the query; Escape clears it and returns to the view before.
    r.key(Key::Other("Backspace".into()));
    assert_eq!(r.ui.lib_state().query(), "album 0");
    r.key(Key::Escape);
    assert_eq!(r.ui.lib_state().query(), "");
    assert_eq!(r.ui.lib_state().view(), View::Artists);
    assert!(!r.ui.lib_state().typing());
    // Letters are shortcuts again outside the box: `m` mutes.
    assert_eq!(r.key(Key::Char('m')), [Action::ToggleMute]);
}

#[test]
fn digits_and_the_rail_switch_views() {
    let mut r = Rig::new();
    for (k, v) in [
        ('1', View::Albums),
        ('2', View::Artists),
        ('3', View::Tracks),
        ('4', View::Playlists),
        ('5', View::Queue),
        ('6', View::NowPlaying),
        ('7', View::Visualizer),
    ] {
        r.key(Key::Char(k));
        assert_eq!(r.ui.lib_state().view(), v);
    }
    r.ui.show_view(View::Albums);
    let rect = r.rail_item(View::Tracks);
    let (x, y) = center(rect);
    r.click(x, y);
    assert_eq!(r.ui.lib_state().view(), View::Tracks);
    // The mode switch asks the app to go to the player.
    let ctx = LibCtx { lib: &r.lib, now_art: None, scan: None, viz: None, video: None };
    let g = r.ui.lib_geom(&r.m, &ctx);
    let (px, py) = center(g.mode[1]);
    assert_eq!(r.click(px, py), [Action::SetMode(Mode::Player)]);
}

#[test]
fn artists_open_their_albums() {
    let mut r = Rig::new();
    r.ui.show_view(View::Artists);
    let (x, y) = center(r.ent_rect(1));
    r.click(x, y);
    assert!(matches!(r.ui.lib_state().detail(), Some(Detail::Artist(_))));
    // Artist 1 has albums 1, 6 and 11 (a % 5 == 1).
    let n = r.ents().iter().filter(|e| matches!(e.kind, EntKind::Album(_))).count();
    assert_eq!(n, 3);
    // The mouse's back button goes back.
    r.handle(InputEvent::PointerDown { x: 10.0, y: 10.0, button: PointerButton::Back });
    assert_eq!(r.ui.lib_state().detail(), None);
}

#[test]
fn the_queue_lists_the_model_and_alt_arrows_move_items() {
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    r.ui.show_view(View::Queue);
    assert_eq!(r.ui.lib_state().entities().len(), 0, "rows are built on the first look");
    r.key(Key::Down);
    assert_eq!(r.ui.lib_state().entities().len(), 5);
    assert_eq!(r.ui.lib_state().selected(), Some(0));
    let out = r.key_mod(Key::Down, Modifiers { alt: true, ..Default::default() });
    assert_eq!(out, [Action::MoveItem(1, 1)]);
    assert_eq!(r.ui.lib_state().selected(), Some(1));
    assert_eq!(r.key(Key::Other("Delete".into())), [Action::RemoveItem(2)]);
    assert_eq!(r.key(Key::Enter), [Action::PlayItem(2)]);
}

#[test]
fn the_name_prompt_collects_text_as_a_command() {
    let mut r = Rig::new();
    r.ui.ask_playlist_name(Some(Scope::Album(7)));
    assert!(r.ui.lib_state().typing());
    r.typed("Road ");
    r.key(Key::Char('T'));
    r.key(Key::Char('1'));
    r.key(Key::Other("Backspace".into()));
    r.key(Key::Char('2'));
    assert!(r.key(Key::Enter).is_empty());
    assert_eq!(
        r.ui.take_commands(),
        [UiCommand::CreatePlaylist { name: "Road T2".into(), from: Some(Scope::Album(7)) }]
    );
    assert!(!r.ui.lib_state().typing());
    // Escape cancels.
    r.ui.ask_playlist_rename(3, "Old");
    r.key(Key::Escape);
    assert!(r.ui.take_commands().is_empty());
}

#[test]
fn the_bar_controls_map_to_actions_and_the_seek_bar_seeks() {
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    let ctx = LibCtx { lib: &r.lib, now_art: None, scan: None, viz: None, video: None };
    let g = r.ui.lib_geom(&r.m, &ctx);
    for (b, a) in [
        (crate::ui::Btn::Play, Action::PlayPause),
        (crate::ui::Btn::Next, Action::Next),
        (crate::ui::Btn::Prev, Action::Prev),
        (crate::ui::Btn::Shuffle, Action::ToggleShuffle),
        (crate::ui::Btn::Repeat, Action::CycleRepeat),
        (crate::ui::Btn::Mute, Action::ToggleMute),
        (crate::ui::Btn::ModeSwitch, Action::SetMode(Mode::Player)),
    ] {
        let (x, y) = center(g.bar_btns.iter().find(|(k, _)| *k == b).unwrap().1);
        assert_eq!(r.click(x, y), [a], "{b:?}");
    }
    let (x, y) = center(g.seek_hit);
    let out = r.click(x, y);
    assert!(out.iter().any(|a| matches!(a, Action::SeekFraction(f) if (*f - 0.5).abs() < 0.05)), "{out:?}");
    // The queue button opens the queue view.
    let (x, y) = center(g.bar_btns.iter().find(|(k, _)| *k == crate::ui::Btn::QueueView).unwrap().1);
    r.click(x, y);
    assert_eq!(r.ui.lib_state().view(), View::Queue);
}

#[test]
fn every_view_draws_without_trouble_and_the_rail_is_there() {
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    let id = r.lib.create_playlist("Mix");
    let ts: Vec<u32> = r.lib.albums()[0].tracks.clone();
    r.lib.playlist_add(id, &ts);
    for v in [
        View::Albums,
        View::Artists,
        View::Tracks,
        View::Playlists,
        View::Queue,
        View::NowPlaying,
        View::Visualizer,
        View::Search,
    ] {
        r.ui.show_view(v);
        let fb = r.draw();
        // The rail is a distinct, darker column on the left; every view paints something other than the background.
        let rail = fb.pixel(4, 600);
        if v != View::Visualizer {
            assert!(rail.r < 20 && rail.b > 15, "{v:?}: rail {rail:?}");
        }
        let distinct = fb
            .pixels
            .chunks_exact(4)
            .step_by(997)
            .map(|p| (p[0], p[1], p[2]))
            .collect::<alloc::collections::BTreeSet<_>>();
        assert!(distinct.len() > 6, "{v:?} looks empty");
    }
    // Details too.
    let aid = r.lib.albums()[0].id;
    r.ui.open_detail(Detail::Album(aid));
    r.draw();
    r.ui.show_view(View::Playlists);
    r.ui.open_detail(Detail::Playlist(id));
    r.draw();
    let artist = r.lib.artists()[0].id;
    r.ui.show_view(View::Artists);
    r.ui.open_detail(Detail::Artist(artist));
    r.draw();
}

#[test]
fn it_still_draws_at_a_small_size_and_with_a_hidpi_scale() {
    for (w, h, dpr) in [(480, 640, 1.0), (800, 500, 1.0), (2560, 1440, 2.0), (1280, 720, 1.5)] {
        let mut r = Rig::new();
        r.ui.set_size(w, h, dpr);
        r.m = playing_model(&r.lib);
        for v in [View::Albums, View::Tracks, View::NowPlaying, View::Queue] {
            r.ui.show_view(v);
            r.draw();
        }
        r.key(Key::Char('/'));
        r.typed("a");
        r.draw();
    }
}

#[test]
fn the_general_menu_reaches_every_view_and_every_shortcut() {
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    r.right_click(900.0, 650.0);
    assert!(r.ui.menu_open());
    let ctx = LibCtx { lib: &r.lib, now_art: None, scan: None, viz: None, video: None };
    let items = super::menus::global_menu(&r.ui, &r.m, &ctx);
    let mut reach = Vec::new();
    menu_actions(&items, &mut reach);
    for v in [
        View::Search,
        View::NowPlaying,
        View::Albums,
        View::Artists,
        View::Tracks,
        View::Playlists,
        View::Queue,
        View::Visualizer,
    ] {
        assert!(reach.contains(&Action::ShowView(v)), "{v:?}");
    }
    for s in crate::actions::SHORTCUTS {
        assert!(
            reach.contains(&s.action) || s.action == Action::ToggleMode,
            "{:?} is not in the library's menu",
            s.action
        );
    }
    assert!(reach.contains(&Action::Lib(LibAction::AddFolder)));
}

#[test]
fn the_empty_library_offers_to_add_a_folder() {
    let mut r = Rig::new();
    r.lib = Library::new();
    r.ui.show_view(View::Albums);
    let ctx = LibCtx { lib: &r.lib, now_art: None, scan: None, viz: None, video: None };
    let g = r.ui.lib_geom(&r.m, &ctx);
    r.ui.ensure_rows(&r.m, &ctx, &g);
    let rect = crate::gfx::RectF::new(g.m.body.x, g.m.body.y, g.m.body.w, 300.0);
    let btns = r.ui.message_buttons(rect);
    let (x, y) = center(btns[0].rect);
    assert_eq!(r.click(x, y), [Action::Lib(LibAction::AddFolder)]);
    let (x, y) = center(btns[1].rect);
    assert_eq!(r.click(x, y), [Action::OpenFile]);
    r.draw();
}

#[test]
fn visualizer_keys_step_effects_and_leave() {
    let mut r = Rig::new();
    r.key(Key::Char('7'));
    assert_eq!(r.ui.lib_state().view(), View::Visualizer);
    assert_eq!(r.key(Key::Right), [Action::Lib(LibAction::VizStep(1))]);
    assert_eq!(r.key(Key::Left), [Action::Lib(LibAction::VizStep(-1))]);
    assert_eq!(r.key(Key::Char('c')), [Action::Lib(LibAction::VizPalette)]);
    assert_eq!(r.key(Key::Enter), [Action::Lib(LibAction::VizToggle)]);
    // Seeking stays on J and L and Ctrl+arrows.
    assert_eq!(r.key(Key::Char('l')), [Action::SeekBy(10_000)]);
    assert_eq!(
        r.key_mod(Key::Right, Modifiers { ctrl: true, ..Default::default() }),
        [Action::SeekBy(5_000)]
    );
    r.key(Key::Escape);
    assert_eq!(r.ui.lib_state().view(), View::NowPlaying);
}

#[test]
fn plain_arrows_still_seek_on_the_now_playing_screen() {
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    r.ui.show_view(View::NowPlaying);
    assert_eq!(r.key(Key::Right), [Action::SeekBy(5_000)]);
    assert_eq!(r.key(Key::Up), [Action::VolumeBy(5)]);
    assert_eq!(r.key(Key::Space), [Action::PlayPause]);
    let _ = vec![0];
}
