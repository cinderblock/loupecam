//! The display hardware, directly: DRM/KMS for output (two dumb buffers, flipped on
//! vblank) and evdev for touch. No X server or Wayland compositor is involved, so the
//! kiosk owns the whole screen from boot.

use super::touch;
use crate::client::Client;
use crate::gfx;
use crate::ui::{App, Input};
use crate::{Event, Options};
use anyhow::Context;
use drm::buffer::DrmFourcc;
use drm::control::{Device as ControlDevice, ModeTypeFlags, PageFlipFlags, connector, crtc, dumbbuffer::DumbBuffer, framebuffer};
use std::collections::HashSet;
use std::fs::File;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tiny_skia::Pixmap;

struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl drm::Device for Card {}
impl ControlDevice for Card {}

/// An output we draw to: a connected connector, its CRTC and mode, and two buffers.
pub struct Display {
    card: Card,
    path: PathBuf,
    crtc: crtc::Handle,
    connector: connector::Handle,
    mode: drm::control::Mode,
    /// Two buffers: one on screen, one to draw into.
    buffers: Vec<(DumbBuffer, framebuffer::Handle)>,
    /// The buffer on screen.
    front: usize,
    flip_ok: bool,
    /// What was on the CRTC before, restored when we let go.
    saved: Option<drm::control::crtc::Info>,
}

impl Display {
    /// The first card with a connected display. (On a Raspberry Pi 4, `card0` may be the
    /// render-only V3D GPU and the display controller another card.)
    pub fn open() -> anyhow::Result<Display> {
        let mut cards: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
            .context("no /dev/dri: is the KMS driver loaded (dtoverlay=vc4-kms-v3d)?")?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("card")))
            .collect();
        cards.sort();
        let mut errors = Vec::new();
        for path in cards {
            match Self::open_card(&path) {
                Ok(d) => return Ok(d),
                Err(e) => errors.push(format!("{}: {e:#}", path.display())),
            }
        }
        anyhow::bail!("no usable display ({})", errors.join("; "))
    }

    fn open_card(path: &Path) -> anyhow::Result<Display> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        let card = Card(file);
        let res = card.resource_handles().context("not a display controller")?;
        let conn = res
            .connectors()
            .iter()
            .flat_map(|c| card.get_connector(*c, true))
            .find(|c| c.state() == connector::State::Connected && !c.modes().is_empty())
            .ok_or_else(|| anyhow::anyhow!("no connected display"))?;
        let mode = *conn
            .modes()
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .unwrap_or(&conn.modes()[0]);
        // The CRTC driving the connector now, or the first one its encoders can use.
        let current = conn.current_encoder().and_then(|e| card.get_encoder(e).ok()).and_then(|e| e.crtc());
        let crtc = current
            .or_else(|| {
                conn.encoders().iter().flat_map(|e| card.get_encoder(*e)).find_map(|e| res.filter_crtcs(e.possible_crtcs()).first().copied())
            })
            .ok_or_else(|| anyhow::anyhow!("no CRTC for the display"))?;
        let saved = card.get_crtc(crtc).ok();
        let (w, h) = mode.size();
        let make = || -> anyhow::Result<(DumbBuffer, framebuffer::Handle)> {
            let db = card.create_dumb_buffer((w as u32, h as u32), DrmFourcc::Xrgb8888, 32)?;
            let fb = card.add_framebuffer(&db, 24, 32)?;
            Ok((db, fb))
        };
        let buffers = vec![make()?, make()?];
        let d = Display { card, path: path.to_path_buf(), crtc, connector: conn.handle(), mode, buffers, front: 0, flip_ok: true, saved };
        d.card
            .set_crtc(d.crtc, Some(d.buffers[0].1), (0, 0), &[d.connector], Some(d.mode))
            .context("setting the display mode (is another program, e.g. a desktop, using the display?)")?;
        tracing::info!(card = %d.path.display(), "display {}x{} on {:?}", w, h, conn.interface());
        Ok(d)
    }

    pub fn size(&self) -> (u32, u32) {
        let (w, h) = self.mode.size();
        (w as u32, h as u32)
    }

    /// Show `pm` (UI pixels, rotated by `rotation` onto the display). Waits for the flip.
    pub fn present(&mut self, pm: &Pixmap, rotation: u16) -> anyhow::Result<()> {
        let back = 1 - self.front;
        let (w, h) = self.size();
        {
            let (db, _) = &mut self.buffers[back];
            let stride = db_pitch(db) / 4;
            let mut map = self.card.map_dumb_buffer(db)?;
            // SAFETY: the mapping is page-aligned and any bit pattern is a valid u32.
            let (pre, px, _) = unsafe { map.as_mut().align_to_mut::<u32>() };
            anyhow::ensure!(pre.is_empty(), "unaligned buffer mapping");
            gfx::to_xrgb(pm, rotation, px, w as usize, h as usize, stride);
        }
        let fb = self.buffers[back].1;
        if self.flip_ok {
            match self.card.page_flip(self.crtc, fb, PageFlipFlags::EVENT, None) {
                Ok(()) => {
                    // Wait for the flip, so the buffer we draw into next is off screen.
                    let _ = self.card.receive_events()?.count();
                    self.front = back;
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!("page flip unsupported ({e}); falling back to mode sets");
                    self.flip_ok = false;
                }
            }
        }
        self.card.set_crtc(self.crtc, Some(fb), (0, 0), &[self.connector], Some(self.mode))?;
        self.front = back;
        Ok(())
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // Put back whatever was shown before (the console), then free our buffers.
        if let Some(s) = &self.saved {
            let _ = self.card.set_crtc(self.crtc, s.framebuffer(), s.position(), &[self.connector], s.mode());
        }
        for (db, fb) in self.buffers.drain(..) {
            let _ = self.card.destroy_framebuffer(fb);
            let _ = self.card.destroy_dumb_buffer(db);
        }
    }
}

