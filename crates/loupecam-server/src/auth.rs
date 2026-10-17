//! Access control: who may use the server, and how they prove it.
//!
//! Two independent settings, chosen in the first-run setup and changeable later by an
//! admin:
//!
//! - [`Access`]: who may view and operate the camera (anyone, or with a password).
//! - [`ConfigLock`]: who may change the configuration (anyone, whoever has the admin
//!   password, or nobody over the network).
//!
//! A caller's [`Role`] comes from the level (anonymous callers), a session cookie (a
//! password login) or an API key (`Authorization: Bearer …` or `?token=`). Secrets are
//! stored hashed in `auth.json`, next to the settings and never inside them.
//!
//! A server that only listens on loopback needs no setup and is open. One that listens
//! on the network starts unconfigured, and stays unusable until someone completes setup
//! (with the setup code, if one was set).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Cookie holding a login session.
pub const SESSION_COOKIE: &str = "loupecam_session";

/// What a caller may do. Ordered: each role includes the ones before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// Watch: state, stats, streams, captures.
    View,
    /// Use the camera: capture, record, change camera settings.
    Operate,
    /// Everything, including access control, updates and calibration.
    Admin,
}

/// Who may view and operate the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Access {
    /// Anyone who can reach the server.
    Open,
    /// Only after logging in with the access (or admin) password.
    Password,
}

/// Who may change the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigLock {
    /// Anyone with access.
    Open,
    /// Only after logging in with the admin password.
    Password,
    /// Nobody over the network; only `loupecam access` on the host.
    Locked,
}

/// Contents of `auth.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    access: Access,
    config: ConfigLock,
    /// Argon2id PHC strings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admin_password: Option<String>,
    #[serde(default)]
    sessions: Vec<Session>,
    #[serde(default)]
    keys: Vec<StoredKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    /// SHA-256 of the cookie value.
    hash: String,
    role: Role,
    /// Unix seconds.
    created: i64,
    last_used: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredKey {
    id: String,
    name: String,
    role: Role,
    /// SHA-256 of the key.
    hash: String,
    created: i64,
    #[serde(default)]
    last_used: Option<i64>,
}

/// An API key as shown to admins (the key itself is only shown when created).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub created: i64,
    pub last_used: Option<i64>,
}

impl From<&StoredKey> for KeyInfo {
    fn from(k: &StoredKey) -> Self {
        KeyInfo { id: k.id.clone(), name: k.name.clone(), role: k.role, created: k.created, last_used: k.last_used }
    }
}

impl Session {
    /// Idle time after which the session ends. Admin sessions are kept short.
    fn lifetime(&self) -> i64 {
        match self.role {
            Role::Admin => 12 * 3600,
            _ => 30 * 24 * 3600,
        }
    }
}

#[derive(Debug)]
enum Mode {
    /// Listening on loopback only, nothing configured: open to the local machine.
    Local,
    /// Only `--token` given: it grants everything, nothing else does.
    Token,
    /// Waiting for first-run setup.
    Unconfigured,
    Configured(Stored),
}

/// Whether first-run setup is still to be done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SetupState {
    Required,
    Done,
    /// Loopback-only server, or access set by `--token`.
    NotNeeded,
}

/// The server's access settings as reported to clients.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub setup: SetupState,
    /// Setup asks for the code printed in the server's log.
    pub setup_code_required: bool,
    /// `open`, `password`, or `token` (only `--token` grants access).
    pub access: &'static str,
    pub config: ConfigLock,
    /// The caller's role (`null`: must log in).
    pub role: Option<Role>,
    /// Whether the caller is logged in with a session (and can log out).
    pub session: bool,
}

/// Credentials found on a request.
#[derive(Debug, Default, Clone, Copy)]
pub struct Credentials<'a> {
    /// Bearer header, `?token=`, or the `loupecam_token` cookie: an API key or the
    /// `--token`.
    pub tokens: [Option<&'a str>; 3],
    /// The session cookie.
    pub session: Option<&'a str>,
}

