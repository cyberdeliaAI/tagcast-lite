//! Compact playback in the existing native window.
use eframe::egui::{self, ViewportCommand as Cmd, pos2, vec2};
use serde::{Deserialize, Serialize};

pub(super) const FULL_MIN: egui::Vec2 = vec2(820.0, 540.0);
const MINI_SIZE: egui::Vec2 = vec2(420.0, 136.0);
const MINI_MIN: egui::Vec2 = vec2(360.0, 136.0);
const COVER: f32 = 88.0;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(super) struct LibraryWindow {
    size: egui::Vec2,
    position: Option<egui::Pos2>,
    maximized: bool,
    fullscreen: bool,
}

impl LibraryWindow {
    fn capture(ctx: &egui::Context) -> Self {
        ctx.input(|i| {
            let v = i.viewport();
            Self {
                size: v
                    .inner_rect
                    .unwrap_or(i.content_rect())
                    .size()
                    .max(FULL_MIN),
                position: v.outer_rect.map(|r| r.min),
                maximized: v.maximized.unwrap_or(false),
                fullscreen: v.fullscreen.unwrap_or(false),
            }
        })
    }

    fn restore(self, ctx: &egui::Context, restore_position: bool) {
        ctx.send_viewport_cmd(Cmd::MinInnerSize(FULL_MIN));
        ctx.send_viewport_cmd(Cmd::InnerSize(self.size));
        if restore_position && let Some(pos) = self.position {
            ctx.send_viewport_cmd(Cmd::OuterPosition(pos));
        }
        if self.maximized {
            ctx.send_viewport_cmd(Cmd::Maximized(true));
        }
        if self.fullscreen {
            ctx.send_viewport_cmd(Cmd::Fullscreen(true));
        }
    }
}

#[derive(Default)]
pub(super) struct MiniPlayer {
    /// Only set while the compact view is open.
    pub(super) library_window: Option<LibraryWindow>,
    pending_resize: bool,
    restore_at_start: Option<LibraryWindow>,
    /// Keeps the compact window above other windows. The library window never floats.
    pub(super) on_top: bool,
}

impl MiniPlayer {
    pub(super) fn new(storage: Option<&dyn eframe::Storage>) -> Self {
        Self {
            restore_at_start: storage
                .and_then(|s| eframe::get_value(s, "mini-return-window"))
                .flatten(),
            on_top: storage
                .and_then(|s| eframe::get_value(s, "mini-on-top"))
                .unwrap_or(false),
            ..Default::default()
        }
    }
    pub(super) fn active(&self) -> bool {
        self.library_window.is_some()
    }

    pub(super) fn toggle(&mut self, ctx: &egui::Context) {
        if let Some(window) = self.library_window.take() {
            self.pending_resize = false;
            ctx.send_viewport_cmd(Cmd::WindowLevel(egui::WindowLevel::Normal));
            window.restore(ctx, true);
        } else {
            let window = LibraryWindow::capture(ctx);
            self.library_window = Some(window);
            if window.fullscreen {
                ctx.send_viewport_cmd(Cmd::Fullscreen(false));
            }
            if window.maximized {
                ctx.send_viewport_cmd(Cmd::Maximized(false));
            }
            self.pending_resize = true;
            self.update(ctx);
            if self.on_top {
                ctx.send_viewport_cmd(Cmd::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            }
        }
        ctx.request_repaint();
    }

    pub(super) fn set_on_top(&mut self, ctx: &egui::Context, on_top: bool) {
        self.on_top = on_top;
        if self.active() {
            let level = if on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            };
            ctx.send_viewport_cmd(Cmd::WindowLevel(level));
        }
    }

