//! Sidebar, pages and the now-playing bar.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, Id, Layout, Rangef, Rect, RichText,
    Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};
use tagcast_player::{
    APP_NAME, Failure,
    account::Track,
    audio::Phase,
    transport::{Quality, Route},
};

use super::Collection;
use super::{
    Action, App, BrowseAlbumRows, Page, QUALITIES, Repeat,
    artwork::Artwork,
    catalog::{
        self, AlbumSort, Artist, ArtistRole, ArtistSort, BrowseGroup, BrowseKind, BrowseSort,
        Catalog, DiscSection, GroupSummary, SongSort, count, format_length, format_time,
    },
    quality_label,
    theme::{self, Icon, clipped, paint_icon, regular, strong},
};

const ROW: f32 = 34.0;
const GAP: f32 = 14.0;

// ---- track lists ----------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Number {
    Track,
    Index,
}

#[derive(Clone, Copy)]
struct ListStyle {
    number: Number,
    artist: bool,
    album: bool,
    year: bool,
    removable: bool,
}

enum RowEvent {
    Play(usize),
    Next(usize),
    Enqueue(usize),
    Album(usize),
    Artist(usize),
    Remove(usize),
    Select(u64),
}

struct Columns {
    number: Rangef,
    title: Rangef,
    artist: Option<Rangef>,
    album: Option<Rangef>,
    year: Option<Rangef>,
    time: Rangef,
    remove: Option<Rangef>,
}

impl Columns {
    fn new(left: f32, width: f32, style: ListStyle) -> Self {
        let fixed = 40.0
            + 56.0
            + if style.year { 52.0 + GAP } else { 0.0 }
            + if style.removable { 30.0 } else { 0.0 }
            + GAP * 2.0;
        let weights =
            3.0 + if style.artist { 2.0 } else { 0.0 } + if style.album { 2.0 } else { 0.0 };
        let flexible_gaps = GAP * (f32::from(style.artist) + f32::from(style.album));
        let unit = ((width - fixed - flexible_gaps) / weights).max(20.0);
        let mut x = left;
        let mut take = |w: f32, gap: f32| {
            let r = Rangef::new(x, x + w);
            x += w + gap;
            r
        };
        let number = take(40.0, GAP);
        let title = take(unit * 3.0, GAP);
        let artist = style.artist.then(|| take(unit * 2.0, GAP));
        let album = style.album.then(|| take(unit * 2.0, GAP));
        let year = style.year.then(|| take(52.0, GAP));
        let time = take(56.0, 0.0);
        let remove = style.removable.then(|| take(30.0, 0.0));
        Self {
            number,
            title,
            artist,
            album,
            year,
            time,
            remove,
        }
    }
}

struct RowState {
    current: bool,
    playing: bool,
    selected: bool,
    dim: bool,
}

fn header_row(ui: &mut Ui, style: ListStyle, sort: Option<(SongSort, bool)>) -> Option<SongSort> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let cols = Columns::new(rect.left() + 8.0, rect.width() - 16.0, style);
    let mut clicked = None;
    let mut cell = |range: Option<Rangef>, label: &str, key: Option<SongSort>, right: bool| {
        let Some(range) = range else { return };
        let cell = Rect::from_x_y_ranges(range, rect.y_range());
        let arrow = match (sort, key) {
            (Some((current, descending)), Some(key)) if current == key => {
                if descending {
                    " ↓"
                } else {
                    " ↑"
                }
            }
            _ => "",
        };
        let text = format!("{label}{arrow}");
        let response = key.filter(|_| sort.is_some()).map(|key| {
            (
                key,
                ui.interact(cell, ui.scope_id().with(("sort", label)), Sense::click()),
            )
        });
        let hovered = response.as_ref().is_some_and(|(_, r)| r.hovered());
        let color = if hovered || !arrow.is_empty() {
            theme::TEXT
        } else {
            theme::FAINT
        };
        let align = if right {
            Align2::RIGHT_CENTER
        } else {
            Align2::LEFT_CENTER
        };
        let pos = if right {
            pos2(cell.right(), cell.center().y)
        } else {
            pos2(cell.left(), cell.center().y)
        };
        ui.painter().text(pos, align, text, strong(11.0), color);
        if let Some((key, response)) = response
            && response.on_hover_cursor(CursorIcon::PointingHand).clicked()
        {
            clicked = Some(key);
        }
    };
    cell(Some(cols.number), "#", None, false);
    cell(Some(cols.title), "TITLE", Some(SongSort::Title), false);
    cell(cols.artist, "ARTIST", Some(SongSort::Artist), false);
    cell(cols.album, "ALBUM", Some(SongSort::Album), false);
    cell(cols.year, "YEAR", Some(SongSort::Year), false);
    cell(Some(cols.time), "TIME", Some(SongSort::Time), true);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::LINE),
    );
    clicked
}

#[allow(clippy::too_many_arguments)]
fn track_row(
    ui: &mut Ui,
    row: usize,
    track: &Track,
    number: usize,
    style: ListStyle,
    state: RowState,
    album_name: Option<&str>,
    events: &mut Vec<RowEvent>,
) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let id = ui.scope_id().with(("track-row", row));
    let hovered = ui.rect_contains_pointer(rect) || response.context_menu_opened();
    let cols = Columns::new(rect.left() + 8.0, rect.width() - 16.0, style);
    let painter = ui.painter().clone();
    let fill = if state.selected {
        theme::SURFACE_HI
    } else if hovered {
        theme::SURFACE
    } else if state.current {
        theme::ACCENT.gamma_multiply(0.10)
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, CornerRadius::same(6), fill);
    let y = rect.center().y;
    let font = regular(13.5);
    let (text, muted) = if state.dim {
        (theme::MUTED, theme::FAINT)
    } else {
        (theme::TEXT, theme::MUTED)
    };

    // Number, or a play button while hovered, or the playing indicator.
    let number_rect = Rect::from_x_y_ranges(cols.number, rect.y_range());
    let icon_rect = Rect::from_center_size(pos2(number_rect.left() + 11.0, y), Vec2::splat(15.0));
    if state.current && state.playing && !hovered {
        paint_icon(&painter, icon_rect, Icon::Bars, theme::ACCENT_TEXT);
    } else if hovered {
        let play = ui.interact(number_rect, id.with("play"), Sense::click());
        paint_icon(
            &painter,
            icon_rect,
            if state.current && state.playing {
                Icon::Pause
            } else {
                Icon::Play
            },
            if play.hovered() {
                theme::CREAM
            } else {
                theme::TEXT
            },
        );
        if play.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            events.push(RowEvent::Play(row));
        }
    } else {
        let label = if number == 0 {
            "–".to_owned()
        } else {
            number.to_string()
        };
        painter.text(
            pos2(number_rect.left() + 2.0, y),
            Align2::LEFT_CENTER,
            label,
            regular(12.5),
            if state.current {
                theme::ACCENT_TEXT
            } else {
                theme::FAINT
            },
        );
    }

    let title_color = if state.current {
        theme::ACCENT_TEXT
    } else {
        text
    };
    clipped(
        &painter,
        pos2(cols.title.min, y - 8.5),
        &track.title,
        if state.current {
            strong(13.5)
        } else {
            font.clone()
        },
        title_color,
        cols.title.span(),
    );

    let link = |range: Option<Rangef>, label: &str, key: &str, enabled: bool| -> bool {
        let Some(range) = range else { return false };
        let shown = clipped(
            &painter,
            pos2(range.min, y - 8.5),
            label,
            font.clone(),
            muted,
            range.span(),
        );
        if !enabled || !hovered {
            return false;
        }
        let response = ui.interact(shown.expand(2.0), id.with(key), Sense::click());
        if response.hovered() {
            painter.hline(
                shown.x_range(),
                shown.bottom() + 1.0,
                Stroke::new(1.0, muted),
            );
        }
        response.on_hover_cursor(CursorIcon::PointingHand).clicked()
    };
    if link(
        cols.artist,
        catalog::artist_label(&track.artist),
        "artist",
        true,
    ) {
        events.push(RowEvent::Artist(row));
    }
    if link(
        cols.album,
        album_name.unwrap_or(&track.album),
        "album",
        album_name.is_some(),
    ) {
        events.push(RowEvent::Album(row));
    }
    if let Some(range) = cols.year
        && track.year > 0
    {
        painter.text(
            pos2(range.min, y),
            Align2::LEFT_CENTER,
            track.year.to_string(),
            font.clone(),
            muted,
        );
    }
    let time = if track.seconds > 0.0 {
        format_time(track.seconds)
    } else {
        "–".into()
    };
    painter.text(
        pos2(cols.time.max, y),
        Align2::RIGHT_CENTER,
        time,
        font,
        muted,
    );
    if let Some(range) = cols.remove
        && hovered
    {
        let r = Rect::from_center_size(pos2(range.center(), y), Vec2::splat(24.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(r));
        if theme::icon_button(
            &mut child,
            Icon::Close,
            24.0,
            theme::MUTED,
            "Remove from queue",
        )
        .clicked()
        {
            events.push(RowEvent::Remove(row));
        }
    }

    if response.double_clicked() {
        events.push(RowEvent::Play(row));
    } else if response.clicked() {
        events.push(RowEvent::Select(track.id));
    }
    response.context_menu(|ui| {
        ui.set_min_width(180.0);
        if ui.button("Play").clicked() {
            events.push(RowEvent::Play(row));
            ui.close();
        }
        if ui.button("Play next").clicked() {
            events.push(RowEvent::Next(row));
            ui.close();
        }
        if ui.button("Add to queue").clicked() {
            events.push(RowEvent::Enqueue(row));
            ui.close();
        }
        ui.separator();
        if album_name.is_some() && ui.button("Go to album").clicked() {
            events.push(RowEvent::Album(row));
            ui.close();
        }
        if ui.button("Go to artist").clicked() {
            events.push(RowEvent::Artist(row));
            ui.close();
        }
        if style.removable {
            ui.separator();
            if ui.button("Remove from queue").clicked() {
                events.push(RowEvent::Remove(row));
                ui.close();
            }
        }
    });
}

#[derive(Debug, PartialEq, Eq)]
enum ListRow {
    Disc(u64),
    Track { position: usize, encoded: bool },
}

fn list_row(sections: &[DiscSection], display_row: usize) -> ListRow {
    let mut preceding = 0;
    let mut encoded = false;
    for (offset, disc) in sections.iter().enumerate() {
        match display_row.cmp(&(disc.start + offset)) {
            std::cmp::Ordering::Less => break,
            std::cmp::Ordering::Equal => return ListRow::Disc(disc.number),
            std::cmp::Ordering::Greater => {
                preceding += 1;
                encoded = disc.encoded;
            }
        }
    }
    ListRow::Track {
        position: display_row - preceding,
        encoded,
    }
}

fn disc_heading(ui: &mut Ui, number: u64) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    let label = if number == 0 {
        "Disc · number unknown".into()
    } else {
        format!("Disc {number}")
    };
    ui.painter().text(
        pos2(rect.left() + 8.0, rect.center().y + 1.0),
        Align2::LEFT_CENTER,
        label,
        strong(12.5),
        theme::MUTED,
    );
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, theme::LINE));
}

