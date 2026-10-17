//! HTTP / WebSocket API.

use crate::auth::{self, Role};
use crate::captures;
use crate::cast;
use crate::geometry::{NormRect, display_to_frame};
use crate::{AppState, WebUi};
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::Extension;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
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
        .route("/record", get(record_get))
        .route("/record/start", post(record_start))
        .route("/record/stop", post(record_stop))
        .route("/white-balance", post(white_balance))
        .route("/roi", post(set_roi))
        .route("/update", get(update_status))
        .route("/update/check", post(update_check))
        .route("/update/install", post(update_install))
        .route("/calibration", get(calibration_get))
        .route("/calibration/run", post(calibration_run))
        .route("/calibration/cancel", post(calibration_cancel))
        .route("/calibration/{section}", axum::routing::delete(calibration_delete))
        .route(
            "/display",
            get(display_get).put(display_put).post(display_put).delete(display_clear).layer(DefaultBodyLimit::max(cast::MAX_BYTES)),
        )
        .route("/display/content", get(display_content))
        .route("/display/events", get(display_events))
        .route("/access", get(access_get).patch(access_patch))
        .route("/access/logout-all", post(logout_all))
        .route("/setup", post(setup))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/keys", get(keys_list).post(key_create))
        .route("/keys/{id}", axum::routing::delete(key_delete))
        .route("/ws", get(ws));
    Router::new()
        .nest("/api", api)
        .route("/stream.mjpg", get(mjpeg))
        .route("/snapshot.jpg", get(snapshot))
        .fallback(static_files)
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}

/// The caller's access, worked out by [`auth`] and available to handlers as an
/// extension. Handlers that need more than viewing take [`Operator`] or [`Admin`].
#[derive(Debug, Clone, Copy)]
pub struct Caller(pub auth::Resolved);

/// Rejects callers below `role`.
fn require(resolved: Option<auth::Resolved>, role: Role) -> Result<(), ApiError> {
    let caller = resolved.and_then(|c| c.role);
    if caller >= Some(role) {
        return Ok(());
    }
    let msg = match role {
        Role::Admin if resolved.is_some_and(|c| c.locked) => "the configuration is locked; change it with `loupecam access` on the server",
        Role::Admin => "admin password required",
        _ if caller.is_some() => "operator access required",
        _ => "login required",
    };
    Err(ApiError(if caller.is_some() { StatusCode::FORBIDDEN } else { StatusCode::UNAUTHORIZED }, msg.into()))
}

/// Extractor: the caller may operate the camera.
pub struct Operator;
/// Extractor: the caller is an admin.
pub struct Admin;

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Operator {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, ApiError> {
        require(parts.extensions.get::<Caller>().map(|c| c.0), Role::Operate).map(|()| Operator)
    }
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Admin {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, ApiError> {
        require(parts.extensions.get::<Caller>().map(|c| c.0), Role::Admin).map(|()| Admin)
    }
}

fn cookie<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|c| c.split(';'))
        .find_map(|kv| kv.trim().strip_prefix(name)?.strip_prefix('='))
}

/// Works out who's calling (see [`crate::auth`]) and enforces the baseline: `view` for
/// the API and streams, and setup first on an unconfigured server. Browsers can't set
/// headers on `<img>`/WebSocket requests, so keys are also accepted as `?token=` and
/// as the `loupecam_token` cookie.
///
/// Casting (`/api/display…`) has its own rule: open to anyone unless a display token is
/// set, in which case it, or an operator's credentials, are required.
async fn auth(State(s): State<AppState>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let h = req.headers();
    let bearer = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    let query = req.uri().query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")));
    let creds = auth::Credentials { tokens: [bearer, query, cookie(h, "loupecam_token")], session: cookie(h, auth::SESSION_COOKIE) };
    let caller = s.auth.resolve(&creds);
    let is_cast = path == "/api/display" || path.starts_with("/api/display/");
    // The UI shell is public so it can run setup or ask for a login; so is the access API.
    let is_ui = !(path.starts_with("/api") || path.ends_with(".mjpg") || path.ends_with(".jpg"));
    let is_access = matches!(path, "/api/access" | "/api/setup" | "/api/login" | "/api/logout");
    if is_cast {
        if let Some(d) = s.display_token.as_deref() {
            let display = [bearer, query].into_iter().flatten().any(|t| auth::eq(t, d));
            if !display && caller.role < Some(Role::Operate) {
                return ApiError(StatusCode::UNAUTHORIZED, "display token required".into()).into_response();
            }
        }
    } else if !(is_ui || is_access) {
        if caller.setup_required {
            return ApiError(StatusCode::CONFLICT, "setup required".into()).into_response();
        }
        if caller.role.is_none() {
            return ApiError(StatusCode::UNAUTHORIZED, "login required".into()).into_response();
        }
    }
    // A session cookie rides along on any request the browser makes to this host, even
    // from another site's page (and SameSite doesn't tell ports apart). So changes made
    // with one must come from this server's own pages.
    if caller.via_session && !req.method().is_safe() && !same_origin(h) {
        return ApiError(StatusCode::FORBIDDEN, "cross-origin request refused".into()).into_response();
    }
    req.extensions_mut().insert(Caller(caller));
    next.run(req).await
}

