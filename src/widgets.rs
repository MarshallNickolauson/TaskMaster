//! Colors, number formatting and the little painters (sparkline, area graph) the UI is built from.

use std::collections::VecDeque;

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Mesh, Painter, Rect, Shape, Stroke, StrokeKind};

pub const ACCENT: Color32 = Color32::from_rgb(45, 212, 160);
pub const BG: Color32 = Color32::from_rgb(9, 12, 18);
pub const SURFACE: Color32 = Color32::from_rgb(15, 20, 29);
pub const SURFACE_2: Color32 = Color32::from_rgb(20, 27, 39);
pub const LINE: Color32 = Color32::from_rgb(31, 39, 53);
pub const LINE_2: Color32 = Color32::from_rgb(44, 55, 73);
pub const TEXT: Color32 = Color32::from_rgb(234, 239, 247);
pub const MUTED: Color32 = Color32::from_rgb(148, 160, 180);
pub const FAINT: Color32 = Color32::from_rgb(103, 116, 138);
pub const AMBER: Color32 = Color32::from_rgb(251, 191, 36);
pub const RED: Color32 = Color32::from_rgb(244, 63, 94);
pub const CYAN: Color32 = Color32::from_rgb(34, 211, 238);
pub const VIOLET: Color32 = Color32::from_rgb(167, 139, 250);

pub const HISTORY: usize = 150;

pub fn push(h: &mut VecDeque<f32>, v: f32) {
    if h.len() == HISTORY {
        h.pop_front();
    }
    h.push_back(v);
}

pub fn fmt_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1000.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    match i {
        0 => format!("{b} B"),
        1 => format!("{v:.0} {}", U[i]),
        2 => format!("{v:.1} {}", U[i]),
        _ => format!("{v:.2} {}", U[i]),
    }
}

pub fn fmt_rate(b: f32) -> String {
    if b < 1024.0 {
        "0".into()
    } else {
        format!("{}/s", fmt_bytes(b as u64))
    }
}

pub fn fmt_age(s: u64) -> String {
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {}s", s / 60, s % 60),
        3600..=86399 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d {}h", s / 86400, s % 86400 / 3600),
    }
}

/// Green → amber → red as load climbs. `t` is 0..=1.
pub fn heat(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |a: Color32, b: Color32, f: f32| {
        let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f) as u8;
        Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
    };
    if t < 0.5 { lerp(ACCENT, AMBER, t * 2.0) } else { lerp(AMBER, RED, (t - 0.5) * 2.0) }
}

/// Stable per-name color for the little app tiles.
pub fn name_color(name: &str) -> Color32 {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    egui::ecolor::Hsva::new((h % 360) as f32 / 360.0, 0.5, 0.78, 1.0).into()
}

pub fn label(p: &Painter, rect: Rect, text: &str, font: FontId, color: Color32) {
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping { max_width: rect.width(), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = p.layout_job(job);
    let y = rect.center().y - g.size().y / 2.0;
    p.galley(pos2(rect.left(), y), g, color);
}

/// Mono, uppercase, letter-spaced caption like the website's section labels.
pub fn caps(p: &Painter, pos: egui::Pos2, anchor: Align2, text: &str, color: Color32) {
    p.text(pos, anchor, text.to_uppercase(), FontId::monospace(10.5), color);
}

fn points(rect: Rect, data: &VecDeque<f32>, max: f32) -> Vec<egui::Pos2> {
    let step = rect.width() / (HISTORY - 1) as f32;
    let n = data.len();
    data.iter()
        .enumerate()
        .map(|(i, v)| pos2(rect.right() - (n - 1 - i) as f32 * step, rect.bottom() - (v / max).clamp(0.0, 1.0) * rect.height()))
        .collect()
}

fn area(p: &Painter, rect: Rect, pts: &[egui::Pos2], color: Color32, fill: f32) {
    if pts.len() < 2 {
        return;
    }
    let mut mesh = Mesh::default();
    let (top, bot) = (color.gamma_multiply(fill), color.gamma_multiply(0.0));
    for w in pts.windows(2) {
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(w[0], top);
        mesh.colored_vertex(w[1], top);
        mesh.colored_vertex(pos2(w[1].x, rect.bottom()), bot);
        mesh.colored_vertex(pos2(w[0].x, rect.bottom()), bot);
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::line(pts.to_vec(), Stroke::new(1.5, color)));
}

/// Tiny filled line chart with no chrome. `max` of `None` auto-scales.
pub fn sparkline(p: &Painter, rect: Rect, series: &[(&VecDeque<f32>, Color32)], max: Option<f32>) {
    let p = p.with_clip_rect(rect);
    let max = max.unwrap_or_else(|| nice_max(series));
    for (d, c) in series {
        area(&p, rect, &points(rect, d, max), *c, 0.25);
    }
}

fn nice_max(series: &[(&VecDeque<f32>, Color32)]) -> f32 {
    let m = series.iter().flat_map(|(d, _)| d.iter().copied()).fold(0.0f32, f32::max).max(1.0);
    let mag = 10f32.powf(m.log10().floor());
    [1.0, 2.0, 5.0, 10.0].iter().map(|k| k * mag).find(|&c| c >= m * 1.05).unwrap_or(m)
}

/// Full chart: frame, grid, labelled max, areas. Returns nothing; purely visual.
pub fn graph(ui: &mut egui::Ui, size: egui::Vec2, title: &str, value: &str, series: &[(&VecDeque<f32>, Color32)], max: Option<f32>, fmt: &dyn Fn(f32) -> String) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, SURFACE);
    p.rect_stroke(rect, 0.0, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let plot = Rect::from_min_max(pos2(rect.left() + 1.0, rect.top() + 44.0), pos2(rect.right() - 1.0, rect.bottom() - 1.0));
    for i in 1..4 {
        let y = plot.top() + plot.height() * i as f32 / 4.0;
        p.hline(plot.x_range(), y, Stroke::new(1.0, LINE.gamma_multiply(0.7)));
    }
    let max = max.unwrap_or_else(|| nice_max(series));
    for (d, c) in series {
        area(&p.with_clip_rect(plot), plot, &points(plot, d, max), *c, 0.22);
    }
    caps(&p, rect.left_top() + vec2(14.0, 14.0), Align2::LEFT_TOP, title, MUTED);
    p.text(rect.left_top() + vec2(14.0, 28.0), Align2::LEFT_TOP, value, FontId::proportional(20.0), TEXT);
    p.text(rect.right_top() + vec2(-12.0, 14.0), Align2::RIGHT_TOP, format!("max {}", fmt(max)), FontId::monospace(10.5), FAINT);
}
