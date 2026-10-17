//! Connection to the LoupeCam server: a WebSocket for state, stats and preview frames,
//! and a worker thread for requests (capture, record, settings), so the UI never waits
//! on the network.

use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tiny_skia::Pixmap;

/// Camera state, as much of it as the kiosk shows.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CamState {
    pub status: Status,
    pub device: Option<Device>,
    pub settings: Settings,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Status {
    pub state: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Device {
    pub model: String,
    pub serial: String,
    pub max_gain: f32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub exposure_us: u32,
    pub auto_exposure: bool,
    pub gain: f32,
    pub white_balance: WhiteBalance,
    pub record: RecordSettings,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WhiteBalance {
    pub mode: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecordSettings {
    pub preset: String,
    pub mode: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Stats {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub exposure_us: f64,
    pub gain: f32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Recording {
    pub name: Option<String>,
    pub mode: String,
    pub seconds: f64,
    pub frames: u64,
    pub stills: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecordStatus {
    pub active: Option<Recording>,
    pub last: Option<Recording>,
    pub error: Option<String>,
    pub encoders: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CastInfo {
    pub id: u64,
    pub kind: String,
    pub mime: String,
    pub name: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug)]
pub enum ServerEvent {
    Connected(bool),
    /// The server wants a token we don't have (or ours is wrong).
    AuthFailed,
    State(CamState),
    Stats(Stats),
    Record(RecordStatus),
    Cast(Option<CastInfo>),
    /// A new preview frame is in [`Client::frame`].
    Frame,
    /// Something to tell the user.
    Message(String),
    /// The content of a cast, fetched for native display.
    CastContent { id: u64, bytes: Vec<u8> },
}

#[derive(Debug, Clone)]
pub enum Command {
    Capture,
    /// Start recording, switching to a preset first if given.
    StartRecording(Option<String>),
    StopRecording,
    /// Make a record preset's settings the current record settings.
    ApplyPreset(String),
    /// A JSON merge patch for the settings.
    Patch(Value),
    /// One-shot white balance over the whole frame.
    WhiteBalance,
    StopCasting,
    FetchCast(u64),
}

/// A decoded preview frame.
pub struct Frame {
    pub pixmap: Pixmap,
}

pub struct Client {
    commands: Sender<Command>,
    /// The latest preview frame not yet taken by the UI.
    pub frame: Arc<Mutex<Option<Frame>>>,
}

impl Client {
    /// Connect to the server at `base` (e.g. `http://127.0.0.1`). Events arrive through
    /// `notify`, which wraps each in the caller's event type.
    pub fn start(base: &str, token: Option<String>, notify: impl Fn(ServerEvent) + Send + Sync + 'static) -> Client {
        let notify = Arc::new(notify);
        let frame = Arc::new(Mutex::new(None));
        let base = base.trim_end_matches('/').to_string();
        {
            let (base, token, notify, frame) = (base.clone(), token.clone(), notify.clone(), frame.clone());
            std::thread::Builder::new()
                .name("kiosk-ws".into())
                .spawn(move || websocket(&base, token.as_deref(), &*notify, &frame))
                .expect("spawn websocket thread");
        }
        let (commands, rx) = channel();
        std::thread::Builder::new()
            .name("kiosk-requests".into())
            .spawn(move || requests(&base, token.as_deref(), rx, &*notify))
            .expect("spawn request thread");
        Client { commands, frame }
    }

    pub fn send(&self, c: Command) {
        let _ = self.commands.send(c);
    }

    pub fn take_frame(&self) -> Option<Frame> {
        self.frame.lock().unwrap().take()
    }
}

pub(crate) fn encode_query(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn websocket(base: &str, token: Option<&str>, notify: &(dyn Fn(ServerEvent) + Send + Sync), slot: &Mutex<Option<Frame>>) {
    let ws_base = base.replacen("http://", "ws://", 1).replacen("https://", "wss://", 1);
    let url = match token {
        Some(t) => format!("{ws_base}/api/ws?token={}", encode_query(t)),
        None => format!("{ws_base}/api/ws"),
    };
    let mut retry = 0u32;
    loop {
        match tungstenite::connect(url.as_str()) {
            Ok((mut ws, _)) => {
                retry = 0;
                notify(ServerEvent::Connected(true));
                let _ = ws.send(tungstenite::Message::text(json!({ "type": "preview", "enabled": true }).to_string()));
                loop {
                    match ws.read() {
                        Ok(tungstenite::Message::Text(t)) => handle_text(&t, notify),
                        Ok(tungstenite::Message::Binary(b)) => {
                            match decode_jpeg(&b) {
                                Ok(pixmap) => {
                                    // Only notify when the slot was empty: the UI takes the
                                    // latest frame when it draws.
                                    let was_empty = slot.lock().unwrap().replace(Frame { pixmap }).is_none();
                                    if was_empty {
                                        notify(ServerEvent::Frame);
                                    }
                                }
                                Err(e) => tracing::debug!("preview frame: {e}"),
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::info!("server connection lost: {e}");
                            break;
                        }
                    }
                }
                notify(ServerEvent::Connected(false));
            }
            Err(tungstenite::Error::Http(r)) if r.status() == 401 => {
                notify(ServerEvent::AuthFailed);
                retry = retry.max(4);
            }
            Err(e) => tracing::debug!("connecting to {base}: {e}"),
        }
        std::thread::sleep(Duration::from_millis(250 * (1 << retry.min(4))));
        retry += 1;
    }
}

fn handle_text(t: &str, notify: &(dyn Fn(ServerEvent) + Send + Sync)) {
    let Ok(v) = serde_json::from_str::<Value>(t) else { return };
    let ev = match v["type"].as_str() {
        Some("state") => serde_json::from_value(v).map(ServerEvent::State),
        Some("stats") => serde_json::from_value(v).map(ServerEvent::Stats),
        Some("record") => serde_json::from_value(v).map(ServerEvent::Record),
        Some("display") => serde_json::from_value(v["cast"].clone()).map(ServerEvent::Cast),
        _ => return,
    };
    match ev {
        Ok(ev) => notify(ev),
        Err(e) => tracing::warn!("unexpected message from server: {e}"),
    }
}

/// Decode a JPEG into an opaque pixmap.
pub fn decode_jpeg(b: &[u8]) -> anyhow::Result<Pixmap> {
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(b), opts);
    let rgba = d.decode().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let info = d.info().ok_or_else(|| anyhow::anyhow!("no image info"))?;
    let size = tiny_skia::IntSize::from_wh(info.width as u32, info.height as u32).ok_or_else(|| anyhow::anyhow!("empty image"))?;
    // Opaque, so straight and premultiplied alpha are the same.
    Pixmap::from_vec(rgba, size).ok_or_else(|| anyhow::anyhow!("bad image size"))
}

fn requests(base: &str, token: Option<&str>, rx: Receiver<Command>, notify: &(dyn Fn(ServerEvent) + Send + Sync)) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let call = |method: &str, path: &str, body: Option<Value>| -> Result<Vec<u8>, String> {
        let url = format!("{base}{path}");
        let auth = token.map(|t| format!("Bearer {t}"));
        let resp = match (method, body) {
            ("GET", _) => {
                let mut r = agent.get(&url);
                if let Some(a) = &auth {
                    r = r.header("Authorization", a);
                }
                r.call()
            }
            ("DELETE", _) => {
                let mut r = agent.delete(&url);
                if let Some(a) = &auth {
                    r = r.header("Authorization", a);
                }
                r.call()
            }
            (m, body) => {
                let mut r = if m == "PATCH" { agent.patch(&url) } else { agent.post(&url) };
                if let Some(a) = &auth {
                    r = r.header("Authorization", a);
                }
                match body {
                    Some(b) => r.content_type("application/json").send(b.to_string()),
                    None => r.send_empty(),
                }
            }
        };
        let mut resp = resp.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let bytes = resp.body_mut().with_config().limit(80 << 20).read_to_vec().map_err(|e| e.to_string())?;
        if status >= 400 {
            let msg = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v["error"].as_str().map(String::from));
            return Err(msg.unwrap_or_else(|| format!("HTTP {status}")));
        }
        Ok(bytes)
    };
    let json = |b: &[u8]| serde_json::from_slice::<Value>(b).unwrap_or(Value::Null);
    for cmd in rx {
        let result = match &cmd {
            Command::Capture => call("POST", "/api/capture", None).map(|b| {
                let v = json(&b);
                Some(format!("Saved {}", v["name"].as_str().unwrap_or("capture")))
            }),
            Command::StartRecording(preset) => {
                let body = preset.as_ref().map(|p| json!({ "preset": p })).unwrap_or(json!({}));
                call("POST", "/api/record/start", Some(body)).map(|_| None)
            }
            Command::StopRecording => call("POST", "/api/record/stop", None).map(|b| {
                let v = json(&b);
                v["name"].as_str().map(|n| format!("Saved {n}"))
            }),
            Command::ApplyPreset(p) => call("GET", "/api/record", None).and_then(|b| {
                let preset = json(&b)["presets"][p].clone();
                if preset.is_null() {
                    return Err(format!("no preset {p}"));
                }
                call("PATCH", "/api/settings", Some(json!({ "record": preset }))).map(|_| None)
            }),
            Command::Patch(p) => call("PATCH", "/api/settings", Some(p.clone())).map(|_| None),
            Command::WhiteBalance => call("POST", "/api/white-balance", Some(json!({ "region": null }))).map(|_| Some("White balance set".into())),
            Command::StopCasting => call("DELETE", "/api/display", None).map(|_| None),
            Command::FetchCast(id) => match call("GET", "/api/display/content", None) {
                Ok(bytes) => {
                    notify(ServerEvent::CastContent { id: *id, bytes });
                    Ok(None)
                }
                Err(e) => Err(e),
            },
        };
        match result {
            Ok(Some(m)) => notify(ServerEvent::Message(m)),
            Ok(None) => {}
            Err(e) => notify(ServerEvent::Message(format!("{}: {e}", describe(&cmd)))),
        }
    }
}

fn describe(c: &Command) -> &'static str {
    match c {
        Command::Capture => "Capture failed",
        Command::StartRecording(_) => "Couldn't record",
        Command::StopRecording => "Stopping the recording failed",
        Command::ApplyPreset(_) => "Couldn't change the record mode",
        Command::Patch(_) => "Setting failed",
        Command::WhiteBalance => "White balance failed",
        Command::StopCasting => "Couldn't stop casting",
        Command::FetchCast(_) => "Couldn't load the cast",
    }
}
