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
        let ctx = LibCtx {
            lib: &self.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
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
        let ctx = LibCtx {
            lib: &self.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
        let g = self.ui.lib_geom(&self.m, &ctx);
        self.ui.ensure_rows(&self.m, &ctx, &g);
        let rows = self.ui.lib.rows.as_ref().unwrap();
        let r = rows.ent_rect(i, &g.m);
        crate::gfx::RectF::new(g.m.body.x + r.x, g.m.body.y + r.y - self.ui.lib.scroll, r.w, r.h)
    }

    /// The entities of the current view (rows are laid out first).
    fn ents(&mut self) -> Vec<super::Ent> {
        let ctx = LibCtx {
            lib: &self.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
        let g = self.ui.lib_geom(&self.m, &ctx);
        self.ui.ensure_rows(&self.m, &ctx, &g);
        self.ui.lib_state().entities().to_vec()
    }

    fn rail_item(&mut self, v: View) -> crate::gfx::RectF {
        let ctx = LibCtx {
            lib: &self.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
        let g = self.ui.lib_geom(&self.m, &ctx);
        g.nav.iter().find(|(x, _)| *x == v).unwrap().1
    }

    fn draw(&mut self) -> FrameBuffer {
        let ctx = LibCtx {
            lib: &self.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
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
        let ctx = LibCtx {
            lib: &r.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
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
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
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
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
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
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
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
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    r.m.can_next = true;
    let g = r.ui.lib_geom(&r.m, &ctx);
    // Left to right: shuffle, restart or previous, back 10 s, play, forward 10 s, next, repeat.
    let order: Vec<crate::ui::Btn> = {
        let mut v: Vec<_> = g
            .bar_btns
            .iter()
            .filter(|(b, _)| {
                use crate::ui::Btn::*;
                matches!(b, Shuffle | Prev | Back | Play | Fwd | Next | Repeat)
            })
            .collect();
        v.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
        v.into_iter().map(|(b, _)| *b).collect()
    };
    {
        use crate::ui::Btn::*;
        assert_eq!(order, [Shuffle, Prev, Back, Play, Fwd, Next, Repeat]);
    }
    for (b, a) in [
        (crate::ui::Btn::Play, Action::PlayPause),
        (crate::ui::Btn::Next, Action::Next),
        (crate::ui::Btn::Prev, Action::Prev),
        (crate::ui::Btn::Back, Action::SeekBy(-10_000)),
        (crate::ui::Btn::Fwd, Action::SeekBy(10_000)),
        (crate::ui::Btn::Shuffle, Action::ToggleShuffle),
        (crate::ui::Btn::Repeat, Action::CycleRepeat),
        (crate::ui::Btn::Mute, Action::ToggleMute),
        (crate::ui::Btn::ModeSwitch, Action::SetMode(Mode::Player)),
    ] {
        let (x, y) = center(g.bar_btns.iter().find(|(k, _)| *k == b).unwrap().1);
        assert_eq!(r.click(x, y), [a], "{b:?}");
    }
    // With nothing after the current item the next button is inert (and greyed out).
    r.m.can_next = false;
    let (x, y) = center(g.bar_btns.iter().find(|(k, _)| *k == crate::ui::Btn::Next).unwrap().1);
    assert!(r.click(x, y).is_empty());
    r.m.can_next = true;
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
    r.m.app.quit = true;
    r.right_click(900.0, 650.0);
    assert!(r.ui.menu_open());
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
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
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
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
    // Escape goes back to where the visualizer was opened from.
    r.key(Key::Escape);
    assert_eq!(r.ui.lib_state().view(), View::Albums);
}

impl Rig {
    /// What the app does for the visualizer's button and key.
    fn toggle_viz(&mut self) {
        let ctx = LibCtx {
            lib: &self.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
        self.ui.toggle_visualizer(&self.m, &ctx);
    }

    fn go_back(&mut self) {
        let ctx = LibCtx {
            lib: &self.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
        self.ui.go_back(&self.m, &ctx);
    }

    /// Click the bar's visualizer button (the bar is drawn first, so the floating bar of the visualizer is up).
    fn click_viz_button(&mut self) {
        self.draw();
        let ctx = LibCtx {
            lib: &self.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
        let g = self.ui.lib_geom(&self.m, &ctx);
        let (x, y) = center(g.bar_btns.iter().find(|(b, _)| *b == crate::ui::Btn::VizView).unwrap().1);
        let out = self.click(x, y);
        // The click itself changes the view; nothing is left for the app to do.
        assert!(out.is_empty(), "{out:?}");
    }

    /// The place the visualizer would return to (the invariant: it is set exactly while the visualizer is the view).
    fn viz_return(&self) -> Option<(Mode, View)> {
        let l = self.ui.lib_state();
        assert_eq!(l.viz_return.is_some(), l.view() == View::Visualizer, "stale or missing viz_return");
        l.viz_return.as_ref().map(|v| (v.mode, v.nav.view))
    }
}

#[test]
fn the_visualizer_returns_to_the_place_it_was_opened_from_by_every_way_in_and_out() {
    // Ways in: the digit key, the bar's button, the toggle (key V and the menus), the view action.
    // Ways out: Escape, Backspace, the button, the toggle, Back.
    let ins: [fn(&mut Rig); 4] = [
        |r| {
            r.key(Key::Char('7'));
        },
        |r| r.click_viz_button(),
        |r| r.toggle_viz(),
        |r| r.ui.show_view(View::Visualizer),
    ];
    let outs: [fn(&mut Rig); 5] = [
        |r| {
            r.key(Key::Escape);
        },
        |r| {
            r.key(Key::Other("Backspace".into()));
        },
        |r| r.click_viz_button(),
        |r| r.toggle_viz(),
        |r| r.go_back(),
    ];
    for (i, enter) in ins.iter().enumerate() {
        for (o, leave) in outs.iter().enumerate() {
            let mut r = Rig::new();
            r.m = playing_model(&r.lib);
            // From an album opened from the artists list, with a row selected.
            r.ui.show_view(View::Artists);
            let artist = r.lib.artists()[1].id;
            r.ui.open_detail(Detail::Artist(artist));
            let album = r.lib.albums().iter().find(|a| a.artist_id == artist).unwrap().id;
            r.ui.open_detail(Detail::Album(album));
            r.ui.lib.sel = Some(2);
            enter(&mut r);
            assert_eq!(r.viz_return(), Some((Mode::Library, View::Artists)), "in {i}");
            assert_eq!(r.ui.lib_state().detail(), None);
            leave(&mut r);
            let l = r.ui.lib_state();
            assert_eq!(l.mode(), Mode::Library, "in {i} out {o}");
            assert_eq!((l.view(), l.detail()), (View::Artists, Some(Detail::Album(album))), "in {i} out {o}");
            assert_eq!(l.selected(), Some(2), "in {i} out {o}");
            assert_eq!(r.viz_return(), None);
            // And Back from the album still walks up to the artist, as before.
            r.go_back();
            assert_eq!(r.ui.lib_state().detail(), Some(Detail::Artist(artist)), "in {i} out {o}");
        }
    }
}

#[test]
fn the_visualizer_returns_to_each_kind_of_view_and_to_the_player_face() {
    for v in [View::Albums, View::Tracks, View::Playlists, View::Queue, View::NowPlaying, View::Search] {
        let mut r = Rig::new();
        r.m = playing_model(&r.lib);
        r.ui.show_view(v);
        if v == View::Search {
            r.typed("track");
        }
        let q = String::from(r.ui.lib_state().query());
        r.toggle_viz();
        assert_eq!(r.ui.lib_state().view(), View::Visualizer);
        r.toggle_viz();
        assert_eq!(r.ui.lib_state().view(), v, "{v:?}");
        assert_eq!(r.ui.lib_state().query(), q);
        assert_eq!(r.viz_return(), None);
    }
    // The scroll position and the selection come back too.
    let mut r = Rig::new();
    r.ui.show_view(View::Tracks);
    r.ents();
    r.ui.lib.scroll = 300.0;
    r.ui.lib.sel = Some(9);
    r.toggle_viz();
    r.toggle_viz();
    assert_eq!((r.ui.lib_state().scroll, r.ui.lib_state().selected()), (300.0, Some(9)));
    // From the Player face: the visualizer opens in the library face and leaves back to the player.
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    r.m.has_video = true;
    r.ui.set_mode(Mode::Player);
    r.toggle_viz();
    assert_eq!((r.ui.mode(), r.ui.lib_state().view()), (Mode::Library, View::Visualizer));
    assert_eq!(r.viz_return(), Some((Mode::Player, View::Albums)));
    r.key(Key::Escape);
    assert_eq!((r.ui.mode(), r.ui.lib_state().view()), (Mode::Player, View::Albums));
    assert_eq!(r.viz_return(), None);
}

#[test]
fn entering_the_visualizer_again_keeps_the_first_place_and_other_exits_leave_no_stale_state() {
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    r.ui.show_view(View::Queue);
    r.ui.show_view(View::Visualizer);
    r.ui.show_view(View::Visualizer);
    r.key(Key::Char('7'));
    assert_eq!(r.viz_return(), Some((Mode::Library, View::Queue)));
    r.key(Key::Escape);
    assert_eq!(r.ui.lib_state().view(), View::Queue, "not the visualizer itself");
    // Going to another view from the visualizer forgets the remembered one: the next visit remembers the new place.
    r.ui.show_view(View::Visualizer);
    r.key(Key::Char('3'));
    assert_eq!(r.ui.lib_state().view(), View::Tracks);
    assert_eq!(r.viz_return(), None);
    r.toggle_viz();
    r.toggle_viz();
    assert_eq!(r.ui.lib_state().view(), View::Tracks);
    // Opening an album from it (or any other detail) forgets it too.
    r.ui.show_view(View::Visualizer);
    r.ui.open_detail(Detail::Album(r.lib.albums()[0].id));
    assert_eq!(r.viz_return(), None);
    // The player face, B and back: the visualizer stays the library's view and still knows where it came from.
    r.ui.show_view(View::Albums);
    r.ui.show_view(View::Visualizer);
    r.ui.set_mode(Mode::Player);
    assert_eq!(r.viz_return(), Some((Mode::Library, View::Albums)));
    r.ui.set_mode(Mode::Library);
    r.key(Key::Escape);
    assert_eq!(r.ui.lib_state().view(), View::Albums);
    // Opened from the player (a second time over a visualizer hidden behind it): it remembers the player, not itself.
    r.ui.show_view(View::Visualizer);
    r.ui.set_mode(Mode::Player);
    r.toggle_viz();
    assert_eq!(r.viz_return(), Some((Mode::Player, View::Albums)));
}

#[test]
fn when_the_place_is_gone_the_visualizer_falls_back_to_the_player_or_the_library_home() {
    // An album that was removed: the list it was opened from stays.
    let mut r = Rig::new();
    r.ui.show_view(View::Albums);
    r.ui.open_detail(Detail::Album(r.lib.albums()[0].id));
    r.toggle_viz();
    r.lib = Library::new();
    r.key(Key::Escape);
    assert_eq!((r.ui.lib_state().view(), r.ui.lib_state().detail()), (View::Albums, None));
    // A playlist that was deleted: the list it was opened from stays, and Back has nothing stale to walk to.
    let mut r = Rig::new();
    r.ui.show_view(View::Playlists);
    let id = r.lib.create_playlist("Mix");
    r.ui.open_detail(Detail::Playlist(id));
    r.toggle_viz();
    r.lib.delete_playlist(id);
    r.toggle_viz();
    assert_eq!((r.ui.lib_state().view(), r.ui.lib_state().detail()), (View::Playlists, None));
    r.go_back(); // nothing stale left to walk back to
    assert_eq!(r.ui.lib_state().view(), View::Playlists);
    // The queue was emptied and nothing is loaded: the library's home.
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    r.ui.show_view(View::Queue);
    r.toggle_viz();
    r.m = model();
    r.toggle_viz();
    assert_eq!((r.ui.mode(), r.ui.lib_state().view()), (Mode::Library, View::Albums));
    // The queue was emptied while a video is loaded: the player.
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    r.ui.show_view(View::Queue);
    r.toggle_viz();
    r.m.playlist = Default::default();
    r.m.has_video = true;
    r.toggle_viz();
    assert_eq!(r.ui.mode(), Mode::Player);
    // Came from the player, which has nothing loaded any more: the library's home.
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    r.ui.set_mode(Mode::Player);
    r.toggle_viz();
    r.m = model();
    r.toggle_viz();
    assert_eq!((r.ui.mode(), r.ui.lib_state().view()), (Mode::Library, View::Albums));
    // No remembered place at all (the visualizer was the view from the start): home.
    let mut r = Rig::new();
    r.ui.lib.view = View::Visualizer;
    r.go_back();
    assert_eq!(r.ui.lib_state().view(), View::Albums);
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

#[test]
fn dragging_a_queue_row_moves_it() {
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    r.ui.show_view(View::Queue);
    r.ents();
    let (x0, y0) = center(r.ent_rect(0));
    let (_, y2) = center(r.ent_rect(2));
    r.handle(InputEvent::PointerMove { x: x0 + 100.0, y: y0 });
    r.handle(InputEvent::PointerDown { x: x0 + 100.0, y: y0, button: PointerButton::Primary });
    // Not yet a drag: a few pixels of wobble are a click.
    r.handle(InputEvent::PointerMove { x: x0 + 100.0, y: y0 + 3.0 });
    r.handle(InputEvent::PointerMove { x: x0 + 100.0, y: y2 });
    let out = r.handle(InputEvent::PointerUp { x: x0 + 100.0, y: y2, button: PointerButton::Primary });
    assert_eq!(out, [Action::MoveItem(1, 2)]);
    // A click without moving selects (and a double click would play); it moves nothing.
    let out = r.click(x0 + 100.0, y0);
    assert!(out.is_empty());
    assert_eq!(r.ui.lib_state().selected(), Some(0));
    // The drag draws its drop line without trouble.
    r.handle(InputEvent::PointerMove { x: x0 + 100.0, y: y0 });
    r.handle(InputEvent::PointerDown { x: x0 + 100.0, y: y0, button: PointerButton::Primary });
    r.handle(InputEvent::PointerMove { x: x0 + 100.0, y: y2 });
    r.draw();
    r.handle(InputEvent::PointerUp { x: x0 + 100.0, y: y2, button: PointerButton::Primary });
}

#[test]
fn the_added_folders_show_in_the_rail_at_every_size() {
    for (w, h) in [(1280, 720), (1024, 600), (800, 500), (1280, 540)] {
        for roots in [1usize, 3, 7] {
            let mut r = Rig::new();
            for i in 1..roots {
                r.lib.begin_scan(&alloc::format!("r{i}"), &alloc::format!("Folder {i}"), &[]);
                r.lib.finish_scan();
            }
            r.ui.set_size(w, h, 1.0);
            let ctx = LibCtx {
                lib: &r.lib,
                now_art: None,
                scan: None,
                viz: None,
                video: None,
                resume: crate::lib_ui::no_resume(),
            };
            let g = r.ui.lib_geom(&r.m, &ctx);
            assert!(!g.m.compact || w < 1000, "{w}x{h}");
            if g.m.compact {
                continue; // the narrow rail has icons only
            }
            let n = g.folders.len();
            // The rail also holds Favorites, Settings and About RW now: at 720 high one folder row is guaranteed, more as the window
            // grows.
            if h >= 900 {
                assert!(n >= roots.min(3), "{w}x{h}, {roots} folders: only {n} rows");
            } else if h >= 720 {
                assert!(n >= 1, "{w}x{h}, {roots} folders: no row");
            }
            if n > 0 {
                assert_eq!(n + g.folders_more, roots, "{w}x{h}: every folder is a row or counted");
            }
            // The rows sit under the entries and above the add button, without overlapping either.
            let nav_bottom = g.nav.last().unwrap().1.bottom();
            for (_, rect) in &g.folders {
                assert!(rect.y >= nav_bottom, "{w}x{h}: a folder row overlaps the entries");
                assert!(rect.bottom() <= g.add_folder.y, "{w}x{h}: a folder row overlaps the add button");
            }
            r.draw();
        }
    }
}

#[test]
fn the_rail_has_a_settings_button_that_opens_settings_and_never_overlaps_the_others() {
    for (w, h) in [(1280, 720), (1024, 600), (800, 500), (1280, 540)] {
        let mut r = Rig::new();
        r.ui.set_size(w, h, 1.0);
        let ctx = LibCtx {
            lib: &r.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
        let g = r.ui.lib_geom(&r.m, &ctx);
        assert!(g.settings.y >= g.add_folder.bottom(), "{w}x{h}: the Settings button sits under Add folder");
        assert!(g.about.y >= g.settings.bottom(), "{w}x{h}: About RW comes last, under Settings");
        assert!(g.about.bottom() <= g.m.rail.bottom());
        let (x, y) = (g.settings.cx(), g.settings.cy());
        assert_eq!(r.click(x, y), [Action::ShowSettings], "{w}x{h}");
    }
}

// ---- videos ------------------------------------------------------------------------------------------------------------------------

fn with_videos(r: &mut Rig, n: usize) {
    for i in 0..n {
        let entry = FileEntry {
            id: alloc::format!("v{i}"),
            path: alloc::format!("Films/Film {i:02}.mkv"),
            size: 1000 + i as u64,
            mtime_ms: 1_000 + i as i64 * 10,
        };
        r.lib.apply_video(
            0,
            &entry,
            Ok(rvp_library::VideoInfo {
                title: if i == 3 { "A Special Film".into() } else { String::new() },
                duration_us: (60 + i as i64 * 7) * 1_000_000,
                width: 1920,
                height: 1080,
                vcodec: "h264".into(),
                acodec: "aac".into(),
            }),
        );
    }
    r.lib.finish_scan();
}

fn video_id(r: &mut Rig, i: usize) -> u32 {
    match r.ents()[i].kind {
        super::EntKind::Video { id, .. } => id,
        k => panic!("not a video: {k:?}"),
    }
}

#[test]
fn the_videos_view_shows_posters_in_a_grid_and_plays_them() {
    let mut r = Rig::new();
    with_videos(&mut r, 14);
    r.ui.show_view(View::Videos);
    let ents = r.ents();
    assert_eq!(ents.len(), 14);
    assert!(ents.iter().any(|e| e.col > 0), "a grid: several to a row");
    // Sorted by title: "A Special Film" comes first.
    let first = video_id(&mut r, 0);
    assert_eq!(r.lib.video(first).unwrap().display_title(), "A Special Film");
    // Cards are side by side; the poster (16:9) fills the top, the title and details sit under it.
    let (a, b) = (r.ent_rect(0), r.ent_rect(1));
    assert!(b.x > a.right() && a.h > a.w * 9.0 / 16.0 && a.h < a.w * 0.9, "{a:?} {b:?}");
    // A double click plays the list from that poster; the play button of a poster plays it at once.
    let id1 = video_id(&mut r, 1);
    let (x, y) = center(b);
    let out = r.click(x, y - b.h * 0.3);
    assert!(out.is_empty(), "a single click only selects");
    let out = r.click(x, y - b.h * 0.3);
    assert_eq!(out, [Action::Lib(LibAction::Play(Scope::ListFrom(1), Enqueue::Now))]);
    let out = r.click(b.cx(), b.y + b.w * 9.0 / 32.0);
    assert_eq!(
        out,
        [Action::Lib(LibAction::Play(Scope::ListFrom(1), Enqueue::Now))],
        "the round button in the middle"
    );
    // The context menu offers play, play next and add to queue, as a track's does.
    r.right_click(x, y - b.h * 0.3);
    assert!(r.ui.menu_open());
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let items = super::menus::ent_menu(&r.ui, 1, &r.m, &ctx);
    let labels: Vec<_> = items.iter().map(|m| m.label.as_str()).collect();
    assert_eq!(labels, ["Play", "Play next", "Add to queue", "Add to favorites"]);
    let mut reach = Vec::new();
    menu_actions(&items, &mut reach);
    assert!(reach.contains(&Action::Lib(LibAction::Play(Scope::Video(id1), Enqueue::Next))));
    assert_eq!(r.ui.scope_tracks(Scope::Video(id1), &ctx, &r.m), [id1]);
}

#[test]
fn the_layout_and_the_order_of_the_videos_can_be_changed_and_search_finds_them() {
    let mut r = Rig::new();
    with_videos(&mut r, 6);
    r.ui.show_view(View::Videos);
    r.draw();
    // The header's buttons: List (id 0) and the sort (id 1).
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let g = r.ui.lib_geom(&r.m, &ctx);
    let labels: Vec<_> = g.header_btns.iter().map(|b| (b.id, b.label.clone())).collect();
    assert!(
        labels.contains(&(0, "List".into()))
            && labels.iter().any(|(id, l)| *id == 1 && l.starts_with("Sort")),
        "{labels:?}"
    );
    let list_btn = g.header_btns.iter().find(|b| b.id == 0).unwrap().rect;
    r.click(list_btn.cx(), list_btn.cy());
    let ents = r.ents();
    assert!(ents.iter().all(|e| e.col == 0), "one to a row in the list");
    assert!(r.ui.lib_state().rows.as_ref().unwrap().video_list);
    // Sorting by when they were added puts the newest first.
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let g = r.ui.lib_geom(&r.m, &ctx);
    let sort_btn = g.header_btns.iter().find(|b| b.id == 1).unwrap().rect;
    r.click(sort_btn.cx(), sort_btn.cy());
    let newest = r.lib.sorted_videos(rvp_library::VideoSort::Added, false)[0];
    assert_eq!(video_id(&mut r, 0), newest);
    // Search finds a video by its title, in its own group.
    r.key(Key::Char('/'));
    r.typed("special");
    let ents = r.ents();
    assert_eq!(ents.len(), 1);
    assert!(matches!(ents[0].kind, super::EntKind::Video { .. }));
    r.draw();
}

#[test]
fn resume_markers_and_the_empty_message_are_drawn() {
    let mut r = Rig::new();
    r.ui.show_view(View::Videos);
    // No videos: the message offers to add a folder.
    r.draw();
    let fb = r.draw();
    assert!(fb.pixels.iter().any(|&b| b != 0));
    with_videos(&mut r, 3);
    let id = r.lib.sorted_videos(rvp_library::VideoSort::Title, true)[1];
    let mut resume = alloc::collections::BTreeMap::new();
    resume.insert(id, 0.5f32);
    let ctx = LibCtx { lib: &r.lib, now_art: None, scan: None, viz: None, video: None, resume: &resume };
    let mut with = FrameBuffer::new(r.ui.size().0, r.ui.size().1);
    r.ui.update(r.now, &r.m);
    r.ui.draw_base_lib(&mut with, &r.m, &ctx);
    let without = r.draw();
    // The marker is a bar along the foot of the second poster: the two pictures differ there and nowhere in the first poster's row.
    assert_ne!(with.pixels, without.pixels, "the resume marker is drawn");
}

#[test]
fn the_empty_player_sits_inside_the_apps_frame_with_an_open_a_video_card() {
    let mut r = Rig::new();
    r.ui.set_mode(Mode::Player);
    r.ui.set_player_empty(true);
    assert!(r.ui.lib_chrome());
    let fb = r.draw();
    assert!(fb.pixels.iter().any(|&b| b != 0));
    // The card's two buttons: opening a video leads, adding a folder follows.
    let btns = r.ui.lib_state().msg_btns.clone();
    assert_eq!(
        btns.iter().map(|b| (b.0, b.2.as_str())).collect::<Vec<_>>(),
        [(11, "Open a video"), (10, "Add folder")]
    );
    let (x, y) = center(btns[0].1);
    assert_eq!(r.click(x, y), [Action::OpenFile]);
    let (x, y) = center(btns[1].1);
    assert_eq!(r.click(x, y), [Action::Lib(LibAction::AddFolder)]);
    // The frame is the library's: the rail, with the Player side of the switch on; a rail entry goes to the Library face.
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let g = r.ui.lib_geom(&r.m, &ctx);
    assert!(!g.nav.is_empty() && g.settings.w > 0.0 && g.add_folder.w > 0.0);
    assert_eq!(r.ui.lib_state().mode(), Mode::Player);
    r.ui.show_view(View::Albums);
    assert_eq!(r.ui.lib_state().mode(), Mode::Library);
    // Once something is loaded the Player is the player again.
    r.ui.set_player_empty(false);
    r.ui.set_mode(Mode::Player);
    assert!(!r.ui.lib_chrome());
}

fn heart_of(r: &mut Rig, i: usize) -> crate::gfx::RectF {
    let rect = r.ent_rect(i);
    let video_list = r.ui.lib.rows.as_ref().unwrap().video_list;
    super::rows::heart_rect(r.ui.lib.rows.as_ref().unwrap().ents[i].kind, video_list, rect, 1.0).unwrap()
}

#[test]
fn hearts_on_songs_and_videos_toggle_favorites_by_click_and_by_h() {
    let mut r = Rig::new();
    with_videos(&mut r, 6);
    // A song's heart, left of its time.
    r.ui.show_view(View::Tracks);
    let id = match r.ents()[2].kind {
        super::EntKind::Track { id, .. } => id,
        k => panic!("{k:?}"),
    };
    let (x, y) = center(heart_of(&mut r, 2));
    assert_eq!(r.click(x, y), [Action::Lib(LibAction::ToggleFavorite(id))]);
    // A click on the row beside the heart is not the heart (it only selects).
    let row = r.ent_rect(2);
    assert!(r.click(row.x + row.w * 0.5, y).is_empty());
    // Ctrl+F hearts the selected row; H opens the help page instead.
    let ctrl = Modifiers { ctrl: true, ..Modifiers::default() };
    assert_eq!(r.key_mod(Key::Char('f'), ctrl), [Action::Lib(LibAction::ToggleFavorite(id))]);
    assert_eq!(r.key(Key::Char('h')), [Action::ShowHelp]);
    // A video poster has its heart in the top right corner of the poster, and a list row at its right end.
    r.ui.show_view(View::Videos);
    let vid = video_id(&mut r, 1);
    let (x, y) = center(heart_of(&mut r, 1));
    assert_eq!(r.click(x, y), [Action::Lib(LibAction::ToggleFavorite(vid))]);
    r.ui.lib.video_list = true;
    let (x, y) = center(heart_of(&mut r, 1));
    assert_eq!(r.click(x, y), [Action::Lib(LibAction::ToggleFavorite(vid))]);
    // The context menu says what the click would do.
    r.lib.set_favorite(vid, true);
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let items = super::menus::ent_menu(&r.ui, 1, &r.m, &ctx);
    assert!(items.iter().any(|m| m.label == "Remove from favorites"));
    // Albums and artists have no heart of their own.
    r.ui.show_view(View::Albums);
    let k = r.ents()[0].kind;
    assert!(super::rows::heart_rect(k, false, r.ent_rect(0), 1.0).is_none());
}

#[test]
fn the_favorites_view_lists_music_and_videos_and_plays_them_as_one_queue() {
    let mut r = Rig::new();
    with_videos(&mut r, 4);
    // Empty at first, with a message.
    r.key(Key::Char('9'));
    assert_eq!(r.ui.lib_state().view(), View::Favorites);
    assert!(r.ents().is_empty());
    let songs: Vec<u32> = r.lib.all_tracks().iter().take(3).map(|t| t.id).collect();
    let film = r.lib.all_videos()[1].id;
    for &s in &songs {
        r.lib.set_favorite(s, true);
    }
    r.lib.set_favorite(film, true);
    let ents = r.ents();
    assert_eq!(ents.len(), 4, "three songs and a film");
    assert!(matches!(ents[0].kind, super::EntKind::Track { .. }));
    assert!(matches!(ents[3].kind, super::EntKind::Video { .. }));
    // Double click: the list from there; the header buttons play all or shuffle.
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    assert_eq!(r.ui.scope_tracks(Scope::FavoriteTracks, &ctx, &r.m).len(), 3);
    assert_eq!(r.ui.scope_tracks(Scope::FavoriteVideos, &ctx, &r.m), [film]);
    assert_eq!(r.ui.scope_tracks(Scope::List, &ctx, &r.m).len(), 4);
    let g = r.ui.lib_geom(&r.m, &ctx);
    let names: Vec<_> = g.header_btns.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(names, ["Shuffle", "Play all"], "drawn right to left");
    let (x, y) = center(g.header_btns[1].rect);
    assert_eq!(r.click(x, y), [Action::Lib(LibAction::Play(Scope::ListFrom(0), Enqueue::Now))]);
    let (x, y) = center(g.header_btns[0].rect);
    assert_eq!(r.click(x, y), [Action::Lib(LibAction::Play(Scope::List, Enqueue::ShuffleNow))]);
    // It draws, and the rail has the entry.
    let _ = r.draw();
    let (x, y) = center(r.rail_item(View::Favorites));
    r.ui.show_view(View::Albums);
    r.click(x, y);
    assert_eq!(r.ui.lib_state().view(), View::Favorites);
}

#[test]
fn the_bar_and_the_now_playing_card_have_a_heart_for_what_is_playing() {
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let g = r.ui.lib_geom(&r.m, &ctx);
    let heart =
        g.bar_btns.iter().find(|(b, _)| *b == crate::ui::Btn::Favorite).expect("a heart in the bar").1;
    let (x, y) = center(heart);
    assert_eq!(r.click(x, y), [Action::ToggleFavorite]);
    // The card of the song that is playing.
    r.ui.show_view(View::NowPlaying);
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let g = r.ui.lib_geom(&r.m, &ctx);
    let hr = r.ui.now_playing_rects(&g, &r.m).heart.expect("a heart on the card");
    let (x, y) = center(hr);
    assert_eq!(r.click(x, y), [Action::ToggleFavorite]);
    // Nothing playing: no heart.
    r.m = model();
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let g = r.ui.lib_geom(&r.m, &ctx);
    assert!(g.bar_btns.iter().all(|(b, _)| *b != crate::ui::Btn::Favorite));
}

#[test]
fn about_rw_is_last_in_the_rail_and_opens_a_page_with_links_and_the_version() {
    for (w, h) in [(1280, 720), (800, 500)] {
        let mut r = Rig::new();
        r.ui.set_size(w, h, 1.0);
        r.m.version = "0.0.5".into();
        r.m.commit = "abc1234def".into();
        r.m.app.links = true;
        let (x, y) = {
            let ctx = LibCtx {
                lib: &r.lib,
                now_art: None,
                scan: None,
                viz: None,
                video: None,
                resume: crate::lib_ui::no_resume(),
            };
            let g = r.ui.lib_geom(&r.m, &ctx);
            assert!(g.about.y >= g.settings.bottom());
            center(g.about)
        };
        r.click(x, y);
        assert_eq!(r.ui.lib_state().view(), View::About, "{w}x{h}");
        // Three paragraphs fit in the body's block, and every button is on screen inside it.
        let ctx = LibCtx {
            lib: &r.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
        let g = r.ui.lib_geom(&r.m, &ctx);
        r.ui.ensure_rows(&r.m, &ctx, &g);
        let body = g.m.body;
        let btns = r.ui.about_page(None, body, &r.m);
        let ids: Vec<u8> = btns.iter().map(|b| b.id).collect();
        assert_eq!(
            ids,
            [0, 1, 2, 3, 4, 5],
            "Rusty Bucket, X, Licenses, Keyboard shortcuts, then X and GitHub in the closing line"
        );
        for b in &btns {
            assert!(b.rect.x >= body.x && b.rect.right() <= body.right(), "{w}x{h}: {b:?}");
        }
        // The block fits in the estimate that sets the page's height (so it can always be scrolled to the end).
        let total = r.ui.lib.rows.as_ref().unwrap().total;
        let last = btns.iter().map(|b| b.rect.bottom()).fold(0.0, f32::max) - body.y;
        assert!(last + 60.0 <= total, "{w}x{h}: buttons end at {last}, the page is {total} high");
        // A click on a button is an About action; the page draws.
        // Scroll the page until the button is on screen (a short window shows only part of it).
        let b = &btns[1];
        r.ui.lib.scroll = (b.rect.bottom() - body.bottom() + 24.0).max(0.0);
        let (bx, by) = center(b.rect);
        assert_eq!(r.click(bx, by - r.ui.lib.scroll), [Action::Lib(LibAction::About(1))]);
        // The two links of the closing line (suggestions, requests and bug reports) are words after the pills, X then GitHub.
        for (i, id) in [(4usize, 4u8), (5, 5)] {
            let w = &btns[i];
            assert_eq!(w.id, id);
            assert!(w.rect.y > btns[3].rect.bottom(), "{w:?} sits under the buttons");
            r.ui.lib.scroll = (w.rect.bottom() - body.bottom() + 24.0).max(0.0);
            let (wx, wy) = center(w.rect);
            assert_eq!(r.click(wx, wy - r.ui.lib.scroll), [Action::Lib(LibAction::About(id))]);
        }
        assert_eq!(btns[4].label, "X");
        assert_eq!(btns[5].label, "GitHub");
        assert!(btns[4].rect.x < btns[5].rect.x || btns[4].rect.y < btns[5].rect.y, "X comes before GitHub");
        let _ = r.draw();
        // F1 gets there from anywhere.
        r.ui.show_view(View::Albums);
        r.key(Key::Other("F1".into()));
        assert_eq!(r.ui.lib_state().view(), View::About);
    }
    // Where the host cannot open a link, the addresses are text and only Licenses and the shortcuts page are buttons.
    let mut r = Rig::new();
    r.ui.show_view(View::About);
    r.m.app.links = false;
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let g = r.ui.lib_geom(&r.m, &ctx);
    let ids: Vec<u8> = r.ui.about_page(None, g.m.body, &r.m).iter().map(|b| b.id).collect();
    assert_eq!(ids, [2, 3]);
}

// ---- the tag editor -------------------------------------------------------------------------------------------------------------

fn spec(read_only: Option<&str>) -> super::TagFormSpec {
    use super::{TagField as F, TagTarget};
    super::TagFormSpec {
        title: "Edit tags".into(),
        subtitle: "01 One.mp3".into(),
        target: TagTarget::Track(7),
        fields: vec![
            (F::Title, "Old title".into(), false),
            (F::Artist, "Old artist".into(), false),
            (F::Album, String::new(), true),
            (F::AlbumArtist, String::new(), false),
            (F::Genre, "Rock".into(), false),
            (F::TrackNo, "3".into(), false),
            (F::TrackTotal, "12".into(), false),
            (F::DiscNo, String::new(), false),
            (F::DiscTotal, String::new(), false),
            (F::Year, "1999".into(), false),
        ],
        art: 0,
        read_only: read_only.map(String::from),
    }
}

fn commands(r: &mut Rig) -> Vec<UiCommand> {
    r.ui.take_commands()
}

#[test]
fn the_tag_form_fits_every_window_keeps_to_digits_and_sends_only_what_changed() {
    for (w, h) in [(1280, 720), (800, 560), (480, 640), (320, 560)] {
        let mut r = Rig::new();
        r.ui.set_size(w, h, 1.0);
        r.ui.open_tag_form(spec(None));
        let l = r.ui.tagform_layout(w as f32, h as f32).unwrap();
        assert!(l.card.x >= -0.5 && l.card.right() <= w as f32 + 0.5, "{w}x{h}: {:?}", l.card);
        assert!(l.card.y >= -0.5, "{w}x{h}");
        // No two boxes overlap and every box and button is inside the card.
        for (i, a) in l.fields.iter().enumerate() {
            assert!(a.2.x >= l.card.x && a.2.right() <= l.card.right() + 0.5, "{w}x{h}: box {i} {:?}", a.2);
            for b in l.fields.iter().skip(i + 1) {
                let (p, q) = (a.2, b.2);
                assert!(
                    p.right() <= q.x + 0.5
                        || q.right() <= p.x + 0.5
                        || p.bottom() <= q.y + 0.5
                        || q.bottom() <= p.y + 0.5,
                    "{w}x{h}: boxes overlap"
                );
            }
        }
        for b in &l.buttons {
            assert!(b.rect.x >= l.card.x - 0.5 && b.rect.right() <= l.card.right() + 0.5, "{w}x{h}: {b:?}");
        }
        r.draw();
    }
    let mut r = Rig::new();
    r.ui.open_tag_form(spec(None));
    assert!(r.ui.tag_form_open() && r.ui.lib_state().typing());
    // Typing goes to the focused box; Tab moves; digits only in a number box.
    r.typed("!");
    r.key(Key::Other("Tab".into()));
    r.key(Key::Other("Backspace".into()));
    r.typed("Z");
    for _ in 0..4 {
        r.key(Key::Other("Tab".into()));
    }
    r.typed("a7");
    r.key_mod(Key::Other("Tab".into()), Modifiers { shift: true, ..Modifiers::default() });
    r.typed("x"); // back on the genre
    // Nothing is sent until Save; Enter is Save.
    assert!(commands(&mut r).is_empty());
    r.key(Key::Enter);
    let cmds = commands(&mut r);
    assert_eq!(cmds.len(), 1);
    let UiCommand::SaveTags { target, changes, cover } = &cmds[0] else { panic!("{cmds:?}") };
    assert_eq!(*target, super::TagTarget::Track(7));
    assert_eq!(*cover, super::CoverAction::Keep);
    use super::TagField as F;
    assert_eq!(
        changes,
        &[
            (F::Title, Some("Old title!".into())),
            (F::Artist, Some("Old artisZ".into())),
            (F::Genre, Some("Rockx".into())),
            (F::TrackNo, Some("37".into())),
        ]
    );
    assert!(!r.ui.tag_form_open());
}

#[test]
fn the_tag_form_clears_a_box_cancels_and_closes_read_only_with_the_reason() {
    use super::TagField as F;
    let mut r = Rig::new();
    r.ui.open_tag_form(spec(None));
    for _ in 0..9 {
        r.key(Key::Other("Backspace".into()));
    }
    r.key(Key::Enter);
    let cmds = commands(&mut r);
    let UiCommand::SaveTags { changes, .. } = &cmds[0] else { panic!("{cmds:?}") };
    assert_eq!(changes, &[(F::Title, None)], "an emptied box clears the tag");
    // Escape cancels; saving without a change sends nothing.
    r.ui.open_tag_form(spec(None));
    r.typed("abc");
    r.key(Key::Escape);
    assert!(!r.ui.tag_form_open() && commands(&mut r).is_empty());
    r.ui.open_tag_form(spec(None));
    r.key(Key::Enter);
    assert!(!r.ui.tag_form_open() && commands(&mut r).is_empty(), "nothing changed, nothing to do");
    // A pasted line goes into the focused box, without control characters, and only digits into a number box.
    r.ui.open_tag_form(spec(None));
    r.handle(InputEvent::Paste("  more\nlines\t".into()));
    r.key(Key::Enter);
    let cmds = commands(&mut r);
    let UiCommand::SaveTags { changes, .. } = &cmds[0] else { panic!("{cmds:?}") };
    assert_eq!(changes, &[(F::Title, Some("Old title  morelines".into()))]);
    // Read-only: the reason is on the card, typing does nothing, Enter and the one button close it.
    r.ui.open_tag_form(spec(Some("This browser cannot change files.")));
    r.typed("zzz");
    r.draw();
    let l = r.ui.tagform_layout(1280.0, 720.0).unwrap();
    assert_eq!(l.buttons.len(), 1, "just Close");
    let (x, y) = center(l.buttons[0].rect);
    r.click(x, y);
    assert!(!r.ui.tag_form_open() && commands(&mut r).is_empty());
    r.ui.open_tag_form(spec(Some("no")));
    r.key(Key::Enter);
    assert!(!r.ui.tag_form_open());
}

#[test]
fn the_tag_form_buttons_and_boxes_answer_the_pointer() {
    let mut r = Rig::new();
    r.ui.open_tag_form(spec(None));
    let l = r.ui.tagform_layout(1280.0, 720.0).unwrap();
    // A click in a box focuses it; the keys then go there.
    let artist = l.fields[1].2;
    r.click(artist.cx(), artist.cy());
    r.typed("!");
    // Replace asks the app for a picture; Remove toggles; Save sends it all.
    let by = |id: u8| l.buttons.iter().find(|b| b.id == id).unwrap().rect;
    r.click(by(2).cx(), by(2).cy());
    assert_eq!(commands(&mut r), [UiCommand::PickCover]);
    r.ui.set_tag_cover(Some("front.jpg".into()));
    r.click(by(3).cx(), by(3).cy());
    r.click(by(3).cx(), by(3).cy());
    r.click(by(0).cx(), by(0).cy());
    let cmds = commands(&mut r);
    let UiCommand::SaveTags { changes, cover, .. } = &cmds[0] else { panic!("{cmds:?}") };
    assert_eq!(changes, &[(super::TagField::Artist, Some("Old artist!".into()))]);
    assert_eq!(
        *cover,
        super::CoverAction::Keep,
        "Remove twice is not Remove, and it drops the chosen picture"
    );
    // While it is open nothing behind it takes clicks or the right button, and Cancel closes it.
    r.ui.open_tag_form(spec(None));
    let l = r.ui.tagform_layout(1280.0, 720.0).unwrap();
    r.right_click(10.0, 300.0);
    assert!(!r.ui.menu_open());
    let c = l.buttons.iter().find(|b| b.id == 1).unwrap().rect;
    r.click(c.cx(), c.cy());
    assert!(!r.ui.tag_form_open());
}

// ---- tooltips ---------------------------------------------------------------------------------------------------------------------

/// Draw a frame (so the geometry and the visible rows are current) and find every control: the first point of each distinct thing the
/// pointer can be over, found by walking the window on a grid.
fn controls_on_screen(r: &mut Rig, step: f32) -> Vec<(LibHit, (f32, f32))> {
    let _ = r.draw();
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let g = r.ui.lib_geom(&r.m, &ctx);
    let (w, h) = r.ui.size();
    let mut seen: Vec<(LibHit, (f32, f32))> = Vec::new();
    let mut y = 2.0;
    while y < h as f32 {
        let mut x = 2.0;
        while x < w as f32 {
            let hit = r.ui.lib_hit(x, y, &g, &r.m, &ctx);
            // Rows and cards repeat the same controls: three of each kind is enough.
            let many = matches!(hit, LibHit::Ent(_) | LibHit::EntPlay(_) | LibHit::EntHeart(_));
            let same = seen
                .iter()
                .filter(|(k, _)| core::mem::discriminant(k) == core::mem::discriminant(&hit))
                .count();
            if hit != LibHit::None && !seen.iter().any(|(k, _)| *k == hit) && (!many || same < 3) {
                seen.push((hit, (x, y)));
            }
            x += step;
        }
        y += step;
    }
    seen
}

fn assert_all_have_tips(r: &mut Rig, what: &str) -> usize {
    // An 18 px grid finds every control (the smallest is 28 px).
    let controls = controls_on_screen(r, 18.0);
    let (w, h) = r.ui.size();
    for (hit, (x, y)) in &controls {
        if matches!(hit, LibHit::Menu(..)) {
            continue; // menu rows carry their own label and key
        }
        let ctx = LibCtx {
            lib: &r.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
        let (text, key) =
            r.ui.lib_tip_text(*hit, &r.m, &ctx)
                .unwrap_or_else(|| panic!("{what}: {hit:?} at ({x}, {y}) has no tooltip"));
        assert!(text.chars().count() >= 8, "{what}: {hit:?}: {text:?}");
        assert!(key.chars().count() <= 14, "{what}: {hit:?}: key {key:?}");
        let a =
            r.ui.lib_tip_anchor(*hit, &r.m, &ctx)
                .unwrap_or_else(|| panic!("{what}: {hit:?} has no place to point at"));
        assert!(
            a.w > 0.0 && a.h > 0.0 && a.right() > 0.0 && a.x < w as f32 && a.bottom() > 0.0 && a.y < h as f32,
            "{what}: {hit:?} anchor {a:?}"
        );
        // Drawn next to it, whatever its size, wrapped and inside the window.
        let mut fb = FrameBuffer::new(w, h);
        r.ui.draw_tip_box(&mut fb, &crate::tips::Tip { text, key, anchor: a }, w as f32, h as f32);
    }
    controls.len()
}

#[test]
fn every_control_of_every_view_has_a_tooltip_that_says_what_it_does() {
    for (w, h) in [(1280u32, 720u32), (800, 560)] {
        let floor = if w > 1000 { 150 } else { 80 };
        let mut total = 0;
        let mut r = Rig::new();
        r.ui.set_size(w, h, 1.0);
        with_videos(&mut r, 5);
        let songs: Vec<u32> = r.lib.all_tracks().iter().take(3).map(|t| t.id).collect();
        for s in &songs {
            r.lib.set_favorite(*s, true);
        }
        let film = r.lib.all_videos()[0].id;
        r.lib.set_favorite(film, true);
        let pl = r.lib.create_playlist("Mix");
        r.lib.playlist_add(pl, &songs);
        r.m = playing_model(&r.lib);
        r.m.app.tags = true;
        r.m.app.links = true;
        let (album, artist) = (r.lib.albums()[0].id, r.lib.artists()[0].id);
        for view in [
            View::Albums,
            View::Artists,
            View::Tracks,
            View::Videos,
            View::Favorites,
            View::Playlists,
            View::Queue,
            View::NowPlaying,
            View::Visualizer,
            View::About,
        ] {
            r.ui.show_view(view);
            total += assert_all_have_tips(&mut r, &alloc::format!("{w}x{h} {view:?}"));
        }
        // The same views as a list, with a query, with something open inside them.
        r.ui.lib.video_list = true;
        r.ui.show_view(View::Videos);
        total += assert_all_have_tips(&mut r, "video list");
        r.ui.show_view(View::Search);
        r.typed("song");
        total += assert_all_have_tips(&mut r, "search results");
        r.ui.show_view(View::Albums);
        for d in [Detail::Album(album), Detail::Artist(artist), Detail::Playlist(pl)] {
            r.ui.open_detail(d);
            total += assert_all_have_tips(&mut r, &alloc::format!("{d:?}"));
            r.ui.go_back(
                &r.m.clone(),
                &LibCtx {
                    lib: &r.lib,
                    now_art: None,
                    scan: None,
                    viz: None,
                    video: None,
                    resume: crate::lib_ui::no_resume(),
                },
            );
        }
        // The tag editor, the name prompt, an empty library with its buttons.
        r.ui.open_tag_form(spec(None));
        total += assert_all_have_tips(&mut r, "tag editor");
        r.ui.lib.tagform = None;
        r.ui.ask_playlist_name(None);
        total += assert_all_have_tips(&mut r, "name prompt");
        r.ui.lib.prompt = None;
        let mut empty = Rig::new();
        empty.ui.set_size(w, h, 1.0);
        empty.lib = Library::new();
        empty.ui.show_view(View::Albums);
        total += assert_all_have_tips(&mut empty, "the empty library");
        assert!(total > floor, "{w}x{h}: only {total} controls were found: the walk is not walking");
    }
}

#[test]
fn tooltips_wait_a_moment_follow_the_keyboard_go_away_when_off_and_stay_in_the_window() {
    let mut r = Rig::new();
    r.m = playing_model(&r.lib);
    r.ui.show_view(View::Albums);
    let _ = r.draw();
    let (x, y) = center(r.rail_item(View::Tracks));
    r.handle(InputEvent::PointerMove { x, y });
    let ctx = || LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    assert_eq!(r.ui.lib_tip_target(), Some(LibHit::Rail(View::Tracks)));
    // Not at once; after the delay, and a redraw is asked for.
    assert!(!r.ui.lib_tip_tick(r.now));
    assert!(r.ui.lib_tip_now(&r.m, &ctx()).is_none());
    r.now += crate::ui::TOOLTIP_DELAY_US + 1000;
    assert!(r.ui.lib_tip_tick(r.now), "a redraw is due to show it");
    r.ui.now = r.now;
    let tip = r.ui.lib_tip_now(&r.m, &ctx()).expect("shown after the delay");
    assert!(tip.text.starts_with("Tracks") && tip.key == "3", "{tip:?}");
    // It goes away with the pointer, with an open menu and with the setting.
    r.handle(InputEvent::PointerMove { x: 1270.0, y: 5.0 });
    assert_eq!(r.ui.lib_tip_target(), None);
    r.handle(InputEvent::PointerMove { x, y });
    r.right_click(x, y);
    assert!(r.ui.menu_open() && r.ui.lib_tip_target().is_none());
    r.key(Key::Escape);
    r.handle(InputEvent::PointerMove { x: x + 4.0, y });
    assert!(r.ui.lib_tip_target().is_some());
    r.ui.set_tooltips(false);
    assert!(r.ui.lib_tip_target().is_none() && !r.ui.tooltips());
    r.ui.set_tooltips(true);
    // The keyboard: the rail's focus, the bar's focus, a selected row and the search box have tooltips too.
    r.key(Key::Other("Tab".into())); // content -> rail
    let _ = r.draw();
    assert!(matches!(r.ui.lib_tip_target(), Some(LibHit::Rail(_))));
    r.key(Key::Other("Tab".into())); // rail -> bar
    let _ = r.draw();
    assert!(matches!(r.ui.lib_tip_target(), Some(LibHit::Bar(_))));
    r.key(Key::Other("Tab".into()));
    r.key(Key::Down);
    let _ = r.draw();
    assert!(matches!(r.ui.lib_tip_target(), Some(LibHit::Ent(_))));
    r.key(Key::Char('/'));
    let _ = r.draw();
    assert_eq!(r.ui.lib_tip_target(), Some(LibHit::Search));
    // A tooltip is kept inside the window wherever its control is, and long text is wrapped at 140 characters.
    let (w, h) = r.ui.size();
    let long = "word ".repeat(120);
    for a in [
        crate::gfx::RectF::new(0.0, 0.0, 30.0, 30.0),
        crate::gfx::RectF::new(w as f32 - 30.0, h as f32 - 30.0, 30.0, 30.0),
        crate::gfx::RectF::new(600.0, 300.0, 30.0, 30.0),
    ] {
        let mut fb = FrameBuffer::new(w, h);
        r.ui.draw_tip_box(
            &mut fb,
            &crate::tips::Tip { text: long.clone(), key: "Shift+V".into(), anchor: a },
            w as f32,
            h as f32,
        );
    }
    for l in crate::tips::wrap_chars(&long, crate::tips::TIP_WIDTH_CHARS) {
        assert!(l.chars().count() <= 140, "{l:?}");
    }
    assert_eq!(
        crate::tips::wrap_chars(&"x".repeat(300), 140).iter().map(|l| l.chars().count()).collect::<Vec<_>>(),
        [140, 140, 20]
    );
    assert_eq!(crate::tips::wrap_chars("a b  c\n d", 140), ["a b c d"]);
    assert!(crate::tips::wrap_chars("", 140).is_empty());
}

#[test]
fn the_player_bar_the_dialogs_and_the_audio_panel_have_tooltips_for_every_control() {
    use crate::ui::{Btn, Target};
    let mut ui = Ui::new(UiConfig { reduce_motion: true });
    ui.set_size(1280, 720, 1.0);
    let m = UiModel { state: MediaState::Playing, volume: 0.5, rate: 1.0, ..UiModel::default() };
    for b in [
        Btn::Play,
        Btn::Back,
        Btn::Fwd,
        Btn::Mute,
        Btn::Speed,
        Btn::Tracks,
        Btn::Playlist,
        Btn::Open,
        Btn::Fullscreen,
        Btn::Welcome,
        Btn::Prev,
        Btn::Next,
        Btn::Shuffle,
        Btn::Repeat,
        Btn::QueueView,
        Btn::VizView,
        Btn::ModeSwitch,
        Btn::Favorite,
    ] {
        let (text, _) =
            ui.play_tip_text(Target::Btn(b), &m).unwrap_or_else(|| panic!("{b:?} has no tooltip"));
        assert!(text.len() >= 8, "{b:?}");
    }
    assert!(ui.play_tip_text(Target::Seek, &m).is_some() && ui.play_tip_text(Target::Volume, &m).is_some());
    // A dialog's switches and buttons: hover each (after the delay) and a tooltip comes.
    let mut spec = crate::dialog::DialogSpec { title: "Settings".into(), ..Default::default() };
    spec.toggles.push(crate::dialog::DialogToggle {
        label: "Show tooltips".into(),
        desc: "Notes on controls.".into(),
        on: true,
    });
    spec.buttons.push(crate::dialog::DialogButton::new("Close", true));
    spec.buttons.push(crate::dialog::DialogButton::new("Theme\u{2026}", false));
    let m = UiModel { dialog: Some(spec), ..m };
    for c in [
        crate::dialog::DialogControl::Toggle(0),
        crate::dialog::DialogControl::Button(0),
        crate::dialog::DialogControl::Button(1),
    ] {
        ui.dialog.hover = Some(c);
        ui.keyboard_mode = false;
        ui.overlay_tip_tick(&m, 0);
        assert!(ui.dialog_tip_now(&m).is_none(), "{c:?} not at once");
        ui.now = crate::ui::TOOLTIP_DELAY_US + 10;
        assert!(ui.overlay_tip_tick(&m, ui.now), "{c:?}");
        let tip = ui.dialog_tip_now(&m).unwrap_or_else(|| panic!("{c:?} has no tooltip"));
        assert!(!tip.text.is_empty());
        ui.now = 0;
    }
    ui.set_tooltips(false);
    ui.dialog.hover = Some(crate::dialog::DialogControl::Button(0));
    assert!(!ui.overlay_tip_tick(&m, 10_000_000) && ui.dialog_tip_now(&m).is_none());
    ui.set_tooltips(true);
    // The audio panel: every control.
    ui.open_audio_settings();
    let m = UiModel { dialog: None, ..m };
    for c in [
        crate::audio_panel::AudioControl::Crossfade,
        crate::audio_panel::AudioControl::CrossfadeLength,
        crate::audio_panel::AudioControl::AutoLevel,
        crate::audio_panel::AudioControl::Target,
        crate::audio_panel::AudioControl::Mode,
        crate::audio_panel::AudioControl::Done,
    ] {
        if let Some(p) = &mut ui.audio_panel {
            p.hover = Some(c);
        }
        ui.keyboard_mode = false;
        ui.now = 0;
        ui.overlay_tip_tick(&m, 0);
        ui.now = crate::ui::TOOLTIP_DELAY_US + 10;
        assert!(ui.overlay_tip_tick(&m, ui.now), "{c:?}");
        assert!(ui.audio_tip_now().is_some(), "{c:?} has no tooltip");
    }
}

#[test]
fn the_history_view_groups_plays_by_day_and_removes_a_row_on_delete() {
    let mut r = Rig::new();
    with_videos(&mut r, 2);
    let now = 1_791_374_400i64;
    r.lib.set_calendar(now, 0);
    let songs: Vec<u32> = r.lib.all_tracks().iter().take(2).map(|t| t.id).collect();
    let film = r.lib.all_videos()[0].id;
    r.lib.record_play(songs[0], now - 5 * 86_400, 45_000, true);
    r.lib.record_play(songs[1], now - 86_400 - 100, 31_000, false);
    r.lib.record_play(songs[0], now - 100, 40_000, true);
    r.lib.record_play(film, now - 50, 60_000, false);
    // The rail has it and so does the key 0; with nothing played the view says so.
    r.key(Key::Char('0'));
    assert_eq!(r.ui.lib_state().view(), View::History);
    let ents = r.ents();
    assert_eq!(ents.len(), 4, "three song plays and a film play");
    assert!(matches!(ents[0].kind, super::EntKind::Track { id, .. } if id == songs[0]), "newest first");
    assert!(matches!(ents[1].kind, super::EntKind::Track { id, .. } if id == songs[1]));
    assert!(matches!(ents[3].kind, super::EntKind::Video { .. }));
    let rows = r.ui.lib.rows.as_ref().unwrap();
    let heads: Vec<&str> = rows
        .rows
        .iter()
        .filter_map(|x| match &x.kind {
            super::rows::RowKind::Header(h) => Some(h.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(heads, ["Music (3 plays)", "Today", "Yesterday", "2 Oct 2026", "Videos (1 play)", "Today"]);
    assert_eq!(rows.hist[&0].count, 2, "the song was played twice");
    assert_eq!(rows.hist[&2].count, 2);
    assert_eq!(rows.hist[&1].count, 1);
    // The context menu offers to remove the row; Delete does the same.
    let seq = rows.hist[&1].seq;
    {
        let ctx = LibCtx {
            lib: &r.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
        let items = super::menus::ent_menu(&r.ui, 1, &r.m, &ctx);
        assert!(items.iter().any(|m| m.label == "Remove from history"));
    }
    r.ui.lib.sel = Some(1);
    r.ui.lib.zone = super::Zone::Content;
    assert_eq!(r.key(Key::Other("Delete".into())), [Action::Lib(LibAction::RemoveFromHistory(seq))]);
    // The header's button asks to clear.
    let rect = {
        let ctx = LibCtx {
            lib: &r.lib,
            now_art: None,
            scan: None,
            viz: None,
            video: None,
            resume: crate::lib_ui::no_resume(),
        };
        let g = r.ui.lib_geom(&r.m, &ctx);
        let names: Vec<_> = g.header_btns.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(names, ["Clear history"]);
        g.header_btns[0].rect
    };
    let (x, y) = center(rect);
    assert_eq!(r.click(x, y), [Action::Lib(LibAction::ClearHistory)]);
    let _ = r.draw();
    // Sorted by plays, the song rows say how often they were played.
    r.ui.show_view(View::Tracks);
    r.ui.lib.track_sort = rvp_library::TrackSort::Plays;
    let _ = r.draw();
}

#[test]
fn an_empty_history_says_so() {
    let mut r = Rig::new();
    r.key(Key::Char('0'));
    assert_eq!(r.ui.lib_state().view(), View::History);
    assert!(r.ents().is_empty());
    let _ = r.draw();
}

// ---- the phone layout ------------------------------------------------------------------------------------------------------

/// The windows a phone has in portrait (CSS pixels at scale 1; the 3x one is the same screen with three times the pixels).
const PHONES: [(u32, u32, f32); 3] = [(390, 844, 1.0), (360, 800, 1.0), (1170, 2532, 3.0)];

fn phone_rig(w: u32, h: u32, s: f32) -> Rig {
    let mut r = Rig::new();
    r.ui.set_size(w, h, s);
    r
}

fn geom_of(r: &mut Rig) -> super::geom::Geom {
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    r.ui.lib_geom(&r.m, &ctx)
}

fn inside(r: crate::gfx::RectF, w: f32, h: f32) -> bool {
    r.x >= -0.5 && r.y >= -0.5 && r.right() <= w + 0.5 && r.bottom() <= h + 0.5
}

fn overlap(a: crate::gfx::RectF, b: crate::gfx::RectF) -> bool {
    a.x < b.right() - 0.5 && b.x < a.right() - 0.5 && a.y < b.bottom() - 0.5 && b.y < a.bottom() - 0.5
}

#[test]
fn a_phone_gets_a_header_menu_a_touch_sized_transport_and_no_rail_beside_the_content() {
    for (w, h, s) in PHONES {
        let mut r = phone_rig(w, h, s);
        let g = geom_of(&mut r);
        let (sw, sh) = (w as f32, h as f32);
        assert!(g.m.phone, "{w}x{h}");
        assert_eq!(g.m.rail.w, 0.0, "the rail is a drawer, closed");
        assert!(g.nav.is_empty(), "nothing in the rail can be hit while it is closed");
        assert_eq!(g.m.body.x, 0.0);
        assert_eq!(g.m.body.w, sw, "the content has the whole width");
        // The menu button, at least 44 px and in the header.
        let menu = g.menu_btn.expect("a menu button");
        assert!(menu.w >= 44.0 * s - 0.5 && menu.h >= 44.0 * s - 0.5 && inside(menu, sw, sh), "{menu:?}");
        // The transport: every control at least 44 px, on screen, none over another, and the bar inside the window.
        assert!(inside(g.m.bar, sw, sh) && g.m.bar.bottom() <= sh + 0.5);
        for (b, rect) in &g.bar_btns {
            assert!(rect.w >= 44.0 * s - 0.5 && rect.h >= 44.0 * s - 0.5, "{b:?} is {rect:?}");
            assert!(inside(*rect, sw, sh) && rect.y >= g.m.bar.y, "{b:?} is {rect:?}");
        }
        for (i, (a, ra)) in g.bar_btns.iter().enumerate() {
            for (b, rb) in &g.bar_btns[i + 1..] {
                assert!(!overlap(*ra, *rb), "{a:?} and {b:?} overlap at {w}x{h}");
            }
        }
        for want in
            [crate::ui::Btn::Prev, crate::ui::Btn::Play, crate::ui::Btn::Next, crate::ui::Btn::ModeSwitch]
        {
            assert!(g.bar_btns.iter().any(|(b, _)| *b == want), "{want:?} is missing");
        }
        assert!(g.seek_hit.h >= 30.0 * s && inside(g.seek_hit, sw, sh));
        // The header: the title starts after the menu button, the search and the buttons are inside and apart.
        assert!(g.title_x >= menu.right());
        assert!(inside(g.search, sw, sh));
        for b in &g.header_btns {
            assert!(
                inside(b.rect, sw, sh) && b.rect.w >= 44.0 * s - 0.5 && !overlap(b.rect, g.search),
                "{b:?}"
            );
        }
    }
}

#[test]
fn the_drawer_opens_from_the_menu_button_and_closes_on_a_choice_a_tap_outside_or_escape() {
    for (w, h, s) in PHONES {
        let mut r = phone_rig(w, h, s);
        let (sw, sh) = (w as f32, h as f32);
        let g = geom_of(&mut r);
        let (x, y) = center(g.menu_btn.unwrap());
        r.click(x, y);
        let g = geom_of(&mut r);
        assert!(g.m.rail.w > 0.0 && g.m.rail.w <= sw * 0.9, "the drawer is open: {:?}", g.m.rail);
        assert!(inside(g.m.rail, sw, sh));
        assert!(g.nav.len() >= 10, "{} entries", g.nav.len());
        for (v, rect) in &g.nav {
            assert!(inside(*rect, sw, sh) && rect.right() <= g.m.rail.right() + 0.5, "{v:?} {rect:?}");
            assert!(rect.h >= 22.0 * s, "{v:?} is {} px tall", rect.h);
        }
        // Choosing a view goes there and closes the drawer.
        let (_, tracks) = g.nav.iter().find(|(v, _)| *v == View::Tracks).copied().unwrap();
        let (x, y) = center(tracks);
        r.click(x, y);
        assert_eq!(r.ui.lib_state().view(), View::Tracks);
        assert_eq!(geom_of(&mut r).m.rail.w, 0.0, "closed after a choice");
        // A tap on the dimmed content closes it without doing anything else; so does Escape.
        let g = geom_of(&mut r);
        r.click(center(g.menu_btn.unwrap()).0, center(g.menu_btn.unwrap()).1);
        assert!(geom_of(&mut r).m.rail.w > 0.0);
        r.click(sw - 6.0 * s, sh * 0.5);
        assert_eq!(geom_of(&mut r).m.rail.w, 0.0, "a tap outside closes it");
        assert_eq!(r.ui.lib_state().view(), View::Tracks, "and changes nothing");
        let g = geom_of(&mut r);
        r.click(center(g.menu_btn.unwrap()).0, center(g.menu_btn.unwrap()).1);
        r.key(Key::Escape);
        assert_eq!(geom_of(&mut r).m.rail.w, 0.0, "Escape closes it");
        // Wider than a phone it is a rail again.
        r.ui.set_size(1280, 720, 1.0);
        assert!(!geom_of(&mut r).m.phone);
    }
}

#[test]
fn every_view_fits_a_phone_width_and_the_toast_and_the_error_card_wrap_inside_it() {
    for (w, h, s) in PHONES {
        let mut r = phone_rig(w, h, s);
        let (sw, sh) = (w as f32, h as f32);
        for view in [
            View::Albums,
            View::Artists,
            View::Tracks,
            View::Videos,
            View::Favorites,
            View::History,
            View::Playlists,
            View::Queue,
            View::Search,
            View::NowPlaying,
            View::About,
        ] {
            r.ui.show_view(view);
            let g = geom_of(&mut r);
            assert!(inside(g.m.body, sw, sh), "{view:?} body {:?}", g.m.body);
            assert!(g.m.body.bottom() <= g.m.bar.y + 0.5, "{view:?}: the body ends where the bar begins");
            for e in r.ents() {
                let _ = e;
            }
            for i in 0..r.ents().len().min(12) {
                let rect = r.ent_rect(i);
                assert!(
                    rect.x >= -0.5 && rect.right() <= sw + 0.5,
                    "{view:?} entity {i} is {rect:?} in {sw} px"
                );
            }
            let _ = r.draw();
        }
        // A long toast wraps and stays inside the window, above the bar.
        let long = "No picture: this video is HEVC (H.265), and WebCodecs can't decode it here (this browser has no HEVC decoder). Convert it to H.264 or AV1 to watch it here. The sound plays on.";
        let l = r.ui.lib_layout();
        let (lines, rect) = r.ui.toast_layout(long, &l);
        assert!(lines.len() >= 3, "{} lines", lines.len());
        assert!(rect.x >= 8.0 * s - 0.5 && rect.right() <= sw - 8.0 * s + 0.5, "{rect:?} in {sw}");
        assert!(rect.y >= 0.0 && rect.bottom() <= g_bar_top(&mut r) + 0.5, "{rect:?}");
        // A short one is still a single pill.
        let (lines, rect) = r.ui.toast_layout("Muted", &l);
        assert_eq!(lines.len(), 1);
        assert!(rect.w < 140.0 * s);
        // The error card.
        r.m.state = MediaState::Failed;
        r.m.error = Some(long.repeat(2));
        let (lines, card) = r.ui.error_layout(&r.m, &l);
        assert!(lines.len() >= 5);
        assert!(
            card.x >= 15.0 * s && card.right() <= sw - 15.0 * s && card.y >= 0.0 && card.bottom() <= sh,
            "{card:?}"
        );
    }
}

fn g_bar_top(r: &mut Rig) -> f32 {
    geom_of(r).m.bar.y
}

#[test]
fn menus_and_dialogs_stay_inside_a_phone_window() {
    for (w, h, s) in PHONES {
        let mut r = phone_rig(w, h, s);
        let (sw, sh) = (w as f32, h as f32);
        // A context menu opened at the far corner is moved back inside the window.
        r.ui.show_view(View::Tracks);
        let _ = r.ents();
        for (x, y) in [(sw - 4.0 * s, sh * 0.4), (sw * 0.5, sh - 160.0 * s), (4.0 * s, 100.0 * s)] {
            r.right_click(x, y);
            for p in &r.ui.menu {
                assert!(p.rect.x >= -0.5 && p.rect.right() <= sw + 0.5, "menu {:?} in {sw}", p.rect);
                assert!(p.rect.y >= -0.5 && p.rect.bottom() <= sh + 0.5, "menu {:?} in {sh}", p.rect);
            }
            r.key(Key::Escape);
        }
        let _ = r.draw();
    }
}

// ---- help, favorites on Ctrl+F, quit -----------------------------------------------------------------------------------------------

#[test]
fn h_and_question_mark_open_help_ctrl_f_no_longer_searches_and_ctrl_q_needs_a_host_that_can_quit() {
    let mut r = Rig::new();
    r.ui.show_view(View::Albums);
    assert_eq!(r.key(Key::Char('h')), [Action::ShowHelp]);
    assert_eq!(r.key(Key::Char('?')), [Action::ShowHelp]);
    let ctrl = Modifiers { ctrl: true, ..Modifiers::default() };
    // Ctrl+F is the heart now: nothing selected in Albums, so the player's own heart (what is playing) answers.
    r.m = playing_model(&r.lib);
    let out = r.key_mod(Key::Char('f'), ctrl);
    assert_eq!(out, [Action::ToggleFavorite]);
    assert_ne!(r.ui.lib_state().view(), View::Search, "Ctrl+F is not search");
    // `/` is.
    r.key(Key::Char('/'));
    assert_eq!(r.ui.lib_state().view(), View::Search);
    r.key(Key::Escape);
    // Ctrl+Q is claimed only where the host can quit (a browser tab leaves it to the browser).
    assert!(r.key_mod(Key::Char('q'), ctrl).is_empty());
    r.m.app.quit = true;
    assert_eq!(r.key_mod(Key::Char('q'), ctrl), [Action::Quit]);
    // Plain Q still shows the queue.
    assert_eq!(r.key(Key::Char('q')), [Action::ShowPlaylist]);
}

#[test]
fn every_library_key_on_the_help_page_still_does_something() {
    for k in crate::help::library_keys() {
        let Some((key, mods)) = k.probe.clone() else { continue };
        let mut r = Rig::new();
        r.ui.show_view(View::Albums);
        let before = r.ui.lib_state().view();
        let out = r.key_mod(key.clone(), mods);
        let after = r.ui.lib_state().view();
        assert!(!out.is_empty() || before != after, "{} ({key:?}) does nothing in the library", k.keys);
    }
}

#[test]
fn the_about_pages_shortcuts_button_and_the_menus_open_help() {
    let mut r = Rig::new();
    r.ui.show_view(View::About);
    r.m.app.links = true;
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let g = r.ui.lib_geom(&r.m, &ctx);
    let btn = r.ui.about_page(None, g.m.body, &r.m).into_iter().find(|b| b.id == 3).expect("the button");
    assert!(btn.label.contains("Keyboard"));
    r.ui.lib.scroll = (btn.rect.bottom() - g.m.body.bottom() + 24.0).max(0.0);
    let out = r.click(btn.rect.cx(), btn.rect.cy() - r.ui.lib.scroll);
    assert_eq!(out, [Action::ShowHelp]);
    // The library's general menu and the player's menu both have it.
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let mut reach = Vec::new();
    menu_actions(&super::menus::global_menu(&r.ui, &r.m, &ctx), &mut reach);
    assert!(reach.contains(&Action::ShowHelp));
    let mut reach = Vec::new();
    menu_actions(&crate::actions::context_menu(&r.m), &mut reach);
    assert!(reach.contains(&Action::ShowHelp));
}

#[test]
fn while_help_is_up_the_library_keys_do_nothing_else() {
    let mut r = Rig::new();
    r.ui.show_view(View::Albums);
    r.ui.toggle_help();
    assert!(r.key(Key::Char('3')).is_empty());
    assert_eq!(r.ui.lib_state().view(), View::Albums, "digits do not change the view behind the page");
    assert!(r.ui.help_open());
    r.key(Key::Char('?'));
    assert!(!r.ui.help_open());
    assert_eq!(r.key(Key::Char('3')), []);
    assert_eq!(r.ui.lib_state().view(), View::Tracks);
}

#[test]
fn the_phone_transport_has_the_five_in_order_at_44_px_and_fits_the_width() {
    use crate::ui::Btn;
    for (w, h, s) in PHONES {
        let mut r = phone_rig(w, h, s);
        r.m = playing_model(&r.lib);
        let g = geom_of(&mut r);
        let mut row: Vec<(Btn, RectF)> = g
            .bar_btns
            .iter()
            .copied()
            .filter(|(b, _)| matches!(b, Btn::Prev | Btn::Back | Btn::Play | Btn::Fwd | Btn::Next))
            .collect();
        row.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
        assert_eq!(
            row.iter().map(|(b, _)| *b).collect::<Vec<_>>(),
            [Btn::Prev, Btn::Back, Btn::Play, Btn::Fwd, Btn::Next]
        );
        for (b, rect) in &g.bar_btns {
            assert!(rect.x >= 0.0 && rect.right() <= w as f32, "{b:?} {rect:?} in {w}");
            assert!(rect.w >= 44.0 * s - 0.5 && rect.h >= 44.0 * s - 0.5, "{b:?} is {rect:?} at {s}x");
        }
        // They do not overlap.
        for (i, (b, a)) in g.bar_btns.iter().enumerate() {
            for (b2, c) in &g.bar_btns[i + 1..] {
                assert!(
                    a.right() <= c.x + 0.01
                        || c.right() <= a.x + 0.01
                        || a.bottom() <= c.y + 0.01
                        || c.bottom() <= a.y + 0.01,
                    "{b:?} overlaps {b2:?}"
                );
            }
        }
    }
}

// ---- collapsing the rail -----------------------------------------------------------------------------------------------------------

#[test]
fn the_rail_collapses_to_icons_keeps_every_entry_and_a_button_brings_it_back() {
    let mut r = Rig::new();
    let (expanded, n) = {
        let g = geom_of(&mut r);
        assert!(g.rail_toggle.is_some(), "there is a choice at 1280 px");
        (g.m.rail.w, g.nav.len())
    };
    assert!((expanded - 236.0).abs() < 0.5, "{expanded}");
    r.ui.set_rail_collapsed(true);
    let g = geom_of(&mut r);
    // Reduced motion: it is there at once. Icon width, every entry still reachable, content takes the room.
    assert!((g.m.rail.w - 76.0).abs() < 0.5, "{}", g.m.rail.w);
    assert_eq!(g.nav.len(), n, "every view is still in the rail");
    assert!(g.m.body.x < 80.0 && g.m.body.w > 1280.0 - 80.0);
    let tg = g.rail_toggle.unwrap();
    assert!(g.m.rail.contains(tg.cx(), tg.cy()));
    assert!(
        g.nav.iter().all(|(_, rc)| rc.y > tg.bottom() && rc.right() <= g.m.rail.right()),
        "nothing under the button"
    );
    // The same rectangles of the nav, none overlapping.
    for (i, (_, a)) in g.nav.iter().enumerate() {
        for (_, b) in &g.nav[i + 1..] {
            assert!(a.bottom() <= b.y + 0.01, "{a:?} {b:?}");
        }
    }
    // A click on the button asks to expand; the key does the same; both ways.
    let (x, y) = center(tg);
    assert_eq!(r.click(x, y), [Action::Lib(LibAction::ToggleRail)]);
    let ctrl = Modifiers { ctrl: true, ..Modifiers::default() };
    assert_eq!(r.key_mod(Key::Char('b'), ctrl), [Action::Lib(LibAction::ToggleRail)]);
    // Plain B is still the switch between the faces.
    assert_eq!(r.key(Key::Char('b')), [Action::ToggleMode]);
}

#[test]
fn right_clicking_the_rail_offers_collapse_or_expand_and_the_choice_is_only_offered_where_it_exists() {
    let mut r = Rig::new();
    let (empty, gap) = {
        let g = geom_of(&mut r);
        // A spot of the rail that is no entry: between the nav and the buttons at the bottom.
        let last = g.nav.last().unwrap().1;
        (g.m.rail, (g.m.rail.cx(), (last.bottom() + g.add_folder.y.min(g.settings.y)) * 0.5))
    };
    assert!(empty.contains(gap.0, gap.1));
    r.right_click(gap.0, gap.1);
    let rows: Vec<String> = r.ui.menu.iter().flat_map(|p| p.items.iter().map(|i| i.label.clone())).collect();
    assert!(rows.contains(&String::from("Collapse sidebar")), "{rows:?}");
    r.key(Key::Escape);
    r.ui.set_rail_collapsed(true);
    r.right_click(gap.0.min(70.0), gap.1);
    let rows: Vec<String> = r.ui.menu.iter().flat_map(|p| p.items.iter().map(|i| i.label.clone())).collect();
    assert!(rows.contains(&String::from("Expand sidebar")), "{rows:?}");
    // An entry's menu has it too.
    r.key(Key::Escape);
    let nav0 = { geom_of(&mut r).nav[2].1 };
    r.right_click(nav0.cx(), nav0.cy());
    let rows: Vec<String> = r.ui.menu.iter().flat_map(|p| p.items.iter().map(|i| i.label.clone())).collect();
    assert!(
        rows.contains(&String::from("Open")) && rows.contains(&String::from("Expand sidebar")),
        "{rows:?}"
    );
    // Under 860 px the rail is icons anyway: no button, no key, no menu entry. A phone has its drawer.
    for (w, h) in [(840u32, 700u32), (500, 800)] {
        let mut small = Rig::new();
        small.ui.set_size(w, h, 1.0);
        assert!(geom_of(&mut small).rail_toggle.is_none(), "{w}");
        let ctrl = Modifiers { ctrl: true, ..Modifiers::default() };
        assert!(small.key_mod(Key::Char('b'), ctrl).is_empty(), "{w}");
    }
}

#[test]
fn the_rail_glides_with_motion_and_the_toggle_has_a_tooltip() {
    let mut r = Rig::new();
    r.ui.config.reduce_motion = false;
    geom_of(&mut r);
    r.ui.set_rail_collapsed(true);
    let mut widths = Vec::new();
    for _ in 0..14 {
        r.now += 16_000;
        r.ui.now = r.now;
        widths.push(geom_of(&mut r).m.rail.w);
    }
    assert!(widths.windows(2).all(|w| w[1] <= w[0] + 0.01), "{widths:?}");
    assert!(widths[0] < 236.0 && widths[0] > 76.0, "it starts to move: {widths:?}");
    assert!((widths.last().unwrap() - 76.0).abs() < 0.5, "and arrives: {widths:?}");
    assert!(!r.ui.rail_moving(), "arrived");
    // The tooltip says what the button does and names the key.
    let ctx = LibCtx {
        lib: &r.lib,
        now_art: None,
        scan: None,
        viz: None,
        video: None,
        resume: crate::lib_ui::no_resume(),
    };
    let tip = r.ui.lib_tip_text(crate::lib_ui::LibHit::RailToggle, &r.m, &ctx).unwrap();
    assert!(tip.0.contains("Show the side menu in full") && tip.1 == "Ctrl+B", "{tip:?}");
}