/// The outcome of checking a request's credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    /// `None`: no access (log in first).
    pub role: Option<Role>,
    /// The role came from the session cookie (so the request needs a CSRF check).
    pub via_session: bool,
    pub setup_required: bool,
    /// The configuration is locked: nobody is admin over the network.
    pub locked: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("setup is already done")]
    AlreadySetUp,
    #[error("setup isn't available on this server")]
    SetupNotNeeded,
    #[error("wrong setup code")]
    WrongCode,
    #[error("wrong password")]
    WrongPassword,
    #[error("too many attempts; try again in {} s", .0.as_secs().max(1))]
    TooManyAttempts(Duration),
    #[error("{0}")]
    Invalid(String),
    #[error("configuration is locked; change it with `loupecam access` on the server")]
    Locked,
    #[error("no such key")]
    NoSuchKey,
    #[error("saving access settings: {0}")]
    Io(#[from] std::io::Error),
}

/// First-run setup, or a later change by an admin.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Levels {
    pub access: Access,
    pub config: ConfigLock,
    /// Required with `access: password`, unless one is already set (changes).
    #[serde(default)]
    pub access_password: Option<String>,
    /// Required with `config: password`, unless one is already set (changes).
    #[serde(default)]
    pub admin_password: Option<String>,
}

/// A new login session: the cookie value and the role it carries.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub cookie: String,
    pub role: Role,
}

struct Inner {
    mode: Mode,
    /// Modification time of `auth.json` when last loaded or saved.
    mtime: Option<SystemTime>,
    last_check: Instant,
    /// Session `last_used` times changed since the last save.
    dirty: bool,
    last_save: Instant,
    /// Failed password / setup-code attempts per client.
    failures: HashMap<IpAddr, (u32, Instant)>,
}

/// The server's access control.
pub struct Auth {
    file: Option<PathBuf>,
    network: bool,
    token: Option<String>,
    kiosk_key: Option<String>,
    setup_code: Option<String>,
    inner: Mutex<Inner>,
}

impl Auth {
    /// `file`: where `auth.json` lives (`None`: keep everything in memory). `network`:
    /// the server listens beyond loopback. `token`: `--token`, a full-access key.
    pub fn new(file: Option<PathBuf>, network: bool, token: Option<String>, setup_code: Option<String>) -> Auth {
        let auth = Auth {
            file,
            network,
            token,
            kiosk_key: None,
            setup_code,
            inner: Mutex::new(Inner {
                mode: Mode::Unconfigured,
                mtime: None,
                last_check: Instant::now(),
                dirty: false,
                last_save: Instant::now(),
                failures: HashMap::new(),
            }),
        };
        let mut inner = auth.inner.lock().unwrap();
        auth.load(&mut inner);
        drop(inner);
        auth
    }

    /// Also accept `key` as an operator key, whatever the levels (for the kiosk on the
    /// server's own display). Unlike `--token`, it doesn't replace setup.
    pub fn with_kiosk_key(mut self, key: Option<String>) -> Auth {
        self.kiosk_key = key.filter(|k| !k.is_empty());
        self
    }

    /// The setup code, if setup needs one.
    pub fn setup_code(&self) -> Option<&str> {
        self.setup_code.as_deref()
    }

    fn unconfigured_mode(&self) -> Mode {
        if self.token.is_some() {
            Mode::Token
        } else if self.network {
            Mode::Unconfigured
        } else {
            Mode::Local
        }
    }

