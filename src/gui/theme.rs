//! Colours, fonts and the small vector icons the player draws itself.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Pos2, Rect,
    Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

use serde::{Deserialize, Serialize};

pub const COVER_INK: Color32 = Color32::from_rgb(255, 247, 222);
const COLORS_ID: &str = "tagcast-palette";

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct Swatches {
    pub bg: [u8; 3],
    pub sidebar: [u8; 3],
    pub bar: [u8; 3],
    pub surface: [u8; 3],
    pub surface_hi: [u8; 3],
    pub line: [u8; 3],
    pub text: [u8; 3],
    pub muted: [u8; 3],
    pub faint: [u8; 3],
    pub accent: [u8; 3],
    pub accent_hi: [u8; 3],
    pub accent_text: [u8; 3],
    pub play: [u8; 3],
    pub on_accent: [u8; 3],
}

#[derive(Clone, Copy)]
pub struct Colors {
    pub bg: Color32,
    pub sidebar: Color32,
    pub bar: Color32,
    pub surface: Color32,
    pub surface_hi: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_hi: Color32,
    pub accent_text: Color32,
    pub play: Color32,
    pub on_accent: Color32,
    pub warn: Color32,
    pub error: Color32,
}

impl Colors {
    pub fn new(swatches: Swatches, dark: bool) -> Self {
        let rgb = |[r, g, b]: [u8; 3]| Color32::from_rgb(r, g, b);
        Self {
            bg: rgb(swatches.bg),
            sidebar: rgb(swatches.sidebar),
            bar: rgb(swatches.bar),
            surface: rgb(swatches.surface),
            surface_hi: rgb(swatches.surface_hi),
            line: rgb(swatches.line),
            text: rgb(swatches.text),
            muted: rgb(swatches.muted),
            faint: rgb(swatches.faint),
            accent: rgb(swatches.accent),
            accent_hi: rgb(swatches.accent_hi),
            accent_text: rgb(swatches.accent_text),
            play: rgb(swatches.play),
            on_accent: rgb(swatches.on_accent),
            warn: if dark {
                Color32::from_rgb(240, 180, 90)
            } else {
                Color32::from_rgb(130, 73, 16)
            },
            error: if dark {
                Color32::from_rgb(242, 124, 124)
            } else {
                Color32::from_rgb(163, 42, 53)
            },
        }
    }
}

pub fn colors(ctx: &egui::Context) -> Colors {
    let dark = ctx.theme() == egui::Theme::Dark;
    ctx.data(|data| data.get_temp::<[Colors; 2]>(egui::Id::unique(COLORS_ID)))
        .map(|colors| colors[usize::from(dark)])
        .unwrap_or_else(|| super::appearance::presets()[0].colors(dark))
}

