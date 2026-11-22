//! Local appearance settings and the settings dialog.

use std::sync::OnceLock;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Layout, Margin, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2, pos2, vec2,
};
use serde::{Deserialize, Serialize};
use tagcast_player::{APP_NAME, Failure, transport::Quality};

use super::{
    Action, QUALITIES,
    catalog::count,
    quality_label,
    theme::{self, Colors, Icon, Size, Swatches, Tone, regular, strong},
    views::nav_item,
};

const MAX_CUSTOM: usize = 20;
const REPOSITORY: &str = "https://github.com/cyberdeliaAI/tagcast-lite";
/// The dialog keeps one size, so it does not move when the section changes.
const WIDTH: f32 = 680.0;
const HEIGHT: f32 = 480.0;
const RAIL: f32 = 176.0;
/// Room beside the content for the floating scroll bar.
const GUTTER: f32 = 14.0;
const FOOTER: f32 = 58.0;

#[derive(Clone, Deserialize, Serialize)]
pub struct Palette {
    name: String,
    light: Swatches,
    dark: Swatches,
}

impl Palette {
    pub fn colors(&self, dark: bool) -> Colors {
        Colors::new(if dark { self.dark } else { self.light }, dark)
    }
}

pub fn presets() -> &'static [Palette] {
    static PRESETS: OnceLock<Vec<Palette>> = OnceLock::new();
    PRESETS.get_or_init(|| {
        serde_json::from_str(include_str!("../../assets/themes/palettes.json"))
            .expect("bundled themes are valid")
    })
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Serialize)]
enum Mode {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Preferences {
    mode: Mode,
    selected: String,
    custom: Vec<Palette>,
}

impl Preferences {
    fn current(&self) -> &Palette {
        presets()
            .iter()
            .chain(&self.custom)
            .find(|palette| palette.name == self.selected)
            .unwrap_or(&presets()[0])
    }

    fn custom_index(&self) -> Option<usize> {
        self.custom
            .iter()
            .position(|item| item.name == self.selected)
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let palette = self.current();
        theme::apply(ctx, palette.colors(false), palette.colors(true));
        ctx.set_theme(match self.mode {
            Mode::Dark => egui::ThemePreference::Dark,
            Mode::Light => egui::ThemePreference::Light,
            Mode::System => egui::ThemePreference::System,
        });
        ctx.request_repaint();
    }

    fn can_save(&self, draft: &Draft) -> bool {
        let name = draft.palette.name.trim();
        !name.is_empty()
            && name.chars().count() <= 60
            && (draft.index.is_some() || self.custom.len() < MAX_CUSTOM)
            && !presets()
                .iter()
                .any(|palette| palette.name.eq_ignore_ascii_case(name))
            && !self.custom.iter().enumerate().any(|(index, palette)| {
                Some(index) != draft.index && palette.name.eq_ignore_ascii_case(name)
            })
    }

    fn save_draft(&mut self, mut draft: Draft) {
        draft.palette.name = draft.palette.name.trim().to_owned();
        self.selected = draft.palette.name.clone();
        if let Some(index) = draft.index {
            self.custom[index] = draft.palette;
        } else {
            self.custom.push(draft.palette);
        }
    }
}

struct Draft {
    palette: Palette,
    index: Option<usize>,
    dark: bool,
}

/// Whether the text stays legible on every panel colour, in both variants.
fn readable(palette: &Palette) -> bool {
    [false, true].into_iter().all(|dark| {
        let colors = palette.colors(dark);
        [
            colors.bg,
            colors.sidebar,
            colors.bar,
            colors.surface,
            colors.surface_hi,
        ]
        .into_iter()
        .all(|background| theme::contrast(colors.text, background) >= 3.0)
    })
}

#[derive(Clone, Copy, Default, PartialEq)]
pub enum Section {
    #[default]
    Playback,
    Appearance,
    Account,
    Shortcuts,
    About,
}

impl Section {
    const ALL: [Self; 5] = [
        Self::Playback,
        Self::Appearance,
        Self::Account,
        Self::Shortcuts,
        Self::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Playback => "Playback",
            Self::Appearance => "Appearance",
            Self::Account => "Account",
            Self::Shortcuts => "Shortcuts",
            Self::About => "About",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Playback => Icon::Play,
            Self::Appearance => Icon::Contrast,
            Self::Account => Icon::Artist,
            Self::Shortcuts => Icon::Keyboard,
            Self::About => Icon::Info,
        }
    }
}

pub struct SettingsState<'a> {
    pub quality: Quality,
    pub premium: bool,
    pub gapless: bool,
    pub signed_in: bool,
    pub loading: bool,
    pub library_error: Option<&'a Failure>,
    /// Songs, albums and playlists, once the library has loaded.
    pub library: Option<[usize; 3]>,
    pub output: Option<&'a str>,
    pub audio_error: Option<&'a str>,
    pub logo: Option<&'a egui::TextureHandle>,
}

impl SettingsState<'_> {
    fn active_quality(&self) -> Quality {
        if self.premium {
            self.quality
        } else {
            Quality::K128
        }
    }
}

#[derive(Default)]
pub struct SettingsWindow {
    pub open: bool,
    section: Section,
    draft: Option<Draft>,
    confirm_delete: bool,
    confirm_sign_out: bool,
}