    /// (Re)load `auth.json`.
    fn load(&self, inner: &mut Inner) {
        inner.mtime = None;
        inner.mode = self.unconfigured_mode();
        let Some(path) = &self.file else { return };
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                inner.mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
                deny(inner, path, &e);
                return;
            }
        };
        inner.mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        match serde_json::from_slice::<Stored>(&bytes) {
            Ok(s) => inner.mode = Mode::Configured(s),
            Err(e) => deny(inner, path, &e),
        }
    }

    /// Pick up changes made by `loupecam access` (checked at most once a second).
    fn refresh(&self, inner: &mut Inner) {
        if inner.last_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        inner.last_check = Instant::now();
        let Some(path) = &self.file else { return };
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if mtime != inner.mtime {
            tracing::info!("access settings changed on disk; reloading");
            self.load(inner);
        } else if inner.dirty && inner.last_save.elapsed() > Duration::from_secs(600) {
            let _ = self.save(inner);
        }
    }

    fn save(&self, inner: &mut Inner) -> std::io::Result<()> {
        inner.dirty = false;
        inner.last_save = Instant::now();
        let (Some(path), Mode::Configured(stored)) = (&self.file, &inner.mode) else { return Ok(()) };
        let bytes = serde_json::to_vec_pretty(stored).map_err(std::io::Error::other)?;
        write_private(path, &bytes)?;
        inner.mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        Ok(())
    }

    /// Work out a request's role.
    pub fn resolve(&self, c: &Credentials) -> Resolved {
        let mut inner = self.inner.lock().unwrap();
        self.refresh(&mut inner);
        let now = now();
        let tokens = || c.tokens.iter().flatten();
        let mut resolved = Resolved { role: None, via_session: false, setup_required: false, locked: false };
        match &mut inner.mode {
            Mode::Local => resolved.role = Some(Role::Admin),
            Mode::Token => {}
            Mode::Unconfigured => resolved.setup_required = true,
            Mode::Configured(s) => {
                resolved.role = match (s.access, s.config) {
                    (Access::Password, _) => None,
                    (Access::Open, ConfigLock::Open) => Some(Role::Admin),
                    (Access::Open, _) => Some(Role::Operate),
                };
                let mut used = false;
                if let Some(cookie) = c.session {
                    let h = sha256_hex(cookie);
                    s.sessions.retain(|x| now - x.last_used < x.lifetime());
                    if let Some(x) = s.sessions.iter_mut().find(|x| eq(&x.hash, &h)) {
                        if x.role >= resolved.role.unwrap_or(Role::View) {
                            resolved.role = Some(x.role);
                            resolved.via_session = true;
                        }
                        used |= now - x.last_used > 60;
                        x.last_used = now;
                    }
                }
                for t in tokens() {
                    let h = sha256_hex(t);
                    if let Some(k) = s.keys.iter_mut().find(|k| eq(&k.hash, &h)) {
                        if Some(k.role) > resolved.role {
                            resolved.role = Some(k.role);
                            resolved.via_session = false;
                        }
                        used |= k.last_used.is_none_or(|t| now - t > 60);
                        k.last_used = Some(now);
                    }
                }
                if s.config == ConfigLock::Locked {
                    resolved.locked = true;
                    resolved.role = resolved.role.map(|r| r.min(Role::Operate));
                }
                inner.dirty |= used;
            }
        }
        if let Some(t) = &self.token
            && tokens().any(|x| eq(x, t))
        {
            resolved.role = Some(Role::Admin);
            resolved.via_session = false;
        }
        if let Some(k) = &self.kiosk_key
            && resolved.role < Some(Role::Operate)
            && tokens().any(|x| eq(x, k))
        {
            resolved.role = Some(Role::Operate);
            resolved.via_session = false;
        }
        resolved
    }

    /// The access settings, for `caller`.
    pub fn info(&self, caller: &Resolved) -> Info {
        let mut inner = self.inner.lock().unwrap();
        self.refresh(&mut inner);
        let (setup, access, config) = match &inner.mode {
            Mode::Local => (SetupState::NotNeeded, "open", ConfigLock::Open),
            Mode::Token => (SetupState::NotNeeded, "token", ConfigLock::Password),
            Mode::Unconfigured => (SetupState::Required, "open", ConfigLock::Open),
            Mode::Configured(s) => (
                SetupState::Done,
                match s.access {
                    Access::Open => "open",
                    Access::Password => "password",
                },
                s.config,
            ),
        };
        Info {
            setup,
            setup_code_required: setup == SetupState::Required && self.setup_code.is_some(),
            access,
            config,
            role: caller.role,
            session: caller.via_session,
        }
    }

    /// The `auth` value announced over mDNS: `setup`, `none`, `password` or `token`.
    pub fn announced(&self) -> &'static str {
        let inner = self.inner.lock().unwrap();
        match &inner.mode {
            Mode::Local => "none",
            Mode::Token => "token",
            Mode::Unconfigured => "setup",
            Mode::Configured(s) => match s.access {
                Access::Open => "none",
                Access::Password => "password",
            },
        }
    }

    fn check_backoff(&self, ip: IpAddr) -> Result<(), Error> {
        let inner = self.inner.lock().unwrap();
        if let Some(&(n, last)) = inner.failures.get(&ip)
            && n >= 5
        {
            // 30 s after the fifth failure, doubling, at most 15 minutes.
            let wait = Duration::from_secs((30u64 << (n - 5).min(5)).min(900));
            let since = last.elapsed();
            if since < wait {
                return Err(Error::TooManyAttempts(wait - since));
            }
        }
        Ok(())
    }

    fn record(&self, ip: IpAddr, ok: bool) {
        let mut inner = self.inner.lock().unwrap();
        if ok {
            inner.failures.remove(&ip);
        } else {
            let e = inner.failures.entry(ip).or_insert((0, Instant::now()));
            // Forget failures after an hour of quiet.
            if e.1.elapsed() > Duration::from_secs(3600) {
                e.0 = 0;
            }
            e.0 += 1;
            e.1 = Instant::now();
        }
        if inner.failures.len() > 10_000 {
            inner.failures.retain(|_, (_, t)| t.elapsed() < Duration::from_secs(3600));
        }
    }

    /// Log in with the admin or access password. Slow (Argon2): call from a blocking
    /// thread.
    pub fn login(&self, ip: IpAddr, password: &str) -> Result<NewSession, Error> {
        self.check_backoff(ip)?;
        let (admin, access, locked) = {
            let mut inner = self.inner.lock().unwrap();
            self.refresh(&mut inner);
            match &inner.mode {
                Mode::Configured(s) => (s.admin_password.clone(), s.access_password.clone(), s.config == ConfigLock::Locked),
                _ => (None, None, false),
            }
        };
        let role = if !locked && admin.as_deref().is_some_and(|h| verify(password, h)) {
            Some(Role::Admin)
        } else if access.as_deref().is_some_and(|h| verify(password, h)) {
            Some(Role::Operate)
        } else {
            None
        };
        self.record(ip, role.is_some());
        let role = role.ok_or(Error::WrongPassword)?;
        self.new_session(role)
    }

    fn new_session(&self, role: Role) -> Result<NewSession, Error> {
        let cookie = random_hex(32);
        let mut inner = self.inner.lock().unwrap();
        let Mode::Configured(s) = &mut inner.mode else { return Err(Error::Invalid("no passwords are set".into())) };
        let t = now();
        s.sessions.push(Session { hash: sha256_hex(&cookie), role, created: t, last_used: t });
        self.save(&mut inner)?;
        Ok(NewSession { cookie, role })
    }

    /// End the session with this cookie.
    pub fn logout(&self, cookie: &str) -> Result<(), Error> {
        let h = sha256_hex(cookie);
        let mut inner = self.inner.lock().unwrap();
        if let Mode::Configured(s) = &mut inner.mode {
            s.sessions.retain(|x| !eq(&x.hash, &h));
        }
        Ok(self.save(&mut inner)?)
    }

    /// Complete first-run setup. Returns a session for whoever did it, when the chosen
    /// levels need one. Slow (Argon2): call from a blocking thread.
    pub fn setup(&self, ip: IpAddr, code: Option<&str>, levels: Levels) -> Result<Option<NewSession>, Error> {
        {
            let inner = self.inner.lock().unwrap();
            match inner.mode {
                Mode::Unconfigured => {}
                Mode::Configured(_) => return Err(Error::AlreadySetUp),
                Mode::Local | Mode::Token => return Err(Error::SetupNotNeeded),
            }
        }
        if let Some(want) = &self.setup_code {
            self.check_backoff(ip)?;
            let ok = code.is_some_and(|c| eq(&normalize_code(c), &normalize_code(want)));
            self.record(ip, ok);
            if !ok {
                return Err(Error::WrongCode);
            }
        }
        let stored = build(levels, None)?;
        let role = session_role(&stored);
        {
            let mut inner = self.inner.lock().unwrap();
            // Two setups racing: the first one wins.
            if !matches!(inner.mode, Mode::Unconfigured) {
                return Err(Error::AlreadySetUp);
            }
            inner.mode = Mode::Configured(stored);
            self.save(&mut inner)?;
        }
        tracing::info!("first-run setup completed");
        role.map(|r| self.new_session(r)).transpose()
    }

    /// Change the levels or passwords (an admin). Ends every session, and returns a
    /// new one for the caller when the new levels need one. Slow (Argon2).
    pub fn change(&self, levels: Levels) -> Result<Option<NewSession>, Error> {
        let old = {
            let inner = self.inner.lock().unwrap();
            match &inner.mode {
                Mode::Configured(s) => s.clone(),
                _ => return Err(Error::SetupNotNeeded),
            }
        };
        if old.config == ConfigLock::Locked {
            return Err(Error::Locked);
        }
        let stored = build(levels, Some(&old))?;
        let role = session_role(&stored);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.mode = Mode::Configured(stored);
            self.save(&mut inner)?;
        }
        tracing::info!("access settings changed");
        role.map(|r| self.new_session(r)).transpose()
    }

    /// Change the levels on the host (`loupecam access set`), even when locked.
    pub fn change_local(&self, levels: Levels) -> Result<(), Error> {
        let old = {
            let inner = self.inner.lock().unwrap();
            match &inner.mode {
                Mode::Configured(s) => Some(s.clone()),
                _ => None,
            }
        };
        let stored = build(levels, old.as_ref())?;
        let mut inner = self.inner.lock().unwrap();
        inner.mode = Mode::Configured(stored);
        Ok(self.save(&mut inner)?)
    }

    pub fn keys(&self) -> Vec<KeyInfo> {
        let mut inner = self.inner.lock().unwrap();
        self.refresh(&mut inner);
        match &inner.mode {
            Mode::Configured(s) => s.keys.iter().map(KeyInfo::from).collect(),
            _ => vec![],
        }
    }

    /// Create an API key. Returns it (shown once) and its details.
    pub fn create_key(&self, name: &str, role: Role) -> Result<(String, KeyInfo), Error> {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err(Error::Invalid("give the key a name (up to 64 characters)".into()));
        }
        let mut inner = self.inner.lock().unwrap();
        let Mode::Configured(s) = &mut inner.mode else { return Err(Error::Invalid("finish setup first".into())) };
        if role == Role::Admin && s.config == ConfigLock::Locked {
            return Err(Error::Locked);
        }
        let key = format!("lc_{}", random_hex(24));
        let stored = StoredKey { id: random_hex(4), name: name.into(), role, hash: sha256_hex(&key), created: now(), last_used: None };
        let info = KeyInfo::from(&stored);
        s.keys.push(stored);
        self.save(&mut inner)?;
        Ok((key, info))
    }

    pub fn delete_key(&self, id: &str) -> Result<(), Error> {
        let mut inner = self.inner.lock().unwrap();
        let Mode::Configured(s) = &mut inner.mode else { return Err(Error::NoSuchKey) };
        let n = s.keys.len();
        s.keys.retain(|k| k.id != id);
        if s.keys.len() == n {
            return Err(Error::NoSuchKey);
        }
        Ok(self.save(&mut inner)?)
    }

    /// End every login session.
    pub fn logout_all(&self) -> Result<(), Error> {
        let mut inner = self.inner.lock().unwrap();
        if let Mode::Configured(s) = &mut inner.mode {
            s.sessions.clear();
        }
        Ok(self.save(&mut inner)?)
    }

    /// Save pending session activity (on shutdown).
    pub fn flush(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.dirty {
            let _ = self.save(&mut inner);
        }
    }
}

