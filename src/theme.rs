//! The app's look: colours for light and dark, type, spacing, fonts and the
//! few painted pieces (cards, buttons, pills, icons, progress, the segmented
//! control). Everything visual that is not layout lives here, so the window
//! code in `app.rs` only says what goes where.
//!
//! The build stage owns this file: pick the app's own accent (the six
//! `accent*`/`on_accent` values in each palette), add the painted pieces the
//! app needs, and delete the ones it does not use. The contrast test at the
//! bottom must keep passing.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, TextStyle, Theme, Vec2, pos2, vec2,
};

/// Corner radius of cards.
pub const CARD_RADIUS: u8 = 5;
/// Corner radius of buttons, inputs and the segmented control.
pub const CONTROL_RADIUS: u8 = 4;
/// Height of buttons and single-line controls.
pub const CONTROL_HEIGHT: f32 = 32.0;
/// Height of the main button.
pub const PRIMARY_HEIGHT: f32 = 44.0;
/// Text sizes.
pub const BODY_SIZE: f32 = 14.5;
pub const SMALL_SIZE: f32 = 12.5;
pub const TITLE_SIZE: f32 = 21.0;

/// Font family for headings and the main button. Always defined; it falls
/// back to the regular face when Segoe UI Semibold is missing.
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// Font family for titles and big numbers: Bahnschrift, Windows' condensed
/// DIN-style face, falling back to the semibold face when it is missing.
pub fn display() -> FontFamily {
    FontFamily::Name("display".into())
}

pub fn body() -> FontId {
    FontId::new(BODY_SIZE, FontFamily::Proportional)
}

pub fn small() -> FontId {
    FontId::new(SMALL_SIZE, FontFamily::Proportional)
}

pub fn title() -> FontId {
    FontId::new(TITLE_SIZE, display())
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Window background.
    pub bg: Color32,
    /// Cards and inputs.
    pub surface: Color32,
    /// A row under the pointer.
    pub hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    /// Fills: the main button, the selected segment, progress.
    pub accent: Color32,
    /// Accent-coloured text, dark enough to read on surfaces and on
    /// `accent_soft`.
    pub accent_text: Color32,
    /// Text on an accent fill.
    pub on_accent: Color32,
    /// Accent at low strength, for tiles, pills and highlights.
    pub accent_soft: Color32,
    /// Resting fill of secondary buttons.
    pub control: Color32,
    pub control_hover: Color32,
    /// Unchecked boxes, progress tracks and the segmented control's track.
    pub track: Color32,
    pub success: Color32,
    pub success_soft: Color32,
    pub warn: Color32,
    pub warn_soft: Color32,
    pub error: Color32,
    pub error_soft: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

const LIGHT: Palette = Palette {
    bg: hex(0xECEFF6),
    surface: hex(0xFFFFFF),
    hover: hex(0xF3F2FD),
    border: hex(0xD9DEEA),
    text: hex(0x161A2B),
    muted: hex(0x555C73),
    accent: hex(0x4338CA),
    accent_text: hex(0x3730A3),
    on_accent: hex(0xFFFFFF),
    accent_soft: hex(0xE6E4FB),
    control: hex(0xFFFFFF),
    control_hover: hex(0xEEEFF8),
    track: hex(0xE3E6F0),
    success: hex(0x0F7A5A),
    success_soft: hex(0xE0F3EC),
    warn: hex(0x965000),
    warn_soft: hex(0xFCEFDC),
    error: hex(0xB4232F),
    error_soft: hex(0xFBE8EA),
};

const DARK: Palette = Palette {
    bg: hex(0x0D1120),
    surface: hex(0x161B2E),
    hover: hex(0x1E2440),
    border: hex(0x272E4A),
    text: hex(0xE8EAF6),
    muted: hex(0x99A0BC),
    accent: hex(0x8B8CF8),
    accent_text: hex(0xAEB0FF),
    on_accent: hex(0x0E1030),
    accent_soft: hex(0x25275A),
    control: hex(0x1D2238),
    control_hover: hex(0x262C47),
    track: hex(0x283050),
    success: hex(0x4FD1A1),
    success_soft: hex(0x15342C),
    warn: hex(0xF0B860),
    warn_soft: hex(0x3B2E17),
    error: hex(0xFF8D96),
    error_soft: hex(0x40202A),
};

pub fn palette(ctx: &egui::Context) -> Palette {
    match ctx.theme() {
        Theme::Dark => DARK,
        Theme::Light => LIGHT,
    }
}

fn visuals(p: Palette, mut v: egui::Visuals) -> egui::Visuals {
    let radius = CornerRadius::same(CONTROL_RADIUS);
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(CARD_RADIUS);
    v.menu_corner_radius = CornerRadius::same(CONTROL_RADIUS);
    v.extreme_bg_color = p.surface;
    v.faint_bg_color = p.bg;
    v.hyperlink_color = p.accent_text;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.error;
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = p.track;
    w.inactive.weak_bg_fill = p.control;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = radius;

    w.hovered.bg_fill = p.control_hover;
    w.hovered.weak_bg_fill = p.control_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    w.hovered.fg_stroke = Stroke::new(1.5, p.text);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.accent_soft;
    w.active.weak_bg_fill = p.accent_soft;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.5, p.text);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.control_hover;
    w.open.weak_bg_fill = p.control_hover;
    w.open.bg_stroke = Stroke::new(1.0, p.accent);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = radius;
    v
}

