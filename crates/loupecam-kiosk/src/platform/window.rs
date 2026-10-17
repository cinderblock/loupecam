//! A desktop window standing in for the display, for development. The mouse acts as
//! one finger, the wheel zooms, and real touch input works where the OS provides it.

use crate::client::Client;
use crate::gfx;
use crate::ui::{App, Input};
use crate::{Event, Options};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct Win {
    app: App,
    rotation: u16,
    size: (u32, u32),
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    pixmap: Pixmap,
    cursor: (f32, f32),
    mouse_down: bool,
}

pub fn run(opts: &Options, size: (u32, u32)) -> anyhow::Result<()> {
    let event_loop = EventLoop::<Event>::with_user_event().build()?;
    let proxy = Mutex::new(event_loop.create_proxy());
    let client = Client::start(&opts.server, opts.token.clone(), move |ev| {
        let _ = proxy.lock().unwrap().send_event(Event::Server(ev));
    });
    let (uw, uh) = gfx::ui_size(size.0, size.1, opts.rotation);
    let mut win = Win {
        app: App::new(client, uw, uh, crate::address(&opts.server)),
        rotation: opts.rotation,
        size,
        window: None,
        surface: None,
        pixmap: Pixmap::new(uw, uh).ok_or_else(|| anyhow::anyhow!("bad window size"))?,
        cursor: (0.0, 0.0),
        mouse_down: false,
    };
    event_loop.run_app(&mut win)?;
    Ok(())
}

impl Win {
    fn input(&mut self, x: f64, y: f64, make: impl FnOnce(f32, f32) -> Input) {
        let (uw, uh) = (self.pixmap.width() as f32, self.pixmap.height() as f32);
        let (x, y) = gfx::display_to_ui(x as f32, y as f32, self.rotation, uw, uh);
        self.app.on_input(make(x, y));
    }

    fn present(&mut self) {
        let (Some(surface), Some(_)) = (self.surface.as_mut(), self.window.as_ref()) else { return };
        let (w, h) = self.size;
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else { return };
        if surface.resize(nw, nh).is_err() {
            return;
        }
        self.app.draw(&mut self.pixmap);
        let Ok(mut buf) = surface.buffer_mut() else { return };
        gfx::to_xrgb(&self.pixmap, self.rotation, &mut buf, w as usize, h as usize, w as usize);
        let _ = buf.present();
    }
}

impl ApplicationHandler<Event> for Win {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("LoupeCam kiosk")
            .with_inner_size(PhysicalSize::new(self.size.0, self.size.1))
            .with_resizable(false);
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        self.surface = Some(softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface"));
        self.window = Some(window);
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, ev: Event) {
        match ev {
            Event::Server(e) => self.app.on_server(e),
            Event::Input(i) => self.app.on_input(i),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(s) => {
                self.size = (s.width.max(1), s.height.max(1));
                let (uw, uh) = gfx::ui_size(self.size.0, self.size.1, self.rotation);
                if let Some(p) = Pixmap::new(uw, uh) {
                    self.pixmap = p;
                    self.app.resize(uw, uh);
                }
            }
            WindowEvent::RedrawRequested => self.present(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                if self.mouse_down {
                    self.input(position.x, position.y, |x, y| Input::Move { id: 0, x, y });
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let (x, y) = (self.cursor.0 as f64, self.cursor.1 as f64);
                self.mouse_down = state == ElementState::Pressed;
                if self.mouse_down {
                    self.input(x, y, |x, y| Input::Down { id: 0, x, y });
                } else {
                    self.app.on_input(Input::Up { id: 0 });
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 120.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                let (x, y) = (self.cursor.0 as f64, self.cursor.1 as f64);
                self.input(x, y, |x, y| Input::Wheel { x, y, delta: d });
            }
            WindowEvent::Touch(t) => {
                // Mouse uses id 0.
                let id = t.id + 1;
                match t.phase {
                    TouchPhase::Started => self.input(t.location.x, t.location.y, |x, y| Input::Down { id, x, y }),
                    TouchPhase::Moved => self.input(t.location.x, t.location.y, |x, y| Input::Move { id, x, y }),
                    TouchPhase::Ended => self.app.on_input(Input::Up { id }),
                    TouchPhase::Cancelled => self.app.on_input(Input::Cancel { id }),
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.app.tick();
        if self.app.needs_redraw()
            && let Some(w) = &self.window
        {
            w.request_redraw();
        }
        el.set_control_flow(match self.app.next_wake() {
            Some(t) => ControlFlow::WaitUntil(t.max(Instant::now())),
            None => ControlFlow::Wait,
        });
    }
}