impl SettingsWindow {
    pub fn open(&mut self, section: Section) {
        self.section = section;
        self.open = true;
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        preferences: &mut Preferences,
        state: SettingsState<'_>,
        actions: &mut Vec<Action>,
    ) {
        if !self.open {
            return;
        }
        if self.section == Section::Account && !state.signed_in {
            self.section = Section::Playback;
        }
        if state.signed_in
            && !state.loading
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::R))
        {
            actions.push(Action::Reload);
        }
        let colors = theme::colors(ctx);
        let editing = self.draft.is_some();
        let screen = ctx.content_rect().size();
        let size = vec2(WIDTH.min(screen.x - 32.0), HEIGHT.min(screen.y - 32.0));
        let frame = egui::Frame::new()
            .fill(colors.bg)
            .stroke(Stroke::new(1.0, colors.line))
            .corner_radius(12)
            .shadow(ctx.global_style().visuals.popup_shadow);
        let mut changed = false;
        let mut closed = false;
        let modal = egui::Modal::new(egui::Id::unique("settings"))
            .frame(frame)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                // The theme editor takes the whole dialog; everything else sits beside the rail.
                let mut left = rect.left();
                if !editing {
                    let rail = Rect::from_min_max(rect.min, pos2(left + RAIL, rect.bottom()));
                    let corners = CornerRadius {
                        nw: 11,
                        ne: 0,
                        sw: 11,
                        se: 0,
                    };
                    ui.painter().rect_filled(rail, corners, colors.sidebar);
                    let mut side = ui.new_child(
                        UiBuilder::new()
                            .id_salt("rail")
                            .max_rect(rail.shrink2(vec2(10.0, 16.0)))
                            .layout(Layout::top_down(Align::Min)),
                    );
                    self.rail(&mut side, state.signed_in);
                    left = rail.right();
                }
                let body = Rect::from_min_max(
                    pos2(left + 24.0, rect.top()),
                    pos2(rect.right() - 10.0, rect.bottom()),
                );
                let mut main = ui.new_child(
                    UiBuilder::new()
                        .id_salt("body")
                        .max_rect(body)
                        .layout(Layout::top_down(Align::Min)),
                );
                (changed, closed) = self.body(&mut main, preferences, &state, actions);
            });
        if changed {
            preferences.apply(ctx);
        }
        let dismissed = modal.should_close();
        if dismissed && editing {
            self.draft = None;
        } else if closed || dismissed {
            self.open = false;
            self.draft = None;
            self.confirm_delete = false;
            self.confirm_sign_out = false;
        }
    }

    fn rail(&mut self, ui: &mut Ui, signed_in: bool) {
        let colors = theme::colors(ui.ctx());
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(
                RichText::new("Settings")
                    .font(strong(Size::LEAD))
                    .color(colors.text),
            );
        });
        ui.add_space(10.0);
        for section in Section::ALL {
            if section == Section::Account && !signed_in {
                continue;
            }
            let selected = self.section == section;
            if nav_item(ui, Some(section.icon()), section.label(), None, selected).clicked()
                && !selected
            {
                self.section = section;
                self.confirm_delete = false;
                self.confirm_sign_out = false;
            }
        }
        ui.painter().text(
            ui.max_rect().left_bottom() + vec2(10.0, 0.0),
            Align2::LEFT_BOTTOM,
            concat!("Version ", env!("CARGO_PKG_VERSION")),
            regular(Size::MICRO),
            colors.faint,
        );
    }

    /// Returns whether the appearance changed and whether the window was closed.
    fn body(
        &mut self,
        ui: &mut Ui,
        preferences: &mut Preferences,
        state: &SettingsState<'_>,
        actions: &mut Vec<Action>,
    ) -> (bool, bool) {
        let colors = theme::colors(ui.ctx());
        let editing = self.draft.is_some();
        let (mut changed, mut closed, mut back) = (false, false, false);
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(14.0);
        let title = match &self.draft {
            Some(draft) if draft.index.is_some() => "Edit theme",
            Some(_) => "New theme",
            None => self.section.label(),
        };
        ui.horizontal(|ui| {
            ui.set_height(32.0);
            if editing {
                back = theme::icon_button(ui, Icon::Back, 28.0, colors.muted, "Back to Appearance")
                    .clicked();
            }
            ui.label(
                RichText::new(title)
                    .font(strong(Size::HEADING))
                    .color(colors.text),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !editing {
                    let close = theme::icon_button(ui, Icon::Close, 28.0, colors.muted, "Close");
                    close.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, "Close settings")
                    });
                    closed = close.clicked();
                }
            });
        });
        ui.add_space(10.0);
        let footer = if editing { FOOTER } else { 0.0 };
        egui::ScrollArea::vertical()
            .id_salt(if editing {
                "theme-editor"
            } else {
                self.section.label()
            })
            .max_height(ui.available_height() - footer)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - GUTTER);
                if let Some(draft) = &mut self.draft {
                    editor(ui, draft);
                } else {
                    match self.section {
                        Section::Playback => playback(ui, state, actions),
                        Section::Appearance => changed |= self.appearance(ui, preferences),
                        Section::Account => closed |= self.account(ui, state, actions),
                        Section::Shortcuts => shortcuts(ui),
                        Section::About => about(ui, state),
                    }
                }
                ui.add_space(18.0);
            });
        if editing {
            changed |= self.editor_footer(ui, preferences, back);
        }
        (changed, closed)
    }

    fn appearance(&mut self, ui: &mut Ui, preferences: &mut Preferences) -> bool {
        let colors = theme::colors(ui.ctx());
        let mut changed = false;
        let mut selection = None;
        caption(ui, "THEME");
        card(ui, |ui| {
            row(ui, "Mode", None, |ui| {
                changed |= theme::segmented(
                    ui,
                    "mode",
                    &mut preferences.mode,
                    &[
                        (Mode::Dark, "Dark"),
                        (Mode::Light, "Light"),
                        (Mode::System, "System"),
                    ],
                );
            });
            divider(ui);
            row(ui, "Theme", None, |ui| {
                egui::ComboBox::from_id_salt("theme-preset")
                    .selected_text(&preferences.current().name)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for palette in presets() {
                            let selected = palette.name == preferences.current().name;
                            if ui.selectable_label(selected, &palette.name).clicked() {
                                selection = Some(palette.name.clone());
                            }
                        }
                        if !preferences.custom.is_empty() {
                            ui.separator();
                            ui.label(RichText::new("Custom themes").color(colors.muted));
                            for palette in &preferences.custom {
                                let selected = palette.name == preferences.selected;
                                if ui.selectable_label(selected, &palette.name).clicked() {
                                    selection = Some(palette.name.clone());
                                }
                            }
                        }
                    });
            });
        });
        if let Some(selection) = selection {
            preferences.selected = selection;
            self.confirm_delete = false;
            changed = true;
        }

        ui.add_space(12.0);
        let dark = ui.ctx().theme() == egui::Theme::Dark;
        preview(ui, preferences.current().colors(dark));
        ui.add_space(12.0);

        let custom_index = preferences.custom_index();
        let name = preferences.current().name.clone();
        let (mut create, mut edit) = (false, false);
        card(ui, |ui| {
            if self.confirm_delete
                && let Some(index) = custom_index
            {
                let title = format!("Delete “{name}”?");
                let detail = ("This cannot be undone.", colors.muted);
                row(ui, &title, Some(detail), |ui| {
                    if theme::button(ui, "Delete", Tone::Danger).clicked() {
                        preferences.custom.remove(index);
                        preferences.selected.clear();
                        self.confirm_delete = false;
                        changed = true;
                    }
                    if theme::button(ui, "Cancel", Tone::Plain).clicked() {
                        self.confirm_delete = false;
                    }
                });
                return;
            }
            let detail = if custom_index.is_some() {
                "This is one of your own themes."
            } else {
                "Duplicate a theme to change its colours."
            };
            row(ui, "Custom themes", Some((detail, colors.muted)), |ui| {
                if custom_index.is_some() {
                    if theme::button(ui, "Delete…", Tone::Plain).clicked() {
                        self.confirm_delete = true;
                    }
                    edit = theme::button(ui, "Edit", Tone::Plain).clicked();
                }
                ui.add_enabled_ui(preferences.custom.len() < MAX_CUSTOM, |ui| {
                    create = theme::button(ui, "Duplicate", Tone::Plain)
                        .on_disabled_hover_text("Delete a custom theme to make room for another.")
                        .clicked();
                });
            });
        });
        if create || edit {
            let mut palette = preferences.current().clone();
            if create {
                palette.name = format!("{} copy", palette.name);
            }
            self.draft = Some(Draft {
                palette,
                index: if edit { custom_index } else { None },
                dark,
            });
            self.confirm_delete = false;
        }
        changed
    }

    /// Returns whether the window should close.
    fn account(
        &mut self,
        ui: &mut Ui,
        state: &SettingsState<'_>,
        actions: &mut Vec<Action>,
    ) -> bool {
        let colors = theme::colors(ui.ctx());
        let mut closed = false;
        caption(ui, "IBROADCAST");
        card(ui, |ui| {
            if self.confirm_sign_out {
                let detail = ("Playback stops and the queue is cleared.", colors.muted);
                row(ui, "Sign out of iBroadcast?", Some(detail), |ui| {
                    if theme::button(ui, "Sign out", Tone::Danger).clicked() {
                        actions.push(Action::SignOut);
                        closed = true;
                    }
                    if theme::button(ui, "Cancel", Tone::Plain).clicked() {
                        self.confirm_sign_out = false;
                    }
                });
                return;
            }
            let plan = match (state.library.is_some(), state.premium) {
                (false, _) => "iBroadcast account",
                (true, true) => "iBroadcast Premium",
                (true, false) => "iBroadcast Free",
            };
            let detail = (
                "Signed in. Tagcast Lite only reads your library.",
                colors.muted,
            );
            row(ui, plan, Some(detail), |ui| {
                if theme::button(ui, "Sign out…", Tone::Plain).clicked() {
                    self.confirm_sign_out = true;
                }
            });
        });

        caption(ui, "LIBRARY");
        card(ui, |ui| {
            let (detail, color) = match (state.library, state.loading, state.library_error) {
                (Some([songs, albums, playlists]), _, _) => (
                    format!(
                        "{} · {} · {}",
                        count(songs, "song", "songs"),
                        count(albums, "album", "albums"),
                        count(playlists, "playlist", "playlists"),
                    ),
                    colors.muted,
                ),
                (None, true, _) => ("Loading…".to_owned(), colors.muted),
                (None, false, Some(error)) => (error.to_string(), colors.warn),
                (None, false, None) => ("Library unavailable.".to_owned(), colors.muted),
            };
            row(ui, "Your library", Some((&detail, color)), |ui| {
                let label = if state.loading {
                    "Updating…"
                } else {
                    "Reload"
                };
                let shortcut = if cfg!(target_os = "macos") {
                    "⌘R"
                } else {
                    "Ctrl+R"
                };
                ui.add_enabled_ui(!state.loading, |ui| {
                    let reload = theme::button(ui, label, Tone::Plain).on_hover_text(format!(
                        "Fetch the latest library from iBroadcast ({shortcut})"
                    ));
                    if reload.clicked() {
                        actions.push(Action::Reload);
                    }
                });
            });
        });
        closed
    }

    fn editor_footer(&mut self, ui: &mut Ui, preferences: &mut Preferences, back: bool) -> bool {
        let colors = theme::colors(ui.ctx());
        let draft = self.draft.as_ref().unwrap();
        let problem = if !preferences.can_save(draft) {
            Some("Give the theme a unique name.")
        } else if !readable(&draft.palette) {
            Some("Text needs more contrast with the panel colours in both variants.")
        } else {
            None
        };
        let width = ui.available_width() - GUTTER;
        let (line, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
        ui.painter().hline(
            line.x_range(),
            line.center().y,
            Stroke::new(1.0, colors.line),
        );
        let (area, _) = ui.allocate_exact_size(vec2(width, FOOTER - 1.0), Sense::hover());
        let mut bar = ui.new_child(
            UiBuilder::new()
                .id_salt("footer")
                .max_rect(area)
                .layout(Layout::right_to_left(Align::Center)),
        );
        let (mut save, mut cancel) = (false, back);
        bar.add_enabled_ui(problem.is_none(), |ui| {
            save = theme::button(ui, "Save and apply", Tone::Primary).clicked();
        });
        cancel |= theme::button(&mut bar, "Cancel", Tone::Plain).clicked();
        if let Some(problem) = problem {
            bar.add_space(8.0);
            bar.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.label(
                    RichText::new(problem)
                        .font(regular(Size::SMALL))
                        .color(colors.warn),
                );
            });
        }
        if save {
            preferences.save_draft(self.draft.take().unwrap());
        } else if cancel {
            self.draft = None;
        }
        save
    }
}