/// Colours, type sizes and spacing. Call once at start.
pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals_of(Theme::Light, visuals(LIGHT, egui::Visuals::light()));
    ctx.set_visuals_of(Theme::Dark, visuals(DARK, egui::Visuals::dark()));
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 6.0);
        style.spacing.interact_size.y = CONTROL_HEIGHT;
        style.spacing.slider_width = 150.0;
        style.spacing.icon_width = 18.0;
        style.spacing.combo_width = 110.0;
        for (text_style, size) in [
            (TextStyle::Body, BODY_SIZE),
            (TextStyle::Button, BODY_SIZE),
            (TextStyle::Small, 12.0),
            (TextStyle::Heading, TITLE_SIZE),
        ] {
            style
                .text_styles
                .insert(text_style, FontId::new(size, FontFamily::Proportional));
        }
    });
}

/// Uses Windows' Segoe UI for the interface, so the app looks at home, with
/// Windows' East Asian fonts behind it so file names in any script show.
/// egui's built-in fonts stay as the last fallback.
/// `wide` adds the East Asian fallback fonts. They are about 45 MB on disk,
/// so the sign-in overlay, which shows no item names, goes without them.
pub fn install_fonts(ctx: &egui::Context, wide: bool) {
    let fonts_dir = PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()))
        .join("Fonts");
    let mut fonts = egui::FontDefinitions::default();
    let add = |fonts: &mut egui::FontDefinitions, file: &str| -> bool {
        let Ok(bytes) = std::fs::read(fonts_dir.join(file)) else {
            return false;
        };
        fonts
            .font_data
            .insert(file.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
        true
    };

    let mut regular = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if add(&mut fonts, "segoeui.ttf") {
        regular.insert(0, "segoeui.ttf".to_owned());
    }
    let mut strong = regular.clone();
    if add(&mut fonts, "seguisb.ttf") {
        strong.insert(0, "seguisb.ttf".to_owned());
    }
    let mut headline = strong.clone();
    if add(&mut fonts, "bahnschrift.ttf") {
        headline.insert(0, "bahnschrift.ttf".to_owned());
    }
    for file in ["YuGothR.ttc", "msyh.ttc", "malgun.ttf"] {
        if wide && add(&mut fonts, file) {
            regular.push(file.to_owned());
            strong.push(file.to_owned());
            headline.push(file.to_owned());
        }
    }
    fonts.families.insert(FontFamily::Proportional, regular);
    fonts.families.insert(semibold(), strong);
    fonts.families.insert(display(), headline);
    ctx.set_fonts(fonts);
}

/// A rounded panel that groups related things.
pub fn card(p: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border.gamma_multiply(0.55)))
        .corner_radius(CARD_RADIUS)
        .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 10, spread: 0, color: Color32::from_black_alpha(22) })
        .inner_margin(egui::Margin::symmetric(16, 12))
}