/// `auth.json` exists but can't be used: refuse access rather than fall back to setup,
/// which would let anyone take the server over.
fn deny(inner: &mut Inner, path: &std::path::Path, e: &dyn std::fmt::Display) {
    tracing::error!("{}: {e}; access denied until it's fixed or removed (`loupecam access reset`)", path.display());
    inner.mode = Mode::Configured(Stored {
        access: Access::Password,
        config: ConfigLock::Locked,
        access_password: None,
        admin_password: None,
        sessions: vec![],
        keys: vec![],
    });
}

/// The role to log in whoever just chose these levels, if logging in is needed.
fn session_role(s: &Stored) -> Option<Role> {
    match (s.access, s.config) {
        (_, ConfigLock::Password) => Some(Role::Admin),
        (Access::Password, _) => Some(Role::Operate),
        (Access::Open, _) => None,
    }
}

/// Validate `levels` and hash new passwords. Passwords left out keep their old value
/// (from `old`), when the level still needs them.
fn build(levels: Levels, old: Option<&Stored>) -> Result<Stored, Error> {
    /// A new password (plain text), or the hash to keep.
    enum Pw {
        New(String),
        Keep(String),
    }
    let pick = |new: Option<String>, old: Option<&String>, needed: bool, what: &str| -> Result<Option<Pw>, Error> {
        if !needed {
            return Ok(None);
        }
        match new.filter(|p| !p.is_empty()) {
            Some(p) => Ok(Some(Pw::New(p))),
            None => old.cloned().map(|h| Some(Pw::Keep(h))).ok_or_else(|| Error::Invalid(format!("set the {what} password"))),
        }
    };
    let access = pick(levels.access_password, old.and_then(|o| o.access_password.as_ref()), levels.access == Access::Password, "access")?;
    let admin = pick(levels.admin_password, old.and_then(|o| o.admin_password.as_ref()), levels.config == ConfigLock::Password, "admin")?;
    // One password can't be both: the admin one would always win at login.
    let same = match (&access, &admin) {
        (Some(Pw::New(a)), Some(Pw::New(b))) => a == b,
        (Some(Pw::New(p)), Some(Pw::Keep(h))) | (Some(Pw::Keep(h)), Some(Pw::New(p))) => verify(p, h),
        _ => false,
    };
    if same {
        return Err(Error::Invalid("use different access and admin passwords".into()));
    }
    let hashed = |p: Option<Pw>| {
        p.map(|p| match p {
            Pw::New(p) => hash(&p),
            Pw::Keep(h) => h,
        })
    };
    let (access_password, admin_password) = (hashed(access), hashed(admin));
    Ok(Stored {
        access: levels.access,
        config: levels.config,
        access_password,
        admin_password,
        // New levels or passwords end every session.
        sessions: vec![],
        keys: old.map(|o| o.keys.clone()).unwrap_or_default(),
    })
}