// ---- building blocks ------------------------------------------------------

/// A small heading above a group of settings.
fn caption(ui: &mut Ui, text: &str) {
    let colors = theme::colors(ui.ctx());
    if ui.min_rect().height() > 1.0 {
        ui.add_space(16.0);
    }
    ui.label(
        RichText::new(text)
            .font(strong(Size::MICRO))
            .color(colors.faint),
    );
    ui.add_space(7.0);
}

/// A rounded panel that groups related rows.
fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let colors = theme::colors(ui.ctx());
    egui::Frame::new()
        .fill(colors.surface)
        .corner_radius(10)
        .inner_margin(Margin::symmetric(14, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            theme::on_surface(ui);
            add(ui);
        });
}

fn divider(ui: &mut Ui) {
    let colors = theme::colors(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, colors.line.gamma_multiply(0.6)),
    );
}

/// One setting: a title and optional detail on the left, its control on the right.
fn row<R>(
    ui: &mut Ui,
    title: &str,
    detail: Option<(&str, Color32)>,
    control: impl FnOnce(&mut Ui) -> R,
) -> R {
    row_with(ui, 50.0, strong(Size::BODY), title, detail, control)
}

fn row_with<R>(
    ui: &mut Ui,
    min_height: f32,
    font: egui::FontId,
    title: &str,
    detail: Option<(&str, Color32)>,
    control: impl FnOnce(&mut Ui) -> R,
) -> R {
    let colors = theme::colors(ui.ctx());
    let id = ui.scope_id().with(("row", title));
    let width = ui.available_width();
    // The control's width is only known once it is laid out, so the text wraps
    // around last frame's width.
    let reserved = ui.data(|data| data.get_temp::<f32>(id)).unwrap_or(120.0);
    let text_width = (width - reserved - 16.0).max(120.0);
    let title = ui
        .painter()
        .layout(title.to_owned(), font, colors.text, text_width);
    let detail = detail.map(|(text, color)| {
        ui.painter()
            .layout(text.to_owned(), regular(Size::SMALL), color, text_width)
    });
    let text_height = title.size().y + detail.as_ref().map_or(0.0, |d| d.size().y + 3.0);
    let height = (text_height + 16.0).max(min_height);
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    // Labels rather than painted text, so assistive technology can read them.
    let mut text = ui.new_child(
        UiBuilder::new()
            .id_salt((id, "text"))
            .max_rect(Rect::from_min_size(
                pos2(rect.left(), rect.center().y - text_height / 2.0),
                vec2(text_width, text_height),
            ))
            .layout(Layout::top_down(Align::Min)),
    );
    text.spacing_mut().item_spacing.y = 3.0;
    text.add(egui::Label::new(title));
    if let Some(detail) = detail {
        text.add(egui::Label::new(detail));
    }
    let mut side = ui.new_child(
        UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(Layout::right_to_left(Align::Center)),
    );
    let result = control(&mut side);
    let used = side.min_rect().width();
    if (used - reserved).abs() > 0.5 {
        ui.data_mut(|data| data.insert_temp(id, used));
        ui.ctx().request_repaint();
    }
    result
}