/// A small heading above a group of controls.
pub fn section_label(ui: &mut egui::Ui, text: &str, p: Palette) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .family(semibold())
            .size(11.5)
            .extra_letter_spacing(0.9)
            .color(p.accent_text),
    );
}

/// The one filled button on the screen: the thing to press next.
pub fn primary_button<'a>(p: Palette, text: impl Into<String>, enabled: bool, width: f32) -> egui::Button<'a> {
    let label = egui::RichText::new(text)
        .family(semibold())
        .size(15.5)
        .color(if enabled { p.on_accent } else { p.muted });
    egui::Button::new(label)
        .fill(if enabled { p.accent } else { p.track })
        .stroke(Stroke::NONE)
        .corner_radius(CONTROL_RADIUS)
        .min_size(Vec2::new(width, PRIMARY_HEIGHT))
}

/// A quiet bordered button for everything that is not the next step.
pub fn secondary_button<'a>(p: Palette, text: impl Into<String>) -> egui::Button<'a> {
    egui::Button::new(egui::RichText::new(text).color(p.text))
        .fill(p.control)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CONTROL_RADIUS)
        .min_size(Vec2::new(0.0, CONTROL_HEIGHT))
}

/// A one-line text cut to `width` with an ellipsis.
pub fn truncated(
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    painter.layout_job(job)
}

/// A small rounded label such as a status, laid out like a widget.
pub fn pill(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) -> egui::Response {
    let max = ui.available_width().max(40.0);
    let galley = truncated(ui.painter(), text, FontId::new(12.0, semibold()), fg, max - 20.0);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 20.0, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, 3, bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
    response
}

/// Icons drawn with lines, so they never depend on a font having a glyph.
/// Never put emoji or symbol characters in labels: draw the icon here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Close,
    Play,
    Stop,
    Plus,
    Folder,
    Retry,
    /// A program window: startup apps.
    App,
    /// A clock face: scheduled tasks and times.
    Clock,
    /// A cog: services.
    Service,
    /// Two sliders: options.
    Options,
    /// A circled "i".
    Info,
    ChevronDown,
    ChevronRight,
    Check,
    /// An expanding square: open the full window.
    Expand,
}