/// The request comes from a page served by this server.
fn same_origin(h: &HeaderMap) -> bool {
    if let Some(site) = h.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        return site == "same-origin";
    }
    let origin = h.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok());
    match (origin, host) {
        (Some(o), Some(host)) => o.split_once("://").is_some_and(|(_, authority)| authority.eq_ignore_ascii_case(host)),
        _ => false,
    }
}

fn auth_error(e: auth::Error) -> ApiError {
    use auth::Error as E;
    let status = match &e {
        E::AlreadySetUp | E::SetupNotNeeded => StatusCode::CONFLICT,
        E::WrongCode | E::WrongPassword => StatusCode::UNAUTHORIZED,
        E::TooManyAttempts(_) => StatusCode::TOO_MANY_REQUESTS,
        E::Invalid(_) => StatusCode::BAD_REQUEST,
        E::Locked => StatusCode::FORBIDDEN,
        E::NoSuchKey => StatusCode::NOT_FOUND,
        E::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiError(status, e.to_string())
}

/// Run a slow (Argon2) access operation off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, auth::Error> + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?.map_err(auth_error)
}

/// The session lives on the server (which ends it after idling, see [`auth`]); the
/// cookie just outlasts it.
fn session_cookie(value: &str, max_age: u64) -> HeaderValue {
    HeaderValue::from_str(&format!("{}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}", auth::SESSION_COOKIE)).unwrap()
}

/// The access info as `caller` sees it, setting (or, with `clear`, removing) the
/// session cookie.
fn access_response(s: &AppState, caller: auth::Resolved, session: Option<auth::NewSession>, clear: bool) -> Response {
    let (cookie, caller) = match &session {
        Some(n) => (Some(session_cookie(&n.cookie, 400 * 24 * 3600)), auth::Resolved { role: Some(n.role), via_session: true, setup_required: false, locked: false }),
        None if clear => (Some(session_cookie("", 0)), s.auth.resolve(&Default::default())),
        None => (None, caller),
    };
    let mut r = Json(s.auth.info(&caller)).into_response();
    if let Some(c) = cookie {
        r.headers_mut().insert(header::SET_COOKIE, c);
    }
    r
}