fn luminance(color: Color32) -> f32 {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    let linear = |channel: u8| {
        let srgb = channel as f32 / 255.0;
        if srgb <= 0.04045 {
            srgb / 12.92
        } else {
            ((srgb + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub fn control_ink(preferred: Color32, background: Color32) -> Color32 {
    if contrast(preferred, background) >= 3.0 {
        preferred
    } else if luminance(background) >= 0.18 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// The type scale. Interface text uses these sizes and no others; only
/// drawings that scale with their box (icons, initials, the theme preview)
/// pick their own.
pub struct Size;

impl Size {
    /// Overlines, column headings and the smallest notes.
    pub const MICRO: f32 = 11.0;
    /// Secondary lines, counts and times.
    pub const SMALL: f32 = 12.0;
    /// Rows, navigation, buttons and running text.
    pub const BODY: f32 = 13.5;
    /// Names that lead a card or a block.
    pub const LEAD: f32 = 15.0;
    /// Section and dialog headings.
    pub const HEADING: f32 = 20.0;
    /// The title of a page.
    pub const TITLE: f32 = 26.0;
    /// One-off large figures, such as the sign-in code.
    pub const DISPLAY: f32 = 32.0;
    /// Inter's ascent as a share of the font size, for setting two sizes on one baseline.
    pub const ASCENT: f32 = 0.97;
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".into(),
        FontData::from_static(include_bytes!("../../assets/fonts/Inter-Regular.ttf")).into(),
    );
    fonts.font_data.insert(
        "Inter-SemiBold".into(),
        FontData::from_static(include_bytes!("../../assets/fonts/Inter-SemiBold.ttf")).into(),
    );
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut proportional = vec!["Inter".to_owned()];
    proportional.extend(fallback.iter().cloned());
    let mut semibold = vec!["Inter-SemiBold".to_owned(), "Inter".to_owned()];
    semibold.extend(fallback);
    fonts
        .families
        .insert(FontFamily::Proportional, proportional);
    fonts
        .families
        .insert(FontFamily::Name("semibold".into()), semibold);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        use egui::TextStyle;
        style.text_styles = [
            (TextStyle::Small, regular(Size::SMALL)),
            (TextStyle::Body, regular(Size::BODY)),
            (TextStyle::Button, regular(Size::BODY)),
            (TextStyle::Heading, strong(Size::HEADING)),
            (TextStyle::Monospace, FontId::monospace(Size::SMALL)),
        ]
        .into();
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(10.0, 5.0);
        style.spacing.interact_size.y = 28.0;
        style.spacing.scroll = egui::style::ScrollStyle::floating();
        style.spacing.scroll.bar_width = 8.0;
        style.interaction.selectable_labels = false;
    });
}

pub fn apply(ctx: &egui::Context, light: Colors, dark: Colors) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique(COLORS_ID), [light, dark]));
    for (mode, colors) in [(egui::Theme::Light, light), (egui::Theme::Dark, dark)] {
        let dark = mode == egui::Theme::Dark;
        let mut style = (*ctx.style_of(mode)).clone();
        let v = &mut style.visuals;
        v.dark_mode = dark;
        v.panel_fill = colors.bg;
        v.window_fill = colors.surface;
        v.window_stroke = Stroke::new(1.0, colors.line);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.extreme_bg_color = colors.surface;
        v.faint_bg_color = colors.surface;
        v.text_edit_bg_color = Some(colors.surface);
        v.hyperlink_color = colors.accent_text;
        v.selection.bg_fill = colors.accent;
        v.selection.stroke = Stroke::new(1.0, colors.on_accent);
        v.warn_fg_color = colors.warn;
        v.error_fg_color = colors.error;
        v.override_text_color = None;
        v.slider_trailing_fill = true;
        let radius = CornerRadius::same(6);
        let w = &mut v.widgets;
        w.noninteractive.fg_stroke = Stroke::new(1.0, colors.text);
        w.noninteractive.bg_stroke = Stroke::new(1.0, colors.line);
        w.noninteractive.bg_fill = colors.bg;
        w.inactive.bg_fill = colors.surface;
        w.inactive.weak_bg_fill = colors.surface;
        w.inactive.fg_stroke = Stroke::new(1.0, colors.text);
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.corner_radius = radius;
        w.hovered.bg_fill = colors.surface_hi;
        w.hovered.weak_bg_fill = colors.surface_hi;
        w.hovered.fg_stroke = Stroke::new(1.0, colors.text);
        w.hovered.bg_stroke = Stroke::NONE;
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;
        w.active.bg_fill = colors.accent;
        w.active.weak_bg_fill = colors.accent;
        w.active.fg_stroke = Stroke::new(1.0, colors.on_accent);
        w.active.bg_stroke = Stroke::NONE;
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;
        w.open.bg_fill = colors.surface_hi;
        w.open.weak_bg_fill = colors.surface_hi;
        w.open.corner_radius = radius;
        ctx.set_style_of(mode, style);
    }
}

pub fn panel(fill: Color32, margin: impl Into<Margin>) -> egui::Frame {
    egui::Frame::new().fill(fill).inner_margin(margin)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Next,
    Previous,
    Shuffle,
    Repeat,
    RepeatOne,
    Volume(u8),
    Muted,
    Search,
    Back,
    Note,
    Album,
    Artist,
    Playlist,
    Queue,
    More,
    Close,
    Bars,
    Settings,
    Contrast,
    Keyboard,
    Info,
    /// A star, filled when the thing is a favourite.
    Star(bool),
    Clock,
    Grid,
    MiniPlayer,
    ExpandPlayer,
    /// A push pin, filled while it holds something in place.
    Pin(bool),
    Microphone,
    Folder,
}