// ---- sections -------------------------------------------------------------

fn playback(ui: &mut Ui, state: &SettingsState<'_>, actions: &mut Vec<Action>) {
    let colors = theme::colors(ui.ctx());
    caption(ui, "STREAMING");
    card(ui, |ui| {
        let current = state.active_quality();
        let detail = if state.premium {
            "Applies to the current track straight away."
        } else if state.library.is_some() {
            "Higher qualities need iBroadcast Premium."
        } else if state.signed_in {
            "Available once your library has loaded."
        } else {
            "Available after you sign in."
        };
        row(ui, "Quality", Some((detail, colors.muted)), |ui| {
            ui.add_enabled_ui(state.premium, |ui| {
                egui::ComboBox::from_id_salt("stream-quality")
                    .selected_text(quality_label(current))
                    .width(132.0)
                    .show_ui(ui, |ui| {
                        for quality in QUALITIES {
                            let label = quality_label(quality);
                            if ui.selectable_label(current == quality, label).clicked() {
                                actions.push(Action::SetQuality(quality));
                            }
                        }
                    });
            });
        });
        divider(ui);
        let detail = (
            "For compatible Original FLAC albums played in order.",
            colors.muted,
        );
        row(ui, "Gapless playback", Some(detail), |ui| {
            let mut gapless = state.gapless;
            if theme::toggle(ui, &mut gapless, "Gapless playback").changed() {
                actions.push(Action::ToggleGapless);
            }
        });
    });

    caption(ui, "OUTPUT");
    card(ui, |ui| {
        let detail = match (state.audio_error, state.output) {
            (Some(error), _) => (error, colors.warn),
            (None, Some(name)) => (name, colors.muted),
            (None, None) => ("The system’s default output", colors.muted),
        };
        row(ui, "Audio output", Some(detail), |ui| {
            let reconnect = theme::button(ui, "Reconnect", Tone::Plain).on_hover_text(
                "Use the system’s current speakers, headphones or DAC. \
                 Playback pauses at the current position.",
            );
            if reconnect.clicked() {
                actions.push(Action::RecoverOutput);
            }
        });
    });
}