fn hash(password: &str) -> String {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).expect("system random source");
    let salt = SaltString::encode_b64(&salt).expect("16-byte salt");
    Argon2::default().hash_password(password.as_bytes(), &salt).expect("argon2 hash").to_string()
}

fn verify(password: &str, phc: &str) -> bool {
    PasswordHash::new(phc).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

/// A random setup code: 8 letters and digits, without look-alikes, as `XXXX-XXXX`.
pub fn random_setup_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).expect("system random source");
    let s: String = b.iter().map(|x| ALPHABET[*x as usize % ALPHABET.len()] as char).collect();
    format!("{}-{}", &s[..4], &s[4..])
}

/// Codes compare case-insensitively, ignoring spaces and dashes.
fn normalize_code(c: &str) -> String {
    c.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).expect("system random source");
    hex(&b)
}

fn sha256_hex(s: &str) -> String {
    hex(&Sha256::digest(s.as_bytes()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Constant-time string comparison.
pub fn eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// Write a file readable only by its owner, replacing it atomically.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        use std::io::Write;
        let mut f = o.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    // Written by root (`sudo loupecam access …`) for a service running as its own user:
    // hand the file to whoever owns the directory, or the service couldn't read it.
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(m) = std::fs::metadata(dir)
    {
        use std::os::unix::fs::MetadataExt;
        let _ = std::os::unix::fs::chown(&tmp, Some(m.uid()), Some(m.gid()));
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IP: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 2));

    fn creds<'a>(token: Option<&'a str>, session: Option<&'a str>) -> Credentials<'a> {
        Credentials { tokens: [token, None, None], session }
    }

    fn levels(access: Access, config: ConfigLock, ap: Option<&str>, adm: Option<&str>) -> Levels {
        Levels { access, config, access_password: ap.map(Into::into), admin_password: adm.map(Into::into) }
    }

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("loupecam-auth-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("auth.json")
    }

    #[test]
    fn loopback_is_open_without_setup() {
        let a = Auth::new(None, false, None, None);
        let r = a.resolve(&Credentials::default());
        assert_eq!(r.role, Some(Role::Admin));
        assert!(!r.setup_required);
        assert_eq!(a.info(&r).setup, SetupState::NotNeeded);
    }

    #[test]
    fn network_needs_setup_and_setup_is_single_use() {
        let a = Auth::new(None, true, None, None);
        let r = a.resolve(&Credentials::default());
        assert!(r.setup_required);
        assert_eq!(r.role, None);
        assert!(a.setup(IP, None, levels(Access::Open, ConfigLock::Open, None, None)).unwrap().is_none());
        assert_eq!(a.resolve(&Credentials::default()).role, Some(Role::Admin));
        assert!(matches!(a.setup(IP, None, levels(Access::Open, ConfigLock::Open, None, None)), Err(Error::AlreadySetUp)));
    }

    #[test]
    fn setup_code_is_required_and_rate_limited() {
        let a = Auth::new(None, true, None, Some("ABCD-2345".into()));
        let l = || levels(Access::Open, ConfigLock::Open, None, None);
        assert!(matches!(a.setup(IP, None, l()), Err(Error::WrongCode)));
        for _ in 0..4 {
            assert!(matches!(a.setup(IP, Some("nope"), l()), Err(Error::WrongCode)));
        }
        assert!(matches!(a.setup(IP, Some("abcd 2345"), l()), Err(Error::TooManyAttempts(_))));
        let other = IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 3));
        a.setup(other, Some("abcd 2345"), l()).unwrap();
    }

    #[test]
    fn protected_config() {
        let a = Auth::new(None, true, None, None);
        let s = a.setup(IP, None, levels(Access::Open, ConfigLock::Password, None, Some("admin-pass"))).unwrap().unwrap();
        assert_eq!(s.role, Role::Admin);
        assert_eq!(a.resolve(&Credentials::default()).role, Some(Role::Operate));
        let r = a.resolve(&creds(None, Some(&s.cookie)));
        assert_eq!(r.role, Some(Role::Admin));
        assert!(r.via_session);
        assert!(matches!(a.login(IP, "wrong-pass"), Err(Error::WrongPassword)));
        assert_eq!(a.login(IP, "admin-pass").unwrap().role, Role::Admin);
        a.logout(&s.cookie).unwrap();
        assert_eq!(a.resolve(&creds(None, Some(&s.cookie))).role, Some(Role::Operate));
    }

    #[test]
    fn passworded_access() {
        let a = Auth::new(None, true, None, None);
        assert!(matches!(
            a.setup(IP, None, levels(Access::Password, ConfigLock::Password, Some(""), Some("admin-pass"))),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            a.setup(IP, None, levels(Access::Password, ConfigLock::Password, Some("same-pass"), Some("same-pass"))),
            Err(Error::Invalid(_))
        ));
        a.setup(IP, None, levels(Access::Password, ConfigLock::Password, Some("access-pass"), Some("admin-pass"))).unwrap();
        assert_eq!(a.resolve(&Credentials::default()).role, None);
        let user = a.login(IP, "access-pass").unwrap();
        assert_eq!(a.resolve(&creds(None, Some(&user.cookie))).role, Some(Role::Operate));
        // The admin password grants access too.
        let admin = a.login(IP, "admin-pass").unwrap();
        assert_eq!(a.resolve(&creds(None, Some(&admin.cookie))).role, Some(Role::Admin));
    }

    #[test]
    fn locked_config_caps_everyone_at_operate() {
        let a = Auth::new(None, true, Some("legacy".into()), None);
        a.change_local(levels(Access::Open, ConfigLock::Locked, None, None)).unwrap();
        assert_eq!(a.resolve(&Credentials::default()).role, Some(Role::Operate));
        assert!(matches!(a.create_key("x", Role::Admin), Err(Error::Locked)));
        assert!(matches!(a.change(levels(Access::Open, ConfigLock::Open, None, None)), Err(Error::Locked)));
        // `--token` is set on the host, so it still grants everything.
        assert_eq!(a.resolve(&creds(Some("legacy"), None)).role, Some(Role::Admin));
    }

    #[test]
    fn api_keys() {
        let a = Auth::new(None, true, None, None);
        a.setup(IP, None, levels(Access::Password, ConfigLock::Password, Some("access-pass"), Some("admin-pass"))).unwrap();
        let (key, info) = a.create_key("Home Assistant", Role::View).unwrap();
        assert_eq!(a.resolve(&creds(Some(&key), None)).role, Some(Role::View));
        assert_eq!(a.keys()[0].id, info.id);
        a.delete_key(&info.id).unwrap();
        assert_eq!(a.resolve(&creds(Some(&key), None)).role, None);
    }

    #[test]
    fn change_keeps_unchanged_passwords_and_ends_sessions() {
        let a = Auth::new(None, true, None, None);
        let s = a.setup(IP, None, levels(Access::Password, ConfigLock::Password, Some("access-pass"), Some("admin-pass"))).unwrap().unwrap();
        let s2 = a.change(levels(Access::Password, ConfigLock::Password, None, Some("new-admin-pass"))).unwrap().unwrap();
        assert_eq!(a.resolve(&creds(None, Some(&s.cookie))).role, None);
        assert_eq!(a.resolve(&creds(None, Some(&s2.cookie))).role, Some(Role::Admin));
        assert_eq!(a.login(IP, "access-pass").unwrap().role, Role::Operate);
        assert!(a.login(IP, "admin-pass").is_err());
    }

    #[test]
    fn kiosk_key_operates_at_every_level() {
        let a = Auth::new(None, true, None, None).with_kiosk_key(Some("kiosk".into()));
        let k = creds(Some("kiosk"), None);
        // Setup still comes first.
        assert!(a.resolve(&k).setup_required);
        a.setup(IP, None, levels(Access::Password, ConfigLock::Password, Some("access-pass"), Some("admin-pass"))).unwrap();
        assert_eq!(a.resolve(&k).role, Some(Role::Operate));
        assert_eq!(a.resolve(&Credentials::default()).role, None);
    }

    #[test]
    fn any_length_password() {
        let a = Auth::new(None, true, None, None);
        a.setup(IP, None, levels(Access::Password, ConfigLock::Open, Some("x"), None)).unwrap();
        assert_eq!(a.login(IP, "x").unwrap().role, Role::Operate);
    }

    #[test]
    fn token_only() {
        let a = Auth::new(None, true, Some("tok".into()), None);
        assert_eq!(a.resolve(&Credentials::default()), Resolved { role: None, via_session: false, setup_required: false, locked: false });
        assert_eq!(a.resolve(&creds(Some("tok"), None)).role, Some(Role::Admin));
    }

    #[test]
    fn persists_and_reloads() {
        let f = temp_file("persist");
        let a = Auth::new(Some(f.clone()), true, None, None);
        let s = a.setup(IP, None, levels(Access::Password, ConfigLock::Open, Some("access-pass"), None)).unwrap().unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(!text.contains("access-pass") && !text.contains(&s.cookie));
        let b = Auth::new(Some(f.clone()), true, None, None);
        assert_eq!(b.resolve(&creds(None, Some(&s.cookie))).role, Some(Role::Operate));
        // `loupecam access reset` removes the file: setup again.
        std::fs::remove_file(&f).unwrap();
        b.inner.lock().unwrap().last_check -= Duration::from_secs(2);
        assert!(b.resolve(&Credentials::default()).setup_required);
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn corrupt_file_denies_access() {
        let f = temp_file("corrupt");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, b"{not json").unwrap();
        let a = Auth::new(Some(f.clone()), true, None, None);
        let r = a.resolve(&Credentials::default());
        assert_eq!(r.role, None);
        assert!(!r.setup_required);
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }
}