pub fn paint_icon(painter: &egui::Painter, centre: Pos2, icon: Icon, color: Color32) {
    let stroke = Stroke::new(1.6, color);
    let c = centre;
    match icon {
        Icon::App => {
            let r = Rect::from_center_size(c, vec2(14.0, 12.0));
            painter.rect_stroke(r, 2, stroke, StrokeKind::Middle);
            painter.line_segment([pos2(r.left(), r.top() + 3.5), pos2(r.right(), r.top() + 3.5)], stroke);
        }
        Icon::Clock => {
            painter.circle_stroke(c, 6.5, stroke);
            painter.line_segment([c, c + vec2(0.0, -4.0)], stroke);
            painter.line_segment([c, c + vec2(3.0, 1.5)], stroke);
        }
        Icon::Service => {
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let d = vec2(a.cos(), a.sin());
                painter.line_segment([c + d * 4.5, c + d * 7.0], Stroke::new(2.2, color));
            }
            painter.circle_stroke(c, 4.5, stroke);
            painter.circle_filled(c, 1.6, color);
        }
        Icon::Options => {
            for (y, x) in [(-3.5, 2.5), (3.5, -2.5)] {
                painter.line_segment([c + vec2(-7.0, y), c + vec2(7.0, y)], stroke);
                painter.circle_filled(c + vec2(x, y), 2.6, color);
            }
        }
        Icon::Info => {
            painter.circle_stroke(c, 6.5, stroke);
            painter.line_segment([c + vec2(0.0, -0.5), c + vec2(0.0, 3.5)], stroke);
            painter.circle_filled(c + vec2(0.0, -3.2), 1.0, color);
        }
        Icon::ChevronDown => {
            let pts = vec![c + vec2(-4.0, -2.0), c + vec2(0.0, 2.0), c + vec2(4.0, -2.0)];
            painter.add(Shape::line(pts, stroke));
        }
        Icon::ChevronRight => {
            let pts = vec![c + vec2(-2.0, -4.0), c + vec2(2.0, 0.0), c + vec2(-2.0, 4.0)];
            painter.add(Shape::line(pts, stroke));
        }
        Icon::Check => {
            let pts = vec![c + vec2(-4.5, 0.0), c + vec2(-1.5, 3.0), c + vec2(4.5, -3.5)];
            painter.add(Shape::line(pts, Stroke::new(1.8, color)));
        }
        Icon::Expand => {
            let d = 5.0;
            for (sx, sy) in [(-1.0, -1.0), (1.0, 1.0)] {
                let corner = c + vec2(d * sx, d * sy);
                painter.line_segment([corner, corner + vec2(-3.5 * sx, 0.0)], stroke);
                painter.line_segment([corner, corner + vec2(0.0, -3.5 * sy)], stroke);
            }
            painter.line_segment([c + vec2(-d, -d), c + vec2(-1.0, -1.0)], stroke);
            painter.line_segment([c + vec2(d, d), c + vec2(1.0, 1.0)], stroke);
        }
        Icon::Close => {
            let d = 4.5;
            painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
            painter.line_segment([c + vec2(d, -d), c + vec2(-d, d)], stroke);
        }
        Icon::Play => {
            let points = vec![c + vec2(-3.5, -5.5), c + vec2(5.5, 0.0), c + vec2(-3.5, 5.5)];
            painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(c, vec2(10.0, 10.0)), 2, color);
        }
        Icon::Plus => {
            let d = 5.0;
            painter.line_segment([c + vec2(-d, 0.0), c + vec2(d, 0.0)], stroke);
            painter.line_segment([c + vec2(0.0, -d), c + vec2(0.0, d)], stroke);
        }
        Icon::Folder => {
            let body = Rect::from_center_size(c + vec2(0.0, 1.0), vec2(14.0, 10.0));
            painter.rect_stroke(body, 2, stroke, StrokeKind::Middle);
            let tab = [
                body.left_top(),
                body.left_top() + vec2(0.0, -2.5),
                body.left_top() + vec2(5.0, -2.5),
                body.left_top() + vec2(6.5, 0.0),
            ];
            painter.add(Shape::line(tab.to_vec(), stroke));
        }
        Icon::Retry => {
            // Three quarters of a circle with an arrow head at its end.
            let r = 5.5;
            let points: Vec<Pos2> = (0..=18)
                .map(|i| {
                    let a = -0.4 + i as f32 / 18.0 * std::f32::consts::PI * 1.6;
                    c + vec2(a.cos() * r, a.sin() * r)
                })
                .collect();
            let end = c + vec2((-0.4f32).cos() * r, (-0.4f32).sin() * r);
            painter.add(Shape::line(points, stroke));
            painter.line_segment([end, end + vec2(-0.5, -4.0)], stroke);
            painter.line_segment([end, end + vec2(-4.0, 0.5)], stroke);
        }
    }
}