fn shortcuts(ui: &mut Ui) {
    let colors = theme::colors(ui.ctx());
    let command = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl"
    };
    let with = |key: &str| format!("{command} {key}");
    let groups = [
        (
            "PLAYBACK",
            vec![
                ("Play or pause", vec!["Space".to_owned()]),
                ("Next track", vec![with("→")]),
                ("Previous track", vec![with("←")]),
                (
                    "Skip back or forward 10 seconds",
                    vec!["←".to_owned(), "→".to_owned()],
                ),
                ("Volume down or up", vec![with("↓"), with("↑")]),
            ],
        ),
        (
            "NAVIGATION",
            vec![
                ("Search", vec![with("F")]),
                ("Back", vec!["Esc".to_owned()]),
                ("Reload library", vec![with("R")]),
                ("Settings", vec![with(",")]),
            ],
        ),
    ];
    for (title, rows) in groups {
        caption(ui, title);
        card(ui, |ui| {
            for (index, (label, keys)) in rows.iter().enumerate() {
                if index > 0 {
                    divider(ui);
                }
                row_with(ui, 34.0, regular(Size::BODY), label, None, |ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    for key in keys.iter().rev() {
                        theme::key_cap(ui, key);
                    }
                });
            }
        });
    }
    ui.add_space(10.0);
    ui.label(
        RichText::new("Media keys and the system’s Now Playing controls work as well.")
            .font(regular(Size::SMALL))
            .color(colors.muted),
    );
}

fn about(ui: &mut Ui, state: &SettingsState<'_>) {
    let colors = theme::colors(ui.ctx());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if let Some(logo) = state.logo {
            ui.add(egui::Image::new(logo).fit_to_exact_size(Vec2::splat(64.0)));
            ui.add_space(8.0);
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.label(
                RichText::new(APP_NAME)
                    .font(strong(Size::HEADING))
                    .color(colors.text),
            );
            ui.label(
                RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION")))
                    .font(regular(Size::SMALL))
                    .color(colors.muted),
            );
            ui.label(
                RichText::new("Lightweight desktop player for iBroadcast.")
                    .font(regular(Size::SMALL))
                    .color(colors.muted),
            );
        });
    });
    ui.add_space(18.0);
    card(ui, |ui| {
        let detail = ("github.com/cyberdeliaAI/tagcast-lite", colors.muted);
        row(ui, "Source code and releases", Some(detail), |ui| {
            if theme::button(ui, "Open GitHub", Tone::Plain).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(REPOSITORY));
            }
        });
        divider(ui);
        let detail = (
            "Tagcast Lite is an independent app by Cyberdelia.",
            colors.muted,
        );
        row(ui, "Powered by iBroadcast", Some(detail), |ui| {
            if theme::button(ui, "Visit iBroadcast", Tone::Plain).clicked() {
                ui.ctx()
                    .open_url(egui::OpenUrl::new_tab("https://www.ibroadcast.com/"));
            }
        });
        divider(ui);
        let detail = (
            "Set in Inter, under the SIL Open Font License.",
            colors.muted,
        );
        row(ui, "MIT licence", Some(detail), |_| ());
    });
}

// ---- theme editor ---------------------------------------------------------