/// The in-app vinyl mark uses the active palette, including unsaved editor previews.
pub fn paint_brand_mark(painter: &egui::Painter, rect: Rect, colors: Colors) {
    let edge = rect.width().min(rect.height());
    let center = rect.center();
    painter.rect_filled(rect, edge * 0.22, colors.accent);

    let radius = edge * 0.412;
    let vinyl = colors.accent.lerp_to_gamma(Color32::BLACK, 0.86);
    let sheen = colors.accent.lerp_to_gamma(Color32::WHITE, 0.35);
    painter.circle_filled(center, radius, vinyl);
    // Two opposing reflections give the grooves the appearance of a pressed record.
    for direction in [-1.0, 1.0] {
        let point = |x, y| center + vec2(x, y) * (radius * direction);
        painter.add(Shape::convex_polygon(
            vec![
                center,
                point(-0.84, -0.54),
                point(-0.69, -0.72),
                point(-0.50, -0.86),
            ],
            vinyl.lerp_to_gamma(sheen, 0.21),
            Stroke::NONE,
        ));
    }
    for ring in [0.69, 0.88] {
        painter.circle_stroke(
            center,
            radius * ring,
            Stroke::new((edge * 0.016).max(0.5), vinyl.lerp_to_gamma(sheen, 0.57)),
        );
    }
    painter.circle_filled(center, edge * 0.185, colors.accent);
    paint_icon(
        painter,
        Rect::from_center_size(center, Vec2::splat(edge * 0.36)),
        Icon::Play,
        control_ink(colors.on_accent, colors.accent),
    );
}

pub fn brand_mark(ui: &mut Ui, edge: f32, colors: Colors) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(edge), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_brand_mark(ui.painter(), rect, colors);
    }
}

