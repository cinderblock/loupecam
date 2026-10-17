//! Touch input from evdev: multitouch screens (protocol B, like the Raspberry Pi
//! display's FT5406), single-touch screens, and mice (handy when testing on a Pi). New
//! devices are picked up as they appear.

use crate::gfx;
use crate::ui::Input;
use crate::Event;
use evdev::{AbsoluteAxisCode, Device, EventSummary, KeyCode, RelativeAxisCode, SynchronizationCode};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy)]
struct Geometry {
    display: (u32, u32),
    rotation: u16,
    ui: (u32, u32),
}

impl Geometry {
    /// Display pixels to UI coordinates.
    fn ui(&self, x: f32, y: f32) -> (f32, f32) {
        gfx::display_to_ui(x, y, self.rotation, self.ui.0 as f32, self.ui.1 as f32)
    }
}

/// Watch `/dev/input` for pointing devices and send their input as [`Event::Input`].
pub fn spawn(tx: Sender<Event>, display: (u32, u32), rotation: u16, ui: (u32, u32)) {
    let geo = Geometry { display, rotation, ui };
    let open: Arc<Mutex<HashSet<PathBuf>>> = Arc::default();
    std::thread::Builder::new()
        .name("kiosk-input-scan".into())
        .spawn(move || {
            let mut index = 0u64;
            loop {
                for (path, dev) in evdev::enumerate() {
                    if open.lock().unwrap().contains(&path) || kind(&dev).is_none() {
                        continue;
                    }
                    tracing::info!("input: {} ({})", dev.name().unwrap_or("?"), path.display());
                    open.lock().unwrap().insert(path.clone());
                    index += 1;
                    let (tx, open, base) = (tx.clone(), open.clone(), index << 16);
                    let _ = std::thread::Builder::new().name("kiosk-input".into()).spawn(move || {
                        read(dev, base, geo, &tx);
                        open.lock().unwrap().remove(&path);
                    });
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        })
        .expect("spawn input thread");
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Multitouch,
    SingleTouch,
    Mouse,
}

fn kind(d: &Device) -> Option<Kind> {
    let abs = d.supported_absolute_axes();
    let keys = d.supported_keys();
    let has_abs = |a| abs.is_some_and(|s| s.contains(a));
    let has_key = |k| keys.is_some_and(|s| s.contains(k));
    if has_abs(AbsoluteAxisCode::ABS_MT_POSITION_X) && has_abs(AbsoluteAxisCode::ABS_MT_SLOT) {
        Some(Kind::Multitouch)
    } else if has_abs(AbsoluteAxisCode::ABS_X) && has_key(KeyCode::BTN_TOUCH) {
        Some(Kind::SingleTouch)
    } else if d.supported_relative_axes().is_some_and(|r| r.contains(RelativeAxisCode::REL_X)) && has_key(KeyCode::BTN_LEFT) {
        Some(Kind::Mouse)
    } else {
        None
    }
}

#[derive(Default, Clone, Copy)]
struct Slot {
    /// Tracking id, while a finger is down.
    active: bool,
    x: i32,
    y: i32,
    changed: bool,
    /// Down was sent for the current contact.
    reported: bool,
}

/// Read one device until it disappears.
fn read(mut dev: Device, base: u64, geo: Geometry, tx: &Sender<Event>) {
    let Some(kind) = kind(&dev) else { return };
    // Axis ranges, to scale to display pixels.
    let mut range: HashMap<AbsoluteAxisCode, (i32, i32)> = HashMap::new();
    if let Ok(info) = dev.get_absinfo() {
        for (axis, i) in info {
            range.insert(axis, (i.minimum(), i.maximum()));
        }
    }
    let scale = |axis: AbsoluteAxisCode, v: i32, size: u32| -> f32 {
        let (lo, hi) = range.get(&axis).copied().unwrap_or((0, size as i32 - 1));
        if hi <= lo { v as f32 } else { (v - lo) as f32 / (hi - lo) as f32 * (size as f32 - 1.0) }
    };
    let (xa, ya) = match kind {
        Kind::Multitouch => (AbsoluteAxisCode::ABS_MT_POSITION_X, AbsoluteAxisCode::ABS_MT_POSITION_Y),
        _ => (AbsoluteAxisCode::ABS_X, AbsoluteAxisCode::ABS_Y),
    };
    let mut slots = [Slot::default(); 16];
    let mut slot = 0usize;
    // Mouse pointer position, in display pixels.
    let mut cursor = (geo.display.0 as f32 / 2.0, geo.display.1 as f32 / 2.0);
    let send = |i: Input| tx.send(Event::Input(i)).is_ok();
    loop {
        let events = match dev.fetch_events() {
            Ok(e) => e.collect::<Vec<_>>(),
            Err(e) => {
                tracing::info!("input device gone: {e}");
                return;
            }
        };
        for ev in events {
            match ev.destructure() {
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_SLOT, v) => slot = (v.max(0) as usize).min(slots.len() - 1),
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_TRACKING_ID, v) => {
                    let s = &mut slots[slot];
                    s.active = v >= 0;
                    s.changed = true;
                }
                EventSummary::AbsoluteAxis(_, a, v) if a == xa => {
                    let s = &mut slots[if kind == Kind::Multitouch { slot } else { 0 }];
                    s.x = v;
                    s.changed = true;
                }
                EventSummary::AbsoluteAxis(_, a, v) if a == ya => {
                    let s = &mut slots[if kind == Kind::Multitouch { slot } else { 0 }];
                    s.y = v;
                    s.changed = true;
                }
                EventSummary::Key(_, KeyCode::BTN_TOUCH, v) if kind == Kind::SingleTouch => {
                    slots[0].active = v != 0;
                    slots[0].changed = true;
                }
                EventSummary::RelativeAxis(_, RelativeAxisCode::REL_X, v) if kind == Kind::Mouse => {
                    cursor.0 = (cursor.0 + v as f32).clamp(0.0, geo.display.0 as f32 - 1.0);
                    slots[0].changed = true;
                }
                EventSummary::RelativeAxis(_, RelativeAxisCode::REL_Y, v) if kind == Kind::Mouse => {
                    cursor.1 = (cursor.1 + v as f32).clamp(0.0, geo.display.1 as f32 - 1.0);
                    slots[0].changed = true;
                }
                EventSummary::RelativeAxis(_, RelativeAxisCode::REL_WHEEL, v) if kind == Kind::Mouse => {
                    let (x, y) = geo.ui(cursor.0, cursor.1);
                    if !send(Input::Wheel { x, y, delta: v as f32 * 120.0 }) {
                        return;
                    }
                }
                EventSummary::Key(_, KeyCode::BTN_LEFT, v) if kind == Kind::Mouse => {
                    slots[0].active = v != 0;
                    slots[0].changed = true;
                }
                EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                    for (i, s) in slots.iter_mut().enumerate().filter(|(_, s)| s.changed) {
                        s.changed = false;
                        let id = base | i as u64;
                        let (dx, dy) = match kind {
                            Kind::Mouse => cursor,
                            _ => (scale(xa, s.x, geo.display.0), scale(ya, s.y, geo.display.1)),
                        };
                        let (x, y) = geo.ui(dx, dy);
                        let input = match (s.active, s.reported) {
                            (true, false) => {
                                s.reported = true;
                                Some(Input::Down { id, x, y })
                            }
                            (true, true) => Some(Input::Move { id, x, y }),
                            (false, true) => {
                                s.reported = false;
                                Some(Input::Up { id })
                            }
                            (false, false) => None,
                        };
                        if let Some(i) = input
                            && !send(i)
                        {
                            return;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