/// A round, borderless icon button with a soft hover disc.
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, p: Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let hot = ui
        .ctx()
        .animate_bool_with_time(response.id, response.hovered(), 0.12);
    if hot > 0.0 {
        ui.painter()
            .circle_filled(rect.center(), rect.width() / 2.0, p.track.gamma_multiply(hot));
    }
    let color = if response.hovered() { p.text } else { p.muted };
    paint_icon(ui.painter(), rect.center(), icon, color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A rounded progress track. `None` is an unknown amount: a short segment
/// sweeps along the track instead.
pub fn progress_track(painter: &egui::Painter, rect: Rect, fraction: Option<f32>, time: f64, p: Palette) {
    let radius = 1.0;
    painter.rect_filled(rect, radius, p.track);
    let fill = match fraction {
        Some(f) => Rect::from_min_size(rect.min, vec2(rect.width() * f.clamp(0.0, 1.0), rect.height())),
        None => {
            let span = rect.width() * 0.3;
            let t = (time * 0.8).fract() as f32;
            let left = rect.left() - span + (rect.width() + span) * t;
            Rect::from_x_y_ranges(left.max(rect.left())..=(left + span).min(rect.right()), rect.y_range())
        }
    };
    if fill.width() > 0.5 {
        painter.rect_filled(fill, radius, p.accent);
    }
}

/// Choices side by side on one track; the chosen one is filled with the
/// accent, which slides when the choice changes. Returns the newly chosen
/// index, if any.
pub fn segmented(ui: &mut egui::Ui, id: egui::Id, labels: &[&str], selected: usize, p: Palette) -> Option<usize> {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, CONTROL_HEIGHT), Sense::hover());
    let enabled = ui.is_enabled();
    ui.painter().rect_filled(rect, CONTROL_RADIUS, p.track);
    let n = labels.len().max(1) as f32;
    let inner = rect.shrink(3.0);
    let seg_w = inner.width() / n;
    let slide = ui.ctx().animate_value_with_time(id, selected as f32, 0.15);
    let knob = Rect::from_min_size(pos2(inner.left() + seg_w * slide, inner.top()), vec2(seg_w, inner.height()));
    let knob_fill = if enabled { p.accent } else { p.muted };
    ui.painter().rect_filled(knob, CONTROL_RADIUS - 2, knob_fill);
    let mut chosen = None;
    for (i, label) in labels.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(inner.left() + seg_w * i as f32, inner.top()), vec2(seg_w, inner.height()));
        let response = ui.interact(cell, id.with(i), Sense::click());
        if response.clicked() && i != selected {
            chosen = Some(i);
        }
        let on = i == selected;
        if response.hovered() && !on {
            ui.painter().rect_filled(cell, CONTROL_RADIUS - 2, p.surface.gamma_multiply(0.7));
        }
        let fg = if on { p.on_accent } else { p.text };
        let galley = truncated(ui.painter(), label, FontId::new(13.5, semibold()), fg, cell.width() - 8.0);
        ui.painter().galley(cell.center() - galley.size() / 2.0, galley, fg);
        let _ = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    chosen
}

/// A checkbox with squarer corners than the buttons: at this size the
/// button radius would make it look like a radio button.
pub fn checkbox(ui: &mut egui::Ui, checked: &mut bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        for state in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active] {
            state.corner_radius = CornerRadius::same(5);
        }
        ui.checkbox(checked, text)
    })
    .inner
}

/// The big number in the summary card.
pub fn hero() -> FontId {
    FontId::new(38.0, display())
}

/// Semibold body text, for names in lists.
pub fn strong() -> FontId {
    FontId::new(BODY_SIZE, semibold())
}

/// The sign-in overlay's window size.
pub const OVERLAY_SIZE: Vec2 = vec2(330.0, 176.0);
/// Height of a row in the startup list.
pub const ROW_HEIGHT: f32 = 48.0;
/// Width of the name column in the timeline.
pub const TIMELINE_LABEL: f32 = 150.0;

/// What a status means, so each one looks different everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Accent,
    Success,
    Warn,
    Error,
    Neutral,
}

/// (text, fill) for a tone.
pub fn tone_colors(p: Palette, tone: Tone) -> (Color32, Color32) {
    match tone {
        Tone::Accent => (p.accent_text, p.accent_soft),
        Tone::Success => (p.success, p.success_soft),
        Tone::Warn => (p.warn, p.warn_soft),
        Tone::Error => (p.error, p.error_soft),
        Tone::Neutral => (p.muted, p.track),
    }
}

/// The solid colour of a tone, for bars and dots.
pub fn tone_solid(p: Palette, tone: Tone) -> Color32 {
    match tone {
        Tone::Accent => p.accent,
        Tone::Success => p.success,
        Tone::Warn => p.warn,
        Tone::Error => p.error,
        Tone::Neutral => p.muted,
    }
}

pub fn tone_pill(ui: &mut egui::Ui, text: &str, tone: Tone, p: Palette) -> egui::Response {
    let (fg, bg) = tone_colors(p, tone);
    pill(ui, text, fg, bg)
}

/// A pill with a dot in front, for live state ("Starting up").
pub fn live_pill(ui: &mut egui::Ui, text: &str, tone: Tone, pulse: bool, p: Palette) -> egui::Response {
    let (fg, bg) = tone_colors(p, tone);
    let galley = truncated(ui.painter(), text, FontId::new(12.0, semibold()), fg, 200.0);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 32.0, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, 3, bg);
    let dot = pos2(rect.left() + 13.0, rect.center().y);
    let alpha = if pulse {
        // Dim and bright on alternate seconds. No repaint is asked for: the
        // collector's once-a-second tick redraws the window anyway, and a
        // smooth 20-frames-a-second pulse made Windows Startup Monitor the most expensive
        // thing at sign-in.
        let t = ui.input(|i| i.time);
        if (t as u64) % 2 == 0 { 1.0 } else { 0.5 }
    } else {
        1.0
    };
    ui.painter().circle_filled(dot, 3.5, tone_solid(p, tone).gamma_multiply(alpha));
    ui.painter()
        .galley(pos2(rect.left() + 22.0, rect.center().y - galley.size().y / 2.0), galley, fg);
    response
}