    pub(super) fn update(&mut self, ctx: &egui::Context) {
        if let Some(window) = self.restore_at_start.take() {
            // Keep the OS-selected monitor if the display setup changed since closing.
            window.restore(ctx, false);
        }
        if self.pending_resize {
            if ctx.input(|i| {
                i.viewport().fullscreen == Some(true) || i.viewport().maximized == Some(true)
            }) {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            } else {
                if let Some(window) = &mut self.library_window
                    && (window.maximized || window.fullscreen)
                {
                    // The OS has now restored the windowed bounds from before maximization.
                    let normal = LibraryWindow::capture(ctx);
                    window.size = normal.size;
                    window.position = normal.position;
                }
                ctx.send_viewport_cmd(Cmd::MinInnerSize(MINI_MIN));
                ctx.send_viewport_cmd(Cmd::InnerSize(MINI_SIZE));
                self.pending_resize = false;
            }
        }
    }
}

impl super::App {
    /// Cover, title, transport and position, in a window about the size of a
    /// sticky note.
    pub(super) fn compact_player(&mut self, ui: &mut egui::Ui, actions: &mut Vec<super::Action>) {
        use super::{
            Action,
            artwork::ArtSize,
            catalog::artist_label,
            theme::{self, Icon, Size, clipped, regular, strong},
            views::play_button,
        };
        use tagcast_player::audio::Phase;
        let colors = theme::colors(ui.ctx());
        let snapshot = self.player.output.snapshot();
        let (_, playing) = self.now();
        let entry = self.queue.current_entry().cloned();
        let area = ui.max_rect();
        let cover = egui::Rect::from_min_size(
            pos2(area.left(), area.center().y - COVER / 2.0),
            vec2(COVER, COVER),
        );
        let texture = entry.as_ref().and_then(|e| {
            let catalog = self.catalog.as_ref()?;
            let id = catalog
                .album_of(&e.track)
                .map_or(e.track.artwork, |a| a.artwork);
            self.art
                .get(id, ArtSize::Grid, &catalog.library)
                .map(|image| image.0)
        });
        let album = entry.as_ref().map_or("", |e| e.track.album.as_str());
        theme::cover(ui.painter(), cover, texture, 8, album);

        // The window corner holds the pin and the way back to the library.
        let corner = |offset: f32| {
            egui::Rect::from_min_size(
                pos2(area.right() - offset, cover.top() - 1.0),
                vec2(28.0, 28.0),
            )
        };
        let mut back = ui.new_child(egui::UiBuilder::new().max_rect(corner(28.0)));
        if theme::icon_button(
            &mut back,
            Icon::ExpandPlayer,
            28.0,
            colors.muted,
            "Return to library",
        )
        .clicked()
        {
            actions.push(Action::ToggleMiniPlayer);
        }
        let (pin_color, pin_tip) = if self.mini.on_top {
            (colors.accent_text, "Stop keeping the mini player on top")
        } else {
            (colors.muted, "Keep the mini player on top")
        };
        let mut pin = ui.new_child(egui::UiBuilder::new().max_rect(corner(60.0)));
        let pinned = self.mini.on_top;
        let pin = theme::icon_button(&mut pin, Icon::Pin(pinned), 28.0, pin_color, pin_tip);
        pin.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, pinned, "Keep on top")
        });
        if pin.clicked() {
            actions.push(Action::ToggleMiniOnTop);
        }

        // Title and credits, left aligned beside the cover.
        let x = cover.right() + 14.0;
        let column =
            egui::Rect::from_min_max(pos2(x, cover.top()), pos2(area.right(), cover.bottom()));
        let (title, credits) = match &entry {
            Some(e) => {
                let artist = artist_label(&e.track.artist);
                let credits = match (artist.is_empty(), album.is_empty()) {
                    (false, false) => format!("{artist} · {album}"),
                    (false, true) => artist.to_owned(),
                    (true, _) => album.to_owned(),
                };
                (e.track.title.as_str(), credits)
            }
            None => ("Nothing playing", String::new()),
        };
        let title_rect = clipped(
            ui.painter(),
            pos2(x, cover.top() + 1.0),
            title,
            strong(Size::BODY),
            if entry.is_some() {
                colors.text
            } else {
                colors.faint
            },
            corner(60.0).left() - x - 6.0,
        );
        ui.interact(
            title_rect,
            ui.scope_id().with("mini-title"),
            egui::Sense::hover(),
        )
        .on_hover_text(title);
        clipped(
            ui.painter(),
            pos2(x, cover.top() + 21.0),
            &credits,
            regular(Size::SMALL),
            colors.muted,
            column.width(),
        );

        // Transport, centred in the column, with the position bar beneath it.
        let has_track = entry.is_some();
        let cx = column.center().x;
        let cy = cover.top() + 54.0;
        let mut controls = ui.new_child(egui::UiBuilder::new().max_rect(column));
        for (offset, icon, name, action, enabled) in [
            (
                -46.0,
                Icon::Previous,
                "Previous",
                Action::Previous,
                has_track,
            ),
            (46.0, Icon::Next, "Next", Action::Next, has_track),
        ] {
            let rect = egui::Rect::from_center_size(pos2(cx + offset, cy), vec2(32.0, 32.0));
            let mut child = controls.new_child(egui::UiBuilder::new().max_rect(rect));
            let clicked = child
                .add_enabled_ui(enabled, |ui| {
                    theme::icon_button(ui, icon, 32.0, colors.text, name).clicked()
                })
                .inner;
            if clicked {
                actions.push(action);
            }
        }
        if play_button(
            ui,
            ui.scope_id().with("mini-play"),
            pos2(cx, cy),
            36.0,
            playing,
            has_track || !self.queue.is_empty(),
            matches!(snapshot.phase, Phase::Buffering | Phase::Recovering),
        ) {
            actions.push(Action::TogglePlay);
        }
        let bar = egui::Rect::from_min_max(
            pos2(column.left() + 42.0, cover.bottom() - 6.0),
            pos2(column.right() - 42.0, cover.bottom() - 2.0),
        );
        if has_track {
            self.seek_bar(
                ui,
                ui.scope_id().with("mini-seek"),
                bar,
                &snapshot,
                entry.as_ref(),
                actions,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_mini_player_floats_and_only_when_pinned() {
        let ctx = egui::Context::default();
        let levels = |mini: &mut MiniPlayer, change: &dyn Fn(&mut MiniPlayer, &egui::Context)| {
            let mut output = ctx.run_ui(Default::default(), |ui| change(mini, ui.ctx()));
            output.textures_delta.clear();
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .filter_map(|cmd| match cmd {
                    Cmd::WindowLevel(level) => Some(*level),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let mut mini = MiniPlayer::default();
        // Pinning in the library window changes nothing yet.
        assert!(levels(&mut mini, &|m, ctx| m.set_on_top(ctx, true)).is_empty());
        let floating = levels(&mut mini, &|m, ctx| m.toggle(ctx));
        assert_eq!(floating, [egui::WindowLevel::AlwaysOnTop]);
        let unpinned = levels(&mut mini, &|m, ctx| m.set_on_top(ctx, false));
        assert_eq!(unpinned, [egui::WindowLevel::Normal]);
        mini.on_top = true;
        let library = levels(&mut mini, &|m, ctx| m.toggle(ctx));
        assert_eq!(library, [egui::WindowLevel::Normal]);
    }

    #[test]
    fn closing_in_mini_mode_preserves_the_library_window_for_the_next_launch() {
        let mut input = egui::RawInput::default();
        let size = vec2(1024.0, 680.0);
        let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        viewport.inner_rect = Some(egui::Rect::from_min_size(pos2(80.0, 100.0), size));
        let ctx = egui::Context::default();
        let mut mini = MiniPlayer::default();
        let mut compact = ctx.run_ui(input, |ui| mini.toggle(ui.ctx()));
        compact.textures_delta.clear();
        assert!(
            compact.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|cmd| matches!(cmd, Cmd::InnerSize(s) if *s == MINI_SIZE))
        );
        let mut reopened = MiniPlayer {
            restore_at_start: mini.library_window,
            ..Default::default()
        };
        let mut restored = ctx.run_ui(Default::default(), |ui| reopened.update(ui.ctx()));
        restored.textures_delta.clear();
        assert!(!reopened.active());
        assert!(
            restored.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|cmd| matches!(cmd, Cmd::InnerSize(s) if *s == size))
        );
        assert!(reopened.restore_at_start.is_none());
    }
}