fn editor(ui: &mut Ui, draft: &mut Draft) {
    let width = ui.available_width();
    let left = (width * 0.42).floor();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        ui.vertical(|ui| {
            ui.set_width(left);
            card(ui, |ui| {
                row(ui, "Name", None, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.palette.name)
                            .char_limit(60)
                            .margin(Margin::symmetric(8, 5))
                            .desired_width(170.0),
                    );
                });
                divider(ui);
                row(ui, "Variant", None, |ui| {
                    theme::segmented(
                        ui,
                        "variant",
                        &mut draft.dark,
                        &[(false, "Light"), (true, "Dark")],
                    );
                });
            });
            ui.add_space(12.0);
            preview(ui, draft.palette.colors(draft.dark));
        });
        ui.vertical(|ui| {
            ui.set_width(width - left - 16.0);
            let mode = if draft.dark {
                &mut draft.palette.dark
            } else {
                &mut draft.palette.light
            };
            card(ui, |ui| {
                ui.add_space(8.0);
                let width = (ui.available_width() - 20.0) / 2.0;
                for [left, right] in [
                    [("Background", &mut mode.bg), ("Text", &mut mode.text)],
                    [
                        ("Sidebar", &mut mode.sidebar),
                        ("Secondary text", &mut mode.muted),
                    ],
                    [
                        ("Player bar", &mut mode.bar),
                        ("Subtle text", &mut mode.faint),
                    ],
                    [("Cards", &mut mode.surface), ("Accent", &mut mode.accent)],
                    [
                        ("Selection", &mut mode.surface_hi),
                        ("Accent hover", &mut mode.accent_hi),
                    ],
                    [
                        ("Dividers", &mut mode.line),
                        ("Link", &mut mode.accent_text),
                    ],
                    [
                        ("Play control", &mut mode.play),
                        ("Text on accent", &mut mode.on_accent),
                    ],
                ] {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 20.0;
                        swatch(ui, width, left.0, left.1);
                        swatch(ui, width, right.0, right.1);
                    });
                }
                ui.add_space(8.0);
            });
        });
    });
}

fn swatch(ui: &mut Ui, width: f32, label: &str, color: &mut [u8; 3]) {
    let colors = theme::colors(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    ui.painter().text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        label,
        regular(Size::BODY),
        colors.text,
    );
    let mut side = ui.new_child(
        UiBuilder::new()
            .id_salt(label)
            .max_rect(rect)
            .layout(Layout::right_to_left(Align::Center)),
    );
    let button = side.color_edit_button_srgb(color);
    button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ColorWell, true, label));
    ui.painter().rect_stroke(
        button.rect,
        4,
        Stroke::new(1.0, colors.line),
        StrokeKind::Inside,
    );
}

