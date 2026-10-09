//! Mapping regions between the displayed (developed, oriented) image and raw frame
//! pixels.

use loupecam_isp::stats::Rect;
use loupecam_isp::{Orientation, Rotation};
use serde::Deserialize;

/// A rectangle in normalised display coordinates (0..1, origin top-left).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Map a display-space point back to normalised frame coordinates.
fn unorient(o: Orientation, (u, v): (f32, f32)) -> (f32, f32) {
    let (u, v) = match o.rotation {
        Rotation::None => (u, v),
        Rotation::Cw90 => (v, 1.0 - u),
        Rotation::Cw180 => (1.0 - u, 1.0 - v),
        Rotation::Cw270 => (1.0 - v, u),
    };
    (if o.flip_h { 1.0 - u } else { u }, if o.flip_v { 1.0 - v } else { v })
}

/// Convert a display rectangle to frame pixels for a `width × height` frame developed
/// with orientation `o`.
pub fn display_to_frame(r: NormRect, o: Orientation, width: u32, height: u32) -> Rect {
    let a = unorient(o, (r.x, r.y));
    let b = unorient(o, (r.x + r.width, r.y + r.height));
    let (u0, u1) = (a.0.min(b.0).clamp(0.0, 1.0), a.0.max(b.0).clamp(0.0, 1.0));
    let (v0, v1) = (a.1.min(b.1).clamp(0.0, 1.0), a.1.max(b.1).clamp(0.0, 1.0));
    let (x, y) = ((u0 * width as f32) as u32, (v0 * height as f32) as u32);
    Rect {
        x,
        y,
        width: ((u1 * width as f32) as u32).saturating_sub(x).max(2),
        height: ((v1 * height as f32) as u32).saturating_sub(y).max(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotations_invert() {
        let r = NormRect { x: 0.1, y: 0.2, width: 0.3, height: 0.1 };
        let none = Orientation::default();
        assert_eq!(display_to_frame(r, none, 1000, 1000), Rect { x: 100, y: 200, width: 300, height: 100 });
        // Displayed rotated 90° CW: the frame's top-left is now top-right.
        let cw = Orientation { rotation: Rotation::Cw90, ..none };
        let top_right = NormRect { x: 0.9, y: 0.0, width: 0.1, height: 0.1 };
        assert_eq!(display_to_frame(top_right, cw, 1000, 1000), Rect { x: 0, y: 0, width: 100, height: 100 });
        let fv = Orientation { flip_v: true, ..none };
        assert_eq!(display_to_frame(r, fv, 1000, 1000), Rect { x: 100, y: 700, width: 300, height: 100 });
    }
}
