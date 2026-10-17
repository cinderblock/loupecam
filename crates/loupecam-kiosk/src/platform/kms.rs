//! The display hardware, directly: DRM/KMS for output (two dumb buffers, flipped on
//! vblank) and evdev for touch. No X server or Wayland compositor is involved, so the
//! kiosk owns the whole screen from boot.

use super::browser::Browser;
use super::touch;
use crate::client::Client;
use crate::gfx;
use crate::ui::{App, Input};
use crate::{Event, Home, Options};
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

/// What the screen is showing.
enum Screen {
    Native { display: Display, _vt: Option<GraphicsVt> },
    /// The browser; `cast` is the cast it shows (`None`: the home web UI).
    Browser { browser: Browser, cast: Option<u64> },
}

/// What the screen should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    Native,
    HomeBrowser,
    Cast(u64),
}

fn url_with(base: &str, path: &str, key: &str, value: Option<&str>) -> String {
    let base = base.trim_end_matches('/');
    match value {
        Some(v) => format!("{base}{path}?{key}={}", crate::client::encode_query(v)),
        None => format!("{base}{path}"),
    }
}

pub fn run(opts: &Options) -> anyhow::Result<()> {
    let display = Display::open()?;
    let (dw, dh) = display.size();
    let (uw, uh) = gfx::ui_size(dw, dh, opts.rotation);
    let (tx, rx) = mpsc::channel::<Event>();
    let client = {
        let tx = tx.clone();
        Client::start(&opts.server, opts.token.clone(), move |ev| {
            let _ = tx.send(Event::Server(ev));
        })
    };
    // Touch is read without grabbing the devices, so a browser on screen gets it too.
    touch::spawn(tx.clone(), (dw, dh), opts.rotation, (uw, uh));
    let mut app = App::new(client, uw, uh, crate::address(&opts.server));
    let mut pixmap = Pixmap::new(uw, uh).ok_or_else(|| anyhow::anyhow!("bad display size"))?;
    let backlight = Backlight::find();
    backlight.set(true);
    let mut screen = Some(Screen::Native { display, _vt: GraphicsVt::enter() });
    let mut asleep = false;
    // Touches that woke the screen: swallowed until lifted.
    let mut waking: HashSet<u64> = HashSet::new();
    // Fingers down while the browser shows a cast (three at once: back to the camera).
    let mut fingers: HashSet<u64> = HashSet::new();
    // A cast whose browser closed on its own: shown natively as a message instead of
    // relaunching the browser in a loop.
    let mut failed_cast: Option<u64> = None;
    let mut home_retry = Instant::now();
    let min_frame = Duration::from_millis(16);
    let mut last_present = Instant::now() - min_frame;
    loop {
        let native = matches!(screen, Some(Screen::Native { .. }));
        let wait = if !native {
            Duration::from_millis(250)
        } else if app.needs_redraw() {
            min_frame.saturating_sub(last_present.elapsed())
        } else {
            app.next_wake().map_or(Duration::from_secs(1), |t| t.saturating_duration_since(Instant::now()))
        };
        let first = match rx.recv_timeout(wait) {
            Ok(ev) => Some(ev),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("event sources stopped"),
        };
        let casting_in_browser = matches!(screen, Some(Screen::Browser { cast: Some(_), .. }));
        for ev in first.into_iter().chain(rx.try_iter()) {
            match ev {
                Event::Server(e) => app.on_server(e),
                Event::Input(i) if !native => {
                    // The browser has the screen; only watch for the way back.
                    app.poke();
                    match i {
                        Input::Down { id, .. } if casting_in_browser => {
                            fingers.insert(id);
                            if fingers.len() >= 3 {
                                fingers.clear();
                                app.stop_casting();
                            }
                        }
                        Input::Up { id } | Input::Cancel { id } => {
                            fingers.remove(&id);
                        }
                        _ => {}
                    }
                }
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

        // A browser that exited on its own.
        if let Some(Screen::Browser { browser, cast }) = screen.as_mut()
            && browser.exited()
        {
            let cast = *cast;
            if let Some(Screen::Browser { browser, .. }) = screen.take() {
                browser.stop();
            }
            match cast {
                Some(id) => {
                    failed_cast = Some(id);
                    app.notify("The browser closed: tap ✕ Camera to go back".into());
                }
                None => home_retry = Instant::now() + Duration::from_secs(3),
            }
        }

        // Switch what's on screen if needed.
        let cast_id = app.browser_cast().map(|c| c.id);
        if cast_id.is_none() {
            failed_cast = None;
        }
        let want = match cast_id {
            Some(id) if failed_cast != Some(id) => Want::Cast(id),
            _ if opts.home == Home::Browser && Instant::now() >= home_retry => Want::HomeBrowser,
            _ => Want::Native,
        };
        let have = match &screen {
            Some(Screen::Native { .. }) => Some(Want::Native),
            Some(Screen::Browser { cast: Some(id), .. }) => Some(Want::Cast(*id)),
            Some(Screen::Browser { cast: None, .. }) => Some(Want::HomeBrowser),
            None => None,
        };
        if have != Some(want) {
            // Let go of the display first: only one program can drive it.
            match screen.take() {
                Some(Screen::Browser { browser, .. }) => browser.stop(),
                Some(Screen::Native { display, _vt }) => drop((display, _vt)),
                None => {}
            }
            fingers.clear();
            let launch = match want {
                Want::Native => None,
                Want::HomeBrowser => Some((url_with(&opts.server, "/", "token", opts.token.as_deref()), None, opts.browser_profile.as_deref())),
                Want::Cast(id) => Some((url_with(&opts.server, "/cast", "displayToken", opts.display_token.as_deref()), Some(id), None)),
            };
            if let Some((url, cast, profile)) = launch {
                match Browser::launch(&url, opts.browser_command.as_deref(), profile) {
                    Ok(browser) => screen = Some(Screen::Browser { browser, cast }),
                    Err(e) => {
                        tracing::warn!("{e:#}");
                        app.notify(format!("Can't open the browser: {e}"));
                        match cast {
                            Some(id) => failed_cast = Some(id),
                            None => home_retry = Instant::now() + Duration::from_secs(30),
                        }
                    }
                }
            }
            if screen.is_none() {
                // Back to the native UI. The browser may take a moment to let go of the
                // display.
                let mut tries = 0;
                let display = loop {
                    match Display::open() {
                        Ok(d) => break d,
                        Err(e) if tries < 20 => {
                            tries += 1;
                            tracing::debug!("waiting for the display: {e:#}");
                            std::thread::sleep(Duration::from_millis(250));
                        }
                        Err(e) => return Err(e),
                    }
                };
                screen = Some(Screen::Native { display, _vt: GraphicsVt::enter() });
                app.redraw();
                asleep = false;
                backlight.set(true);
            }
        }

        let Some(Screen::Native { display, .. }) = screen.as_mut() else { continue };
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