/// Draws an icon centred in `rect`. Icons are designed on a 20-point grid.
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height()) / 20.0;
    let c = rect.center();
    let p = |x: f32, y: f32| pos2(c.x + x * s, c.y + y * s);
    let stroke = Stroke::new(1.7 * s.max(0.75), color);
    let fill = |points: Vec<Pos2>| Shape::convex_polygon(points, color, Stroke::NONE);
    match icon {
        Icon::MiniPlayer | Icon::ExpandPlayer => {
            painter.rect_stroke(
                Rect::from_min_max(p(-8.0, -6.5), p(8.0, 6.5)),
                1.5 * s,
                stroke,
                egui::StrokeKind::Inside,
            );
            if icon == Icon::MiniPlayer {
                painter.rect_filled(Rect::from_min_max(p(0.0, 0.0), p(6.0, 4.5)), 0.5 * s, color);
            } else {
                painter.line_segment([p(3.0, 3.0), p(-4.0, -3.0)], stroke);
                painter.add(Shape::line(
                    vec![p(-4.0, 1.5), p(-4.0, -3.0), p(0.5, -3.0)],
                    stroke,
                ));
            }
        }
        Icon::Pin(pinned) => {
            // A push pin leaning to the right, its needle towards the lower left.
            let (sin, cos) = 40f32.to_radians().sin_cos();
            let q = |x: f32, y: f32| p(x * cos - y * sin, x * sin + y * cos + 1.0);
            let parts = [
                [(-4.2, -9.2), (4.2, -9.2), (4.2, -6.4), (-4.2, -6.4)],
                [(-2.5, -6.4), (2.5, -6.4), (2.5, -2.6), (-2.5, -2.6)],
                [(-5.6, -2.6), (5.6, -2.6), (5.6, -0.8), (-5.6, -0.8)],
            ];
            if pinned {
                for part in parts {
                    painter.add(fill(part.iter().map(|&(x, y)| q(x, y)).collect()));
                }
            } else {
                let outline = [
                    (-4.2, -9.2),
                    (4.2, -9.2),
                    (4.2, -6.4),
                    (2.5, -6.4),
                    (2.5, -2.6),
                    (5.6, -2.6),
                    (5.6, -0.8),
                    (-5.6, -0.8),
                    (-5.6, -2.6),
                    (-2.5, -2.6),
                    (-2.5, -6.4),
                    (-4.2, -6.4),
                ];
                let points = outline.iter().map(|&(x, y)| q(x, y)).collect();
                painter.add(Shape::closed_line(points, Stroke::new(1.4 * s, color)));
            }
            painter.line_segment([q(0.0, -0.8), q(0.0, 8.6)], stroke);
        }
        Icon::Play => {
            painter.add(fill(vec![p(-5.0, -7.5), p(7.5, 0.0), p(-5.0, 7.5)]));
        }
        Icon::Pause => {
            for x in [-4.0, 4.0] {
                painter.rect_filled(
                    Rect::from_center_size(p(x, 0.0), vec2(3.6 * s, 15.0 * s)),
                    1.2 * s,
                    color,
                );
            }
        }
        Icon::Next | Icon::Previous => {
            let d = if icon == Icon::Next { 1.0 } else { -1.0 };
            painter.add(fill(if d > 0.0 {
                vec![p(-6.0, -6.5), p(4.0, 0.0), p(-6.0, 6.5)]
            } else {
                vec![p(6.0, -6.5), p(6.0, 6.5), p(-4.0, 0.0)]
            }));
            painter.rect_filled(
                Rect::from_center_size(p(5.5 * d, 0.0), vec2(2.4 * s, 13.0 * s)),
                0.8 * s,
                color,
            );
        }
        Icon::Shuffle => {
            painter.add(Shape::line(
                vec![p(-8.0, -5.0), p(-4.5, -5.0), p(3.0, 5.0), p(7.0, 5.0)],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(-8.0, 5.0), p(-4.5, 5.0), p(-1.8, 1.4)],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(0.8, -1.8), p(3.0, -5.0), p(7.0, -5.0)],
                stroke,
            ));
            for y in [-5.0, 5.0] {
                painter.add(Shape::line(
                    vec![p(5.0, y - 2.6), p(8.0, y), p(5.0, y + 2.6)],
                    stroke,
                ));
            }
        }
        Icon::Repeat | Icon::RepeatOne => {
            painter.add(Shape::line(
                vec![
                    p(-4.0, 5.5),
                    p(-6.0, 5.5),
                    p(-8.0, 3.5),
                    p(-8.0, -3.0),
                    p(-6.0, -5.0),
                    p(7.0, -5.0),
                ],
                stroke,
            ));
            painter.add(Shape::line(
                vec![
                    p(4.0, -5.0),
                    p(6.0, -5.0),
                    p(8.0, -3.0),
                    p(8.0, 3.5),
                    p(6.0, 5.5),
                    p(-7.0, 5.5),
                ],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(4.5, -7.6), p(7.2, -5.0), p(4.5, -2.4)],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(-4.5, 2.9), p(-7.2, 5.5), p(-4.5, 8.1)],
                stroke,
            ));
            if icon == Icon::RepeatOne {
                painter.text(
                    p(0.2, 0.4),
                    egui::Align2::CENTER_CENTER,
                    "1",
                    strong(8.0 * s),
                    color,
                );
            }
        }
        Icon::Volume(_) | Icon::Muted => {
            painter.add(fill(vec![
                p(-8.0, -2.8),
                p(-4.5, -2.8),
                p(0.5, -7.5),
                p(0.5, 7.5),
                p(-4.5, 2.8),
                p(-8.0, 2.8),
            ]));
            match icon {
                Icon::Muted => {
                    painter.line_segment([p(3.5, -3.5), p(8.5, 3.5)], stroke);
                    painter.line_segment([p(8.5, -3.5), p(3.5, 3.5)], stroke);
                }
                Icon::Volume(level) => {
                    for (i, r) in [4.5_f32, 8.0].into_iter().enumerate() {
                        if level as usize > i {
                            arc(painter, p(0.5, 0.0), r * s, -0.9, 0.9, stroke);
                        }
                    }
                }
                _ => {}
            }
        }
        Icon::Search => {
            painter.circle_stroke(p(-1.5, -1.5), 5.5 * s, stroke);
            painter.line_segment([p(2.6, 2.6), p(7.0, 7.0)], stroke);
        }
        Icon::Back => {
            painter.add(Shape::line(
                vec![p(3.0, -7.0), p(-4.0, 0.0), p(3.0, 7.0)],
                Stroke::new(2.0 * s, color),
            ));
        }
        Icon::Note => {
            painter.circle_filled(p(-3.5, 5.0), 3.2 * s, color);
            painter.line_segment([p(-0.6, 5.0), p(-0.6, -7.5)], stroke);
            painter.add(Shape::line(
                vec![p(-0.6, -7.5), p(5.5, -5.5), p(5.5, -2.0), p(-0.6, -4.0)],
                stroke,
            ));
        }
        Icon::Album => {
            painter.circle_stroke(c, 7.5 * s, stroke);
            painter.circle_filled(c, 2.2 * s, color);
        }
        Icon::Artist => {
            painter.circle_stroke(p(0.0, -3.5), 3.8 * s, stroke);
            arc(painter, p(0.0, 9.0), 7.5 * s, -2.6, -0.54, stroke);
        }
        Icon::Playlist | Icon::Queue => {
            for (i, y) in [-5.5_f32, 0.0, 5.5].into_iter().enumerate() {
                let end = if icon == Icon::Queue && i == 2 {
                    -1.0
                } else {
                    7.5
                };
                painter.line_segment([p(-7.5, y), p(end, y)], stroke);
            }
            if icon == Icon::Queue {
                painter.add(fill(vec![p(2.5, 2.5), p(8.0, 5.5), p(2.5, 8.5)]));
            }
        }
        Icon::More => {
            for x in [-6.0, 0.0, 6.0] {
                painter.circle_filled(p(x, 0.0), 1.7 * s, color);
            }
        }
        Icon::Close => {
            painter.line_segment([p(-5.0, -5.0), p(5.0, 5.0)], stroke);
            painter.line_segment([p(5.0, -5.0), p(-5.0, 5.0)], stroke);
        }
        Icon::Bars => {
            let t = painter.ctx().input(|i| i.time) as f32;
            for (i, x) in [-5.0_f32, 0.0, 5.0].into_iter().enumerate() {
                let h = 5.0 + 9.0 * (0.5 + 0.5 * (t * 7.0 + i as f32 * 2.1).sin());
                let r = Rect::from_min_max(p(x - 1.6, 7.0 - h), p(x + 1.6, 7.0));
                painter.rect_filled(r, 1.0 * s, color);
            }
        }
        Icon::Settings => {
            let teeth = Stroke::new(3.2 * s, color);
            for step in 0..8 {
                let direction = Vec2::angled(step as f32 * std::f32::consts::FRAC_PI_4);
                painter.line_segment([c + direction * 5.0 * s, c + direction * 9.0 * s], teeth);
            }
            painter.circle_stroke(c, 4.6 * s, Stroke::new(2.6 * s, color));
        }
        Icon::Contrast => {
            painter.circle_stroke(c, 7.5 * s, stroke);
            let half = (0..=12)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 12.0;
                    c + Vec2::angled(a) * 7.5 * s
                })
                .collect();
            painter.add(fill(half));
        }
        Icon::Keyboard => {
            painter.rect_stroke(
                Rect::from_min_max(p(-8.5, -5.5), p(8.5, 5.5)),
                2.4 * s,
                stroke,
                StrokeKind::Middle,
            );
            for x in [-4.5, 0.0, 4.5] {
                painter.circle_filled(p(x, -1.6), 1.1 * s, color);
            }
            painter.line_segment([p(-3.6, 2.2), p(3.6, 2.2)], stroke);
        }
        Icon::Info => {
            painter.circle_stroke(c, 7.5 * s, stroke);
            painter.circle_filled(p(0.0, -3.7), 1.2 * s, color);
            painter.line_segment([p(0.0, -0.9), p(0.0, 4.3)], stroke);
        }
        Icon::Star(filled) => {
            // Ten points, alternating between the tips and the notches.
            let points: Vec<Pos2> = (0..10)
                .map(|i| {
                    let radius = if i % 2 == 0 { 8.4 } else { 3.9 };
                    let angle = (i as f32 * 36.0 - 90.0).to_radians();
                    p(0.0, 0.8) + Vec2::angled(angle) * radius * s
                })
                .collect();
            if filled {
                // A star is not convex, so it is filled as five tips around a pentagon.
                for tip in (0..10).step_by(2) {
                    painter.add(fill(vec![
                        points[(tip + 9) % 10],
                        points[tip],
                        points[tip + 1],
                    ]));
                }
                painter.add(fill(points.iter().copied().skip(1).step_by(2).collect()));
            }
            painter.add(Shape::closed_line(points, Stroke::new(1.5 * s, color)));
        }
        Icon::Clock => {
            painter.circle_stroke(c, 7.5 * s, stroke);
            painter.add(Shape::line(
                vec![p(0.0, -4.4), p(0.0, 0.3), p(3.3, 2.3)],
                stroke,
            ));
        }
        Icon::Grid => {
            for (x, y) in [(-4.6, -4.6), (4.6, -4.6), (-4.6, 4.6), (4.6, 4.6)] {
                painter.rect_stroke(
                    Rect::from_center_size(p(x, y), Vec2::splat(6.0 * s)),
                    1.6 * s,
                    stroke,
                    StrokeKind::Middle,
                );
            }
        }
        Icon::Microphone => {
            painter.rect_stroke(
                Rect::from_min_max(p(-2.9, -8.4), p(2.9, 1.8)),
                2.9 * s,
                stroke,
                StrokeKind::Middle,
            );
            arc(
                painter,
                p(0.0, 0.6),
                5.7 * s,
                0.0,
                std::f32::consts::PI,
                stroke,
            );
            painter.line_segment([p(0.0, 6.3), p(0.0, 8.6)], stroke);
            painter.line_segment([p(-3.2, 8.6), p(3.2, 8.6)], stroke);
        }
        Icon::Folder => {
            painter.add(Shape::closed_line(
                vec![
                    p(-8.0, -6.0),
                    p(-3.0, -6.0),
                    p(-0.8, -3.6),
                    p(8.0, -3.6),
                    p(8.0, 6.2),
                    p(-8.0, 6.2),
                ],
                stroke,
            ));
        }
    }
}

