//! The context menus of the library: one per kind of row or card, a general one, and the little ones (sort, folders).
use super::rows::EntKind;
use super::{Detail, Enqueue, LibAction, LibCtx, Scope, View};
use crate::actions::{Action, MenuItem, context_menu};
use crate::model::UiModel;
use crate::ui::Ui;
use alloc::string::String;
use alloc::vec::Vec;
use rvp_library::TrackSort;

fn item(label: &str, a: Action) -> MenuItem {
    let mut m = MenuItem::act(label, a);
    m.hint = String::new();
    m
}

fn hinted(label: &str, hint: &str, a: Action) -> MenuItem {
    let mut m = MenuItem::act(label, a);
    m.hint = hint.into();
    m
}

fn play(label: &str, hint: &str, scope: Scope, how: Enqueue) -> MenuItem {
    hinted(label, hint, Action::Lib(LibAction::Play(scope, how)))
}

/// "Add to playlist" with the saved playlists as its entries.
fn add_to_playlist(scope: Scope, ctx: &LibCtx<'_>) -> MenuItem {
    let mut sub = alloc::vec![item("New playlist\u{2026}", Action::Lib(LibAction::NewPlaylistFrom(scope)))];
    for (i, p) in ctx.lib.playlists().iter().enumerate().take(14) {
        let mut m = item(&p.name, Action::Lib(LibAction::AddToPlaylist(p.id, scope)));
        if i == 0 {
            m.separator = true;
        }
        sub.push(m);
    }
    MenuItem::parent("Add to playlist", sub)
}

/// The heart entry for a song or video.
fn favorite(id: u32, ctx: &LibCtx<'_>) -> MenuItem {
    let on = ctx.lib.is_favorite(id);
    hinted(
        if on { "Remove from favorites" } else { "Add to favorites" },
        "Ctrl+F",
        Action::Lib(LibAction::ToggleFavorite(id)),
    )
}

fn go_to(track: Option<u32>, ctx: &LibCtx<'_>) -> Vec<MenuItem> {
    let mut v = Vec::new();
    if let Some(a) = track.and_then(|t| ctx.lib.album_of(t)) {
        let mut m = item("Go to album", Action::OpenDetail(Detail::Album(a.id)));
        m.separator = true;
        v.push(m);
        v.push(item("Go to artist", Action::OpenDetail(Detail::Artist(a.artist_id))));
    }
    v
}

/// The menu of entity `i` of the current view.
pub(crate) fn ent_menu(ui: &Ui, i: usize, model: &UiModel, ctx: &LibCtx<'_>) -> Vec<MenuItem> {
    let Some(rows) = &ui.lib.rows else { return global_menu(ui, model, ctx) };
    let Some(e) = rows.ents.get(i).copied() else { return global_menu(ui, model, ctx) };
    let lib = ctx.lib;
    let from_history = rows.hist.get(&i).map(|h| h.seq);
    let mut menu = ent_menu_base(ui, e.kind, model, ctx, lib);
    if let Some(seq) = from_history {
        menu.push(hinted("Remove from history", "Delete", Action::Lib(LibAction::RemoveFromHistory(seq))));
    }
    menu
}

