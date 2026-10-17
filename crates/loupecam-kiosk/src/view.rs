//! Pan and zoom of an image (camera frame or cast) inside a viewport.

use tiny_skia::Rect;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomView {
    /// Screen pixels per image pixel; `None` = fit the viewport.
    pub zoom: Option<f32>,
    /// Image point at the viewport centre (when zoomed).
    pub cx: f32,
    pub cy: f32,
}

impl Default for ZoomView {
    fn default() -> Self {
        ZoomView { zoom: None, cx: 0.0, cy: 0.0 }
    }
}

const MAX_ZOOM: f32 = 32.0;

impl ZoomView {
    pub fn fit_zoom(content: (f32, f32), vp: Rect) -> f32 {
        (vp.width() / content.0).min(vp.height() / content.1)
    }

    /// Current (zoom, centre x, centre y).
    pub fn resolve(&self, content: (f32, f32), vp: Rect) -> (f32, f32, f32) {
        match self.zoom {
            None => (Self::fit_zoom(content, vp), content.0 / 2.0, content.1 / 2.0),
            Some(z) => (z, self.cx, self.cy),
        }
    }

    /// Image → screen: `screen = image * scale + offset`.
    pub fn transform(&self, content: (f32, f32), vp: Rect) -> (f32, f32, f32) {
        let (z, cx, cy) = self.resolve(content, vp);
        let (mx, my) = (vp.x() + vp.width() / 2.0, vp.y() + vp.height() / 2.0);
        (z, mx - cx * z, my - cy * z)
    }

    pub fn image_point(&self, content: (f32, f32), vp: Rect, sx: f32, sy: f32) -> (f32, f32) {
        let (z, ox, oy) = self.transform(content, vp);
        ((sx - ox) / z, (sy - oy) / z)
    }

    pub fn fit(&mut self) {
        *self = ZoomView::default();
    }

    /// Zoom to `zoom`, keeping image point `at` under screen point `screen`.
    pub fn zoom_at(&mut self, content: (f32, f32), vp: Rect, zoom: f32, at: (f32, f32), screen: (f32, f32)) {
        let min = Self::fit_zoom(content, vp) / 2.0;
        let z = zoom.clamp(min, MAX_ZOOM);
        let (mx, my) = (vp.x() + vp.width() / 2.0, vp.y() + vp.height() / 2.0);
        *self = ZoomView { zoom: Some(z), cx: at.0 - (screen.0 - mx) / z, cy: at.1 - (screen.1 - my) / z };
        self.clamp(content);
    }

    /// Move by a screen-pixel delta.
    pub fn pan(&mut self, content: (f32, f32), vp: Rect, dx: f32, dy: f32) {
        let (z, cx, cy) = self.resolve(content, vp);
        *self = ZoomView { zoom: Some(z), cx: cx - dx / z, cy: cy - dy / z };
        self.clamp(content);
    }

    /// Keep the centre on the image.
    fn clamp(&mut self, content: (f32, f32)) {
        self.cx = self.cx.clamp(0.0, content.0);
        self.cy = self.cy.clamp(0.0, content.1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_point_under_finger() {
        let vp = Rect::from_xywh(0.0, 0.0, 640.0, 480.0).unwrap();
        let content = (1228.0, 922.0);
        let mut v = ZoomView::default();
        let at = v.image_point(content, vp, 100.0, 50.0);
        v.zoom_at(content, vp, 2.0, at, (100.0, 50.0));
        let back = v.image_point(content, vp, 100.0, 50.0);
        assert!((back.0 - at.0).abs() < 1e-3 && (back.1 - at.1).abs() < 1e-3);
        // Panning right by 10 screen px moves the image centre left by 10 / zoom.
        let before = v.cx;
        v.pan(content, vp, 10.0, 0.0);
        assert!((before - v.cx - 5.0).abs() < 1e-3);
    }
}