fn arc(painter: &egui::Painter, center: Pos2, radius: f32, from: f32, to: f32, stroke: Stroke) {
    let points = (0..=12)
        .map(|i| {
            let a = from + (to - from) * i as f32 / 12.0;
            center + Vec2::angled(a) * radius
        })
        .collect();
    painter.add(Shape::line(points, stroke));
}

/// Draw outside the control so keyboard focus remains distinct from its fill and selection.
pub(super) fn focus_ring(ui: &Ui, response: &Response, rect: Rect, radius: CornerRadius) {
    if response.has_focus() && response.enabled() {
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(2.0, colors(ui.ctx()).text),
            StrokeKind::Outside,
        );
    }
}

/// A round, borderless icon button.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, color: Color32, tip: &str) -> Response {
    let colors = colors(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if !tip.is_empty() {
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tip));
    }
    if ui.is_rect_visible(rect) {
        let color = if !ui.is_enabled() {
            colors.faint
        } else if response.hovered() && color == colors.muted {
            colors.text
        } else {
            color
        };
        if response.hovered() && ui.is_enabled() {
            ui.painter().circle_filled(
                rect.center(),
                size * 0.5,
                colors.surface_hi.gamma_multiply(0.8),
            );
        }
        paint_icon(ui.painter(), rect.shrink(size * 0.22), icon, color);
        focus_ring(ui, &response, rect, CornerRadius::same((size / 2.0) as u8));
    }
    if tip.is_empty() {
        response
    } else {
        response.on_hover_text(tip)
    }
}