fn db_pitch(db: &DumbBuffer) -> usize {
    use drm::buffer::Buffer;
    db.pitch() as usize
}

/// Switches the console (VT) to graphics mode while alive, so the kernel doesn't draw
/// text or a cursor over the UI.
struct GraphicsVt;

const KDSETMODE: libc::c_ulong = 0x4B3A;
const KD_TEXT: libc::c_int = 0;
const KD_GRAPHICS: libc::c_int = 1;

impl GraphicsVt {
    fn enter() -> Option<GraphicsVt> {
        // stdin is the VT when run from a systemd unit with TTYPath=.
        // SAFETY: plain ioctls on fd 0.
        unsafe {
            if libc::isatty(0) == 0 {
                return None;
            }
            (libc::ioctl(0, KDSETMODE as _, KD_GRAPHICS) == 0).then_some(GraphicsVt)
        }
    }
}

impl Drop for GraphicsVt {
    fn drop(&mut self) {
        // SAFETY: as above.
        unsafe {
            libc::ioctl(0, KDSETMODE as _, KD_TEXT);
        }
    }
}

/// The display's backlight, if the kernel exposes one we may write.
struct Backlight(Option<PathBuf>);

impl Backlight {
    fn find() -> Backlight {
        let path = std::fs::read_dir("/sys/class/backlight").ok().and_then(|mut d| d.next()).and_then(|e| e.ok()).map(|e| e.path().join("bl_power"));
        Backlight(path)
    }

    fn set(&self, on: bool) {
        let Some(p) = &self.0 else { return };
        // bl_power: 0 = on, 4 = powered down (FB_BLANK_POWERDOWN).
        if let Err(e) = std::fs::write(p, if on { "0" } else { "4" }) {
            tracing::warn!("backlight {}: {e} (the kiosk user needs write access; see the udev rule)", p.display());
        }
    }
}

pub fn run(opts: &Options) -> anyhow::Result<()> {
    let mut display = Display::open()?;
    let _vt = GraphicsVt::enter();
    let (dw, dh) = display.size();
    let (uw, uh) = gfx::ui_size(dw, dh, opts.rotation);
    let (tx, rx) = mpsc::channel::<Event>();
    let client = {
        let tx = tx.clone();
        Client::start(&opts.server, opts.token.clone(), move |ev| {
            let _ = tx.send(Event::Server(ev));
        })
    };
    touch::spawn(tx.clone(), (dw, dh), opts.rotation, (uw, uh));
    let mut app = App::new(client, uw, uh, crate::address(&opts.server));
    let mut pixmap = Pixmap::new(uw, uh).ok_or_else(|| anyhow::anyhow!("bad display size"))?;
    let backlight = Backlight::find();
    backlight.set(true);
    let mut asleep = false;
    // Touches that woke the screen: swallowed until lifted.
    let mut waking: HashSet<u64> = HashSet::new();
    let min_frame = Duration::from_millis(16);
    let mut last_present = Instant::now() - min_frame;
    loop {
        let wait = if app.needs_redraw() {
            min_frame.saturating_sub(last_present.elapsed())
        } else {
            app.next_wake().map_or(Duration::from_secs(1), |t| t.saturating_duration_since(Instant::now()))
        };
        let first = match rx.recv_timeout(wait) {
            Ok(ev) => Some(ev),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("event sources stopped"),
        };
        for ev in first.into_iter().chain(rx.try_iter()) {
            match ev {
                Event::Server(e) => app.on_server(e),
                Event::Input(i) => {
                    let id = match i {
                        Input::Down { id, .. } | Input::Move { id, .. } | Input::Up { id } | Input::Cancel { id } => Some(id),
                        Input::Wheel { .. } => None,
                    };
                    if asleep {
                        // The first touch only wakes the screen.
                        asleep = false;
                        backlight.set(true);
                        waking.extend(id);
                        continue;
                    }
                    if let Some(id) = id
                        && waking.contains(&id)
                    {
                        if matches!(i, Input::Up { .. } | Input::Cancel { .. }) {
                            waking.remove(&id);
                        }
                        continue;
                    }
                    app.on_input(i);
                }
            }
        }
        if let Some(after) = opts.sleep_after
            && !asleep
            && app.idle() > after
        {
            asleep = true;
            backlight.set(false);
        }
        app.tick();
        if app.needs_redraw() && last_present.elapsed() >= min_frame {
            app.draw(&mut pixmap);
            display.present(&pixmap, opts.rotation)?;
            last_present = Instant::now();
        }
    }
}