/// `GET /api/access`: whether setup is needed, the levels, and the caller's role.
async fn access_get(State(s): State<AppState>, Extension(c): Extension<Caller>) -> Json<auth::Info> {
    Json(s.auth.info(&c.0))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupBody {
    #[serde(default)]
    code: Option<String>,
    #[serde(flatten)]
    levels: auth::Levels,
}

/// `POST /api/setup`: first-run setup. Logs the caller in when the levels need it.
async fn setup(
    State(s): State<AppState>,
    Extension(c): Extension<Caller>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(b): Json<SetupBody>,
) -> ApiResult<Response> {
    let a = s.auth.clone();
    let session = blocking(move || a.setup(peer.ip(), b.code.as_deref(), b.levels)).await?;
    // Re-resolve: with open levels the caller's role comes from the new levels.
    let caller = if session.is_none() { s.auth.resolve(&Default::default()) } else { c.0 };
    Ok(access_response(&s, caller, session, false))
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

/// `POST /api/login {password}`: the admin password gives `admin`, the access password
/// `operate`.
async fn login(
    State(s): State<AppState>,
    Extension(c): Extension<Caller>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(b): Json<LoginBody>,
) -> ApiResult<Response> {
    let a = s.auth.clone();
    let session = blocking(move || a.login(peer.ip(), &b.password)).await?;
    Ok(access_response(&s, c.0, Some(session), false))
}

async fn logout(State(s): State<AppState>, Extension(c): Extension<Caller>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(cookie) = cookie(&headers, auth::SESSION_COOKIE) {
        s.auth.logout(cookie).map_err(auth_error)?;
    }
    Ok(access_response(&s, c.0, None, true))
}

/// `PATCH /api/access`: change the levels or passwords. Ends every session; the caller
/// gets a new one when the new levels need it.
async fn access_patch(_: Admin, Extension(c): Extension<Caller>, State(s): State<AppState>, Json(levels): Json<auth::Levels>) -> ApiResult<Response> {
    let a = s.auth.clone();
    let session = blocking(move || a.change(levels)).await?;
    let clear = session.is_none() && c.0.via_session;
    Ok(access_response(&s, s.auth.resolve(&Default::default()), session, clear))
}

/// `POST /api/access/logout-all`: end every login session, the caller's included.
async fn logout_all(_: Admin, Extension(c): Extension<Caller>, State(s): State<AppState>) -> ApiResult<Response> {
    s.auth.logout_all().map_err(auth_error)?;
    Ok(access_response(&s, c.0, None, true))
}

async fn keys_list(_: Admin, State(s): State<AppState>) -> Json<Vec<auth::KeyInfo>> {
    Json(s.auth.keys())
}

#[derive(Deserialize)]
struct NewKey {
    name: String,
    role: Role,
}

/// `POST /api/keys {name, role}`: the response holds the key, which isn't shown again.
async fn key_create(_: Admin, State(s): State<AppState>, Json(b): Json<NewKey>) -> ApiResult<Json<serde_json::Value>> {
    let (key, info) = s.auth.create_key(&b.name, b.role).map_err(auth_error)?;
    Ok(Json(json!({ "key": key, "info": info })))
}

async fn key_delete(_: Admin, State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.auth.delete_key(&id).map_err(auth_error)?;
    Ok(StatusCode::NO_CONTENT)
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

async fn patch_settings(
    _: Operator,
    Extension(c): Extension<Caller>,
    State(s): State<AppState>,
    Json(patch): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    // Updates and calibration are configuration: admins only.
    if ["updates", "calibration"].iter().any(|k| patch.get(k).is_some()) {
        require(Some(c.0), Role::Admin)?;
    }
    let settings = s.service.patch(patch).await.map_err(bad)?;
    s.persist(&settings);
    Ok(Json(serde_json::to_value(settings).unwrap()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureQuery {
    full_resolution: Option<bool>,
}

async fn capture(_: Operator, State(s): State<AppState>, Query(q): Query<CaptureQuery>) -> ApiResult<Json<captures::CaptureInfo>> {
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

/// A saved capture, with range requests (so videos can be seeked while streaming).
async fn get_capture(State(s): State<AppState>, Path(name): Path<String>, req: Request) -> ApiResult<Response> {
    use tower::ServiceExt;
    let path = captures::resolve(&s.captures_dir, &name).ok_or(ApiError(StatusCode::NOT_FOUND, "no such capture".into()))?;
    let r = tower_http::services::ServeFile::new(path).oneshot(req).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(r.map(Body::new))
}

async fn delete_capture(_: Admin, State(s): State<AppState>, Path(name): Path<String>) -> ApiResult<StatusCode> {
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
    let o = settings.develop_params_uncalibrated(env.model, settings.preview.demosaic).orientation;
    Some((o, env.raw.width, env.raw.height))
}

async fn white_balance(_: Operator, State(s): State<AppState>, Json(b): Json<RegionBody>) -> ApiResult<Json<serde_json::Value>> {
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
async fn set_roi(_: Operator, State(s): State<AppState>, Json(b): Json<RegionBody>) -> ApiResult<Json<serde_json::Value>> {
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

async fn update_status(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(&*s.updates.status.borrow()).unwrap())
}

async fn update_check(_: Operator, State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(s.updates.check().await).unwrap())
}

/// Install the available update; the server restarts shortly after responding.
async fn update_install(_: Admin, State(s): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let version = s.updates.install().await.map_err(bad)?;
    Ok(Json(json!({ "installed": version, "restarting": true })))
}

async fn calibration_get(State(s): State<AppState>) -> Json<serde_json::Value> {
    let profile = s.service.shared.calibration.borrow().profile.clone();
    Json(json!({
        "profile": profile,
        "status": &*s.calibration.status.borrow(),
        "targetConnected": s.calibration.target.is_connected(),
    }))
}

async fn calibration_run(
    _: Admin,
    State(s): State<AppState>,
    Json(req): Json<crate::calibration::runner::StepRequest>,
) -> ApiResult<StatusCode> {
    s.calibration.start(req).map_err(|e| ApiError(StatusCode::CONFLICT, e))?;
    Ok(StatusCode::ACCEPTED)
}

async fn calibration_cancel(_: Admin, State(s): State<AppState>) -> StatusCode {
    s.calibration.cancel();
    StatusCode::NO_CONTENT
}

/// Forget a calibration section: `dark`, `gain`, `color`, `flatField-<size>`, or
/// `scale-<name>`.
async fn calibration_delete(_: Admin, State(s): State<AppState>, Path(section): Path<String>) -> ApiResult<StatusCode> {
    let serial = s.service.shared.state.borrow().device.as_ref().map(|d| d.serial.clone()).ok_or_else(|| unavailable("no camera connected"))?;
    let mut p = crate::calibration::load(&serial);
    match section.as_str() {
        "dark" => p.dark = None,
        "gain" => p.gain = None,
        "color" => p.color = None,
        s if s.starts_with("flatField-") => {
            let size: usize = s["flatField-".len()..].parse().map_err(bad)?;
            p.flat_field.remove(&size);
        }
        s if s.starts_with("scale-") => {
            let name = &s["scale-".len()..];
            p.scale.retain(|q| q.name != name);
        }
        _ => return Err(ApiError(StatusCode::NOT_FOUND, "unknown calibration section".into())),
    }
    crate::calibration::save(&p).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    s.service.reload_calibration();
    Ok(StatusCode::NO_CONTENT)
}

async fn record_get(State(s): State<AppState>) -> Json<serde_json::Value> {
    let current = s.service.shared.state.borrow().settings.record.clone();
    let presets: serde_json::Map<_, _> = crate::settings::RecordPreset::ALL
        .iter()
        .map(|p| (serde_json::to_value(p).unwrap().as_str().unwrap().to_string(), serde_json::to_value(p.settings(&current)).unwrap()))
        .collect();
    Json(json!({ "status": &*s.recorder.status.borrow(), "presets": presets }))
}

#[derive(Deserialize, Default)]
struct RecordStart {
    /// Switch the record settings to this preset first.
    preset: Option<crate::settings::RecordPreset>,
}

/// Start recording with the `record` settings (optionally switching to a preset first).
async fn record_start(_: Operator, State(s): State<AppState>, body: Option<Json<RecordStart>>) -> ApiResult<Json<crate::record::Recording>> {
    if let Some(p) = body.and_then(|Json(b)| b.preset) {
        let current = s.service.shared.state.borrow().settings.record.clone();
        let settings = s.service.patch(json!({ "record": p.settings(&current) })).await.map_err(bad)?;
        s.persist(&settings);
    }
    s.recorder.start().await.map(Json).map_err(|e| ApiError(StatusCode::CONFLICT, e))
}

/// Stop recording; returns the finished recording (204 if nothing was recording).
async fn record_stop(_: Operator, State(s): State<AppState>) -> Response {
    match s.recorder.stop().await {
        Some(r) => Json(r).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn display_get(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "cast": s.casting.info() }))
}

#[derive(Deserialize)]
struct CastQuery {
    name: Option<String>,
}

#[derive(Deserialize)]
struct CastUrl {
    url: String,
}

/// Cast to the display: a file as the body (its type is detected from the content), or
/// a web page as `{"url": "…"}` (or a `text/uri-list` body).
async fn display_put(State(s): State<AppState>, Query(q): Query<CastQuery>, headers: HeaderMap, body: Bytes) -> ApiResult<Json<cast::CastInfo>> {
    let content_type = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let base = content_type.map(|t| t.split(';').next().unwrap_or("").trim().to_ascii_lowercase());
    let info = match base.as_deref() {
        Some("application/json") => {
            let u: CastUrl = serde_json::from_slice(&body).map_err(bad)?;
            s.casting.cast_url(&u.url)
        }
        Some("text/uri-list") => {
            let text = std::str::from_utf8(&body).map_err(bad)?;
            let url = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or("");
            s.casting.cast_url(url)
        }
        _ => s.casting.cast_file(body, content_type, q.name),
    }
    .map_err(|e| match e {
        cast::CastError::Unsupported => ApiError(StatusCode::UNSUPPORTED_MEDIA_TYPE, e.to_string()),
        e => bad(e),
    })?;
    tracing::info!(id = info.id, kind = ?info.kind, bytes = info.bytes, "casting");
    Ok(Json(info))
}

async fn display_clear(State(s): State<AppState>) -> StatusCode {
    if s.casting.clear() {
        tracing::info!("cast cleared");
    }
    StatusCode::NO_CONTENT
}

async fn display_content(State(s): State<AppState>) -> ApiResult<Response> {
    let cast = s.casting.current.borrow().clone();
    let cast = cast.filter(|c| c.info.kind != cast::CastKind::Url).ok_or(ApiError(StatusCode::NOT_FOUND, "no file is being cast".into()))?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&cast.info.mime).map_err(bad)?);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    // Cast documents are arbitrary content from the network: scripts in them must not run
    // with this origin (and so with anyone's camera-token cookie).
    if matches!(cast.info.kind, cast::CastKind::Html | cast::CastKind::Svg) {
        headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox allow-scripts"));
    }
    Ok((headers, cast.content).into_response())
}

/// Server-sent events: a `display` event with `{"cast": …}` now and on every change.
/// For displays and casting tools that don't hold the camera token (the WebSocket
/// needs it).
async fn display_events(State(s): State<AppState>) -> impl IntoResponse {
    use axum::response::sse::{Event, KeepAlive, Sse};
    let rx = s.casting.current.subscribe();
    let stream = tokio_stream::wrappers::WatchStream::new(rx).map(|c| {
        let info = c.map(|c| c.info);
        Ok::<_, std::convert::Infallible>(Event::default().event("display").data(json!({ "cast": info }).to_string()))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
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
    upgrade.on_upgrade(move |socket| ws_session(socket, s))
}

/// Keeps a WebSocket registered as a screen target while alive.
struct TargetGuard(Arc<crate::calibration::target::TargetHub>);

impl Drop for TargetGuard {
    fn drop(&mut self) {
        self.0.connected.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// Server → client: `{"type":"state",…}` on change, `{"type":"stats",…}` ~5×/s,
/// `{"type":"update",…}`, `{"type":"calibration",…}`, `{"type":"record",…}`,
/// `{"type":"display","cast":…}`
/// (see [`display_get`]) and, after the client sends
/// `{"type":"preview","enabled":true}`, binary JPEG frames. A client that sends
/// `{"type":"target"}` becomes a screen target: it receives
/// `{"type":"targetShow","id":n,"pattern":{…}}` and answers `{"type":"targetAck","id":n}`.
async fn ws_session(socket: WebSocket, app: AppState) {
    let (service, preview) = (app.service.clone(), app.preview.clone());
    let hub = app.calibration.target.clone();
    let (mut tx, mut rx) = socket.split();
    let mut state = service.shared.state.subscribe();
    let mut stats = service.shared.stats.subscribe();
    let mut frames = preview.frames.subscribe();
    let mut update = app.updates.status.subscribe();
    update.mark_changed();
    let mut calib = app.calibration.status.subscribe();
    calib.mark_changed();
    let mut show = hub.show.subscribe();
    let mut casting = app.casting.current.subscribe();
    casting.mark_changed();
    let mut record = app.recorder.status.subscribe();
    record.mark_changed();
    let mut target: Option<TargetGuard> = None;
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
            r = update.changed() => {
                if r.is_err() { break }
                msg("update", serde_json::to_value(&*update.borrow_and_update()).unwrap())
            }
            r = stats.changed() => {
                if r.is_err() { break }
                msg("stats", serde_json::to_value(&*stats.borrow_and_update()).unwrap())
            }
            r = calib.changed() => {
                if r.is_err() { break }
                msg("calibration", serde_json::to_value(&*calib.borrow_and_update()).unwrap())
            }
            r = record.changed() => {
                if r.is_err() { break }
                msg("record", serde_json::to_value(&*record.borrow_and_update()).unwrap())
            }
            r = casting.changed() => {
                if r.is_err() { break }
                let info = casting.borrow_and_update().as_ref().map(|c| c.info.clone());
                msg("display", json!({ "cast": info }))
            }
            r = show.changed(), if target.is_some() => {
                if r.is_err() { break }
                let (id, pattern) = show.borrow_and_update().clone();
                msg("targetShow", json!({ "id": id, "pattern": pattern }))
            }
            r = frames.changed(), if viewer.is_some() => {
                if r.is_err() { break }
                let Some(p) = frames.borrow_and_update().clone() else { continue };
                Message::Binary(p.jpeg.clone())
            }
            m = rx.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
                    match v["type"].as_str() {
                        Some("preview") => {
                            viewer = v["enabled"].as_bool().unwrap_or(false).then(|| preview.viewer());
                            frames.mark_changed();
                        }
                        Some("target") if target.is_none() => {
                            hub.connected.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                            target = Some(TargetGuard(hub.clone()));
                            show.mark_changed();
                        }
                        Some("targetAck") => {
                            if let Some(id) = v["id"].as_u64() {
                                hub.ack.send_if_modified(|a| {
                                    let newer = id > *a;
                                    if newer {
                                        *a = id;
                                    }
                                    newer
                                });
                            }
                        }
                        _ => {}
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