pub const PILL_HEIGHT: f32 = 34.0;

/// A pill-shaped button with an optional icon.
pub fn pill(ui: &mut Ui, icon: Option<Icon>, text: &str, primary: bool) -> Response {
    let colors = colors(ui.ctx());
    let font = strong(Size::BODY);
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        font,
        if primary {
            colors.on_accent
        } else {
            colors.text
        },
    );
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 30.0, PILL_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() && ui.is_enabled();
        let fill = match (primary, hovered) {
            (true, true) => colors.accent_hi,
            (true, false) => colors.accent,
            (false, true) => colors.surface_hi,
            (false, false) => colors.surface,
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(17), fill);
        if !primary {
            painter.rect_stroke(
                rect,
                CornerRadius::same(17),
                Stroke::new(1.0, colors.line),
                StrokeKind::Inside,
            );
        }
        let mut x = rect.left() + 15.0;
        if let Some(icon) = icon {
            let r = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(15.0));
            paint_icon(
                painter,
                r,
                icon,
                if primary {
                    colors.on_accent
                } else {
                    colors.text
                },
            );
            x += icon_w;
        }
        let y = rect.center().y - galley.size().y / 2.0;
        painter.galley(pos2(x, y), galley, colors.text);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// How a dialog button presents itself.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Primary,
    Danger,
}

