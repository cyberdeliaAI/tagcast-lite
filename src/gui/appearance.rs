//! Local appearance settings and the settings dialog.

use std::sync::OnceLock;

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use tagcast_player::transport::Quality;

use super::{
    Action, QUALITIES, quality_label,
    theme::{self, Colors, Swatches},
};

const MAX_CUSTOM: usize = 20;

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

#[derive(Clone, Copy, Default, PartialEq)]
pub enum Section {
    #[default]
    Playback,
    Appearance,
}

pub struct SettingsState {
    pub quality: Quality,
    pub premium: bool,
    pub gapless: bool,
    pub signed_in: bool,
    pub loading: bool,
}

impl SettingsState {
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
        state: SettingsState,
        actions: &mut Vec<Action>,
    ) {
        if !self.open {
            return;
        }
        let editing = self.draft.is_some();
        let mut changed = false;
        let mut closed = false;
        let modal = egui::Modal::new(egui::Id::unique("settings")).show(ctx, |ui| {
            ui.set_width(448.0);
            if let Some(draft) = &self.draft {
                ui.heading(if draft.index.is_some() {
                    "Edit theme"
                } else {
                    "New theme"
                });
            } else {
                ui.heading("Settings");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.section, Section::Playback, "Playback");
                    ui.selectable_value(&mut self.section, Section::Appearance, "Appearance");
                });
            }

            ui.separator();
            let reserved = if editing { 118.0 } else { 160.0 };
            egui::ScrollArea::vertical()
                .id_salt(if editing {
                    "theme-editor"
                } else {
                    "settings-section"
                })
                .max_height((ctx.content_rect().height() - reserved).max(220.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if let Some(draft) = &mut self.draft {
                        editor(ui, draft);
                    } else {
                        match self.section {
                            Section::Playback => playback(ui, &state, actions),
                            Section::Appearance => changed |= self.appearance(ui, preferences),
                        }
                    }
                });

            ui.separator();
            if editing {
                changed |= self.editor_footer(ui, preferences);
            } else {
                closed = settings_footer(ui, &state, actions);
            }
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
        }
    }

    fn appearance(&mut self, ui: &mut egui::Ui, preferences: &mut Preferences) -> bool {
        let old_mode = preferences.mode;
        ui.horizontal(|ui| {
            ui.label("Appearance");
            ui.selectable_value(&mut preferences.mode, Mode::Dark, "Dark");
            ui.selectable_value(&mut preferences.mode, Mode::Light, "Light");
            ui.selectable_value(&mut preferences.mode, Mode::System, "System");
        });

        let mut selection = None;
        ui.horizontal(|ui| {
            ui.label("Theme");
            egui::ComboBox::from_id_salt("theme-preset")
                .selected_text(&preferences.current().name)
                .width(200.0)
                .show_ui(ui, |ui| {
                    for palette in presets() {
                        if ui
                            .selectable_label(
                                palette.name == preferences.current().name,
                                &palette.name,
                            )
                            .clicked()
                        {
                            selection = Some(palette.name.clone());
                        }
                    }
                    if !preferences.custom.is_empty() {
                        ui.separator();
                        ui.label("Custom themes");
                        for palette in &preferences.custom {
                            if ui
                                .selectable_label(
                                    palette.name == preferences.selected,
                                    &palette.name,
                                )
                                .clicked()
                            {
                                selection = Some(palette.name.clone());
                            }
                        }
                    }
                });
        });
        let mut changed = preferences.mode != old_mode;
        if let Some(selection) = selection {
            preferences.selected = selection;
            self.confirm_delete = false;
            changed = true;
        }

        preview(
            ui,
            preferences
                .current()
                .colors(ui.ctx().theme() == egui::Theme::Dark),
        );
        let custom_index = preferences.custom_index();
        let mut create = false;
        let mut edit = false;
        ui.horizontal(|ui| {
            create = ui
                .add_enabled(
                    preferences.custom.len() < MAX_CUSTOM,
                    egui::Button::new("Create a copy"),
                )
                .clicked();
            if custom_index.is_some() {
                edit = ui.button("Edit theme").clicked();
                if ui.button("Delete theme…").clicked() {
                    self.confirm_delete = true;
                }
            }
        });
        if self.confirm_delete
            && let Some(index) = custom_index
        {
            ui.horizontal(|ui| {
                ui.label("Delete this custom theme?");
                if ui.button("Delete").clicked() {
                    preferences.custom.remove(index);
                    preferences.selected.clear();
                    self.confirm_delete = false;
                    changed = true;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_delete = false;
                }
            });
        }
        if create || edit {
            let mut palette = preferences.current().clone();
            if create {
                palette.name = format!("{} copy", palette.name);
            }
            self.draft = Some(Draft {
                palette,
                index: if edit { custom_index } else { None },
                dark: ui.ctx().theme() == egui::Theme::Dark,
            });
            self.confirm_delete = false;
        }
        changed
    }

    fn editor_footer(&mut self, ui: &mut egui::Ui, preferences: &mut Preferences) -> bool {
        let draft = self.draft.as_ref().unwrap();
        let valid_name = preferences.can_save(draft);
        let readable = [false, true].into_iter().all(|dark| {
            let colors = draft.palette.colors(dark);
            [
                colors.bg,
                colors.sidebar,
                colors.bar,
                colors.surface,
                colors.surface_hi,
            ]
            .into_iter()
            .all(|background| theme::contrast(colors.text, background) >= 3.0)
        });
        if !valid_name {
            ui.label("Give the theme a unique name of up to 60 characters.");
        }
        if !readable {
            ui.label("Increase text contrast against the panel colours in both variants.");
        }
        let mut save = false;
        let mut cancel = false;
        let colors = theme::colors(ui.ctx());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let save_button =
                egui::Button::new(RichText::new("Save and apply").color(colors.on_accent))
                    .fill(colors.accent);
            save = ui
                .add_enabled(valid_name && readable, save_button)
                .clicked();
            cancel = ui.button("Cancel").clicked();
        });
        if save {
            preferences.save_draft(self.draft.take().unwrap());
        } else if cancel {
            self.draft = None;
        }
        save
    }
}