/// A virtualised list of library tracks. Row events refer to positions in `rows`.
#[allow(clippy::too_many_arguments)]
fn library_list(
    ui: &mut Ui,
    salt: &str,
    catalog: &Catalog,
    rows: &[usize],
    sections: &[DiscSection],
    style: ListStyle,
    now: (Option<u64>, bool),
    selected: Option<u64>,
    sort: Option<(SongSort, bool)>,
    events: &mut Vec<RowEvent>,
) -> Option<SongSort> {
    let sorted = header_row(ui, style, sort);
    ui.add_space(4.0);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical()
            .id_salt(salt)
            .auto_shrink([false, false])
            .show_rows(ui, ROW, rows.len() + sections.len(), |ui, range| {
                for display_row in range {
                    let (row, encoded) = match list_row(sections, display_row) {
                        ListRow::Disc(number) => {
                            disc_heading(ui, number);
                            continue;
                        }
                        ListRow::Track { position, encoded } => (position, encoded),
                    };
                    let index = rows[row];
                    let track = catalog.track(index);
                    let number = match style.number {
                        Number::Track if encoded => (track.number % 100) as usize,
                        Number::Track => track.number as usize,
                        Number::Index => row + 1,
                    };
                    let state = RowState {
                        current: now.0 == Some(track.id),
                        playing: now.1,
                        selected: selected == Some(track.id),
                        dim: false,
                    };
                    let album_name = catalog.album_of(track).map(|album| album.name.as_str());
                    track_row(ui, row, track, number, style, state, album_name, events);
                }
                ui.add_space(24.0);
            });
    });
    sorted
}

fn row_actions(
    catalog: &Catalog,
    rows: &[usize],
    events: Vec<RowEvent>,
    app_selected: &mut Option<u64>,
    shuffle: Option<bool>,
    actions: &mut Vec<Action>,
) {
    for event in events {
        match event {
            RowEvent::Play(row) => actions.push(Action::Play {
                tracks: rows.to_vec(),
                start: Some(row),
                shuffle,
            }),
            RowEvent::Next(row) => actions.push(Action::Enqueue {
                tracks: vec![rows[row]],
                next: true,
            }),
            RowEvent::Enqueue(row) => actions.push(Action::Enqueue {
                tracks: vec![rows[row]],
                next: false,
            }),
            RowEvent::Album(row) => {
                if let Some(album) = catalog.album_of(catalog.track(rows[row])) {
                    actions.push(Action::Go(Page::Album(album.id)));
                }
            }
            RowEvent::Artist(row) => {
                if let Some(artist) =
                    catalog.artist_by_name(&catalog.track(rows[row]).artist, ArtistRole::Track)
                {
                    actions.push(Action::Go(Page::Artist(
                        ArtistRole::Track,
                        artist.key.clone(),
                    )));
                }
            }
            RowEvent::Remove(_) => {}
            RowEvent::Select(id) => *app_selected = Some(id),
        }
    }
}

// ---- album grid -----------------------------------------------------------

enum TileEvent {
    Open(usize),
    Play(usize, bool),
    Enqueue(usize, bool),
    Artist(usize),
}

fn album_tile(
    ui: &mut Ui,
    rect: Rect,
    catalog: &Catalog,
    art: &mut Artwork,
    album_index: usize,
    playing_album: Option<u64>,
    events: &mut Vec<TileEvent>,
) {
    let album = &catalog.albums[album_index];
    let id = ui.scope_id().with(("album-tile", album.id));
    let response = ui.interact(rect, id, Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    let cover_rect = Rect::from_min_size(rect.min, Vec2::splat(rect.width()));
    let texture = art.get(
        album.artwork,
        catalog.library.artwork_url(album.artwork, 300),
    );
    let painter = ui.painter();
    theme::cover(painter, cover_rect, texture, 8, &album.name);
    if hovered {
        painter.rect_filled(
            cover_rect,
            CornerRadius::same(8),
            Color32::from_black_alpha(70),
        );
    }
    let y = cover_rect.bottom() + 8.0;
    let current = playing_album == Some(album.id);
    clipped(
        painter,
        pos2(rect.left(), y),
        &album.name,
        strong(13.0),
        if current {
            theme::ACCENT_TEXT
        } else {
            theme::TEXT
        },
        rect.width(),
    );
    let sub = if album.year > 0 {
        format!("{} · {}", album.artist, album.year)
    } else {
        album.artist.clone()
    };
    clipped(
        painter,
        pos2(rect.left(), y + 19.0),
        &sub,
        regular(12.0),
        theme::MUTED,
        rect.width(),
    );

    if hovered {
        let center = cover_rect.right_bottom() - vec2(26.0, 26.0);
        let button = Rect::from_center_size(center, Vec2::splat(40.0));
        let play = ui.interact(button, id.with("play"), Sense::click());
        let fill = if play.hovered() {
            theme::ACCENT_HI
        } else {
            theme::ACCENT
        };
        let painter = ui.painter();
        painter.circle_filled(center + vec2(0.0, 2.0), 21.0, Color32::from_black_alpha(90));
        painter.circle_filled(center, 20.0, fill);
        paint_icon(
            painter,
            Rect::from_center_size(center + vec2(1.5, 0.0), Vec2::splat(18.0)),
            Icon::Play,
            theme::CREAM,
        );
        if play.on_hover_text("Play").clicked() {
            events.push(TileEvent::Play(album_index, false));
        } else if response.clicked() {
            events.push(TileEvent::Open(album_index));
        }
    }
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .context_menu(|ui| {
            ui.set_min_width(180.0);
            if ui.button("Play").clicked() {
                events.push(TileEvent::Play(album_index, false));
                ui.close();
            }
            if ui.button("Shuffle").clicked() {
                events.push(TileEvent::Play(album_index, true));
                ui.close();
            }
            if ui.button("Play next").clicked() {
                events.push(TileEvent::Enqueue(album_index, true));
                ui.close();
            }
            if ui.button("Add to queue").clicked() {
                events.push(TileEvent::Enqueue(album_index, false));
                ui.close();
            }
            ui.separator();
            if ui.button("Go to artist").clicked() {
                events.push(TileEvent::Artist(album_index));
                ui.close();
            }
        });
}

fn grid_metrics(width: f32, target: f32) -> (usize, f32, f32) {
    let gap = 20.0;
    let columns = (((width + gap) / (target + gap)).floor() as usize).max(1);
    let tile = ((width - gap * (columns as f32 - 1.0)) / columns as f32).min(240.0);
    (columns, tile, gap)
}

fn grid_scroll(ui: &Ui, salt: &str, reset: bool) -> egui::ScrollArea {
    if reset {
        let id = ui.make_persistent_id(egui::IdSalt::new(salt));
        egui::scroll_area::State::default().store(ui.ctx(), id);
    }
    egui::ScrollArea::vertical()
        .id_salt(salt)
        .auto_shrink([false, false])
}

struct AlbumGrid {
    salt: &'static str,
    reset: bool,
    detail: Option<AlbumSort>,
}

fn group_card(ui: &mut Ui, rect: Rect, group: &GroupSummary) -> bool {
    let id = ui.make_persistent_id(("browse-group", &group.name));
    let response = ui.interact(rect, id, Sense::click());
    let albums = count(group.albums, "album", "albums");
    let tracks = count(group.tracks, "track", "tracks");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            ui.is_enabled(),
            format!("{} · {albums} · {tracks}", group.name),
        )
    });
    let active = response.hovered() || response.has_focus();
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(10),
        if active {
            theme::SURFACE_HI
        } else {
            theme::SURFACE
        },
        Stroke::new(
            1.0,
            if active {
                theme::ACCENT_TEXT
            } else {
                theme::LINE
            },
        ),
        egui::StrokeKind::Inside,
    );
    let right = rect.right() - 13.0;
    let center = rect.center().y;
    let albums = painter.layout_no_wrap(albums, regular(12.0), theme::MUTED);
    let tracks = painter.layout_no_wrap(tracks, regular(11.5), theme::MUTED);
    let label_width = rect.width() - 40.0 - albums.size().x.max(tracks.size().x);
    let top = center - 9.0 - albums.size().y / 2.0;
    let bottom = center + 9.0 - tracks.size().y / 2.0;
    painter.galley(pos2(right - albums.size().x, top), albums, theme::MUTED);
    painter.galley(pos2(right - tracks.size().x, bottom), tracks, theme::MUTED);
    let font = strong(15.0);
    let name_height = painter.fonts_mut(|f| f.row_height(&font));
    clipped(
        painter,
        pos2(rect.left() + 13.0, center - name_height / 2.0),
        &group.name,
        font,
        theme::TEXT,
        label_width.max(0.0),
    );
    response
        .on_hover_text(&group.name)
        .on_hover_cursor(CursorIcon::PointingHand)
        .clicked()
}

fn album_grid(
    ui: &mut Ui,
    display: AlbumGrid,
    catalog: &Catalog,
    art: &mut Artwork,
    albums: &[usize],
    playing_album: Option<u64>,
    actions: &mut Vec<Action>,
) {
    let width = ui.available_width() - 4.0;
    let (columns, tile, gap) = grid_metrics(width, 172.0);
    let row_height = tile + if display.detail.is_some() { 66.0 } else { 54.0 };
    let rows = albums.len().div_ceil(columns);
    let mut events = Vec::new();
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = gap;
        grid_scroll(ui, display.salt, display.reset).show_rows(
            ui,
            row_height,
            rows,
            |ui, range| {
                for row in range {
                    let (rect, _) = ui.allocate_exact_size(vec2(width, row_height), Sense::hover());
                    for column in 0..columns {
                        let Some(&album) = albums.get(row * columns + column) else {
                            break;
                        };
                        let x = rect.left() + column as f32 * (tile + gap);
                        let tile_rect =
                            Rect::from_min_size(pos2(x, rect.top()), vec2(tile, row_height));
                        album_tile(
                            ui,
                            tile_rect,
                            catalog,
                            art,
                            album,
                            playing_album,
                            &mut events,
                        );
                        if let Some(detail) = display
                            .detail
                            .and_then(|sort| catalog.albums[album].sort_detail(sort))
                        {
                            clipped(
                                ui.painter(),
                                pos2(x, rect.top() + tile + 44.0),
                                &detail,
                                regular(11.5),
                                theme::MUTED,
                                tile,
                            );
                        }
                    }
                }
                ui.add_space(12.0);
            },
        );
    });
    tile_actions(catalog, events, actions);
}

fn tile_actions(catalog: &Catalog, events: Vec<TileEvent>, actions: &mut Vec<Action>) {
    for event in events {
        match event {
            TileEvent::Open(album) => {
                actions.push(Action::Go(Page::Album(catalog.albums[album].id)))
            }
            TileEvent::Play(album, shuffle) => actions.push(Action::Play {
                tracks: catalog.albums[album].tracks.clone(),
                start: if shuffle { None } else { Some(0) },
                shuffle: Some(shuffle),
            }),
            TileEvent::Enqueue(album, next) => actions.push(Action::Enqueue {
                tracks: catalog.albums[album].tracks.clone(),
                next,
            }),
            TileEvent::Artist(album) => {
                if let Some(artist) =
                    catalog.artist_by_name(&catalog.albums[album].artist, ArtistRole::Album)
                {
                    actions.push(Action::Go(Page::Artist(
                        ArtistRole::Album,
                        artist.key.clone(),
                    )));
                }
            }
        }
    }
}

