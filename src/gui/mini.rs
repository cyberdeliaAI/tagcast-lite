//! Compact playback in the existing native window.
use eframe::egui::{self, ViewportCommand as Cmd, pos2, vec2};
use serde::{Deserialize, Serialize};

pub(super) const FULL_MIN: egui::Vec2 = vec2(820.0, 540.0);
const MINI_SIZE: egui::Vec2 = vec2(392.0, 116.0);
const MINI_MIN: egui::Vec2 = vec2(300.0, 112.0);

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
}

impl MiniPlayer {
    pub(super) fn new(storage: Option<&dyn eframe::Storage>) -> Self {
        Self {
            restore_at_start: storage
                .and_then(|s| eframe::get_value(s, "mini-return-window"))
                .flatten(),
            ..Default::default()
        }
    }
    pub(super) fn active(&self) -> bool {
        self.library_window.is_some()
    }

    pub(super) fn toggle(&mut self, ctx: &egui::Context) {
        if let Some(window) = self.library_window.take() {
            self.pending_resize = false;
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
        }
        ctx.request_repaint();
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
    pub(super) fn compact_player(&mut self, ui: &mut egui::Ui, actions: &mut Vec<super::Action>) {
        use super::{
            Action,
            artwork::ArtSize,
            catalog::artist_label,
            theme::{self, Icon, Size, clipped, regular, strong},
        };
        let colors = theme::colors(ui.ctx());
        let (_, playing) = self.now();
        let entry = self.queue.current_entry();
        let cover = egui::Rect::from_min_size(
            ui.max_rect().left_center() - vec2(0.0, 32.0),
            vec2(64.0, 64.0),
        );
        let texture = entry.and_then(|e| {
            let catalog = self.catalog.as_ref()?;
            let id = catalog
                .album_of(&e.track)
                .map_or(e.track.artwork, |a| a.artwork);
            self.art
                .get(id, ArtSize::Small, &catalog.library)
                .map(|image| image.0)
        });
        let (title, artist, album) = entry.map_or(("Nothing playing", "", ""), |e| {
            (
                e.track.title.as_str(),
                artist_label(&e.track.artist),
                e.track.album.as_str(),
            )
        });
        theme::cover(ui.painter(), cover, texture, 7, album);
        let x = cover.right() + 12.0;
        let width = ui.max_rect().right() - x - 38.0;
        let top = cover.top() - 4.0;
        let title_rect = clipped(
            ui.painter(),
            pos2(x, top),
            title,
            strong(Size::BODY),
            colors.text,
            width,
        );
        ui.interact(
            title_rect,
            ui.scope_id().with("mini-title"),
            egui::Sense::hover(),
        )
        .on_hover_text(title);
        clipped(
            ui.painter(),
            pos2(x, top + 19.0),
            artist,
            regular(Size::SMALL),
            colors.muted,
            width,
        );
        let controls = egui::Rect::from_min_size(pos2(x - 4.0, top + 36.0), vec2(120.0, 32.0));
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(controls)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.add_enabled_ui(entry.is_some() || !self.queue.is_empty(), |ui| {
                    for (icon, name, action) in [
                        (Icon::Previous, "Previous", Action::Previous),
                        (
                            if playing { Icon::Pause } else { Icon::Play },
                            if playing { "Pause" } else { "Play" },
                            Action::TogglePlay,
                        ),
                        (Icon::Next, "Next", Action::Next),
                    ] {
                        if theme::icon_button(ui, icon, 30.0, colors.text, name).clicked() {
                            actions.push(action);
                        }
                    }
                });
            },
        );
        let restore = egui::Rect::from_min_size(
            pos2(ui.max_rect().right() - 32.0, cover.top() - 4.0),
            vec2(32.0, 32.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(restore));
        if theme::icon_button(
            &mut child,
            Icon::ExpandPlayer,
            32.0,
            colors.muted,
            "Return to library",
        )
        .clicked()
        {
            actions.push(Action::ToggleMiniPlayer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