fn editor(ui: &mut egui::Ui, draft: &mut Draft) {
    ui.horizontal(|ui| {
        ui.label("Theme name");
        ui.add(egui::TextEdit::singleline(&mut draft.palette.name).char_limit(60));
    });
    ui.horizontal(|ui| {
        ui.label("Colours");
        ui.selectable_value(&mut draft.dark, false, "Light");
        ui.selectable_value(&mut draft.dark, true, "Dark");
    });
    let mode = if draft.dark {
        &mut draft.palette.dark
    } else {
        &mut draft.palette.light
    };
    egui::Grid::new("theme-swatches")
        .num_columns(4)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            for pair in [
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
                for (label, color) in pair {
                    ui.label(label);
                    let swatch = ui.color_edit_button_srgb(color);
                    ui.painter().rect_stroke(
                        swatch.rect,
                        4,
                        ui.visuals().window_stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                ui.end_row();
            }
        });
    let colors = draft.palette.colors(draft.dark);
    ui.add_space(6.0);
    ui.label("Preview");
    preview(ui, colors);
}

fn playback(ui: &mut egui::Ui, state: &SettingsState, actions: &mut Vec<Action>) {
    let current = state.active_quality();
    let colors = theme::colors(ui.ctx());
    ui.horizontal(|ui| {
        ui.label("Quality");
        ui.add_enabled_ui(state.premium, |ui| {
            let inactive = &mut ui.visuals_mut().widgets.inactive;
            inactive.bg_fill = colors.bg;
            inactive.weak_bg_fill = colors.bg;
            egui::ComboBox::from_id_salt("stream-quality")
                .selected_text(quality_label(current))
                .width(146.0)
                .show_ui(ui, |ui| {
                    for quality in QUALITIES {
                        if ui
                            .selectable_label(current == quality, quality_label(quality))
                            .clicked()
                        {
                            actions.push(Action::SetQuality(quality));
                        }
                    }
                })
                .response
                .on_disabled_hover_text("Higher qualities require iBroadcast Premium");
        });
    });

    ui.add_space(6.0);
    let mut gapless = state.gapless;
    let border = egui::Stroke::new(1.0, colors.muted);
    let changed = ui
        .scope(|ui| {
            let widgets = &mut ui.visuals_mut().widgets;
            widgets.inactive.bg_stroke = border;
            widgets.hovered.bg_stroke = border;
            widgets.active.bg_stroke = border;
            widgets.open.bg_stroke = border;
            ui.checkbox(&mut gapless, "Gapless playback").changed()
        })
        .inner;
    if changed {
        actions.push(Action::ToggleGapless);
    }
    ui.label(RichText::new("For compatible Original FLAC albums played in order.").weak());

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Audio output");
        if ui.button("Reconnect")
            .on_hover_text("Use the system’s current speakers, headphones or DAC. Playback pauses at the current position.")
            .clicked()
        {
            actions.push(Action::RecoverOutput);
        }
    });
}

fn settings_footer(ui: &mut egui::Ui, state: &SettingsState, actions: &mut Vec<Action>) -> bool {
    let mut closed = false;
    let colors = theme::colors(ui.ctx());
    ui.horizontal(|ui| {
        if state.signed_in {
            let label = if state.loading {
                "Reloading…"
            } else {
                "Reload library"
            };
            let shortcut = if cfg!(target_os = "macos") {
                "⌘R"
            } else {
                "Ctrl+R"
            };
            let reload = ui.add_enabled(
                !state.loading,
                egui::Button::new(label).shortcut_text(shortcut),
            );
            if !state.loading
                && (reload.clicked()
                    || ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::COMMAND, egui::Key::R)
                    }))
            {
                actions.push(Action::Reload);
            }
            if ui.button("Sign out").clicked() {
                actions.push(Action::SignOut);
                closed = true;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            closed |= ui
                .add(egui::Button::new("Done").fill(colors.surface_hi))
                .clicked();
        });
    });
    closed
}

fn preview(ui: &mut egui::Ui, colors: Colors) {
    let style = |fill| {
        egui::Frame::new()
            .fill(fill)
            .inner_margin(egui::Margin::symmetric(10, 7))
    };
    style(colors.bg).corner_radius(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            style(colors.sidebar).show(ui, |ui| {
                ui.label(RichText::new("Albums").color(colors.text));
            });
            style(colors.surface_hi).show(ui, |ui| {
                ui.label(RichText::new("Track title").color(colors.text));
            });
            style(colors.accent).corner_radius(8).show(ui, |ui| {
                ui.label(RichText::new("Play").color(colors.on_accent));
            });
        });
        style(colors.bar).show(ui, |ui| {
            ui.label(RichText::new("Now playing").color(colors.muted));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_accounts_show_128_kbps_even_with_a_saved_premium_quality() {
        let mut state = SettingsState {
            quality: Quality::Original,
            premium: false,
            gapless: false,
            signed_in: true,
            loading: false,
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