fn ent_menu_base(
    ui: &Ui,
    kind: EntKind,
    model: &UiModel,
    ctx: &LibCtx<'_>,
    lib: &rvp_library::Library,
) -> Vec<MenuItem> {
    match kind {
        EntKind::Track { id, pos } => {
            let mut v = alloc::vec![
                play("Play", "Enter", Scope::ListFrom(pos as u32), Enqueue::Now),
                play("Play next", "Ctrl+Enter", Scope::Track(id), Enqueue::Next),
                play("Add to queue", "Shift+Enter", Scope::Track(id), Enqueue::Append),
                add_to_playlist(Scope::Track(id), ctx),
                favorite(id, ctx).sep(),
            ];
            if model.app.tags {
                v.push(hinted("Edit tags\u{2026}", "E", Action::Lib(LibAction::EditTags(Scope::Track(id)))));
            }
            v.extend(go_to(Some(id), ctx));
            v
        }
        EntKind::Video { id, pos } => alloc::vec![
            play("Play", "Enter", Scope::ListFrom(pos as u32), Enqueue::Now),
            play("Play next", "Ctrl+Enter", Scope::Video(id), Enqueue::Next),
            play("Add to queue", "Shift+Enter", Scope::Video(id), Enqueue::Append),
            favorite(id, ctx).sep(),
        ],
        EntKind::Album(ai) => {
            let Some(a) = lib.albums().get(ai) else { return global_menu(ui, model, ctx) };
            let id = a.id;
            let mut v = alloc::vec![
                item("Open", Action::OpenDetail(Detail::Album(id))),
                play("Play", "", Scope::Album(id), Enqueue::Now),
                play("Shuffle", "", Scope::Album(id), Enqueue::ShuffleNow),
                play("Play next", "", Scope::Album(id), Enqueue::Next).sep(),
                play("Add to queue", "", Scope::Album(id), Enqueue::Append),
                add_to_playlist(Scope::Album(id), ctx),
                item("Favorite all or none", Action::Lib(LibAction::FavoriteScope(Scope::Album(id)))).sep(),
            ];
            if model.app.tags {
                v.push(item("Edit tags\u{2026}", Action::Lib(LibAction::EditTags(Scope::Album(id)))));
            }
            v.push(item("Go to artist", Action::OpenDetail(Detail::Artist(a.artist_id))));
            v
        }
        EntKind::Artist(ai) => {
            let Some(a) = lib.artists().get(ai) else { return global_menu(ui, model, ctx) };
            let id = a.id;
            alloc::vec![
                item("Open", Action::OpenDetail(Detail::Artist(id))),
                play("Play all", "", Scope::Artist(id), Enqueue::Now),
                play("Shuffle", "", Scope::Artist(id), Enqueue::ShuffleNow),
                play("Play next", "", Scope::Artist(id), Enqueue::Next).sep(),
                play("Add to queue", "", Scope::Artist(id), Enqueue::Append),
                add_to_playlist(Scope::Artist(id), ctx),
            ]
        }
        EntKind::Playlist(id) => alloc::vec![
            item("Open", Action::OpenDetail(Detail::Playlist(id))),
            play("Play", "", Scope::Playlist(id), Enqueue::Now),
            play("Shuffle", "", Scope::Playlist(id), Enqueue::ShuffleNow),
            play("Play next", "", Scope::Playlist(id), Enqueue::Next),
            play("Add to queue", "", Scope::Playlist(id), Enqueue::Append),
            item("Rename\u{2026}", Action::Lib(LibAction::RenamePlaylist(id))).sep(),
            item("Export as M3U8", Action::Lib(LibAction::ExportPlaylist(id, false))),
            item("Export as PLS", Action::Lib(LibAction::ExportPlaylist(id, true))),
            item("Delete", Action::Lib(LibAction::DeletePlaylist(id))).sep(),
        ],
        EntKind::Queue(id) => {
            let track = model.playlist.iter().find(|q| q.id == id).and_then(|q| q.track);
            let mut v = alloc::vec![
                hinted("Play", "Enter", Action::PlayItem(id)),
                item("Play next", Action::Lib(LibAction::QueueToNext(id))),
                hinted("Remove from queue", "Delete", Action::RemoveItem(id)),
                hinted("Move up", "Alt+Up", Action::MoveItem(id, -1)).sep(),
                hinted("Move down", "Alt+Down", Action::MoveItem(id, 1)),
                item("Clear queue", Action::ClearPlaylist).sep(),
                item("Save queue as playlist\u{2026}", Action::Lib(LibAction::NewPlaylistFrom(Scope::Queue))),
            ];
            if let Some(t) = track {
                v.push(favorite(t, ctx).sep());
                if model.app.tags {
                    v.push(item("Edit tags\u{2026}", Action::Lib(LibAction::EditTags(Scope::Track(t)))));
                }
            }
            v.extend(go_to(track, ctx));
            v
        }
        EntKind::PlEntry { pl, idx } => {
            let track = lib.playlist(pl).and_then(|p| p.entries.get(idx)).and_then(|e| e.track);
            let present = track.is_some();
            let mut v = alloc::vec![
                play("Play from here", "Enter", Scope::PlaylistFrom(pl, idx as u32), Enqueue::Now)
                    .enabled(present),
                item(
                    "Play next",
                    Action::Lib(LibAction::Play(
                        track.map_or(Scope::PlaylistFrom(pl, idx as u32), Scope::Track),
                        Enqueue::Next
                    ))
                )
                .enabled(present),
                item(
                    "Add to queue",
                    Action::Lib(LibAction::Play(
                        track.map_or(Scope::PlaylistFrom(pl, idx as u32), Scope::Track),
                        Enqueue::Append
                    ))
                )
                .enabled(present),
                hinted(
                    "Remove from playlist",
                    "Delete",
                    Action::Lib(LibAction::RemoveFromPlaylist(pl, idx as u32))
                )
                .sep(),
                hinted("Move up", "Alt+Up", Action::Lib(LibAction::MovePlaylistEntry(pl, idx as u32, -1))),
                hinted("Move down", "Alt+Down", Action::Lib(LibAction::MovePlaylistEntry(pl, idx as u32, 1))),
            ];
            if let Some(t) = track {
                v.push(favorite(t, ctx).sep());
                if model.app.tags {
                    v.push(item("Edit tags\u{2026}", Action::Lib(LibAction::EditTags(Scope::Track(t)))));
                }
            }
            v.extend(go_to(track, ctx));
            v
        }
    }
}

