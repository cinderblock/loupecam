//! Drawing helpers on top of tiny-skia: text (DejaVu Sans via fontdue), rounded
//! rectangles, and the few icons the UI needs.

use std::collections::HashMap;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Rect, Stroke, Transform};

pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color::from_rgba8(r, g, b, a)
}

pub fn text() -> Color {
    Color::from_rgba8(230, 232, 234, 255)
}
pub fn muted() -> Color {
    Color::from_rgba8(139, 147, 156, 255)
}
pub fn accent() -> Color {
    Color::from_rgba8(63, 169, 255, 255)
}
pub fn red() -> Color {
    Color::from_rgba8(229, 72, 77, 255)
}
pub fn panel() -> Color {
    Color::from_rgba8(30, 33, 37, 255)
}
pub fn button() -> Color {
    Color::from_rgba8(44, 49, 56, 255)
}
pub fn button_active() -> Color {
    Color::from_rgba8(36, 66, 92, 255)
}

/// Text renderer with a glyph cache.
pub struct Fonts {
    regular: fontdue::Font,
    bold: fontdue::Font,
    cache: HashMap<(char, u16, bool), (fontdue::Metrics, Vec<u8>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Fonts {
    pub fn new() -> Self {
        let load = |b: &'static [u8]| fontdue::Font::from_bytes(b, fontdue::FontSettings::default()).expect("embedded font");
        Fonts { regular: load(dejavu::sans::regular()), bold: load(dejavu::sans::bold()), cache: HashMap::new() }
    }

    fn glyph(&mut self, c: char, size: f32, bold: bool) -> &(fontdue::Metrics, Vec<u8>) {
        let key = (c, (size * 4.0) as u16, bold);
        let font = if bold { &self.bold } else { &self.regular };
        self.cache.entry(key).or_insert_with(|| font.rasterize(c, size))
    }

    /// Width of `s` in pixels.
    pub fn measure(&mut self, s: &str, size: f32, bold: bool) -> f32 {
        s.chars().map(|c| self.glyph(c, size, bold).0.advance_width).sum()
    }

    /// Draw `s` with its baseline at `y`. Returns the width drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, pm: &mut Pixmap, s: &str, x: f32, y: f32, size: f32, bold: bool, color: Color, align: Align) -> f32 {
        let w = self.measure(s, size, bold);
        let mut pen = match align {
            Align::Left => x,
            Align::Center => x - w / 2.0,
            Align::Right => x - w,
        };
        let c = color.to_color_u8();
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        for ch in s.chars() {
            let (m, cov) = self.glyph(ch, size, bold).clone();
            let gx = (pen + m.xmin as f32).round() as i32;
            let gy = (y - m.height as f32 - m.ymin as f32).round() as i32;
            let pixels = pm.pixels_mut();
            for row in 0..m.height as i32 {
                let py = gy + row;
                if !(0..ph).contains(&py) {
                    continue;
                }
                for col in 0..m.width as i32 {
                    let px = gx + col;
                    if !(0..pw).contains(&px) {
                        continue;
                    }
                    let a = cov[(row * m.width as i32 + col) as usize] as u32 * c.alpha() as u32 / 255;
                    if a == 0 {
                        continue;
                    }
                    let d = &mut pixels[(py * pw + px) as usize];
                    let blend = |s: u8, d: u8| ((s as u32 * a + d as u32 * (255 - a)) / 255) as u8;
                    let out_a = (a + d.alpha() as u32 * (255 - a) / 255) as u8;
                    // Premultiplied: source channels scaled by coverage; keep r,g,b <= a.
                    let r = blend(c.red(), d.red()).min(out_a);
                    let g = blend(c.green(), d.green()).min(out_a);
                    let b = blend(c.blue(), d.blue()).min(out_a);
                    *d = PremultipliedColorU8::from_rgba(r, g, b, out_a).unwrap_or(*d);
                }
            }
            pen += m.advance_width;
        }
        w
    }

    /// Draw `s` shrunk if needed to fit `max_width`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_fit(&mut self, pm: &mut Pixmap, s: &str, x: f32, y: f32, size: f32, bold: bool, color: Color, align: Align, max_width: f32) {
        let w = self.measure(s, size, bold);
        let size = if w > max_width && w > 0.0 { (size * max_width / w).max(8.0) } else { size };
        self.draw(pm, s, x, y, size, bold, color, align);
    }
}

/// Whether (x, y) is inside `r`.
pub fn hit(r: &Rect, x: f32, y: f32) -> bool {
    x >= r.left() && x < r.right() && y >= r.top() && y < r.bottom()
}

pub fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

pub fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

pub fn fill_round_rect(pm: &mut Pixmap, r: Rect, radius: f32, color: Color) {
    if let Some(path) = round_rect(r.x(), r.y(), r.width(), r.height(), radius) {
        pm.fill_path(&path, &paint(color), FillRule::Winding, Transform::identity(), None);
    }
}