/// A compact rounded button for dialogs. It keeps an outline, so it stays
/// visible on every panel colour.
pub fn button(ui: &mut Ui, text: &str, tone: Tone) -> Response {
    let colors = colors(ui.ctx());
    let enabled = ui.is_enabled();
    let ink = match tone {
        _ if !enabled => colors.faint,
        Tone::Plain => colors.text,
        Tone::Primary => colors.on_accent,
        Tone::Danger => colors.error,
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), strong(Size::BODY), ink);
    let size = vec2(galley.size().x + 26.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, text));
    if ui.is_rect_visible(rect) {
        let hovered = enabled && response.hovered();
        let radius = CornerRadius::same(14);
        let painter = ui.painter();
        if tone == Tone::Primary && enabled {
            let fill = if hovered {
                colors.accent_hi
            } else {
                colors.accent
            };
            painter.rect_filled(rect, radius, fill);
        } else {
            let danger = tone == Tone::Danger && enabled;
            if hovered {
                let wash = if danger {
                    colors.error.gamma_multiply(0.16)
                } else {
                    colors.surface_hi
                };
                painter.rect_filled(rect, radius, wash);
            }
            let edge = if danger {
                colors.error.gamma_multiply(0.55)
            } else {
                colors.line
            };
            painter.rect_stroke(rect, radius, Stroke::new(1.0, edge), StrokeKind::Inside);
        }
        let pos = rect.center() - galley.size() / 2.0;
        painter.galley(pos, galley, ink);
        focus_ring(ui, &response, rect, radius);
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// An on/off switch. `label` names it for assistive technology.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let colors = colors(ui.ctx());
    let (rect, mut response) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let (enabled, state) = (ui.is_enabled(), *on);
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, enabled, state, label));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, state);
        let off = colors.surface.lerp_to_gamma(colors.muted, 0.45);
        let track = off.lerp_to_gamma(colors.accent, t);
        let knob = Color32::WHITE.lerp_to_gamma(control_ink(colors.on_accent, colors.accent), t);
        let dim = if enabled { 1.0 } else { 0.5 };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(11), track.gamma_multiply(dim));
        let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
        painter.circle_filled(pos2(x, rect.center().y), 8.0, knob.gamma_multiply(dim));
        focus_ring(ui, &response, rect, CornerRadius::same(11));
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Mutually exclusive choices drawn as one control. Returns whether the choice changed.
pub fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    salt: &str,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    let colors = colors(ui.ctx());
    let font = strong(Size::BODY);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, text)| {
            let galley = ui
                .painter()
                .layout_no_wrap((*text).to_owned(), font.clone(), colors.text);
            galley.size().x + 24.0
        })
        .collect();
    let size = vec2(widths.iter().sum::<f32>() + 4.0, 28.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let radius = CornerRadius::same(8);
    ui.painter().rect_filled(rect, radius, colors.bg);
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, colors.line),
        StrokeKind::Inside,
    );
    let enabled = ui.is_enabled();
    let mut changed = false;
    let mut x = rect.left() + 2.0;
    for ((option, text), width) in options.iter().zip(widths) {
        let cell = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(width, 24.0));
        x += width;
        let selected = *value == *option;
        let response = ui.interact(cell, ui.scope_id().with((salt, *text)), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::RadioButton, enabled, selected, *text)
        });
        if response.clicked() && !selected {
            *value = *option;
            changed = true;
        }
        let fill = if selected {
            colors.surface_hi
        } else if response.hovered() {
            colors.surface
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(cell, CornerRadius::same(6), fill);
        ui.painter().text(
            cell.center(),
            egui::Align2::CENTER_CENTER,
            *text,
            font.clone(),
            if selected { colors.text } else { colors.muted },
        );
        focus_ring(ui, &response, cell, CornerRadius::same(6));
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    changed
}