/// A miniature of the player in the given colours, drawn at its own reduced sizes.
fn preview(ui: &mut Ui, colors: Colors) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 132.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 34.0), rect.max);
    let rail = (rect.width() * 0.26).clamp(96.0, 116.0);
    let side = Rect::from_min_max(rect.min, pos2(rect.left() + rail, bar.top()));
    let corner = |nw, ne, sw, se| CornerRadius { nw, ne, sw, se };
    painter.rect_filled(rect, CornerRadius::same(10), colors.bg);
    painter.rect_filled(side, corner(10, 0, 0, 0), colors.sidebar);
    painter.rect_filled(bar, corner(0, 0, 10, 10), colors.bar);

    for (index, (icon, label)) in [
        (Icon::Note, "Songs"),
        (Icon::Album, "Albums"),
        (Icon::Artist, "Artists"),
    ]
    .into_iter()
    .enumerate()
    {
        let item = Rect::from_min_size(
            pos2(side.left() + 8.0, side.top() + 10.0 + index as f32 * 24.0),
            vec2(side.width() - 16.0, 21.0),
        );
        let selected = index == 1;
        if selected {
            painter.rect_filled(item, CornerRadius::same(5), colors.surface_hi);
        }
        theme::paint_icon(
            &painter,
            Rect::from_center_size(pos2(item.left() + 13.0, item.center().y), Vec2::splat(11.0)),
            icon,
            if selected {
                colors.accent_text
            } else {
                colors.muted
            },
        );
        painter.text(
            pos2(item.left() + 26.0, item.center().y),
            Align2::LEFT_CENTER,
            label,
            if selected {
                strong(11.0)
            } else {
                regular(11.0)
            },
            if selected { colors.text } else { colors.muted },
        );
    }

    let left = side.right() + 16.0;
    let right = rect.right() - 16.0;
    painter.text(
        pos2(left, rect.top() + 21.0),
        Align2::LEFT_CENTER,
        "Albums",
        strong(14.0),
        colors.text,
    );
    let pill = Rect::from_min_max(
        pos2(right - 50.0, rect.top() + 11.0),
        pos2(right, rect.top() + 31.0),
    );
    painter.rect_filled(pill, CornerRadius::same(10), colors.accent);
    painter.text(
        pill.center(),
        Align2::CENTER_CENTER,
        "Play",
        strong(10.5),
        colors.on_accent,
    );
    let tiles = if right - left >= 260.0 { 2 } else { 1 };
    let width = (right - left - 10.0 * (tiles - 1) as f32) / tiles as f32;
    let albums = [("Album title", "one"), ("Another album", "Evening Glass")];
    for (index, (title, seed)) in albums.into_iter().take(tiles).enumerate() {
        let tile = Rect::from_min_size(
            pos2(left + index as f32 * (width + 10.0), rect.top() + 41.0),
            vec2(width, 48.0),
        );
        painter.rect_filled(tile, CornerRadius::same(6), colors.surface);
        let cover = Rect::from_min_size(tile.min + vec2(6.0, 6.0), Vec2::splat(36.0));
        theme::cover(&painter, cover, None, 4, seed);
        let x = cover.right() + 8.0;
        let text = |y: f32, text: &str, font, color| {
            painter.text(
                pos2(x, tile.top() + y),
                Align2::LEFT_CENTER,
                text,
                font,
                color,
            );
        };
        text(12.0, title, strong(10.5), colors.text);
        text(24.5, "Artist name", regular(10.0), colors.accent_text);
        text(36.5, "2024 · 10 songs", regular(9.5), colors.faint);
    }

    let middle = bar.center().y;
    let cover = Rect::from_center_size(pos2(bar.left() + 21.0, middle), Vec2::splat(22.0));
    theme::cover(&painter, cover, None, 3, "one");
    painter.text(
        pos2(cover.right() + 8.0, middle - 5.5),
        Align2::LEFT_CENTER,
        "Track title",
        strong(10.5),
        colors.text,
    );
    painter.text(
        pos2(cover.right() + 8.0, middle + 6.5),
        Align2::LEFT_CENTER,
        "Artist name",
        regular(9.5),
        colors.muted,
    );
    let play = pos2(rect.center().x, middle);
    painter.circle_filled(play, 10.0, colors.play);
    theme::paint_icon(
        &painter,
        Rect::from_center_size(play + vec2(0.8, 0.0), Vec2::splat(8.5)),
        Icon::Play,
        theme::control_ink(colors.bar, colors.play),
    );
    let track = Rect::from_min_max(pos2(play.x + 28.0, middle - 1.5), pos2(right, middle + 1.5));
    painter.rect_filled(track, CornerRadius::same(2), colors.line);
    let played = Rect::from_min_max(
        track.min,
        pos2(track.left() + track.width() * 0.4, track.bottom()),
    );
    painter.rect_filled(
        played,
        CornerRadius::same(2),
        colors.text.gamma_multiply(0.85),
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(10),
        Stroke::new(1.0, colors.line),
        StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dialog in a window of its own, driven by pointer events.
    struct Dialog {
        ctx: egui::Context,
        window: SettingsWindow,
        preferences: Preferences,
        state: SettingsState<'static>,
    }

    impl Dialog {
        fn new(section: Section, signed_in: bool) -> Self {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            ctx.enable_accesskit();
            let preferences = Preferences::default();
            preferences.apply(&ctx);
            let mut window = SettingsWindow::default();
            window.open(section);
            Self {
                ctx,
                window,
                preferences,
                state: SettingsState {
                    quality: Quality::Original,
                    premium: true,
                    gapless: false,
                    signed_in,
                    loading: false,
                    library_error: None,
                    library: signed_in.then_some([1200, 90, 4]),
                    output: Some("Speakers"),
                    audio_error: None,
                    logo: None,
                },
            }
        }

        /// Draws one frame and returns its actions and every named widget.
        fn frame(&mut self, events: Vec<egui::Event>) -> (Vec<Action>, Vec<(String, Rect)>) {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 780.0))),
                events,
                ..Default::default()
            };
            let mut actions = Vec::new();
            let mut output = self.ctx.run_pass(input, |ctx| {
                let state = SettingsState { ..self.state };
                self.window
                    .show(ctx, &mut self.preferences, state, &mut actions);
            });
            // Nothing paints these frames, so the font atlas has nowhere to go.
            output.textures_delta.clear();
            let widgets = output
                .platform_output
                .accesskit_update
                .into_iter()
                .flat_map(|update| update.nodes)
                .filter_map(|(_, node)| {
                    let bounds = node.bounds()?;
                    let rect = Rect::from_min_max(
                        pos2(bounds.x0 as f32, bounds.y0 as f32),
                        pos2(bounds.x1 as f32, bounds.y1 as f32),
                    );
                    // Plain text reports itself as a value, controls as a label.
                    Some((node.label().or(node.value())?.to_owned(), rect))
                })
                .collect();
            (actions, widgets)
        }

        fn labels(&mut self) -> Vec<String> {
            self.frame(Vec::new());
            let (_, widgets) = self.frame(Vec::new());
            widgets.into_iter().map(|(label, _)| label).collect()
        }

        fn click(&mut self, label: &str) -> Vec<Action> {
            // Rows settle on their control's width after the first frame.
            self.frame(Vec::new());
            let (_, widgets) = self.frame(Vec::new());
            let pos = widgets
                .iter()
                .find(|(name, _)| name == label)
                .unwrap_or_else(|| panic!("nothing is labelled {label:?}"))
                .1
                .center();
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(vec![egui::Event::PointerMoved(pos)]);
            self.frame(vec![button(true)]);
            self.frame(vec![button(false)]).0
        }
    }

    #[test]
    fn playback_controls_report_their_actions() {
        let mut dialog = Dialog::new(Section::Playback, true);
        let actions = dialog.click("Gapless playback");
        assert!(matches!(actions[..], [Action::ToggleGapless]));
        let actions = dialog.click("Reconnect");
        assert!(matches!(actions[..], [Action::RecoverOutput]));
        assert!(dialog.labels().iter().any(|label| label == "Speakers"));
    }

    #[test]
    fn account_shows_the_library_error_and_allows_a_retry() {
        let mut dialog = Dialog::new(Section::Account, true);
        dialog.state.library = None;
        dialog.state.loading = true;
        assert!(dialog.labels().iter().any(|label| label == "Loading…"));

        dialog.state.loading = false;
        dialog.state.library_error = Some(&Failure::Network);
        let labels = dialog.labels();
        assert!(!labels.iter().any(|label| label == "Loading…"));
        assert!(labels.contains(&Failure::Network.to_string()));
        assert!(matches!(dialog.click("Reload")[..], [Action::Reload]));

        dialog.state.loading = true;
        let labels = dialog.labels();
        assert!(labels.iter().any(|label| label == "Loading…"));
        assert!(!labels.contains(&Failure::Network.to_string()));
    }

    #[test]
    fn only_the_focused_confirmation_button_has_a_focus_ring() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let draw = || {
            let mut buttons = Vec::new();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                buttons.push(theme::button(ui, "Sign out", Tone::Danger));
                buttons.push(theme::button(ui, "Cancel", Tone::Plain));
            });
            output.textures_delta.clear();
            let rings: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.stroke_kind == StrokeKind::Outside => {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .collect();
            (buttons, rings)
        };
        let (buttons, rings) = draw();
        assert!(rings.is_empty());
        for button in buttons {
            button.request_focus();
            assert_eq!(draw().1, [button.rect]);
        }
    }

    #[test]
    fn signing_out_asks_first() {
        let mut dialog = Dialog::new(Section::Account, true);
        assert!(dialog.click("Sign out…").is_empty());
        assert!(dialog.window.open);
        assert!(dialog.click("Cancel").is_empty());
        assert!(dialog.click("Sign out…").is_empty());
        let actions = dialog.click("Sign out");
        assert!(matches!(actions[..], [Action::SignOut]));
        assert!(!dialog.window.open);
    }

    #[test]
    fn the_rail_switches_sections_and_close_dismisses() {
        let mut dialog = Dialog::new(Section::Playback, true);
        dialog.click("Shortcuts");
        assert!(dialog.window.section == Section::Shortcuts);
        dialog.click("Account");
        assert!(dialog.window.section == Section::Account);
        dialog.click("Close settings");
        assert!(!dialog.window.open);
    }

    #[test]
    fn signed_out_settings_have_no_account_section() {
        let mut dialog = Dialog::new(Section::Account, false);
        let labels = dialog.labels();
        assert!(dialog.window.section == Section::Playback);
        assert!(!labels.iter().any(|label| label == "Account"));
        assert!(labels.iter().any(|label| label == "Appearance"));
    }

    #[test]
    fn duplicating_a_theme_opens_the_editor_and_cancel_returns() {
        let mut dialog = Dialog::new(Section::Appearance, true);
        dialog.click("Duplicate");
        assert!(dialog.window.draft.is_some());
        assert!(dialog.labels().iter().any(|label| label == "Background"));
        dialog.click("Cancel");
        assert!(dialog.window.draft.is_none());
        assert!(dialog.window.open);
        assert!(dialog.preferences.custom.is_empty());
    }

    #[test]
    fn free_accounts_show_128_kbps_even_with_a_saved_premium_quality() {
        let mut state = SettingsState {
            quality: Quality::Original,
            premium: false,
            gapless: false,
            signed_in: true,
            loading: false,
            library_error: None,
            library: None,
            output: None,
            audio_error: None,
            logo: None,
        };
        assert_eq!(state.active_quality(), Quality::K128);
        state.premium = true;
        assert_eq!(state.active_quality(), Quality::Original);
    }

    #[test]
    fn old_preferences_restore_the_original_palette() {
        let prefs: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs.current().name, "Lite");
        assert_eq!(
            prefs.current().colors(true).bg,
            egui::Color32::from_rgb(16, 20, 30)
        );
        assert_eq!(presets().len(), 18);
        for name in [
            "Forest",
            "Ocean",
            "Warm",
            "iBroadcast inspired",
            "Apple Music Classic",
            "Amber",
        ] {
            assert!(
                presets().iter().any(|palette| palette.name == name),
                "{name}"
            );
        }
    }

    #[test]
    fn custom_theme_round_trip_and_unique_names() {
        let mut prefs = Preferences::default();
        let mut draft = Draft {
            palette: presets()[0].clone(),
            index: None,
            dark: true,
        };
        assert!(!prefs.can_save(&draft));
        draft.palette.name = "My theme".into();
        draft.palette.dark.bg = [20, 30, 40];
        assert!(prefs.can_save(&draft));
        prefs.save_draft(draft);
        let encoded = serde_json::to_string(&prefs).unwrap();
        let decoded: Preferences = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.current().name, "My theme");
        assert_eq!(
            decoded.current().colors(true).bg,
            egui::Color32::from_rgb(20, 30, 40)
        );
    }

    #[test]
    fn selecting_a_theme_updates_both_mode_styles() {
        let ctx = egui::Context::default();
        let prefs = Preferences {
            mode: Mode::Light,
            selected: "Nord".into(),
            ..Preferences::default()
        };
        prefs.apply(&ctx);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert_eq!(theme::colors(&ctx).bg, prefs.current().colors(false).bg);
        let dark = ctx.style_of(egui::Theme::Dark);
        assert_eq!(dark.visuals.panel_fill, prefs.current().colors(true).bg);
    }
}