// ---- shared page pieces ---------------------------------------------------

fn page_title(
    ui: &mut Ui,
    can_go_back: bool,
    title: &str,
    subtitle: &str,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        if can_go_back && theme::icon_button(ui, Icon::Back, 30.0, theme::MUTED, "Back").clicked() {
            actions.push(Action::Back);
        }
        ui.label(RichText::new(title).font(strong(26.0)).color(theme::TEXT));
        if !subtitle.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(subtitle).color(theme::MUTED));
        }
    });
}

/// Play, Shuffle and a menu with the queue commands. `rtl` is for right-to-left layouts.
fn play_buttons(ui: &mut Ui, tracks: &[usize], actions: &mut Vec<Action>, rtl: bool) {
    let enabled = !tracks.is_empty();
    ui.add_enabled_ui(enabled, |ui| {
        let mut order = [0, 1, 2];
        if rtl {
            order.reverse();
        }
        let mut more = None;
        for item in order {
            match item {
                0 => {
                    if theme::pill(ui, Some(Icon::Play), "Play", true).clicked() {
                        actions.push(Action::Play {
                            tracks: tracks.to_vec(),
                            start: Some(0),
                            shuffle: Some(false),
                        });
                    }
                }
                1 => {
                    if theme::pill(ui, Some(Icon::Shuffle), "Shuffle", false).clicked() {
                        actions.push(Action::Play {
                            tracks: tracks.to_vec(),
                            start: None,
                            shuffle: Some(true),
                        });
                    }
                }
                _ => {
                    more = Some(theme::icon_button(
                        ui,
                        Icon::More,
                        34.0,
                        theme::MUTED,
                        "More",
                    ))
                }
            }
        }
        let Some(more) = more else { return };
        egui::Popup::menu(&more).show(|ui| {
            ui.set_min_width(170.0);
            if ui.button("Play next").clicked() {
                actions.push(Action::Enqueue {
                    tracks: tracks.to_vec(),
                    next: true,
                });
            }
            if ui.button("Add to queue").clicked() {
                actions.push(Action::Enqueue {
                    tracks: tracks.to_vec(),
                    next: false,
                });
            }
        });
    });
}

fn centered_message(ui: &mut Ui, spinner: bool, title: &str, body: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.32).max(20.0));
        if spinner {
            ui.add(egui::Spinner::new().size(28.0).color(theme::ACCENT_TEXT));
            ui.add_space(10.0);
        }
        ui.label(RichText::new(title).font(strong(17.0)).color(theme::TEXT));
        if !body.is_empty() {
            ui.add_space(2.0);
            ui.label(RichText::new(body).color(theme::MUTED));
        }
        ui.add_space(12.0);
    });
}

impl App {
    fn playing_album(&self) -> Option<u64> {
        let track = &self.queue.current_entry()?.track;
        Some(self.catalog.as_ref()?.album_of(track)?.id)
    }

    fn now(&self) -> (Option<u64>, bool) {
        let snapshot = self.player.output.snapshot();
        let playing = matches!(
            snapshot.phase,
            Phase::Playing | Phase::Buffering | Phase::Recovering
        ) && !snapshot.paused;
        (self.queue.current_entry().map(|e| e.track.id), playing)
    }

    // ---- sidebar ----------------------------------------------------------