/// Text that acts as a link: accent coloured, underlined on hover.
pub fn text_link(ui: &mut egui::Ui, text: &str, p: Palette) -> egui::Response {
    let galley = truncated(ui.painter(), text, body(), p.accent_text, ui.available_width());
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::click());
    let y = rect.bottom() - 1.0;
    if response.hovered() {
        ui.painter().line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, p.accent_text));
    }
    ui.painter().galley(rect.min, galley, p.accent_text);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small icon in a soft rounded square, at the start of a list row.
pub fn icon_tile(painter: &egui::Painter, centre: Pos2, icon: Icon, tone: Tone, p: Palette) {
    let (fg, bg) = tone_colors(p, tone);
    painter.circle_filled(centre, 15.0, bg);
    paint_icon(painter, centre, icon, fg);
}

/// A labelled reading with a short bar under it (CPU, memory, disk).
pub fn meter(ui: &mut egui::Ui, label: &str, value: &str, fraction: f32, width: f32, p: Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 44.0), Sense::hover());
    let painter = ui.painter();
    let l = truncated(painter, label, small(), p.muted, width);
    painter.galley(rect.min, l, p.muted);
    let v = truncated(painter, value, strong(), p.text, width);
    painter.galley(rect.min + vec2(0.0, 16.0), v, p.text);
    let bar = Rect::from_min_size(pos2(rect.left(), rect.bottom() - 5.0), vec2(width, 5.0));
    painter.rect_filled(bar, 1, p.track);
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.01 {
        let color = if f > 0.85 { p.warn } else { p.accent };
        painter.rect_filled(Rect::from_min_size(bar.min, vec2(width * f, 5.0)), 1, color);
    }
}

/// The last few boots as small bars, the newest highlighted, with a line at
/// the usual (median) value.
pub fn boot_bars(ui: &mut egui::Ui, values: &[u64], usual: Option<u64>, size: Vec2, p: Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let max = values.iter().copied().max().unwrap_or(1).max(1) as f32;
    // Start the scale a little under the shortest boot, so the differences
    // between boots show.
    let floor = values.iter().copied().min().unwrap_or(0) as f32 * 0.6;
    let scale = |v: f32| ((v - floor) / (max - floor).max(1.0)).clamp(0.0, 1.0);
    let n = values.len().max(1) as f32;
    let gap = 4.0;
    let w = ((rect.width() - gap * (n - 1.0)) / n).min(14.0);
    let total = w * n + gap * (n - 1.0);
    let left = rect.right() - total;
    for (i, &v) in values.iter().enumerate() {
        let h = (scale(v as f32) * rect.height()).max(3.0);
        let x = left + i as f32 * (w + gap);
        let bar = Rect::from_min_max(pos2(x, rect.bottom() - h), pos2(x + w, rect.bottom()));
        let last = i + 1 == values.len();
        ui.painter().rect_filled(bar, 1, if last { p.accent } else { p.track });
    }
    if let Some(u) = usual {
        let y = rect.bottom() - scale(u as f32) * rect.height();
        dashed(ui.painter(), pos2(left - 4.0, y), pos2(rect.right(), y), Stroke::new(1.0, p.muted));
    }
    response
}

fn dashed(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke) {
    let len = (b - a).length();
    let dir = (b - a) / len.max(1.0);
    let mut d = 0.0;
    while d < len {
        let e = (d + 3.0).min(len);
        painter.line_segment([a + dir * d, a + dir * e], stroke);
        d += 6.0;
    }
}

/// One bar of the timeline.
pub struct Span {
    pub label: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub tone: Tone,
    /// Worked out afterwards: drawn lighter.
    pub estimated: bool,
}