pub fn stroke_round_rect(pm: &mut Pixmap, r: Rect, radius: f32, width: f32, color: Color) {
    if let Some(path) = round_rect(r.x(), r.y(), r.width(), r.height(), radius) {
        let s = Stroke { width, ..Stroke::default() };
        pm.stroke_path(&path, &paint(color), &s, Transform::identity(), None);
    }
}

pub fn fill_circle(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, color: Color) {
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        pm.fill_path(&path, &paint(color), FillRule::Winding, Transform::identity(), None);
    }
}

pub fn stroke_circle(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, width: f32, color: Color) {
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        pm.stroke_path(&path, &paint(color), &Stroke { width, ..Stroke::default() }, Transform::identity(), None);
    }
}

pub fn line(pm: &mut Pixmap, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: Color) {
    let mut pb = PathBuilder::new();
    pb.move_to(x0, y0);
    pb.line_to(x1, y1);
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &paint(color), &Stroke { width, ..Stroke::default() }, Transform::identity(), None);
    }
}

pub fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &paint(color), Transform::identity(), None);
    }
}

/// A simple camera glyph centred on (cx, cy).
pub fn camera_icon(pm: &mut Pixmap, cx: f32, cy: f32, s: f32, color: Color) {
    if let Some(body) = Rect::from_xywh(cx - s * 0.5, cy - s * 0.3, s, s * 0.65) {
        fill_round_rect(pm, body, s * 0.1, color);
    }
    if let Some(top) = Rect::from_xywh(cx - s * 0.18, cy - s * 0.42, s * 0.36, s * 0.16) {
        fill_round_rect(pm, top, s * 0.05, color);
    }
    fill_circle(pm, cx, cy + s * 0.03, s * 0.2, panel());
    fill_circle(pm, cx, cy + s * 0.03, s * 0.12, color);
}

/// Convert premultiplied RGBA to the display's XRGB8888 (little-endian B, G, R, X),
/// rotated by `rotation` degrees clockwise (0, 90, 180, 270). `dst` is `dst_w × dst_h`
/// u32s with `stride` u32s per row; the source is `dst` rotated back.
pub fn to_xrgb(src: &Pixmap, rotation: u16, dst: &mut [u32], dst_w: usize, dst_h: usize, stride: usize) {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let px = src.pixels();
    let get = |x: usize, y: usize| {
        let p = px[y * sw + x];
        // Opaque in practice (the UI paints a full background), so no unpremultiply.
        (p.red() as u32) << 16 | (p.green() as u32) << 8 | p.blue() as u32
    };
    for y in 0..dst_h {
        let row = &mut dst[y * stride..y * stride + dst_w];
        for (x, d) in row.iter_mut().enumerate() {
            // Source pixel shown at display (x, y).
            let (u, v) = match rotation {
                90 => (y, sh - 1 - x),
                180 => (sw - 1 - x, sh - 1 - y),
                270 => (sw - 1 - y, x),
                _ => (x, y),
            };
            *d = if u < sw && v < sh { get(u, v) } else { 0 };
        }
    }
}

/// Map a display coordinate to the UI's (unrotated) coordinate.
pub fn display_to_ui(x: f32, y: f32, rotation: u16, ui_w: f32, ui_h: f32) -> (f32, f32) {
    match rotation {
        90 => (y, ui_h - x),
        180 => (ui_w - x, ui_h - y),
        270 => (ui_w - y, x),
        _ => (x, y),
    }
}

/// UI size for a display of `w × h` at `rotation`.
pub fn ui_size(w: u32, h: u32, rotation: u16) -> (u32, u32) {
    if rotation % 180 == 90 { (h, w) } else { (w, h) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_is_consistent() {
        // For each rotation, mark one UI pixel, rotate onto a 2×4 display, and map the
        // lit display pixel's centre back: it must land in the marked UI pixel.
        for rot in [0, 90, 180, 270] {
            let (w, h) = ui_size(2, 4, rot);
            for (u, v) in [(0, 0), (w - 1, 0), (0, h - 1), (1, 1)] {
                let mut pm = Pixmap::new(w, h).unwrap();
                pm.pixels_mut()[(v * w + u) as usize] = PremultipliedColorU8::from_rgba(255, 0, 0, 255).unwrap();
                let mut dst = vec![0u32; 8];
                to_xrgb(&pm, rot, &mut dst, 2, 4, 2);
                let pos = dst.iter().position(|&p| p == 0xff0000).unwrap();
                let (x, y) = display_to_ui((pos % 2) as f32 + 0.5, (pos / 2) as f32 + 0.5, rot, w as f32, h as f32);
                assert_eq!((x.floor() as u32, y.floor() as u32), (u, v), "rotation {rot}");
            }
        }
    }
}