    pub(super) fn sidebar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(Vec2::splat(28.0)));
            }
            ui.label(
                RichText::new(APP_NAME)
                    .font(strong(15.5))
                    .color(theme::TEXT),
            );
        });
        ui.add_space(14.0);

        // Search
        let search_frame = egui::Frame::new()
            .fill(theme::SURFACE)
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(8, 3));
        search_frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                paint_icon(ui.painter(), rect, Icon::Search, theme::MUTED);
                let clear_width = if self.query.is_empty() { 0.0 } else { 30.0 };
                let edit = egui::TextEdit::singleline(&mut self.query)
                    .hint_text(RichText::new("Search").color(theme::FAINT))
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::symmetric(2, 6))
                    .align(Align2::LEFT_CENTER)
                    .desired_width(ui.available_width() - clear_width);
                let response = ui.add(edit);
                if self.search_focus {
                    response.request_focus();
                    self.search_focus = false;
                }
                if !self.query.is_empty()
                    && theme::icon_button(ui, Icon::Close, 20.0, theme::MUTED, "Clear").clicked()
                {
                    self.query.clear();
                    if self.page == Page::Search {
                        actions.push(Action::Back);
                    }
                }
                if response.changed() {
                    if self.query.trim().is_empty() {
                        if self.page == Page::Search {
                            actions.push(Action::Back);
                        }
                    } else if self.page != Page::Search {
                        actions.push(Action::Go(Page::Search));
                    }
                }
            });
        });
        ui.add_space(14.0);

        let catalog = self.catalog.as_ref();
        let page = self.page.clone();
        let items = [
            (
                Icon::Note,
                "Songs",
                Page::Songs,
                catalog.map(|c| c.library.tracks.len()),
            ),
            (
                Icon::Album,
                "Albums",
                Page::Albums,
                catalog.map(|c| c.albums.len()),
            ),
            (
                Icon::Artist,
                ArtistRole::Album.label(),
                Page::Artists(ArtistRole::Album),
                catalog.map(|c| c.artist_count(ArtistRole::Album)),
            ),
            (
                Icon::Artist,
                ArtistRole::Track.label(),
                Page::Artists(ArtistRole::Track),
                catalog.map(|c| c.artist_count(ArtistRole::Track)),
            ),
            (Icon::Search, "Browse", Page::Browse, None),
            (
                Icon::Note,
                "Favourites",
                Page::Collection(Collection::Favourites),
                None,
            ),
            (
                Icon::Note,
                "Recently played",
                Page::Collection(Collection::Recent),
                None,
            ),
            (
                Icon::Queue,
                "Queue",
                Page::Queue,
                Some(self.queue.len()).filter(|n| *n > 0),
            ),
        ];

        let footer_height = 66.0
            + if self.audio_error.is_some() {
                44.0
            } else {
                0.0
            };
        let bottom = ui.max_rect().bottom();
        let height = (bottom - footer_height - ui.cursor().top() - 8.0).max(0.0);
        egui::ScrollArea::vertical()
            .id_salt("library-navigation")
            .auto_shrink([false, false])
            .max_height(height)
            .min_scrolled_height(height)
            .show(ui, |ui| {
                section_label(ui, "LIBRARY");
                for (icon, label, target, n) in items {
                    let selected = page == target
                        || matches!((&page, &target),
                            (Page::Album(_), Page::Albums) | (Page::BrowseAlbums(_), Page::Browse))
                        || matches!((&page, &target),
                            (Page::Artist(role, _), Page::Artists(target_role)) if role == target_role);
                    if nav_item(ui, Some(icon), label, n, selected).clicked() {
                        actions.push(Action::Go(target));
                    }
                }
                ui.add_space(14.0);
                section_label(ui, "PLAYLISTS");
                if let Some(catalog) = catalog {
                    if catalog.library.playlists.is_empty() {
                        ui.label(RichText::new("  No playlists").color(theme::FAINT));
                    } else {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        for (index, playlist) in catalog.library.playlists.iter().enumerate() {
                            let selected = page == Page::Playlist(index);
                            let label = if playlist.folder {
                                format!("▸ {}", playlist.title)
                            } else {
                                playlist.title.clone()
                            };
                            if nav_item(ui, Some(Icon::Playlist), &label, None, selected).clicked() {
                                actions.push(Action::Go(Page::Playlist(index)));
                            }
                        }
                    }
                }
            });
        let max = ui.max_rect();
        let footer = Rect::from_min_max(
            pos2(max.left(), bottom - footer_height),
            pos2(max.right(), bottom),
        );
        ui.scope_builder(UiBuilder::new().max_rect(footer), |ui| {
            self.sidebar_footer(ui, actions)
        });
    }

    fn sidebar_footer(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.max_rect().top(),
            Stroke::new(1.0, theme::LINE),
        );
        ui.add_space(10.0);
        let premium = self.catalog.as_ref().is_some_and(|c| c.library.premium);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Quality").color(theme::MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let more = theme::icon_button(ui, Icon::More, 28.0, theme::MUTED, "");
                egui::Popup::menu(&more).show(|ui| {
                    ui.set_min_width(200.0);
                    let reload = if self.loading.is_some() {
                        "Reloading…"
                    } else {
                        "Reload library"
                    };
                    if ui
                        .add_enabled(
                            self.loading.is_none(),
                            egui::Button::new(reload).shortcut_text(if cfg!(target_os = "macos") {
                                "⌘R"
                            } else {
                                "Ctrl+R"
                            }),
                        )
                        .clicked()
                    {
                        actions.push(Action::Reload);
                    }
                    ui.separator();
                    let mut gapless = self.settings.gapless;
                    let status = if gapless { "On" } else { "Off" };
                    if ui.checkbox(&mut gapless, format!("Gapless · Original FLAC · {status}"))
                        .on_hover_text("Play compatible Original FLAC albums without gaps in album order.")
                        .changed()
                    {
                        actions.push(Action::ToggleGapless);
                    }
                    if ui.button("Reconnect audio output")
                        .on_hover_text("Use the system’s current speakers, headphones or DAC. The track stays paused at its current position.")
                        .clicked()
                    {
                        actions.push(Action::RecoverOutput);
                    }
                    ui.separator();
                    if ui.button("Sign out").clicked() {
                        actions.push(Action::SignOut);
                    }
                });
                let current = if premium {
                    self.settings.quality()
                } else {
                    Quality::K128
                };
                ui.add_enabled_ui(premium, |ui| {
                    egui::ComboBox::from_id_salt("quality")
                        .selected_text(quality_label(current))
                        .width(104.0)
                        .show_ui(ui, |ui| {
                            for quality in QUALITIES {
                                if ui
                                    .selectable_label(quality == current, quality_label(quality))
                                    .clicked()
                                {
                                    actions.push(Action::SetQuality(quality));
                                }
                            }
                        })
                        .response
                        .on_disabled_hover_text("Higher qualities need iBroadcast Premium");
                });
            });
        });
        if let Some(catalog) = &self.catalog {
            let plan = if premium { "Premium" } else { "Free" };
            let status = if self.loading.is_some() {
                "Updating library…".to_owned()
            } else {
                format!(
                    "{} · {plan}",
                    count(catalog.library.tracks.len(), "song", "songs")
                )
            };
            ui.label(
                RichText::new(status)
                    .font(regular(11.5))
                    .color(theme::FAINT),
            );
        }
        if let Some(error) = &self.audio_error
            && ui
                .link(RichText::new(error).font(regular(11.5)).color(theme::WARN))
                .clicked()
        {
            actions.push(Action::RecoverOutput);
        }
    }

    // ---- pages ------------------------------------------------------------

    pub(super) fn main(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        if self.catalog.is_none() {
            match &self.library_error {
                Some(error) => {
                    let auth = *error == Failure::Auth;
                    centered_message(ui, false, "Could not load your library", &error.to_string());
                    ui.vertical_centered(|ui| {
                        ui.horizontal(|ui| {
                            let width = if auth { 250.0 } else { 104.0 };
                            ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                            if theme::pill(ui, None, "Try again", true).clicked() {
                                actions.push(Action::Reload);
                            }
                            if auth && theme::pill(ui, None, "Sign in again", false).clicked() {
                                actions.push(Action::SignOut);
                            }
                        });
                    });
                }
                None => {
                    centered_message(
                        ui,
                        true,
                        "Loading your library…",
                        "The first load can take a moment. After that it opens from a local copy.",
                    );
                }
            }
            return;
        }
        let back = !self.history.is_empty();
        match self.page.clone() {
            Page::Songs => self.songs_page(ui, back, actions),
            Page::Albums => self.albums_page(ui, back, None, actions),
            Page::Artists(role) => {
                ui.push_id(role, |ui| self.artists_page(ui, role, back, actions));
            }
            Page::Album(id) => self.album_page(ui, id, actions),
            Page::Artist(role, key) => {
                ui.push_id((role, &key), |ui| self.artist_page(ui, role, &key, actions));
            }
            Page::Playlist(index) => self.playlist_page(ui, index, back, actions),
            Page::Queue => self.queue_page(ui, back, actions),
            Page::Search => self.search_page(ui, back, actions),
            Page::Browse => self.browse_page(ui, actions),
            Page::BrowseAlbums(group) => self.albums_page(ui, back, Some(&group), actions),
            Page::Collection(collection) => self.collection_page(ui, collection, actions),
        }
    }

    fn browse_page(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let browse = &mut self.browse;
        let previous = browse.kind;
        ui.horizontal(|ui| {
            if !self.history.is_empty()
                && theme::icon_button(ui, Icon::Back, 30.0, theme::MUTED, "Back").clicked()
            {
                actions.push(Action::Back);
            }
            for kind in [BrowseKind::Genres, BrowseKind::Tags, BrowseKind::Years] {
                if ui
                    .add(egui::Button::new(kind.label()).selected(browse.kind == kind))
                    .clicked()
                {
                    browse.kind = kind;
                }
            }
        });
        let mut reset = browse.kind != previous;
        let years = browse.kind == BrowseKind::Years;
        if reset {
            browse.query.clear();
            browse.sort = if years {
                BrowseSort::Descending
            } else {
                BrowseSort::Tracks
            };
        }
        let label = browse.kind.label();
        ui.add_space(10.0);
        ui.label(RichText::new(label).font(strong(34.0)).color(theme::TEXT));
        ui.add_space(8.0);
        let singular = match browse.kind {
            BrowseKind::Genres => "genre",
            BrowseKind::Tags => "tag",
            BrowseKind::Years => "year",
        };
        let plural = label.to_ascii_lowercase();
        let sorts = [
            (BrowseSort::Tracks, "Most tracks first"),
            (BrowseSort::Albums, "Most albums first"),
            (
                BrowseSort::Ascending,
                if years { "Oldest first" } else { "Name A–Z" },
            ),
            (
                BrowseSort::Descending,
                if years { "Newest first" } else { "Name Z–A" },
            ),
        ];
        ui.horizontal(|ui| {
            let search_width = (ui.available_width() - 168.0).max(56.0);
            reset |= ui
                .add_sized(
                    vec2(search_width, 32.0),
                    egui::TextEdit::singleline(&mut browse.query)
                        .id_salt("browse-search")
                        .char_limit(200)
                        .hint_text(format!("Search {plural}")),
                )
                .changed();
            let previous_sort = browse.sort;
            egui::ComboBox::from_id_salt("browse-sort")
                .selected_text(
                    sorts
                        .iter()
                        .find(|(sort, _)| *sort == previous_sort)
                        .unwrap()
                        .1,
                )
                .width(140.0)
                .show_ui(ui, |ui| {
                    for (sort, name) in sorts {
                        ui.selectable_value(&mut browse.sort, sort, name);
                    }
                });
            reset |= browse.sort != previous_sort;
        });
        if reset {
            browse.rows = None;
        }
        let groups = self.catalog.as_mut().unwrap().groups(browse.kind);
        let rows = browse
            .rows
            .get_or_insert_with(|| GroupSummary::order(groups, &browse.query, browse.sort));
        ui.add_space(8.0);
        ui.label(RichText::new(count(rows.len(), singular, &plural)).color(theme::MUTED));
        ui.add_space(8.0);
        if rows.is_empty() {
            centered_message(ui, false, &format!("No {plural} found"), "");
            return;
        }

        let width = ui.available_width() - 4.0;
        let (columns, _, gap) = grid_metrics(width, 180.0);
        let tile = (width - gap * (columns - 1) as f32) / columns as f32;
        let height = 68.0;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 14.0;
            grid_scroll(ui, "browse-groups", reset).show_rows(
                ui,
                height,
                rows.len().div_ceil(columns),
                |ui, range| {
                    for row in range {
                        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
                        let start = row * columns;
                        for (column, &index) in rows[start..].iter().take(columns).enumerate() {
                            let group = &groups[index];
                            let left = rect.left() + column as f32 * (tile + gap);
                            let card =
                                Rect::from_min_size(pos2(left, rect.top()), vec2(tile, height));
                            if group_card(ui, card, group) {
                                let group = match browse.kind {
                                    BrowseKind::Genres => BrowseGroup::Genre(group.name.clone()),
                                    BrowseKind::Tags => BrowseGroup::Tag(group.name.clone()),
                                    BrowseKind::Years => BrowseGroup::Year(group.year()),
                                };
                                actions.push(Action::Go(Page::BrowseAlbums(group)));
                            }
                        }
                    }
                    ui.add_space(12.0);
                },
            );
        });
    }

    pub(super) fn queue_preview(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.heading("Up next");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    actions.push(Action::ToggleQueuePanel);
                }
            });
        });
        let from = self.queue.current.unwrap_or(0);
        let rows = &self.queue.items[from.min(self.queue.items.len())..];
        if rows.is_empty() {
            ui.label("The queue is empty");
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("queue-preview-rows")
            .show_rows(ui, 22.0, rows.len(), |ui, visible| {
                for index in visible {
                    let row = &rows[index];
                    let label = egui::Label::new(&row.track.title)
                        .truncate()
                        .sense(Sense::click());
                    let response = ui
                        .add(label)
                        .on_hover_text(format!("{} · {}", row.track.artist, row.track.title));
                    if response.double_clicked() {
                        actions.push(Action::QueueJump(from + index));
                    }
                }
            });
    }

    fn collection_page(&mut self, ui: &mut Ui, group: Collection, actions: &mut Vec<Action>) {
        let catalog = self.catalog.as_ref().unwrap();
        let now = self.now();
        if self
            .collection_rows
            .as_ref()
            .is_none_or(|(active, _)| *active != group)
        {
            let mut rows: Vec<usize> = match &group {
                Collection::Recent => self
                    .listening
                    .recent
                    .iter()
                    .filter_map(|&id| catalog.track_index(id))
                    .collect(),
                Collection::Favourites => {
                    let albums: std::collections::HashSet<_> = catalog
                        .library
                        .favourite_albums
                        .iter()
                        .chain(&self.listening.albums)
                        .filter_map(|id| catalog.album_index.get(id).copied())
                        .collect();
                    let tracks: std::collections::HashSet<_> =
                        catalog.library.favourite_tracks.iter().copied().collect();
                    catalog
                        .library
                        .tracks
                        .iter()
                        .enumerate()
                        .filter_map(|(index, track)| {
                            (catalog
                                .album_slot(track)
                                .is_some_and(|slot| albums.contains(&slot))
                                || tracks.contains(&track.id))
                            .then_some(index)
                        })
                        .collect()
                }
            };
            if !matches!(group, Collection::Recent) {
                rows.sort_unstable();
                rows.dedup();
            }
            self.collection_rows = Some((group.clone(), Arc::new(rows)));
        }
        let rows = self.collection_rows.as_ref().unwrap().1.clone();
        let title = match group {
            Collection::Favourites => "Favourites",
            Collection::Recent => "Recently played",
        };
        ui.horizontal(|ui| {
            page_title(
                ui,
                !self.history.is_empty(),
                title,
                &count(rows.len(), "song", "songs"),
                actions,
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                play_buttons_reversed(ui, &rows, actions)
            });
        });
        ui.add_space(10.0);
        if rows.is_empty() {
            centered_message(ui, false, "No songs", "");
            return;
        }
        let style = ListStyle {
            number: Number::Index,
            artist: true,
            album: true,
            year: true,
            removable: false,
        };
        let mut events = Vec::new();
        ui.push_id(&group, |ui| {
            library_list(
                ui,
                "collection",
                catalog,
                &rows,
                &[],
                style,
                now,
                self.selected,
                None,
                &mut events,
            );
        });
        row_actions(catalog, &rows, events, &mut self.selected, None, actions);
    }

    fn songs_page(&mut self, ui: &mut Ui, back: bool, actions: &mut Vec<Action>) {
        let (sort, descending) = self.song_sort;
        let now = self.now();
        let selected = self.selected;
        let catalog = self.catalog.as_mut().unwrap();
        let order = catalog.song_order(sort, descending);
        let subtitle = format!(
            "{} · {}",
            count(order.len(), "song", "songs"),
            format_length(catalog.total_seconds)
        );
        ui.horizontal(|ui| {
            page_title(ui, back, "Songs", &subtitle, actions);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                play_buttons_reversed(ui, &order, actions);
            });
        });
        ui.add_space(10.0);
        let mut events = Vec::new();
        let style = ListStyle {
            number: Number::Index,
            artist: true,
            album: true,
            year: true,
            removable: false,
        };
        let catalog = self.catalog.as_ref().unwrap();
        if let Some(key) = library_list(
            ui,
            "songs",
            catalog,
            &order,
            &[],
            style,
            now,
            selected,
            Some((sort, descending)),
            &mut events,
        ) {
            self.song_sort = if key == sort {
                (key, !descending)
            } else {
                (key, false)
            };
        }
        row_actions(catalog, &order, events, &mut self.selected, None, actions);
    }

    fn albums_page(
        &mut self,
        ui: &mut Ui,
        back: bool,
        group: Option<&BrowseGroup>,
        actions: &mut Vec<Action>,
    ) {
        let playing_album = self.playing_album();
        let catalog = self.catalog.as_mut().unwrap();
        let selected = group.map(|group| {
            let selected = &mut self.browse.album_rows;
            if selected.as_ref().is_none_or(|rows| &rows.group != group) {
                *selected = Some(BrowseAlbumRows {
                    group: group.clone(),
                    slots: catalog.group_album_slots(group),
                    order: None,
                });
            }
            selected.as_mut().unwrap()
        });
        let year;
        let title = match group {
            Some(BrowseGroup::Genre(label) | BrowseGroup::Tag(label)) => label,
            Some(BrowseGroup::Year(value)) => {
                year = value.to_string();
                &year
            }
            None => "Albums",
        };
        let total = selected
            .as_ref()
            .map_or(catalog.albums.len(), |rows| rows.slots.len());
        page_title(ui, back, title, &count(total, "album", "albums"), actions);
        ui.add_space(8.0);
        let mut reset = false;
        ui.horizontal_wrapped(|ui| {
            let current = self.settings.album_sort;
            for sort in [
                AlbumSort::Artist,
                AlbumSort::Title,
                AlbumSort::Year,
                AlbumSort::Added,
                AlbumSort::Rating,
                AlbumSort::Random,
            ] {
                let active = sort == current;
                let label = if active && sort != AlbumSort::Random {
                    let arrow = if sort.descending(self.settings.album_sort_reversed) {
                        "↓"
                    } else {
                        "↑"
                    };
                    format!("{}  {arrow}", sort.label())
                } else {
                    sort.label().to_owned()
                };
                let help = match sort {
                    AlbumSort::Artist | AlbumSort::Title => {
                        "Click again to switch between A–Z and Z–A."
                    }
                    AlbumSort::Year => "Release year. Click again to reverse.",
                    AlbumSort::Added => {
                        "Latest upload date within the album. Click again to reverse."
                    }
                    AlbumSort::Rating => "iBroadcast album rating. Click again to reverse.",
                    AlbumSort::Random => "Click to shuffle the order of albums.",
                };
                if ui
                    .add(
                        egui::Button::new(label)
                            .selected(active)
                            .min_size(vec2(74.0, 32.0)),
                    )
                    .on_hover_text(help)
                    .clicked()
                {
                    self.settings.album_sort = sort;
                    self.settings.album_sort_reversed =
                        active && sort != AlbumSort::Random && !self.settings.album_sort_reversed;
                    if sort == AlbumSort::Random {
                        catalog.reshuffle_albums();
                    }
                    reset = true;
                    ui.ctx().request_repaint();
                }
            }
        });
        let sort = self.settings.album_sort;
        let order = catalog.album_order(sort, self.settings.album_sort_reversed);
        let indices = if let Some(selected) = selected {
            if selected
                .order
                .as_ref()
                .is_none_or(|(previous, _)| !Arc::ptr_eq(previous, &order))
            {
                let indices = order
                    .iter()
                    .copied()
                    .filter(|index| selected.slots.contains(index))
                    .collect();
                selected.order = Some((order.clone(), indices));
            }
            &selected.order.as_ref().unwrap().1
        } else {
            &order
        };
        let detail = matches!(sort, AlbumSort::Added | AlbumSort::Rating).then_some(sort);
        if let Some(detail) = detail {
            if !catalog.library.has_album_metadata() {
                ui.horizontal(|ui| {
                    if self.loading.is_some() {
                        ui.label(
                            RichText::new("Loading upload dates and album ratings…")
                                .color(theme::MUTED),
                        );
                    } else {
                        let status = ui.label(
                            RichText::new("Upload dates and album ratings need a library update")
                                .color(theme::WARN),
                        );
                        if let Some(error) = &self.library_error {
                            status.on_hover_text(error.to_string());
                        }
                        if ui.button("Reload library").clicked() {
                            actions.push(Action::Reload);
                        }
                    }
                });
            } else if !indices.is_empty()
                && indices
                    .first()
                    .is_none_or(|&index| catalog.albums[index].sort_detail(detail).is_none())
            {
                let message = if detail == AlbumSort::Added {
                    "No upload dates in these albums"
                } else {
                    "No rated albums in this view"
                };
                ui.label(RichText::new(message).color(theme::MUTED));
            }
        }
        ui.add_space(14.0);
        if indices.is_empty() {
            centered_message(ui, false, "No albums", "");
            return;
        }
        ui.push_id(group, |ui| {
            album_grid(
                ui,
                AlbumGrid {
                    salt: "albums",
                    reset,
                    detail,
                },
                catalog,
                &mut self.art,
                indices,
                playing_album,
                actions,
            );
        });
    }

    fn artists_page(
        &mut self,
        ui: &mut Ui,
        role: ArtistRole,
        back: bool,
        actions: &mut Vec<Action>,
    ) {
        let catalog = self.catalog.as_ref().unwrap();
        let browser = &mut self.artist_browsers[role as usize];
        let artist_sort = match role {
            ArtistRole::Album => &mut self.settings.artist_sort,
            ArtistRole::Track => &mut self.settings.track_artist_sort,
        };
        let title = role.label();
        let plural = title.to_ascii_lowercase();
        let singular = role.singular().to_ascii_lowercase();
        page_title(ui, back, title, "", actions);
        ui.add_space(12.0);
        let mut reset = false;
        ui.horizontal(|ui| {
            let clear = !browser.query.is_empty();
            let width = (ui.available_width() - if clear { 40.0 } else { 0.0 }).max(48.0);
            reset |= ui
                .add_sized(
                    vec2(width, 34.0),
                    egui::TextEdit::singleline(&mut browser.query)
                        .hint_text(format!("Search {plural}"))
                        .char_limit(200)
                        .id_salt("artists-search"),
                )
                .changed();
            if clear
                && theme::icon_button(ui, Icon::Close, 30.0, theme::MUTED, "Clear artist search")
                    .clicked()
            {
                browser.query.clear();
                reset = true;
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let sort = *artist_sort;
            let name = match sort {
                ArtistSort::Name => "Name  ↑",
                ArtistSort::ReverseName => "Name  ↓",
                ArtistSort::Random => "Name",
            };
            if ui
                .add(
                    egui::Button::new(name)
                        .selected(sort != ArtistSort::Random)
                        .min_size(vec2(82.0, 32.0)),
                )
                .on_hover_text("Switch between A–Z and Z–A")
                .clicked()
            {
                *artist_sort = if sort == ArtistSort::Name {
                    ArtistSort::ReverseName
                } else {
                    ArtistSort::Name
                };
                reset = true;
            }
            if ui
                .add(
                    egui::Button::new("Random")
                        .selected(sort == ArtistSort::Random)
                        .min_size(vec2(82.0, 32.0)),
                )
                .on_hover_text("Shuffle the displayed artists")
                .clicked()
            {
                *artist_sort = ArtistSort::Random;
                reset = true;
            }
        });
        if reset || browser.rows.is_none() {
            browser.rows = Some(catalog.artist_order(
                role,
                &browser.query,
                *artist_sort,
                browser.rows.as_deref(),
            ));
            if reset {
                ui.ctx().request_repaint();
            }
        }
        let order = browser.rows.as_deref().unwrap_or_default();
        ui.add_space(12.0);
        ui.label(RichText::new(count(order.len(), &singular, &plural)).color(theme::MUTED));
        ui.add_space(8.0);
        if order.is_empty() {
            centered_message(ui, false, &format!("No {plural} found"), "");
            return;
        }
        let width = (ui.available_width() - 4.0).max(1.0);
        let (columns, tile, gap) = grid_metrics(width, 172.0);
        let row_height = tile + 55.0;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = gap;
            grid_scroll(ui, "artist-cards", reset).show_rows(
                ui,
                row_height,
                order.len().div_ceil(columns),
                |ui, range| {
                    for row in range {
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(width, row_height), Sense::hover());
                        for (column, &index) in
                            order[row * columns..].iter().take(columns).enumerate()
                        {
                            let artist = &catalog.artists[index];
                            let left = rect.left() + column as f32 * (tile + gap);
                            let card =
                                Rect::from_min_size(pos2(left, rect.top()), vec2(tile, row_height));
                            if ui.is_rect_visible(card) {
                                let credits = artist.credits(role).unwrap();
                                let texture =
                                    artist_texture(&mut self.art, catalog, artist, credits.artwork);
                                if artist_card(ui, card, artist, role, texture) {
                                    actions
                                        .push(Action::Go(Page::Artist(role, artist.key.clone())));
                                }
                            }
                        }
                    }
                    ui.add_space(12.0);
                },
            );
        });
    }

    fn album_page(&mut self, ui: &mut Ui, id: u64, actions: &mut Vec<Action>) {
        let now = self.now();
        let selected = self.selected;
        let catalog = self.catalog.as_ref().unwrap();
        let Some(&index) = catalog.album_index.get(&id) else {
            centered_message(ui, false, "This album is no longer in your library", "");
            return;
        };
        let album = &catalog.albums[index];
        let back = !self.history.is_empty();
        let texture = self.art.get(
            album.artwork,
            catalog.library.artwork_url(album.artwork, 300),
        );
        ui.horizontal(|ui| {
            if back && theme::icon_button(ui, Icon::Back, 30.0, theme::MUTED, "Back").clicked() {
                actions.push(Action::Back);
            }
        });
        ui.add_space(2.0);
        ui.horizontal_top(|ui| {
            let (cover, _) = ui.allocate_exact_size(Vec2::splat(168.0), Sense::hover());
            theme::cover(ui.painter(), cover, texture, 10, &album.name);
            ui.add_space(14.0);
            ui.vertical(|ui| {
                ui.set_min_height(168.0);
                ui.add_space(24.0);
                ui.label(
                    RichText::new("ALBUM")
                        .font(strong(11.0))
                        .color(theme::FAINT),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(&album.name)
                            .font(strong(28.0))
                            .color(theme::TEXT),
                    )
                    .truncate(),
                );
                let artist = ui.add(
                    egui::Label::new(
                        RichText::new(&album.artist)
                            .font(strong(14.5))
                            .color(theme::ACCENT_TEXT),
                    )
                    .sense(Sense::click()),
                );
                if artist.on_hover_cursor(CursorIcon::PointingHand).clicked()
                    && let Some(a) = catalog.artist_by_name(&album.artist, ArtistRole::Album)
                {
                    actions.push(Action::Go(Page::Artist(ArtistRole::Album, a.key.clone())));
                }
                let mut facts = Vec::new();
                if album.year > 0 {
                    facts.push(album.year.to_string());
                }
                if album.discs.len() > 1 {
                    facts.push(count(album.discs.len(), "disc", "discs"));
                }
                facts.push(count(album.tracks.len(), "song", "songs"));
                facts.push(format_length(album.seconds));
                ui.label(RichText::new(facts.join(" · ")).color(theme::MUTED));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    play_buttons(ui, &album.tracks, actions, false);
                    let saved = self
                        .listening
                        .albums
                        .iter()
                        .any(|id| catalog.album_index.get(id) == Some(&index));
                    let label = if saved { "★" } else { "☆" };
                    if ui
                        .button(label)
                        .on_hover_text(if saved {
                            "Remove local favourite"
                        } else {
                            "Save local favourite"
                        })
                        .clicked()
                    {
                        actions.push(Action::ToggleFavourite(album.id));
                    }
                });
            });
        });
        ui.add_space(16.0);
        let various = album
            .tracks
            .iter()
            .any(|&i| catalog::artist_label(&catalog.track(i).artist) != album.artist);
        let style = ListStyle {
            number: Number::Track,
            artist: various,
            album: false,
            year: false,
            removable: false,
        };
        let mut events = Vec::new();
        let rows = &album.tracks;
        let sections = if album.discs.len() > 1 {
            album.discs.as_slice()
        } else {
            &[]
        };
        ui.push_id(album.id, |ui| {
            library_list(
                ui,
                "album",
                catalog,
                rows,
                sections,
                style,
                now,
                selected,
                None,
                &mut events,
            );
        });
        row_actions(
            catalog,
            rows,
            events,
            &mut self.selected,
            (album.discs.len() > 1).then_some(false),
            actions,
        );
    }

    fn artist_page(&mut self, ui: &mut Ui, role: ArtistRole, key: &str, actions: &mut Vec<Action>) {
        let back = !self.history.is_empty();
        let playing_album = self.playing_album();
        let catalog = self.catalog.as_ref().unwrap();
        let Some(artist) = catalog.artist_by_key(key, role) else {
            centered_message(ui, false, "This artist is no longer in your library", "");
            return;
        };
        let credits = artist.credits(role).unwrap();
        let texture = artist_texture(&mut self.art, catalog, artist, credits.artwork);
        ui.horizontal(|ui| {
            if back && theme::icon_button(ui, Icon::Back, 30.0, theme::MUTED, "Back").clicked() {
                actions.push(Action::Back);
            }
            let (avatar, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
            artist_picture(ui.painter(), avatar, texture, &artist.name, 32);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(&artist.name)
                        .font(strong(26.0))
                        .color(theme::TEXT),
                );
                ui.label(
                    RichText::new(format!(
                        "{} · {} · {}",
                        role.singular(),
                        count(credits.albums.len(), "album", "albums"),
                        count(credits.tracks.len(), "song", "songs")
                    ))
                    .color(theme::MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                play_buttons_reversed(ui, &credits.tracks, actions);
            });
        });
        ui.add_space(16.0);
        album_grid(
            ui,
            AlbumGrid {
                salt: "artist-albums",
                reset: false,
                detail: None,
            },
            catalog,
            &mut self.art,
            &credits.albums,
            playing_album,
            actions,
        );
    }

    fn playlist_page(&mut self, ui: &mut Ui, index: usize, back: bool, actions: &mut Vec<Action>) {
        let now = self.now();
        let selected = self.selected;
        let catalog = self.catalog.as_ref().unwrap();
        let Some(playlist) = catalog.library.playlists.get(index) else {
            centered_message(ui, false, "This playlist is no longer in your library", "");
            return;
        };
        if playlist.folder {
            page_title(ui, back, &playlist.title, "Playlist folder", actions);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for child in &playlist.tracks {
                    if let Some(index) = catalog.playlist_index(*child)
                        && catalog.library.playlists[index].id != playlist.id
                    {
                        let child = &catalog.library.playlists[index];
                        if nav_item(ui, Some(Icon::Playlist), &child.title, None, false).clicked() {
                            actions.push(Action::Go(Page::Playlist(index)));
                        }
                    }
                }
            });
            return;
        }
        let rows = catalog.playlist_tracks(index);
        let seconds: f64 = rows.iter().map(|&i| catalog.track(i).seconds).sum();
        let subtitle = format!(
            "{} · {}",
            count(rows.len(), "song", "songs"),
            format_length(seconds)
        );
        ui.horizontal(|ui| {
            page_title(ui, back, &playlist.title, &subtitle, actions);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                play_buttons_reversed(ui, &rows, actions);
            });
        });
        ui.add_space(10.0);
        if rows.is_empty() {
            centered_message(ui, false, "This playlist is empty", "");
            return;
        }
        let style = ListStyle {
            number: Number::Index,
            artist: true,
            album: true,
            year: false,
            removable: false,
        };
        let mut events = Vec::new();
        library_list(
            ui,
            "playlist",
            catalog,
            &rows,
            &[],
            style,
            now,
            selected,
            None,
            &mut events,
        );
        row_actions(catalog, &rows, events, &mut self.selected, None, actions);
    }

    fn queue_page(&mut self, ui: &mut Ui, back: bool, actions: &mut Vec<Action>) {
        let (_, playing) = self.now();
        let seconds: f64 = self.queue.items.iter().map(|e| e.track.seconds).sum();
        let subtitle = if self.queue.is_empty() {
            String::new()
        } else {
            format!(
                "{} · {}",
                count(self.queue.len(), "song", "songs"),
                format_length(seconds)
            )
        };
        ui.horizontal(|ui| {
            page_title(ui, back, "Queue", &subtitle, actions);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let upcoming = self
                    .queue
                    .current
                    .map_or(self.queue.len(), |c| self.queue.len() - c - 1);
                if ui
                    .add_enabled(upcoming > 0, egui::Button::new("Clear upcoming"))
                    .clicked()
                {
                    actions.push(Action::QueueClear);
                }
            });
        });
        ui.add_space(10.0);
        if self.queue.is_empty() {
            centered_message(
                ui,
                false,
                "The queue is empty",
                "Double-click a song, or use Play on an album or playlist.",
            );
            return;
        }
        let style = ListStyle {
            number: Number::Index,
            artist: true,
            album: true,
            year: false,
            removable: true,
        };
        header_row(ui, style, None);
        ui.add_space(4.0);
        let mut events = Vec::new();
        let current = self.queue.current;
        let catalog = self.catalog.as_ref();
        let selected = self.selected;
        let items = &self.queue.items;
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("queue")
            .auto_shrink([false, false]);
        if let Some(current) = current
            && ui.memory(|m| m.data.get_temp::<usize>(Id::unique("queue-scrolled")))
                != Some(current)
        {
            scroll = scroll.vertical_scroll_offset((current as f32 * ROW - ROW * 2.0).max(0.0));
            ui.memory_mut(|m| m.data.insert_temp(Id::unique("queue-scrolled"), current));
        }
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            scroll.show_rows(ui, ROW, items.len(), |ui, range| {
                for row in range {
                    let entry = &items[row];
                    let state = RowState {
                        current: current == Some(row),
                        playing,
                        selected: selected == Some(entry.track.id) && current != Some(row),
                        dim: current.is_some_and(|c| row < c),
                    };
                    let album_name = catalog
                        .and_then(|c| c.album_of(&entry.track))
                        .map(|album| album.name.as_str());
                    track_row(
                        ui,
                        row,
                        &entry.track,
                        row + 1,
                        style,
                        state,
                        album_name,
                        &mut events,
                    );
                }
                ui.add_space(24.0);
            });
        });
        for event in events {
            match event {
                RowEvent::Play(row) => actions.push(Action::QueueJump(row)),
                RowEvent::Remove(row) => actions.push(Action::QueueRemove(row)),
                RowEvent::Select(id) => self.selected = Some(id),
                RowEvent::Album(row) => {
                    if let Some(album) =
                        catalog.and_then(|c| c.album_of(&self.queue.items[row].track))
                    {
                        actions.push(Action::Go(Page::Album(album.id)));
                    }
                }
                RowEvent::Artist(row) => {
                    if let Some(c) = catalog
                        && let Some(a) =
                            c.artist_by_name(&self.queue.items[row].track.artist, ArtistRole::Track)
                    {
                        actions.push(Action::Go(Page::Artist(ArtistRole::Track, a.key.clone())));
                    }
                }
                RowEvent::Next(row) | RowEvent::Enqueue(row) => {
                    if let Some(c) = catalog
                        && let Some(&index) = c.by_id.get(&self.queue.items[row].track.id)
                    {
                        actions.push(Action::Enqueue {
                            tracks: vec![index],
                            next: matches!(event, RowEvent::Next(_)),
                        });
                    }
                }
            }
        }
    }

    fn search_page(&mut self, ui: &mut Ui, back: bool, actions: &mut Vec<Action>) {
        let now = self.now();
        let selected = self.selected;
        let playing_album = self.playing_album();
        let catalog = self.catalog.as_ref().unwrap();
        let query = self.query.trim().to_owned();
        let results = match &self.search {
            Some((q, results)) if *q == query => results.clone(),
            _ => {
                let results = Arc::new(catalog.search(&query));
                self.search = Some((query.clone(), results.clone()));
                self.search_all_albums = false;
                results
            }
        };
        page_title(ui, back, "Search", &format!("“{query}”"), actions);
        ui.add_space(8.0);
        if results.artists.is_empty() && results.albums.is_empty() && results.tracks.is_empty() {
            centered_message(ui, false, "No results", "Try fewer or different words.");
            return;
        }
        let mut show_all = self.search_all_albums;
        let mut events = Vec::new();
        let mut tiles = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt(("search", &query))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if !results.artists.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        for &index in &results.artists {
                            let artist = &catalog.artists[index];
                            for role in ArtistRole::ALL {
                                if artist.credits(role).is_none() {
                                    continue;
                                }
                                let label = format!("{} · {}", artist.name, role.singular());
                                let chip =
                                    egui::Button::new(RichText::new(label).color(theme::TEXT))
                                        .corner_radius(14)
                                        .fill(theme::SURFACE);
                                if ui
                                    .add(chip)
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    actions
                                        .push(Action::Go(Page::Artist(role, artist.key.clone())));
                                }
                            }
                        }
                    });
                    ui.add_space(16.0);
                }
                if !results.albums.is_empty() {
                    let width = ui.available_width() - 4.0;
                    let (columns, tile, gap) = grid_metrics(width, 140.0);
                    let limit = if show_all {
                        results.albums.len()
                    } else {
                        columns * 2
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("ALBUMS · {}", results.albums.len()))
                                .font(strong(11.0))
                                .color(theme::FAINT),
                        );
                        if results.albums.len() > columns * 2 {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let label = if show_all { "Show less" } else { "Show all" };
                                if ui
                                    .link(RichText::new(label).color(theme::ACCENT_TEXT))
                                    .clicked()
                                {
                                    show_all = !show_all;
                                }
                            });
                        }
                    });
                    ui.add_space(6.0);
                    let shown = &results.albums[..limit.min(results.albums.len())];
                    for row in shown.chunks(columns) {
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(width, tile + 54.0), Sense::hover());
                        if ui.is_rect_visible(rect) {
                            for (column, &album) in row.iter().enumerate() {
                                let x = rect.left() + column as f32 * (tile + gap);
                                let tile_rect = Rect::from_min_size(
                                    pos2(x, rect.top()),
                                    vec2(tile, tile + 54.0),
                                );
                                album_tile(
                                    ui,
                                    tile_rect,
                                    catalog,
                                    &mut self.art,
                                    album,
                                    playing_album,
                                    &mut tiles,
                                );
                            }
                        }
                        ui.add_space(gap - 6.0);
                    }
                    ui.add_space(8.0);
                }
                if !results.tracks.is_empty() {
                    let label = if results.tracks.len() >= catalog::SEARCH_SONGS {
                        format!("SONGS · FIRST {}", catalog::SEARCH_SONGS)
                    } else {
                        format!("SONGS · {}", results.tracks.len())
                    };
                    ui.label(RichText::new(label).font(strong(11.0)).color(theme::FAINT));
                    ui.add_space(4.0);
                    let style = ListStyle {
                        number: Number::Index,
                        artist: true,
                        album: true,
                        year: true,
                        removable: false,
                    };
                    header_row(ui, style, None);
                    ui.add_space(4.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (row, &index) in results.tracks.iter().enumerate() {
                        let track = catalog.track(index);
                        let state = RowState {
                            current: now.0 == Some(track.id),
                            playing: now.1,
                            selected: selected == Some(track.id),
                            dim: false,
                        };
                        let album_name = catalog.album_of(track).map(|album| album.name.as_str());
                        track_row(
                            ui,
                            row,
                            track,
                            row + 1,
                            style,
                            state,
                            album_name,
                            &mut events,
                        );
                    }
                    ui.add_space(24.0);
                }
            });
        self.search_all_albums = show_all;
        tile_actions(catalog, tiles, actions);
        let rows = results.tracks.clone();
        row_actions(catalog, &rows, events, &mut self.selected, None, actions);
    }

    // ---- now playing ------------------------------------------------------

    pub(super) fn now_playing(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let snapshot = self.player.output.snapshot();
        let entry = self.queue.current_entry().cloned();
        let full = ui.max_rect();
        let left_w = (full.width() * 0.3).clamp(220.0, 360.0);
        let right_w = (full.width() * 0.26).clamp(190.0, 300.0);
        let left = Rect::from_min_size(full.min, vec2(left_w, full.height()));
        let right = Rect::from_min_max(pos2(full.right() - right_w, full.top()), full.max);
        let center = Rect::from_min_max(
            pos2(left.right() + 16.0, full.top()),
            pos2(right.left() - 16.0, full.bottom()),
        );
        ui.allocate_rect(full, Sense::hover());

        // Left: cover, title, artist and any playback note.
        let catalog = self.catalog.as_ref();
        let cover =
            Rect::from_min_size(pos2(left.left(), left.center().y - 30.0), Vec2::splat(60.0));
        match &entry {
            Some(entry) => {
                let track = &entry.track;
                let texture = catalog.and_then(|c| {
                    let artwork = c.album_of(track).map_or(track.artwork, |a| a.artwork);
                    self.art.get(artwork, c.library.artwork_url(artwork, 300))
                });
                theme::cover(ui.painter(), cover, texture, 6, &track.album);
                let x = cover.right() + 12.0;
                let width = left.right() - x - 4.0;
                let title = clipped(
                    ui.painter(),
                    pos2(x, cover.top() + 6.0),
                    &track.title,
                    strong(14.0),
                    theme::TEXT,
                    width,
                );
                let title_click = ui.interact(title, Id::unique("np-title"), Sense::click());
                if title_click.hovered() {
                    ui.painter().hline(
                        title.x_range(),
                        title.bottom(),
                        Stroke::new(1.0, theme::TEXT),
                    );
                }
                if title_click
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                    && let Some(album) = catalog.and_then(|c| c.album_of(track))
                {
                    actions.push(Action::Go(Page::Album(album.id)));
                }
                let (line, color) = match (&snapshot.phase, &snapshot.note) {
                    (Phase::Failed(error), _) => (error.to_string(), theme::ERROR),
                    (_, Some(note)) => (note.clone(), theme::WARN),
                    (Phase::Buffering, _) => ("Buffering…".to_owned(), theme::MUTED),
                    _ => (
                        catalog::artist_label(&track.artist).to_owned(),
                        theme::MUTED,
                    ),
                };
                let sub = clipped(
                    ui.painter(),
                    pos2(x, cover.top() + 27.0),
                    &line,
                    regular(12.5),
                    color,
                    width,
                );
                if color == theme::MUTED && !matches!(snapshot.phase, Phase::Buffering) {
                    let artist_click = ui.interact(sub, Id::unique("np-artist"), Sense::click());
                    if artist_click.hovered() {
                        ui.painter().hline(
                            sub.x_range(),
                            sub.bottom(),
                            Stroke::new(1.0, theme::MUTED),
                        );
                    }
                    if artist_click
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                        && let Some(c) = catalog
                        && let Some(a) = c.artist_by_name(&track.artist, ArtistRole::Track)
                    {
                        actions.push(Action::Go(Page::Artist(ArtistRole::Track, a.key.clone())));
                    }
                }
                if !track.album.is_empty() {
                    clipped(
                        ui.painter(),
                        pos2(x, cover.top() + 44.0),
                        &track.album,
                        regular(12.0),
                        theme::FAINT,
                        width,
                    );
                }
            }
            None => {
                ui.painter()
                    .rect_filled(cover, CornerRadius::same(6), theme::SURFACE);
                paint_icon(ui.painter(), cover.shrink(18.0), Icon::Note, theme::FAINT);
                ui.painter().text(
                    pos2(cover.right() + 12.0, cover.center().y),
                    Align2::LEFT_CENTER,
                    "Nothing playing",
                    regular(13.0),
                    theme::FAINT,
                );
            }
        }

        // Centre: transport buttons and the position bar.
        let has_track = entry.is_some();
        let playing = matches!(
            snapshot.phase,
            Phase::Playing | Phase::Buffering | Phase::Recovering
        ) && !snapshot.paused;
        let cy = center.top() + 22.0;
        let cx = center.center().x;
        let button = |ui: &mut Ui,
                      x: f32,
                      size: f32,
                      icon: Icon,
                      color: Color32,
                      tip: &str,
                      enabled: bool| {
            let rect = Rect::from_center_size(pos2(x, cy), Vec2::splat(size));
            let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
            child
                .add_enabled_ui(enabled, |ui| {
                    theme::icon_button(ui, icon, size, color, tip).clicked()
                })
                .inner
        };
        let shuffle_color = if self.settings.shuffle {
            theme::ACCENT_TEXT
        } else {
            theme::MUTED
        };
        if button(
            ui,
            cx - 104.0,
            32.0,
            Icon::Shuffle,
            shuffle_color,
            "Shuffle",
            true,
        ) {
            actions.push(Action::SetShuffle(!self.settings.shuffle));
        }
        if button(
            ui,
            cx - 54.0,
            36.0,
            Icon::Previous,
            theme::TEXT,
            "Previous",
            has_track,
        ) {
            actions.push(Action::Previous);
        }
        {
            let rect = Rect::from_center_size(pos2(cx, cy), Vec2::splat(42.0));
            let response = ui.interact(rect, Id::unique("np-play"), Sense::click());
            let enabled = has_track || !self.queue.is_empty();
            let fill = if !enabled {
                theme::SURFACE_HI
            } else if response.hovered() {
                Color32::WHITE
            } else {
                theme::CREAM
            };
            ui.painter().circle_filled(rect.center(), 20.0, fill);
            let icon = if playing { Icon::Pause } else { Icon::Play };
            let nudge = if playing { 0.0 } else { 1.5 };
            paint_icon(
                ui.painter(),
                Rect::from_center_size(rect.center() + vec2(nudge, 0.0), Vec2::splat(18.0)),
                icon,
                theme::BAR,
            );
            if enabled && response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                actions.push(Action::TogglePlay);
            }
            if matches!(snapshot.phase, Phase::Buffering | Phase::Recovering) {
                let t = ui.input(|i| i.time) as f32;
                let points: Vec<_> = (0..=16)
                    .map(|i| rect.center() + Vec2::angled(t * 5.0 + i as f32 * 0.1) * 23.5)
                    .collect();
                ui.painter().add(egui::Shape::line(
                    points,
                    Stroke::new(2.0, theme::ACCENT_TEXT),
                ));
            }
        }
        if button(
            ui,
            cx + 54.0,
            36.0,
            Icon::Next,
            theme::TEXT,
            "Next",
            has_track,
        ) {
            actions.push(Action::Next);
        }
        let (repeat_icon, repeat_color, repeat_tip) = match self.settings.repeat {
            Repeat::Off => (Icon::Repeat, theme::MUTED, "Repeat"),
            Repeat::All => (Icon::Repeat, theme::ACCENT_TEXT, "Repeat all"),
            Repeat::One => (Icon::RepeatOne, theme::ACCENT_TEXT, "Repeat one"),
        };
        if button(
            ui,
            cx + 104.0,
            32.0,
            repeat_icon,
            repeat_color,
            repeat_tip,
            true,
        ) {
            actions.push(Action::CycleRepeat);
        }
        for (on, x) in [
            (self.settings.shuffle, cx - 104.0),
            (self.settings.repeat != Repeat::Off, cx + 104.0),
        ] {
            if on {
                ui.painter()
                    .circle_filled(pos2(x, cy + 15.0), 1.8, theme::ACCENT_TEXT);
            }
        }

        let duration = snapshot
            .duration
            .or_else(|| entry.as_ref().map(|e| e.track.seconds).filter(|s| *s > 0.0));
        let position = self.seeking.map_or(snapshot.position, |(target, _)| target);
        let bar_y = center.bottom() - 14.0;
        let label_w = 46.0;
        let bar_width = (center.width() - 2.0 * label_w - 16.0).min(560.0);
        let bar = Rect::from_center_size(pos2(cx, bar_y), vec2(bar_width, 4.0));
        let can_seek = has_track
            && duration.is_some()
            && matches!(
                snapshot.phase,
                Phase::Playing | Phase::Buffering | Phase::Recovering
            );
        let fraction = duration.map_or(0.0, |d| (position / d).clamp(0.0, 1.0) as f32);
        let (shown, release, _) = slider(ui, Id::unique("seek"), bar, fraction, can_seek);
        let shown_position = duration.map_or(position, |d| f64::from(shown) * d);
        ui.painter().text(
            pos2(bar.left() - 10.0, bar_y),
            Align2::RIGHT_CENTER,
            if has_track {
                format_time(shown_position)
            } else {
                String::new()
            },
            regular(11.5),
            theme::MUTED,
        );
        ui.painter().text(
            pos2(bar.right() + 10.0, bar_y),
            Align2::LEFT_CENTER,
            match (has_track, duration) {
                (true, Some(d)) => format_time(d),
                (true, None) => "–:––".into(),
                _ => String::new(),
            },
            regular(11.5),
            theme::MUTED,
        );
        if let (Some(fraction), Some(duration)) = (release, duration) {
            actions.push(Action::Seek(f64::from(fraction) * duration));
        }

        // Right: stream quality, queue and volume.
        let vy = right.center().y;
        let volume_bar =
            Rect::from_min_size(pos2(right.right() - 100.0, vy - 2.0), vec2(96.0, 4.0));
        let shown_volume = if self.settings.muted {
            0.0
        } else {
            self.settings.volume
        };
        let (volume, release, dragging) =
            slider(ui, Id::unique("volume"), volume_bar, shown_volume, true);
        if dragging || release.is_some() {
            self.settings.volume = volume;
            self.settings.muted = false;
        }
        let level = if self.settings.muted || self.settings.volume < 0.01 {
            Icon::Muted
        } else if self.settings.volume < 0.5 {
            Icon::Volume(1)
        } else {
            Icon::Volume(2)
        };
        let mute_rect =
            Rect::from_center_size(pos2(volume_bar.left() - 20.0, vy), Vec2::splat(30.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(mute_rect));
        if theme::icon_button(
            &mut child,
            level,
            30.0,
            theme::MUTED,
            if self.settings.muted {
                "Unmute"
            } else {
                "Mute"
            },
        )
        .clicked()
        {
            self.settings.muted = !self.settings.muted;
        }
        let queue_rect =
            Rect::from_center_size(pos2(mute_rect.left() - 22.0, vy), Vec2::splat(30.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(queue_rect));
        let on_queue = self.page == Page::Queue || self.settings.queue_panel;
        if theme::icon_button(
            &mut child,
            Icon::Queue,
            30.0,
            if on_queue {
                theme::ACCENT_TEXT
            } else {
                theme::MUTED
            },
            "Queue",
        )
        .clicked()
        {
            actions.push(
                if ui.ctx().content_rect().width() >= 920.0 && self.page != Page::Queue {
                    Action::ToggleQueuePanel
                } else if self.page == Page::Queue {
                    Action::Back
                } else {
                    Action::Go(Page::Queue)
                },
            );
        }
        let route = match snapshot.route {
            Some(Route::Hls(q)) => format!("{} · HLS", quality_label(q)),
            Some(Route::Direct(q)) => quality_label(q).to_owned(),
            None => String::new(),
        };
        if has_track && !route.is_empty() {
            let galley = ui
                .painter()
                .layout_no_wrap(route.clone(), regular(11.0), theme::MUTED);
            let badge = Rect::from_min_size(
                pos2(queue_rect.left() - 12.0 - galley.size().x - 14.0, vy - 10.0),
                vec2(galley.size().x + 14.0, 20.0),
            );
            if badge.left() > right.left() {
                let rate = snapshot
                    .source_rate
                    .map(|rate| {
                        format!(
                            "{:.1} kHz → {:.1} kHz output",
                            rate as f64 / 1000.0,
                            self.player.output.rate as f64 / 1000.0
                        )
                    })
                    .unwrap_or_else(|| {
                        format!("{:.1} kHz output", self.player.output.rate as f64 / 1000.0)
                    });
                let format = self
                    .queue
                    .current_entry()
                    .map(|entry| entry.track.mime.as_str())
                    .unwrap_or("");
                ui.interact(badge, Id::unique("active-stream"), Sense::hover())
                    .on_hover_text(format!("{route} · {rate}\nStored file: {format}"));
                ui.painter().rect_stroke(
                    badge,
                    CornerRadius::same(10),
                    Stroke::new(1.0, theme::LINE),
                    egui::StrokeKind::Inside,
                );
                ui.painter().galley(
                    badge.min + vec2(7.0, 10.0 - galley.size().y / 2.0),
                    galley,
                    theme::MUTED,
                );
            }
        }
    }
}

