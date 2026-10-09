//! HTTP / WebSocket API.

use crate::captures;
use crate::geometry::{NormRect, display_to_frame};
use crate::preview::Preview;
use crate::service::Service;
use crate::{AppState, WebUi};
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

type ApiResult<T> = Result<T, ApiError>;

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

fn bad(e: impl ToString) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, e.to_string())
}

fn unavailable(e: impl ToString) -> ApiError {
    ApiError(StatusCode::SERVICE_UNAVAILABLE, e.to_string())
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/state", get(get_state))
        .route("/stats", get(get_stats))
        .route("/settings", get(get_settings).patch(patch_settings))
        .route("/capture", post(capture))
        .route("/captures", get(list_captures))
        .route("/captures/{name}", get(get_capture).delete(delete_capture))
        .route("/white-balance", post(white_balance))
        .route("/roi", post(set_roi))
        .route("/ws", get(ws));
    Router::new()
        .nest("/api", api)
        .route("/stream.mjpg", get(mjpeg))
        .route("/snapshot.jpg", get(snapshot))
        .fallback(static_files)
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}

/// Bearer-token check when a token is configured. Browsers can't set headers on
/// `<img>`/WebSocket requests, so `?token=` and a cookie are accepted too.
async fn auth(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let Some(token) = s.token.as_deref() else { return next.run(req).await };
    let path = req.uri().path();
    // The UI shell itself is public so it can ask for the token.
    let is_ui = !(path.starts_with("/api") || path.ends_with(".mjpg") || path.ends_with(".jpg"));
    let h = req.headers();
    let bearer = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    let cookie = h
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| c.split(';').find_map(|kv| kv.trim().strip_prefix("loupecam_token=")));
    let query = req.uri().query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")));
    let ok = [bearer, cookie, query].into_iter().flatten().any(|t| constant_time_eq(t.as_bytes(), token.as_bytes()));
    if ok || is_ui { next.run(req).await } else { ApiError(StatusCode::UNAUTHORIZED, "token required".into()).into_response() }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn get_state(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(&*s.service.shared.state.borrow()).unwrap())
}

async fn get_stats(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(&*s.service.shared.stats.borrow()).unwrap())
}

async fn get_settings(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(&s.service.shared.state.borrow().settings).unwrap())
}