/// The menu of a rail entry.
pub(crate) fn rail_menu(v: View, collapsed: bool, can_toggle: bool) -> Vec<MenuItem> {
    let mut m = alloc::vec![item("Open", Action::ShowView(v))];
    if can_toggle {
        m.extend(rail_only_menu(collapsed).into_iter().map(MenuItem::sep));
    }
    m
}

/// The menu of the rail's empty parts: collapse it to icons, or expand it again.
pub(crate) fn rail_only_menu(collapsed: bool) -> Vec<MenuItem> {
    alloc::vec![hinted(
        if collapsed { "Expand sidebar" } else { "Collapse sidebar" },
        "Ctrl+B",
        Action::Lib(LibAction::ToggleRail)
    )]
}

/// The menu of a folder in the rail.
pub(crate) fn folder_menu(i: usize) -> Vec<MenuItem> {
    alloc::vec![
        item("Scan again", Action::Lib(LibAction::Rescan(i as u32))),
        item("Remove from library", Action::Lib(LibAction::ForgetFolder(i as u32))).sep(),
    ]
}

/// The sort choices of the Tracks view.
pub(crate) fn sort_menu(ui: &Ui) -> Vec<MenuItem> {
    let cur = (ui.lib.track_sort, ui.lib.track_asc);
    let mut v: Vec<MenuItem> = Vec::new();
    for (by, name) in [
        (TrackSort::Title, "Title"),
        (TrackSort::Artist, "Artist"),
        (TrackSort::Album, "Album"),
        (TrackSort::Duration, "Length"),
        (TrackSort::Year, "Year"),
        (TrackSort::Added, "Recently added"),
        (TrackSort::Plays, "Most played"),
        (TrackSort::LastPlayed, "Last played"),
    ] {
        // Choosing the sort that is on reverses it; a new one starts in its natural direction.
        let asc = if by == cur.0 {
            !cur.1
        } else {
            !matches!(by, TrackSort::Added | TrackSort::Plays | TrackSort::LastPlayed)
        };
        let mut m = item(name, Action::Lib(LibAction::SortTracks(by, asc)));
        m.checked = cur.0 == by;
        v.push(m);
    }
    v
}

/// The menu of the empty space: the library's own commands, then everything the player menu has.
pub(crate) fn global_menu(_ui: &Ui, model: &UiModel, ctx: &LibCtx<'_>) -> Vec<MenuItem> {
    let go = alloc::vec![
        item("Search", Action::ShowView(View::Search)),
        item("Now playing", Action::ShowView(View::NowPlaying)),
        item("Albums", Action::ShowView(View::Albums)).sep(),
        item("Artists", Action::ShowView(View::Artists)),
        item("Tracks", Action::ShowView(View::Tracks)),
        item("Videos", Action::ShowView(View::Videos)),
        hinted("Favorites", "9", Action::ShowView(View::Favorites)),
        hinted("History", "0", Action::ShowView(View::History)),
        item("Playlists", Action::ShowView(View::Playlists)),
        item("Queue", Action::ShowView(View::Queue)),
        item("Visualizer", Action::ShowView(View::Visualizer)).sep(),
        hinted("About RW", "F1", Action::ShowView(View::About)),
        hinted("Keyboard shortcuts", "H / ?", Action::ShowHelp),
    ];
    let has_root = !ctx.lib.roots().is_empty();
    let mut v = alloc::vec![
        MenuItem::parent("Go to", go),
        item("Add folder\u{2026}", Action::Lib(LibAction::AddFolder)).sep(),
        item("Scan folders again", Action::Lib(LibAction::Rescan(u32::MAX))).enabled(has_root),
        item("Import playlist\u{2026}", Action::Lib(LibAction::ImportPlaylist)),
        item("Shuffle all tracks", Action::Lib(LibAction::Play(Scope::AllTracks, Enqueue::ShuffleNow)))
            .enabled(ctx.lib.track_count() > 0),
        item("Player", Action::SetMode(super::Mode::Player)).sep(),
    ];
    let mut rest = context_menu(model);
    // The player menu's own "Library" entry is this face.
    // (The shortcuts page sits under "Go to" here, which keeps this menu short enough for a phone.)
    rest.retain(|m| m.action != Some(Action::ToggleMode) && m.action != Some(Action::ShowHelp));
    if let Some(first) = rest.first_mut() {
        first.separator = true;
    }
    v.extend(rest);
    v
}