/// A thin horizontal slider. Returns the fraction to show and, when the user lets go, the chosen fraction.
fn slider(
    ui: &mut Ui,
    id: Id,
    bar: Rect,
    fraction: f32,
    enabled: bool,
) -> (f32, Option<f32>, bool) {
    let hit = bar.expand2(vec2(6.0, 9.0));
    let sense = if enabled {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let response = ui.interact(hit, id, sense);
    let mut shown = fraction;
    let mut release = None;
    if enabled && let Some(pointer) = response.interact_pointer_pos() {
        let f = ((pointer.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
        if response.dragged() || response.is_pointer_button_down_on() {
            shown = f;
        }
        if response.drag_stopped() || response.clicked() {
            shown = f;
            release = Some(f);
        }
    }
    let active = enabled && (response.hovered() || response.dragged());
    let painter = ui.painter();
    painter.rect_filled(bar, CornerRadius::same(2), theme::LINE);
    let filled = Rect::from_min_max(
        bar.min,
        pos2(bar.left() + bar.width() * shown, bar.bottom()),
    );
    painter.rect_filled(
        filled,
        CornerRadius::same(2),
        if active {
            theme::ACCENT_TEXT
        } else {
            theme::TEXT.gamma_multiply(0.85)
        },
    );
    if active {
        painter.circle_filled(pos2(filled.right(), bar.center().y), 6.0, theme::CREAM);
    }
    let dragging = enabled && response.dragged();
    if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand);
    }
    (shown, release, dragging)
}

fn play_buttons_reversed(ui: &mut Ui, tracks: &[usize], actions: &mut Vec<Action>) {
    play_buttons(ui, tracks, actions, true);
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new(text).font(strong(10.5)).color(theme::FAINT));
    });
    ui.add_space(2.0);
}

