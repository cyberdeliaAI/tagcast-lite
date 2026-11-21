//! Colours, fonts and the small vector icons the player draws itself.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Pos2, Rect,
    Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

pub const BG: Color32 = Color32::from_rgb(0x10, 0x14, 0x1e);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x0b, 0x0e, 0x16);
pub const BAR: Color32 = Color32::from_rgb(0x15, 0x1a, 0x27);
pub const SURFACE: Color32 = Color32::from_rgb(0x1a, 0x20, 0x2f);
pub const SURFACE_HI: Color32 = Color32::from_rgb(0x25, 0x2d, 0x44);
pub const LINE: Color32 = Color32::from_rgb(0x23, 0x2a, 0x3b);
pub const TEXT: Color32 = Color32::from_rgb(0xe9, 0xec, 0xf5);
pub const MUTED: Color32 = Color32::from_rgb(0x95, 0x9d, 0xb2);
pub const FAINT: Color32 = Color32::from_rgb(0x60, 0x68, 0x7d);
pub const ACCENT: Color32 = Color32::from_rgb(0x52, 0x63, 0xe8);
pub const ACCENT_HI: Color32 = Color32::from_rgb(0x66, 0x76, 0xf2);
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0x9d, 0xa8, 0xff);
pub const CREAM: Color32 = Color32::from_rgb(0xff, 0xf7, 0xde);
pub const WARN: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x5a);
pub const ERROR: Color32 = Color32::from_rgb(0xf2, 0x7c, 0x7c);

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
            (TextStyle::Small, regular(11.5)),
            (TextStyle::Body, regular(13.5)),
            (TextStyle::Button, regular(13.5)),
            (TextStyle::Heading, strong(24.0)),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(10.0, 5.0);
        style.spacing.interact_size.y = 28.0;
        style.spacing.scroll = egui::style::ScrollStyle::floating();
        style.spacing.scroll.bar_width = 8.0;
        style.interaction.selectable_labels = false;

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0, LINE);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.extreme_bg_color = SURFACE;
        v.faint_bg_color = SURFACE;
        v.text_edit_bg_color = Some(SURFACE);
        v.hyperlink_color = ACCENT_TEXT;
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.0, CREAM);
        v.warn_fg_color = WARN;
        v.error_fg_color = ERROR;
        v.override_text_color = None;
        v.slider_trailing_fill = true;
        let radius = CornerRadius::same(6);
        let w = &mut v.widgets;
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        w.noninteractive.bg_fill = BG;
        w.inactive.bg_fill = SURFACE;
        w.inactive.weak_bg_fill = SURFACE;
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.corner_radius = radius;
        w.hovered.bg_fill = SURFACE_HI;
        w.hovered.weak_bg_fill = SURFACE_HI;
        w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.bg_stroke = Stroke::NONE;
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;
        w.active.bg_fill = ACCENT;
        w.active.weak_bg_fill = ACCENT;
        w.active.fg_stroke = Stroke::new(1.0, CREAM);
        w.active.bg_stroke = Stroke::NONE;
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;
        w.open.bg_fill = SURFACE_HI;
        w.open.weak_bg_fill = SURFACE_HI;
        w.open.corner_radius = radius;
    });
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
}

/// Draws an icon centred in `rect`. Icons are designed on a 20-point grid.
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height()) / 20.0;
    let c = rect.center();
    let p = |x: f32, y: f32| pos2(c.x + x * s, c.y + y * s);
    let stroke = Stroke::new(1.7 * s.max(0.75), color);
    let fill = |points: Vec<Pos2>| Shape::convex_polygon(points, color, Stroke::NONE);
    match icon {
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

/// A round, borderless icon button.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, color: Color32, tip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        let color = if !ui.is_enabled() {
            FAINT
        } else if response.hovered() && color == MUTED {
            TEXT
        } else {
            color
        };
        if response.hovered() && ui.is_enabled() {
            ui.painter()
                .circle_filled(rect.center(), size * 0.5, SURFACE_HI.gamma_multiply(0.8));
        }
        paint_icon(ui.painter(), rect.shrink(size * 0.22), icon, color);
    }
    if tip.is_empty() {
        response
    } else {
        response.on_hover_text(tip)
    }
}

/// A pill-shaped button with an optional icon.
pub fn pill(ui: &mut Ui, icon: Option<Icon>, text: &str, primary: bool) -> Response {
    let font = strong(13.0);
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), font, if primary { CREAM } else { TEXT });
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 30.0, 34.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() && ui.is_enabled();
        let fill = match (primary, hovered) {
            (true, true) => ACCENT_HI,
            (true, false) => ACCENT,
            (false, true) => SURFACE_HI,
            (false, false) => SURFACE,
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(17), fill);
        if !primary {
            painter.rect_stroke(
                rect,
                CornerRadius::same(17),
                Stroke::new(1.0, LINE),
                StrokeKind::Inside,
            );
        }
        let mut x = rect.left() + 15.0;
        if let Some(icon) = icon {
            let r = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(15.0));
            paint_icon(painter, r, icon, if primary { CREAM } else { TEXT });
            x += icon_w;
        }
        let y = rect.center().y - galley.size().y / 2.0;
        painter.galley(pos2(x, y), galley, TEXT);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
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
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let galley = painter.layout_job(job);
    let rect = Rect::from_min_size(pos, galley.size());
    painter.galley(pos, galley, color);
    rect
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