/// What started when: one row per item with a bar from start to settled, a
/// time axis, and a line where the PC became usable. Returns the height used.
pub fn timeline(ui: &mut egui::Ui, spans: &[Span], usable_ms: Option<u64>, now_ms: Option<u64>, p: Palette) {
    let row_h = 22.0;
    let axis_h = 22.0;
    let width = ui.available_width();
    let height = axis_h + row_h * spans.len() as f32 + 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter_at(rect.expand(2.0));
    let plot = Rect::from_min_max(pos2(rect.left() + TIMELINE_LABEL, rect.top()), pos2(rect.right() - 8.0, rect.bottom()));
    let span_ms = spans
        .iter()
        .map(|s| s.end_ms)
        .chain(usable_ms)
        .chain(now_ms)
        .max()
        .unwrap_or(10_000)
        .max(10_000) as f32
        * 1.05;
    let x_of = |ms: u64| plot.left() + ms as f32 / span_ms * plot.width();

    // Axis: a tick every 10, 20 or 30 s, whichever keeps labels apart.
    let step = [5_000u64, 10_000, 20_000, 30_000, 60_000]
        .into_iter()
        .find(|s| plot.width() / (span_ms / *s as f32) >= 56.0)
        .unwrap_or(120_000);
    let mut t = 0;
    while (t as f32) <= span_ms {
        let x = x_of(t);
        painter.line_segment([pos2(x, plot.top() + axis_h - 4.0), pos2(x, plot.bottom())], Stroke::new(1.0, p.border));
        let label = if t == 0 { "Sign-in".to_owned() } else { crate::history::fmt_secs(t) };
        let g = truncated(&painter, &label, small(), p.muted, 80.0);
        let gx = if t == 0 { x } else { x - g.size().x / 2.0 };
        // Leave room for the "Usable" label at the dashed line.
        let clear = usable_ms.is_none_or(|u| (x_of(u) - x).abs() > 70.0);
        if clear && gx + g.size().x <= plot.right() + 8.0 {
            painter.galley(pos2(gx, plot.top()), g, p.muted);
        }
        t += step;
    }

    for (i, s) in spans.iter().enumerate() {
        let y = rect.top() + axis_h + row_h * i as f32;
        let mid = y + row_h / 2.0;
        let name = truncated(&painter, &s.label, small(), p.text, TIMELINE_LABEL - 12.0);
        painter.galley(pos2(rect.left(), mid - name.size().y / 2.0), name, p.text);
        let x0 = x_of(s.start_ms);
        let x1 = x_of(s.end_ms).max(x0 + 4.0);
        let bar = Rect::from_min_max(pos2(x0, mid - 6.0), pos2(x1, mid + 6.0));
        let color = tone_solid(p, s.tone);
        painter.rect_filled(bar, 1, if s.estimated { color.gamma_multiply(0.45) } else { color });
    }

    if let Some(now) = now_ms {
        let x = x_of(now);
        painter.line_segment([pos2(x, plot.top() + axis_h - 4.0), pos2(x, plot.bottom())], Stroke::new(1.5, p.accent));
    }
    if let Some(u) = usable_ms {
        let x = x_of(u);
        dashed(&painter, pos2(x, plot.top() + axis_h - 4.0), pos2(x, plot.bottom()), Stroke::new(1.5, p.success));
        let text = format!("Usable at {}", crate::history::fmt_secs(u));
        let g = truncated(&painter, &text, FontId::new(SMALL_SIZE, semibold()), p.success, 160.0);
        let gx = (x - g.size().x / 2.0).min(rect.right() - g.size().x);
        painter.galley(pos2(gx, plot.top()), g, p.success);
    }
}

/// A rounded bar showing a share of the largest value, for the cost list.
pub fn paint_share_bar(painter: &egui::Painter, rect: Rect, fraction: f32, tone: Tone, p: Palette) {
    let r = 1.0;
    painter.rect_filled(rect, r, p.track);
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let fill = Rect::from_min_size(rect.min, vec2((rect.width() * f).max(rect.height()), rect.height()));
        painter.rect_filled(fill, r, tone_solid(p, tone));
    }
}