async fn patch_settings(State(s): State<AppState>, Json(patch): Json<serde_json::Value>) -> ApiResult<Json<serde_json::Value>> {
    let settings = s.service.patch(patch).await.map_err(bad)?;
    s.persist(&settings);
    Ok(Json(serde_json::to_value(settings).unwrap()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureQuery {
    full_resolution: Option<bool>,
}

async fn capture(State(s): State<AppState>, Query(q): Query<CaptureQuery>) -> ApiResult<Json<captures::CaptureInfo>> {
    let full = q.full_resolution.unwrap_or_else(|| s.service.shared.state.borrow().settings.capture.full_resolution);
    let c = s.service.capture(full).await.map_err(unavailable)?;
    let dir = s.captures_dir.clone();
    let info = tokio::task::spawn_blocking(move || captures::save(&c, &dir))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(info))
}

async fn list_captures(State(s): State<AppState>) -> ApiResult<Json<Vec<captures::CaptureEntry>>> {
    let dir = s.captures_dir.clone();
    tokio::task::spawn_blocking(move || captures::list(&dir))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map(Json)
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn get_capture(State(s): State<AppState>, Path(name): Path<String>) -> ApiResult<Response> {
    let path = captures::resolve(&s.captures_dir, &name).ok_or(ApiError(StatusCode::NOT_FOUND, "no such capture".into()))?;
    let bytes = tokio::fs::read(&path).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    Ok(([(header::CONTENT_TYPE, mime.as_ref().to_string())], bytes).into_response())
}

async fn delete_capture(State(s): State<AppState>, Path(name): Path<String>) -> ApiResult<StatusCode> {
    let path = captures::resolve(&s.captures_dir, &name).ok_or(ApiError(StatusCode::NOT_FOUND, "no such capture".into()))?;
    tokio::fs::remove_file(path).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegionBody {
    /// Region of the displayed image, normalised. `null` = whole frame / clear.
    region: Option<NormRect>,
}

/// The orientation applied when developing, plus the latest frame size.
fn display_mapping(s: &AppState) -> Option<(loupecam_isp::Orientation, u32, u32)> {
    let env = s.service.shared.frame.borrow().clone()?;
    let settings = s.service.shared.state.borrow().settings.clone();
    let o = settings.develop_params(env.model, settings.preview.demosaic).orientation;
    Some((o, env.raw.width, env.raw.height))
}

async fn white_balance(State(s): State<AppState>, Json(b): Json<RegionBody>) -> ApiResult<Json<serde_json::Value>> {
    let region = match b.region {
        Some(r) => {
            let (o, w, h) = display_mapping(&s).ok_or_else(|| unavailable("no frame yet"))?;
            Some(display_to_frame(r, o, w, h))
        }
        None => None,
    };
    let gains = s.service.white_balance(region).await.map_err(unavailable)?;
    s.persist(&s.service.shared.state.borrow().settings);
    Ok(Json(json!({ "gains": gains })))
}

/// Set the sensor ROI from a region drawn on the displayed image (relative to the
/// current view, so ROIs can be refined), or clear it.
async fn set_roi(State(s): State<AppState>, Json(b): Json<RegionBody>) -> ApiResult<Json<serde_json::Value>> {
    let patch = match b.region {
        None => json!({ "roi": null }),
        Some(r) => {
            let (o, w, h) = display_mapping(&s).ok_or_else(|| unavailable("no frame yet"))?;
            let f = display_to_frame(r, o, w, h);
            let cur = s.service.shared.state.borrow().settings.roi;
            let (ox, oy) = cur.map_or((0, 0), |c| (c.x as u32, c.y as u32));
            json!({ "roi": { "x": ox + f.x, "y": oy + f.y, "width": f.width.max(64), "height": f.height.max(64) } })
        }
    };
    let settings = s.service.patch(patch).await.map_err(bad)?;
    s.persist(&settings);
    Ok(Json(serde_json::to_value(settings).unwrap()))
}

async fn snapshot(State(s): State<AppState>) -> ApiResult<Response> {
    let p = s.preview.next(Duration::from_secs(3)).await.ok_or_else(|| unavailable("no frame from camera"))?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")], p.jpeg.clone()).into_response())
}

/// Motion-JPEG over `multipart/x-mixed-replace`: works in `<img>`, VLC, and most
/// camera integrations.
async fn mjpeg(State(s): State<AppState>) -> Response {
    let guard = s.preview.viewer();
    let rx = s.preview.frames.subscribe();
    let stream = futures_util::stream::unfold((rx, guard), |(mut rx, guard)| async move {
        rx.changed().await.ok()?;
        let p = rx.borrow_and_update().clone()?;
        let mut part = format!("--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n", p.jpeg.len()).into_bytes();
        part.extend_from_slice(&p.jpeg);
        part.extend_from_slice(b"\r\n");
        Some((Ok::<_, std::convert::Infallible>(Bytes::from(part)), (rx, guard)))
    });
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("multipart/x-mixed-replace; boundary=frame"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    (headers, Body::from_stream(stream)).into_response()
}

async fn ws(State(s): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| ws_session(socket, s.service.clone(), s.preview.clone()))
}

/// Server → client: `{"type":"state",…}` on change, `{"type":"stats",…}` ~5×/s, and,
/// after the client sends `{"type":"preview","enabled":true}`, binary JPEG frames.
async fn ws_session(socket: WebSocket, service: Arc<Service>, preview: Arc<Preview>) {
    let (mut tx, mut rx) = socket.split();
    let mut state = service.shared.state.subscribe();
    let mut stats = service.shared.stats.subscribe();
    let mut frames = preview.frames.subscribe();
    let mut viewer = None;
    let msg = |kind: &str, v: serde_json::Value| {
        let mut v = v;
        v["type"] = json!(kind);
        Message::Text(v.to_string().into())
    };
    let first = msg("state", serde_json::to_value(&*state.borrow_and_update()).unwrap());
    if tx.send(first).await.is_err() {
        return;
    }
    loop {
        let out = tokio::select! {
            r = state.changed() => {
                if r.is_err() { break }
                msg("state", serde_json::to_value(&*state.borrow_and_update()).unwrap())
            }
            r = stats.changed() => {
                if r.is_err() { break }
                msg("stats", serde_json::to_value(&*stats.borrow_and_update()).unwrap())
            }
            r = frames.changed(), if viewer.is_some() => {
                if r.is_err() { break }
                let Some(p) = frames.borrow_and_update().clone() else { continue };
                Message::Binary(p.jpeg.clone())
            }
            m = rx.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t)
                        && v["type"] == "preview"
                    {
                        viewer = v["enabled"].as_bool().unwrap_or(false).then(|| preview.viewer());
                        frames.mark_changed();
                    }
                    continue;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => continue,
            },
        };
        if tx.send(out).await.is_err() {
            break;
        }
    }
}

async fn static_files(State(s): State<AppState>, req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match &s.web {
        WebUi::Disabled => (StatusCode::NOT_FOUND, "web UI disabled (start with --web-ui)").into_response(),
        WebUi::Dir(dir) => {
            let safe = !path.split('/').any(|c| c == ".." || c.contains('\\'));
            let file = dir.join(path);
            let file = if safe && file.is_file() { file } else { dir.join("index.html") };
            match tokio::fs::read(&file).await {
                Ok(b) => asset_response(&file.to_string_lossy(), b),
                Err(_) => (StatusCode::NOT_FOUND, "web UI not built").into_response(),
            }
        }
        #[cfg(feature = "embed-ui")]
        WebUi::Embedded => match crate::Assets::get(path).or_else(|| crate::Assets::get("index.html")) {
            Some(f) => asset_response(path, f.data.into_owned()),
            None => (StatusCode::NOT_FOUND, "web UI not embedded").into_response(),
        },
    }
}

fn asset_response(name: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let cache = if name.contains("/assets/") || name.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    ([(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, cache.to_string())], bytes).into_response()
}