/// A keyboard key, as shown in the shortcut list.
pub fn key_cap(ui: &mut Ui, text: &str) {
    let colors = colors(ui.ctx());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), regular(Size::SMALL), colors.text);
    let size = vec2((galley.size().x + 14.0).max(26.0), 22.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let radius = CornerRadius::same(5);
    let painter = ui.painter();
    painter.rect_filled(rect, radius, colors.bg);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, colors.line),
        StrokeKind::Inside,
    );
    painter.galley(rect.center() - galley.size() / 2.0, galley, colors.text);
}

/// Restyles the stock egui widgets in `ui` for a `surface`-coloured container,
/// where their default fill is the container's own colour.
pub fn on_surface(ui: &mut Ui) {
    let colors = colors(ui.ctx());
    let edge = Stroke::new(1.0, colors.line);
    let visuals = ui.visuals_mut();
    visuals.extreme_bg_color = colors.bg;
    visuals.text_edit_bg_color = Some(colors.bg);
    let widgets = &mut visuals.widgets;
    widgets.inactive.bg_fill = colors.bg;
    widgets.inactive.weak_bg_fill = colors.bg;
    widgets.inactive.bg_stroke = edge;
    widgets.hovered.bg_stroke = edge;
    widgets.open.bg_stroke = edge;
}

/// Single-line text that ends in an ellipsis when it does not fit.
pub fn clipped(
    painter: &egui::Painter,
    pos: Pos2,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> Rect {
    if width <= 4.0 {
        return Rect::from_min_size(pos, Vec2::ZERO);
    }
    let galley = line(painter, text, font, color, width);
    let rect = Rect::from_min_size(pos, galley.size());
    painter.galley(pos, galley, color);
    rect
}

/// Lays out single-line text that ends in an ellipsis when it does not fit.
pub fn line(
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    painter.layout_job(job)
}

/// A deterministic colour for an artist's initial badge.
pub fn tint(text: &str) -> Color32 {
    let hash = text.bytes().fold(2166136261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16777619)
    });
    let palette = [
        Color32::from_rgb(0x52, 0x63, 0xe8),
        Color32::from_rgb(0x2f, 0x8f, 0x9d),
        Color32::from_rgb(0x9b, 0x67, 0x3e),
        Color32::from_rgb(0x8a, 0x4f, 0xb5),
        Color32::from_rgb(0x3d, 0x8b, 0x5a),
        Color32::from_rgb(0xb5, 0x4f, 0x6a),
        Color32::from_rgb(0x4f, 0x6d, 0xb5),
    ];
    palette[hash as usize % palette.len()]
}

/// Cover art, or a placeholder while it loads or when the album has none.
pub fn cover(
    painter: &egui::Painter,
    rect: Rect,
    texture: Option<egui::TextureId>,
    radius: u8,
    seed: &str,
) {
    let corner = CornerRadius::same(radius);
    match texture {
        Some(texture) => {
            painter.add(
                egui::epaint::RectShape::filled(rect, corner, Color32::WHITE)
                    .with_texture(texture, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
            );
        }
        None => {
            let base = tint(seed);
            painter.rect_filled(rect, corner, base.gamma_multiply(0.35));
            let inner = Rect::from_center_size(rect.center(), Vec2::splat(rect.width() * 0.34));
            paint_icon(painter, inner, Icon::Note, base.gamma_multiply(0.9));
        }
    }
}