fn nav_item(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    count: Option<usize>,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if selected {
            theme::SURFACE_HI
        } else if response.hovered() {
            theme::SURFACE
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(rect, CornerRadius::same(7), fill);
        let mut x = rect.left() + 10.0;
        let color = if selected { theme::TEXT } else { theme::MUTED };
        if let Some(icon) = icon {
            let r = Rect::from_center_size(pos2(x + 8.0, rect.center().y), Vec2::splat(16.0));
            paint_icon(
                painter,
                r,
                icon,
                if selected {
                    theme::ACCENT_TEXT
                } else {
                    theme::MUTED
                },
            );
            x += 26.0;
        }
        let count_text = count.map(|n| catalog::count(n, "", "").trim().to_owned());
        let count_w = count_text
            .as_ref()
            .map_or(0.0, |t| t.len() as f32 * 7.0 + 10.0);
        clipped(
            painter,
            pos2(x, rect.center().y - 9.0),
            label,
            if selected {
                strong(13.5)
            } else {
                regular(13.5)
            },
            if selected { theme::TEXT } else { color },
            rect.right() - x - count_w - 8.0,
        );
        if let Some(text) = count_text {
            painter.text(
                pos2(rect.right() - 10.0, rect.center().y),
                Align2::RIGHT_CENTER,
                text,
                regular(11.5),
                theme::FAINT,
            );
        }
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

fn artist_texture(
    art: &mut Artwork,
    catalog: &Catalog,
    artist: &Artist,
    cover: u64,
) -> Option<(egui::TextureId, Rect)> {
    let image = |art: &mut Artwork, id| {
        let texture = art.get(id, catalog.library.artwork_url(id, 300))?;
        let [width, height] = art.size(id)?;
        let side = width.min(height) as f32;
        let size = vec2(side / width as f32, side / height as f32);
        Some((texture, Rect::from_center_size(pos2(0.5, 0.5), size)))
    };
    if let Some(&portrait) = catalog.library.artist_portraits.get(&artist.key) {
        let texture = image(art, portrait);
        if texture.is_some() || !art.unavailable(portrait) {
            return texture;
        }
    }
    image(art, cover)
}

fn artist_card(
    ui: &mut Ui,
    rect: Rect,
    artist: &Artist,
    role: ArtistRole,
    texture: Option<(egui::TextureId, Rect)>,
) -> bool {
    let id = ui.make_persistent_id(("artist-card", &artist.key));
    let response = ui.interact(rect, id, Sense::click());
    let credits = artist.credits(role).unwrap();
    let albums = count(credits.albums.len(), "album", "albums");
    let songs = count(credits.tracks.len(), "song", "songs");
    let subtitle = match role {
        ArtistRole::Album => &albums,
        ArtistRole::Track => &songs,
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            ui.is_enabled(),
            format!("{} · {subtitle}", artist.name),
        )
    });
    let active = response.hovered() || response.has_focus();
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(10),
        if active {
            theme::SURFACE_HI
        } else {
            theme::SURFACE
        },
        Stroke::new(
            1.0,
            if active {
                theme::ACCENT_TEXT
            } else {
                theme::MUTED.gamma_multiply(0.35)
            },
        ),
        egui::StrokeKind::Inside,
    );
    let pad = 12.0;
    let width = (rect.width() - 2.0 * pad).max(1.0);
    let image = Rect::from_min_size(rect.min + vec2(pad, pad), Vec2::splat(width));
    artist_picture(painter, image, texture, &artist.name, 8);
    let mut title =
        egui::text::LayoutJob::simple_singleline(artist.name.clone(), strong(15.0), theme::TEXT);
    title.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let name = painter.layout_job(title);
    painter.galley(
        pos2(rect.center().x - name.size().x / 2.0, image.bottom() + 13.0),
        name,
        theme::TEXT,
    );
    painter.text(
        pos2(rect.center().x, image.bottom() + 36.0),
        Align2::CENTER_TOP,
        subtitle,
        regular(12.5),
        theme::MUTED,
    );
    response
        .on_hover_text(format!("{} · {albums} · {songs}", artist.name))
        .on_hover_cursor(CursorIcon::PointingHand)
        .clicked()
}

