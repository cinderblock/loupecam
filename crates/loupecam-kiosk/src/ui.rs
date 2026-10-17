//! The kiosk's screen: the camera view with pan/zoom, a side panel of big touch
//! buttons, a settings sheet, and natively rendered casts (images, SVG).
//!
//! Immediate mode: [`App::draw`] paints everything and records each control's hit
//! rectangle; touches are routed against the rectangles from the last draw.

use crate::client::{CamState, CastInfo, Client, Command, RecordStatus, ServerEvent, Stats};
use crate::gfx::{self, Align, Fonts};
use crate::view::ZoomView;
use serde_json::json;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tiny_skia::{Color, FilterQuality, Pixmap, PixmapPaint, Rect, Transform};

/// Touch (or mouse) input in UI coordinates.
#[derive(Debug, Clone, Copy)]
pub enum Input {
    Down { id: u64, x: f32, y: f32 },
    Move { id: u64, x: f32, y: f32 },
    Up { id: u64 },
    Cancel { id: u64 },
    /// Mouse wheel (development): `delta` > 0 zooms in.
    Wheel { x: f32, y: f32, delta: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    Snap,
    Record,
    CyclePreset,
    ToggleAe,
    WhiteBalance,
    ToggleSettings,
    Fit,
    Grid,
    Crosshair,
    Preset(&'static str),
    StopCast,
    Slider(SliderKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SliderKind {
    Exposure,
    Gain,
}

const PRESETS: [(&str, &str); 3] = [("smooth", "Smooth"), ("detailed", "Detailed"), ("timelapse", "Timelapse")];
const EXPOSURE_RANGE: (f32, f32) = (50.0, 2_000_000.0);

/// A finger on the screen, and what it's doing.
#[derive(Debug, Clone, Copy)]
enum Grab {
    /// Pressed on a control; fires if released inside it.
    Control(Action, Rect),
    Slider(SliderKind, Rect),
    /// On the image: pans (and pinches with a second finger).
    View,
}

struct Pinch {
    dist: f32,
    zoom: f32,
    at: (f32, f32),
}

pub struct App {
    client: Client,
    fonts: Fonts,
    width: u32,
    height: u32,
    connected: bool,
    auth_failed: bool,
    state: Option<CamState>,
    stats: Stats,
    record: RecordStatus,
    cast: Option<CastInfo>,
    /// The current cast, rendered (when the kiosk can show it natively).
    cast_image: Option<Pixmap>,
    frame: Option<Pixmap>,
    view: ZoomView,
    cast_view: ZoomView,
    grid: bool,
    crosshair: bool,
    settings_open: bool,
    toast: Option<(String, Instant)>,
    flash: Option<Instant>,
    hits: Vec<(Rect, Action)>,
    pointers: HashMap<u64, ((f32, f32), Grab)>,
    pinch: Option<Pinch>,
    last_tap: Option<(Instant, f32, f32)>,
    /// Slider values while dragging (shown instead of the server's), and when the
    /// last patch was sent.
    sliding: HashMap<SliderKind, f32>,
    last_patch: Instant,
    last_input: Instant,
    address: String,
    dirty: bool,
}

impl App {
    pub fn new(client: Client, width: u32, height: u32, address: String) -> App {
        App {
            client,
            fonts: Fonts::new(),
            width,
            height,
            connected: false,
            auth_failed: false,
            state: None,
            stats: Stats::default(),
            record: RecordStatus::default(),
            cast: None,
            cast_image: None,
            frame: None,
            view: ZoomView::default(),
            cast_view: ZoomView::default(),
            grid: false,
            crosshair: false,
            settings_open: false,
            toast: None,
            flash: None,
            hits: Vec::new(),
            pointers: HashMap::new(),
            pinch: None,
            last_tap: None,
            sliding: HashMap::new(),
            last_patch: Instant::now(),
            last_input: Instant::now(),
            address,
            dirty: true,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        (self.width, self.height) = (width, height);
        self.dirty = true;
    }

    /// How long since the screen was last touched.
    pub fn idle(&self) -> Duration {
        self.last_input.elapsed()
    }

    /// The cast the kiosk can't render itself (it needs a browser), if any.
    pub fn browser_cast(&self) -> Option<&CastInfo> {
        self.cast.as_ref().filter(|c| !native(c))
    }

    pub fn stop_casting(&self) {
        self.client.send(Command::StopCasting);
    }

    pub fn needs_redraw(&self) -> bool {
        self.dirty
    }

    /// When to wake for time-driven changes (recording timer, toasts) without input.
    pub fn next_wake(&self) -> Option<Instant> {
        let mut t: Option<Instant> = None;
        let mut at = |i: Instant| t = Some(t.map_or(i, |t| t.min(i)));
        if self.record.active.is_some() {
            at(Instant::now() + Duration::from_millis(500));
        }
        if let Some((_, since)) = &self.toast {
            at(*since + Duration::from_secs(4));
        }
        if let Some(f) = self.flash {
            at(f + Duration::from_millis(16));
        }
        t
    }

    pub fn on_server(&mut self, ev: ServerEvent) {
        match ev {
            ServerEvent::Connected(c) => {
                self.connected = c;
                if c {
                    self.auth_failed = false;
                }
            }
            ServerEvent::AuthFailed => self.auth_failed = true,
            ServerEvent::State(s) => self.state = Some(s),
            ServerEvent::Stats(s) => self.stats = s,
            ServerEvent::Record(r) => {
                if let (Some(_), None) = (&self.record.active, &r.active)
                    && let Some(e) = &r.error
                {
                    self.toast(format!("Recording stopped: {e}"));
                }
                self.record = r;
            }
            ServerEvent::Cast(c) => {
                if c.as_ref().map(|c| c.id) != self.cast.as_ref().map(|c| c.id) {
                    self.cast_image = None;
                    self.cast_view.fit();
                    if let Some(c) = c.as_ref().filter(|c| native(c)) {
                        self.client.send(Command::FetchCast(c.id));
                    }
                }
                self.cast = c;
            }
            ServerEvent::CastContent { id, bytes } => {
                if self.cast.as_ref().is_some_and(|c| c.id == id) {
                    match render_cast(&self.cast.clone().unwrap(), &bytes) {
                        Ok(image) => self.cast_image = Some(image),
                        Err(e) => self.toast(format!("Can't show the cast: {e}")),
                    }
                }
            }
            ServerEvent::Frame => {
                if let Some(f) = self.client.take_frame() {
                    self.frame = Some(f.pixmap);
                }
            }
            ServerEvent::Message(m) => self.toast(m),
        }
        self.dirty = true;
    }

    fn toast(&mut self, m: String) {
        self.toast = Some((m, Instant::now()));
        self.dirty = true;
    }

    /// Advance time-driven state; call before deciding whether to draw.
    pub fn tick(&mut self) {
        if self.toast.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(4)) {
            self.toast = None;
            self.dirty = true;
        }
        if self.flash.is_some_and(|f| f.elapsed() > Duration::from_millis(250)) {
            self.flash = None;
        }
        if self.flash.is_some() || self.record.active.is_some() {
            self.dirty = true;
        }
    }

    // ---- Layout ----

    /// (image area, side panel): the panel goes on the right in landscape, at the
    /// bottom in portrait.
    fn layout(&self) -> (Rect, Rect) {
        let (w, h) = (self.width as f32, self.height as f32);
        const SIDE: f32 = 160.0;
        if w >= h {
            (Rect::from_xywh(0.0, 0.0, w - SIDE, h).unwrap(), Rect::from_xywh(w - SIDE, 0.0, SIDE, h).unwrap())
        } else {
            (Rect::from_xywh(0.0, 0.0, w, h - SIDE).unwrap(), Rect::from_xywh(0.0, h - SIDE, w, SIDE).unwrap())
        }
    }

    fn showing_cast(&self) -> bool {
        self.cast.as_ref().is_some_and(native)
    }

    /// What the view shows: (content size) for pan/zoom.
    fn content(&self) -> Option<(f32, f32)> {
        let img = if self.showing_cast() { self.cast_image.as_ref() } else { self.frame.as_ref() };
        img.map(|p| (p.width() as f32, p.height() as f32))
    }

    fn view_mut(&mut self) -> &mut ZoomView {
        if self.showing_cast() { &mut self.cast_view } else { &mut self.view }
    }

    // ---- Input ----

    pub fn on_input(&mut self, input: Input) {
        self.last_input = Instant::now();
        self.dirty = true;
        let (vp, _) = self.layout();
        match input {
            Input::Down { id, x, y } => {
                let grab = match self.hits.iter().rev().find(|(r, _)| gfx::hit(r, x, y)) {
                    Some(&(r, Action::Slider(k))) => {
                        self.slide(k, r, x);
                        Grab::Slider(k, r)
                    }
                    Some(&(r, a)) => Grab::Control(a, r),
                    None if gfx::hit(&vp, x, y) => Grab::View,
                    None => return,
                };
                self.pointers.insert(id, ((x, y), grab));
                if matches!(grab, Grab::View) {
                    self.view_down(x, y);
                }
            }
            Input::Move { id, x, y } => {
                let Some(&(prev, grab)) = self.pointers.get(&id) else { return };
                self.pointers.insert(id, ((x, y), grab));
                match grab {
                    Grab::Slider(k, r) => self.slide(k, r, x),
                    Grab::View => self.view_move(prev, (x, y)),
                    Grab::Control(..) => {}
                }
            }
            Input::Up { id } => {
                let Some(((x, y), grab)) = self.pointers.remove(&id) else { return };
                match grab {
                    Grab::Control(a, r) if gfx::hit(&r, x, y) => self.act(a),
                    Grab::Slider(k, _) => {
                        if let Some(v) = self.sliding.remove(&k) {
                            self.send_slider(k, v);
                        }
                    }
                    _ => {}
                }
                self.view_up();
            }
            Input::Cancel { id } => {
                self.pointers.remove(&id);
                self.view_up();
            }
            Input::Wheel { x, y, delta } => {
                if let Some(content) = self.content() {
                    let v = *self.view_mut();
                    let (z, _, _) = v.resolve(content, vp);
                    let at = v.image_point(content, vp, x, y);
                    self.view_mut().zoom_at(content, vp, z * 1.0015f32.powf(delta), at, (x, y));
                }
            }
        }
    }

    fn view_fingers(&self) -> Vec<(f32, f32)> {
        self.pointers.values().filter(|(_, g)| matches!(g, Grab::View)).map(|(p, _)| *p).collect()
    }

    fn view_down(&mut self, x: f32, y: f32) {
        let (vp, _) = self.layout();
        let fingers = self.view_fingers();
        if fingers.len() == 2 {
            let Some(content) = self.content() else { return };
            let (a, b) = (fingers[0], fingers[1]);
            let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
            let v = *self.view_mut();
            self.pinch = Some(Pinch {
                dist: ((a.0 - b.0).hypot(a.1 - b.1)).max(1.0),
                zoom: v.resolve(content, vp).0,
                at: v.image_point(content, vp, mid.0, mid.1),
            });
            return;
        }
        // Double tap: fit.
        let now = Instant::now();
        if let Some((t, tx, ty)) = self.last_tap
            && now - t < Duration::from_millis(300)
            && (x - tx).hypot(y - ty) < 40.0
        {
            self.view_mut().fit();
            self.last_tap = None;
            return;
        }
        self.last_tap = Some((now, x, y));
    }

    fn view_move(&mut self, prev: (f32, f32), cur: (f32, f32)) {
        let (vp, _) = self.layout();
        let Some(content) = self.content() else { return };
        let fingers = self.view_fingers();
        if let (Some(p), 2) = (&self.pinch, fingers.len()) {
            let (a, b) = (fingers[0], fingers[1]);
            let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
            let zoom = p.zoom * (a.0 - b.0).hypot(a.1 - b.1) / p.dist;
            let at = p.at;
            self.view_mut().zoom_at(content, vp, zoom, at, mid);
        } else if fingers.len() == 1 {
            self.view_mut().pan(content, vp, cur.0 - prev.0, cur.1 - prev.1);
        }
    }

    fn view_up(&mut self) {
        if self.view_fingers().len() < 2 {
            self.pinch = None;
        }
    }

    fn slide(&mut self, k: SliderKind, r: Rect, x: f32) {
        let t = ((x - r.x()) / r.width()).clamp(0.0, 1.0);
        let (lo, hi) = self.slider_range(k);
        let v = lo * (hi / lo).powf(t);
        self.sliding.insert(k, v);
        if self.last_patch.elapsed() > Duration::from_millis(120) {
            self.send_slider(k, v);
        }
    }

    fn send_slider(&mut self, k: SliderKind, v: f32) {
        let patch = match k {
            SliderKind::Exposure => json!({ "exposureUs": v.round() as u32 }),
            SliderKind::Gain => json!({ "gain": v }),
        };
        self.client.send(Command::Patch(patch));
        self.last_patch = Instant::now();
    }

    fn slider_range(&self, k: SliderKind) -> (f32, f32) {
        match k {
            SliderKind::Exposure => EXPOSURE_RANGE,
            SliderKind::Gain => (1.0, self.state.as_ref().and_then(|s| s.device.as_ref()).map_or(16.0, |d| d.max_gain.max(1.5))),
        }
    }

    fn act(&mut self, a: Action) {
        let settings = self.state.as_ref().map(|s| &s.settings);
        match a {
            Action::Snap => {
                self.flash = Some(Instant::now());
                self.client.send(Command::Capture);
            }
            Action::Record => {
                if self.record.active.is_some() {
                    self.client.send(Command::StopRecording);
                } else {
                    self.client.send(Command::StartRecording(None));
                }
            }
            Action::CyclePreset => {
                let cur = settings.map_or("smooth", |s| s.record.preset.as_str());
                let i = PRESETS.iter().position(|(p, _)| *p == cur).map_or(0, |i| (i + 1) % PRESETS.len());
                self.act(Action::Preset(PRESETS[i].0));
            }
            Action::Preset(p) => {
                self.client.send(Command::ApplyPreset(p.to_string()));
                if let Some(s) = self.state.as_mut() {
                    s.settings.record.preset = p.to_string();
                }
                self.toast(format!("Record mode: {}", PRESETS.iter().find(|(k, _)| *k == p).map_or(p, |(_, l)| l)));
            }
            Action::ToggleAe => {
                let on = !settings.is_some_and(|s| s.auto_exposure);
                self.client.send(Command::Patch(json!({ "autoExposure": on })));
            }
            Action::WhiteBalance => self.client.send(Command::WhiteBalance),
            Action::ToggleSettings => self.settings_open = !self.settings_open,
            Action::Fit => self.view_mut().fit(),
            Action::Grid => self.grid = !self.grid,
            Action::Crosshair => self.crosshair = !self.crosshair,
            Action::StopCast => self.client.send(Command::StopCasting),
            Action::Slider(_) => {}
        }
    }

    // ---- Drawing ----

    pub fn draw(&mut self, pm: &mut Pixmap) {
        self.dirty = false;
        self.hits.clear();
        pm.fill(Color::BLACK);
        let (vp, side) = self.layout();
        self.draw_view(pm, vp);
        self.draw_side(pm, side);
        if self.settings_open {
            self.draw_settings(pm, vp);
        }
        if let Some((m, _)) = &self.toast {
            let m = m.clone();
            let w = (self.fonts.measure(&m, 16.0, false) + 32.0).min(vp.width() - 16.0);
            let r = Rect::from_xywh(vp.x() + (vp.width() - w) / 2.0, vp.bottom() - 56.0, w, 40.0).unwrap();
            gfx::fill_round_rect(pm, r, 10.0, gfx::rgba(20, 22, 25, 230));
            self.fonts.draw_fit(pm, &m, r.x() + w / 2.0, r.y() + 26.0, 16.0, false, gfx::text(), Align::Center, w - 24.0);
        }
        if let Some(f) = self.flash {
            let a = (1.0 - f.elapsed().as_secs_f32() / 0.25).clamp(0.0, 1.0);
            gfx::fill_rect(pm, vp.x(), vp.y(), vp.width(), vp.height(), Color::from_rgba(1.0, 1.0, 1.0, 0.6 * a).unwrap());
        }
    }

    fn draw_view(&mut self, pm: &mut Pixmap, vp: Rect) {
        let (image, view) = if self.showing_cast() {
            (self.cast_image.as_ref(), self.cast_view)
        } else {
            (self.frame.as_ref(), self.view)
        };
        if let Some(img) = image {
            let content = (img.width() as f32, img.height() as f32);
            let (z, ox, oy) = view.transform(content, vp);
            let quality = if z >= 2.0 { FilterQuality::Nearest } else { FilterQuality::Bilinear };
            let paint = PixmapPaint { quality, ..PixmapPaint::default() };
            pm.draw_pixmap(0, 0, img.as_ref(), &paint, Transform::from_row(z, 0.0, 0.0, z, ox, oy), None);
            if !self.showing_cast() {
                self.draw_overlays(pm, (content.0 * z, content.1 * z), (ox, oy));
            }
        }
        // What's going on, when there's nothing (or something odd) to show.
        let message = if self.auth_failed {
            Some("The server rejected the token (set LOUPECAM_TOKEN)".to_string())
        } else if !self.connected {
            Some("Connecting to the camera server…".to_string())
        } else if let Some(c) = self.cast.as_ref().filter(|c| !native(c)) {
            Some(format!("Showing {} in the browser…", cast_label(c)))
        } else if self.showing_cast() && self.cast_image.is_none() {
            Some("Loading the cast…".to_string())
        } else {
            match self.state.as_ref().map(|s| (&s.status.state, &s.status.detail)) {
                Some((s, d)) if s == "searching" => Some(format!("Looking for the camera… {}", d.as_deref().unwrap_or(""))),
                Some((s, d)) if s == "busy" => Some(d.clone().unwrap_or_default()),
                _ => None,
            }
        };
        if let Some(m) = message {
            let y = vp.y() + vp.height() / 2.0;
            gfx::fill_rect(pm, vp.x(), y - 30.0, vp.width(), 44.0, gfx::rgba(0, 0, 0, 160));
            self.fonts.draw_fit(pm, m.trim(), vp.x() + vp.width() / 2.0, y, 18.0, false, gfx::text(), Align::Center, vp.width() - 24.0);
        }
        if let Some(c) = self.cast.clone().filter(native) {
            // A chip to go back to the camera.
            let label = format!("{}   ✕ Camera", cast_label(&c));
            let w = (self.fonts.measure(&label, 16.0, false) + 32.0).min(vp.width() - 16.0);
            let r = Rect::from_xywh(vp.x() + (vp.width() - w) / 2.0, vp.y() + 10.0, w, 40.0).unwrap();
            gfx::fill_round_rect(pm, r, 20.0, gfx::rgba(20, 22, 25, 220));
            self.fonts.draw_fit(pm, &label, r.x() + w / 2.0, r.y() + 26.0, 16.0, false, gfx::text(), Align::Center, w - 24.0);
            self.hits.push((r, Action::StopCast));
        }
    }

    fn draw_overlays(&mut self, pm: &mut Pixmap, size: (f32, f32), origin: (f32, f32)) {
        let (w, h) = size;
        let (ox, oy) = origin;
        if self.grid {
            let c = gfx::rgba(255, 255, 255, 90);
            for i in 1..3 {
                let f = i as f32 / 3.0;
                gfx::line(pm, ox + w * f, oy, ox + w * f, oy + h, 1.0, c);
                gfx::line(pm, ox, oy + h * f, ox + w, oy + h * f, 1.0, c);
            }
        }
        if self.crosshair {
            let (mx, my) = (ox + w / 2.0, oy + h / 2.0);
            let c = gfx::rgba(255, 60, 60, 230);
            gfx::line(pm, mx - 30.0, my, mx + 30.0, my, 1.5, c);
            gfx::line(pm, mx, my - 30.0, mx, my + 30.0, 1.5, c);
            gfx::stroke_circle(pm, mx, my, 12.0, 1.5, c);
        }
    }

    fn button(&mut self, pm: &mut Pixmap, r: Rect, label: &str, action: Action, active: bool, enabled: bool) {
        let pressed = self.pointers.values().any(|(p, g)| matches!(g, Grab::Control(a, _) if *a == action) && gfx::hit(&r, p.0, p.1));
        let bg = if pressed {
            gfx::rgba(70, 78, 88, 255)
        } else if active {
            gfx::button_active()
        } else {
            gfx::button()
        };
        gfx::fill_round_rect(pm, r, 10.0, bg);
        if active {
            gfx::stroke_round_rect(pm, r, 10.0, 1.5, gfx::accent());
        }
        let color = if enabled { gfx::text() } else { gfx::muted() };
        let size = if r.height() >= 60.0 { 18.0 } else { 15.0 };
        self.fonts.draw_fit(pm, label, r.x() + r.width() / 2.0, r.y() + r.height() / 2.0 + size * 0.35, size, true, color, Align::Center, r.width() - 12.0);
        if enabled {
            self.hits.push((r, action));
        }
    }

    fn draw_side(&mut self, pm: &mut Pixmap, side: Rect) {
        gfx::fill_rect(pm, side.x(), side.y(), side.width(), side.height(), gfx::panel());
        let streaming = self.connected && self.state.as_ref().is_some_and(|s| s.status.state == "streaming");
        let settings = self.state.as_ref().map(|s| s.settings.clone()).unwrap_or_default();
        let recording = self.record.active.clone();
        // Controls in a column (landscape) or a row of two columns (portrait).
        let pad = 8.0;
        let (x, y, w) = (side.x() + pad, side.y() + pad, side.width() - 2.0 * pad);
        let portrait = side.width() > side.height();
        let col_w = if portrait { (w - pad) / 3.0 } else { w };
        let cell = |i: usize, row_y: f32, h: f32| -> Rect {
            if portrait {
                Rect::from_xywh(x + (col_w + pad) * (i % 3) as f32, row_y, col_w, h).unwrap()
            } else {
                Rect::from_xywh(x, row_y, w, h).unwrap()
            }
        };

        // Status.
        let status = match self.state.as_ref().map(|s| s.status.state.as_str()) {
            _ if !self.connected => "Not connected".to_string(),
            Some("streaming") => format!("{:.0} fps · {} · {:.1}×", self.stats.fps, exposure_label(self.stats.exposure_us), self.stats.gain),
            Some("busy") => "Working…".to_string(),
            _ => "No camera".to_string(),
        };
        self.fonts.draw_fit(pm, &status, x, y + 14.0, 13.0, false, gfx::muted(), Align::Left, w);
        let top = y + 26.0;

        // Snap.
        let snap = cell(0, top, if portrait { 56.0 } else { 80.0 });
        self.button(pm, snap, "", Action::Snap, false, streaming);
        gfx::camera_icon(pm, snap.x() + snap.width() / 2.0, snap.y() + snap.height() / 2.0 - 8.0, 30.0, if streaming { gfx::text() } else { gfx::muted() });
        self.fonts.draw(pm, "Snap", snap.x() + snap.width() / 2.0, snap.bottom() - 10.0, 14.0, true, gfx::text(), Align::Center);

        // Record / stop.
        let rec = cell(1, if portrait { top } else { snap.bottom() + pad }, snap.height());
        let label = match &recording {
            Some(r) if r.mode == "timelapse" => format!("{} · {}", duration_label(r.seconds), r.frames),
            Some(r) => duration_label(r.seconds),
            None => String::new(),
        };
        self.button(pm, rec, "", Action::Record, recording.is_some(), streaming || recording.is_some());
        let (cx, cy) = (rec.x() + rec.width() / 2.0, rec.y() + rec.height() / 2.0 - 8.0);
        if recording.is_some() {
            gfx::fill_rect(pm, cx - 11.0, cy - 11.0, 22.0, 22.0, gfx::red());
            self.fonts.draw(pm, &label, cx, rec.bottom() - 10.0, 14.0, true, gfx::text(), Align::Center);
        } else {
            gfx::fill_circle(pm, cx, cy, 13.0, gfx::red());
            self.fonts.draw(pm, "Record", cx, rec.bottom() - 10.0, 14.0, true, gfx::text(), Align::Center);
        }

        // Record mode.
        let preset = PRESETS.iter().find(|(k, _)| *k == settings.record.preset).map_or("Custom", |(_, l)| *l);
        let mode = cell(2, if portrait { top } else { rec.bottom() + pad }, if portrait { snap.height() } else { 36.0 });
        self.button(pm, mode, &format!("{preset} ▸"), Action::CyclePreset, false, recording.is_none());

        // AE / WB.
        let row = if portrait { snap.bottom() + pad } else { mode.bottom() + pad };
        let half = (w - pad) / 2.0;
        let (ae, wb) = if portrait {
            (Rect::from_xywh(x, row, col_w, 44.0).unwrap(), Rect::from_xywh(x + col_w + pad, row, col_w, 44.0).unwrap())
        } else {
            (Rect::from_xywh(x, row, half, 44.0).unwrap(), Rect::from_xywh(x + half + pad, row, half, 44.0).unwrap())
        };
        self.button(pm, ae, "Auto exp", Action::ToggleAe, settings.auto_exposure, streaming);
        self.button(pm, wb, "WB", Action::WhiteBalance, false, streaming);
        let more = if portrait {
            Rect::from_xywh(x + 2.0 * (col_w + pad), row, col_w, 44.0).unwrap()
        } else {
            Rect::from_xywh(x, row + 44.0 + pad, w, 44.0).unwrap()
        };
        self.button(pm, more, "Settings", Action::ToggleSettings, self.settings_open, true);

        // Where to find this camera from elsewhere (landscape only: no room otherwise).
        if !portrait {
            let addr = self.address.clone();
            self.fonts.draw_fit(pm, &addr, x, side.bottom() - 12.0, 12.0, false, gfx::muted(), Align::Left, w);
        }
    }

    fn draw_settings(&mut self, pm: &mut Pixmap, vp: Rect) {
        let r = Rect::from_xywh(vp.x() + 16.0, vp.y() + 16.0, vp.width() - 32.0, vp.height() - 32.0).unwrap();
        gfx::fill_round_rect(pm, r, 14.0, gfx::rgba(24, 27, 31, 235));
        let settings = self.state.as_ref().map(|s| s.settings.clone()).unwrap_or_default();
        let (x, w) = (r.x() + 16.0, r.width() - 32.0);
        let mut y = r.y() + 16.0;
        let row_h = 48.0;

        let exposure = self.sliding.get(&SliderKind::Exposure).copied().unwrap_or(if settings.auto_exposure && self.stats.exposure_us > 0.0 {
            self.stats.exposure_us as f32
        } else {
            settings.exposure_us as f32
        });
        let gain = self.sliding.get(&SliderKind::Gain).copied().unwrap_or(settings.gain);
        let ae = settings.auto_exposure;
        self.slider(pm, x, y, w, "Exposure", &exposure_label(exposure as f64), SliderKind::Exposure, exposure, !ae);
        y += row_h + 8.0;
        self.slider(pm, x, y, w, "Gain", &format!("{gain:.2}×"), SliderKind::Gain, gain, !ae);
        y += row_h + 8.0;
        if ae {
            self.fonts.draw(pm, "Auto exposure is on: turn it off to set these.", x, y + 4.0, 13.0, false, gfx::muted(), Align::Left);
            y += 20.0;
        }

        // Record mode.
        self.fonts.draw(pm, "Record", x, y + 26.0, 15.0, true, gfx::text(), Align::Left);
        let seg_x = x + 90.0;
        let seg_w = (w - 90.0) / 3.0;
        let recording = self.record.active.is_some();
        for (i, (key, label)) in PRESETS.iter().enumerate() {
            let b = Rect::from_xywh(seg_x + seg_w * i as f32, y, seg_w - 4.0, 40.0).unwrap();
            self.button(pm, b, label, Action::Preset(key), settings.record.preset == *key, !recording);
        }
        y += 52.0;

        // Overlays and view.
        let bw = (w - 3.0 * 8.0) / 4.0;
        let row = [("Grid", Action::Grid, self.grid), ("Crosshair", Action::Crosshair, self.crosshair), ("Fit view", Action::Fit, false), ("Close", Action::ToggleSettings, false)];
        for (i, (label, a, on)) in row.into_iter().enumerate() {
            let b = Rect::from_xywh(x + (bw + 8.0) * i as f32, y, bw, 44.0).unwrap();
            self.button(pm, b, label, a, on, true);
        }
        y += 56.0;
        if let Some(e) = self.record.encoders.is_empty().then_some("This server has no H.264 encoder: only timelapse stills can be recorded.") {
            self.fonts.draw_fit(pm, e, x, y + 10.0, 13.0, false, gfx::muted(), Align::Left, w);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn slider(&mut self, pm: &mut Pixmap, x: f32, y: f32, w: f32, label: &str, value: &str, k: SliderKind, v: f32, enabled: bool) {
        let color = if enabled { gfx::text() } else { gfx::muted() };
        self.fonts.draw(pm, label, x, y + 18.0, 15.0, true, color, Align::Left);
        self.fonts.draw(pm, value, x + w, y + 18.0, 15.0, false, color, Align::Right);
        let track = Rect::from_xywh(x, y + 26.0, w, 22.0).unwrap();
        let (lo, hi) = self.slider_range(k);
        let t = ((v.max(lo) / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0);
        gfx::fill_round_rect(pm, Rect::from_xywh(x, track.y() + 8.0, w, 6.0).unwrap(), 3.0, gfx::button());
        gfx::fill_round_rect(pm, Rect::from_xywh(x, track.y() + 8.0, w * t, 6.0).unwrap(), 3.0, if enabled { gfx::accent() } else { gfx::muted() });
        gfx::fill_circle(pm, x + w * t, track.y() + 11.0, 11.0, color);
        if enabled {
            // A taller hit area than the drawn track, for fingers.
            self.hits.push((Rect::from_xywh(x - 10.0, y + 14.0, w + 20.0, 44.0).unwrap(), Action::Slider(k)));
        }
    }
}

/// Casts the kiosk renders itself.
fn native(c: &CastInfo) -> bool {
    matches!(c.mime.as_str(), "image/png" | "image/jpeg" | "image/svg+xml")
}

fn cast_label(c: &CastInfo) -> String {
    c.name.clone().or_else(|| c.url.clone()).unwrap_or_else(|| format!("a {}", c.kind))
}

/// Largest side of a rendered SVG cast.
const SVG_PIXELS: f32 = 3000.0;

fn render_cast(c: &CastInfo, bytes: &[u8]) -> anyhow::Result<Pixmap> {
    match c.mime.as_str() {
        "image/png" => Ok(Pixmap::decode_png(bytes)?),
        "image/jpeg" => crate::client::decode_jpeg(bytes),
        "image/svg+xml" => {
            let mut opt = resvg::usvg::Options::default();
            let db = opt.fontdb_mut();
            db.load_system_fonts();
            db.load_font_data(dejavu::sans::regular().to_vec());
            db.load_font_data(dejavu::sans::bold().to_vec());
            db.set_sans_serif_family("DejaVu Sans");
            let tree = resvg::usvg::Tree::from_data(bytes, &opt)?;
            let size = tree.size();
            // Render big enough to zoom into.
            let scale = (SVG_PIXELS / size.width().max(size.height())).max(1.0);
            let (w, h) = ((size.width() * scale).ceil() as u32, (size.height() * scale).ceil() as u32);
            let mut pm = Pixmap::new(w.max(1), h.max(1)).ok_or_else(|| anyhow::anyhow!("SVG too large"))?;
            // Diagrams usually assume a white page.
            pm.fill(Color::WHITE);
            resvg::render(&tree, Transform::from_scale(scale, scale), &mut pm.as_mut());
            Ok(pm)
        }
        m => anyhow::bail!("{m} isn't shown natively"),
    }
}

fn exposure_label(us: f64) -> String {
    if us >= 1e6 {
        format!("{:.2} s", us / 1e6)
    } else if us >= 1000.0 {
        format!("{:.1} ms", us / 1000.0)
    } else {
        format!("{us:.0} µs")
    }
}

fn duration_label(s: f64) -> String {
    let t = s as u64;
    if t >= 3600 { format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60) } else { format!("{}:{:02}", t / 60, t % 60) }
}