/// A status pill painted with its right edge at `right_centre`. Returns its
/// width.
pub fn paint_tone_pill(painter: &egui::Painter, right_centre: Pos2, text: &str, tone: Tone, p: Palette) -> f32 {
    let (fg, bg) = tone_colors(p, tone);
    let galley = truncated(painter, text, FontId::new(12.0, semibold()), fg, 160.0);
    let w = galley.size().x + 20.0;
    let rect = Rect::from_min_size(pos2(right_centre.x - w, right_centre.y - 12.0), vec2(w, 24.0));
    painter.rect_filled(rect, 3, bg);
    painter.galley(rect.center() - galley.size() / 2.0, galley, fg);
    w
}

/// The empty-state drawing: a half-dial gauge with tick marks and a needle
/// partway round, over a short baseline.
pub fn paint_startup_motif(painter: &egui::Painter, centre: Pos2, p: Palette) {
    use std::f32::consts::PI;
    let c = centre + vec2(0.0, 14.0);
    let r = 44.0;
    // Filled half disc behind the dial.
    let disc: Vec<Pos2> = std::iter::once(c)
        .chain((0..=32).map(|i| {
            let a = PI + i as f32 / 32.0 * PI;
            c + vec2(a.cos() * r, a.sin() * r)
        }))
        .collect();
    painter.add(Shape::convex_polygon(disc, p.accent_soft, Stroke::NONE));
    let arc = |from: f32, to: f32, rad: f32| -> Vec<Pos2> {
        (0..=24)
            .map(|i| {
                let a = PI + (from + (to - from) * i as f32 / 24.0) * PI;
                c + vec2(a.cos() * rad, a.sin() * rad)
            })
            .collect()
    };
    painter.add(Shape::line(arc(0.0, 1.0, r - 6.0), Stroke::new(4.0, p.track)));
    painter.add(Shape::line(arc(0.0, 0.62, r - 6.0), Stroke::new(4.0, p.accent)));
    for i in 0..=8 {
        let a = PI + i as f32 / 8.0 * PI;
        let d = vec2(a.cos(), a.sin());
        painter.line_segment([c + d * (r - 18.0), c + d * (r - 13.0)], Stroke::new(1.5, p.accent));
    }
    let needle = PI + 0.62 * PI;
    painter.line_segment([c, c + vec2(needle.cos(), needle.sin()) * (r - 14.0)], Stroke::new(2.6, p.accent));
    painter.circle_filled(c, 4.5, p.accent);
    painter.line_segment([c + vec2(-r, 0.0), c + vec2(r, 0.0)], Stroke::new(2.0, p.accent));
}

/// WCAG 2 contrast ratio between two colours (1 to 21).
#[cfg(test)]
fn contrast(a: Color32, b: Color32) -> f32 {
    fn channel(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    fn luminance(c: Color32) -> f32 {
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_of_known_pairs() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.01);
    }

    /// Every text colour reaches WCAG AA (4.5:1) on what it is drawn on.
    #[test]
    fn text_is_readable_in_both_palettes() {
        for (name, p) in [("light", LIGHT), ("dark", DARK)] {
            let pairs = [
                ("text/bg", p.text, p.bg),
                ("text/surface", p.text, p.surface),
                ("text/hover", p.text, p.hover),
                ("text/track", p.text, p.track),
                ("muted/bg", p.muted, p.bg),
                ("muted/surface", p.muted, p.surface),
                ("muted/hover", p.muted, p.hover),
                ("muted/track", p.muted, p.track),
                ("on_accent/accent", p.on_accent, p.accent),
                ("accent_text/surface", p.accent_text, p.surface),
                ("accent_text/accent_soft", p.accent_text, p.accent_soft),
                ("success/surface", p.success, p.surface),
                ("warn/surface", p.warn, p.surface),
                ("error/surface", p.error, p.surface),
                ("success/success_soft", p.success, p.success_soft),
                ("warn/warn_soft", p.warn, p.warn_soft),
                ("error/error_soft", p.error, p.error_soft),
                ("text/control", p.text, p.control),
            ];
            for (pair, fg, bg) in pairs {
                let ratio = contrast(fg, bg);
                assert!(ratio >= 4.5, "{name} {pair}: {ratio:.2}");
            }
        }
    }
}