fn artist_picture(
    painter: &egui::Painter,
    rect: Rect,
    texture: Option<(egui::TextureId, Rect)>,
    name: &str,
    radius: u8,
) {
    match texture {
        Some((texture, uv)) => {
            painter.add(
                egui::epaint::RectShape::filled(rect, CornerRadius::same(radius), Color32::WHITE)
                    .with_texture(texture, uv),
            );
        }
        None => {
            painter.rect_filled(
                rect,
                CornerRadius::same(radius),
                theme::tint(name).gamma_multiply(0.6),
            );
            let initial: String = name
                .chars()
                .find(|c| c.is_alphanumeric())
                .unwrap_or('?')
                .to_uppercase()
                .collect();
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                initial,
                strong((rect.width() * 0.4).min(54.0)),
                theme::CREAM,
            );
        }
    }
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    #[test]
    fn disc_headings_do_not_change_playback_row_indices() {
        let sections = [
            DiscSection {
                start: 0,
                number: 1,
                encoded: true,
            },
            DiscSection {
                start: 2,
                number: 2,
                encoded: true,
            },
        ];
        assert_eq!(list_row(&sections, 0), ListRow::Disc(1));
        assert_eq!(list_row(&sections, 3), ListRow::Disc(2));
        assert_eq!(
            list_row(&sections, 4),
            ListRow::Track {
                position: 2,
                encoded: true
            }
        );
        assert_eq!(
            list_row(&[], 4),
            ListRow::Track {
                position: 4,
                encoded: false
            }
        );
    }

    #[test]
    fn changing_order_discards_the_previous_scroll_position() {
        let ctx = egui::Context::default();
        let show = |offset, reset| {
            let mut first_row = 0;
            ctx.run_ui(egui::RawInput::default(), |ui| {
                let mut scroll = grid_scroll(ui, "test-albums", reset).max_height(150.0);
                if let Some(offset) = offset {
                    scroll = scroll.vertical_scroll_offset(offset);
                }
                first_row = scroll.show_rows(ui, 60.0, 100, |_, rows| rows.start).inner;
            })
            .drop_without_applying_deltas();
            first_row
        };
        assert!(show(Some(800.0), false) > 0);
        assert!(show(None, false) > 0);
        assert_eq!(show(None, true), 0);
        assert_eq!(show(None, false), 0);
    }
}
