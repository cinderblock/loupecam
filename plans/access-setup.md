# Access control and first-run setup

## Goal

A LoupeCam server on the network (the Pi deployment, see
[pi-deployment.md](pi-deployment.md)) needs a one-time setup: the first connection
runs a setup wizard that chooses how the server is secured. Today the only option is
a single bearer token passed on the command line, which the installer generates. That
is replaced by a choice of security levels, set in the UI and changeable later.

## Environment / context

- Work happens in the worktree `../loupecam-access` (branch `access-setup`, from master
  `3dbf232`). The main tree is shared with the kiosk thread
  ([pi-kiosk.md](pi-kiosk.md)), which has uncommitted recording work in `http.rs`,
  `lib.rs` and `settings.rs`, the same files this touches. Expect merge conflicts there.
- Build in this worktree with `CARGO_BUILD_BUILD_DIR="$PWD/target/build-dir"` (see
  ISSUES.md: the shared Cargo build-dir collides across worktrees).
- Current auth: `http.rs::auth`. One optional token (`--token` / `LOUPECAM_TOKEN`),
  accepted as a Bearer header, `?token=`, or `loupecam_token` cookie. The UI shell is
  public; `/api/display*` (casting) is open unless `--display-token` is set. The web UI
  keeps the token in localStorage. mDNS TXT `auth` = `token` | `none`.
- CORS is `CorsLayer::permissive()` (no credentials), so cookies need
  `SameSite=Strict` plus a CSRF rule (below).
- Test Pi: odriver2 (port 80, see pi-deployment.md). Its env file currently has a
  `LOUPECAM_TOKEN`, which must be cleared to test setup.

## Decisions already made (don't re-ask)

From the user (2026-10-16):

- **Security levels** (the user's list):
  1. **Open**: no protection. Allowed but discouraged.
  2. **Configuration protected** (behind an admin / config password) **or locked**.
     "Configuration" means high-level settings, possibly defaults and limits.
  3. **Passworded**: a password is needed for even basic access. The config password
     may either require this password first or bypass it.
  4. **Full auth**: requires HTTPS. Passkeys, and an admin user is set up.
- **First-run setup**: by default the first connection runs setup. A CLI option
  `--setup-code` makes setup require a code set at start, random or specified.

Ours (record of choices made, change if the user disagrees):

- Loopback-only servers (the desktop app, the CLI default `127.0.0.1:8080`) skip setup
  and stay open, since nobody else can reach them.
- `--token` / `LOUPECAM_TOKEN`, when given, keeps working: it's a full-access API key,
  and the server skips setup if nothing else is configured. The installer stops
  generating one; the wizard decides instead.
- The admin password **bypasses** the access password: logging in as admin also grants
  access, so an admin types one password. The "require access first" variant only adds
  friction, so it isn't built unless asked for.
- Secrets live in `auth.json` next to `settings.json`, mode 0600, never in
  `settings.json`, which the API serves.

## Design

### Two axes, presented as levels

Stored as two independent settings, shown in the wizard as the user's four levels:

| Level (wizard) | `access` | `config` |
| --- | --- | --- |
| Open | `open` | `open` |
| Configuration protected | `open` | `password` |
| Configuration locked | `open` | `locked` |
| Passworded | `password` | `password` or `locked` |
| Full auth (phase 3) | `accounts` | admin users |

- `access` decides who can view and operate the camera at all.
- `config: password` means admin actions need the admin password.
  `config: locked` means admin actions are impossible over the network. They're done
  on the host with `loupecam access …`, which edits `auth.json` and the settings
  defaults (the server reloads it when the file changes).

### Roles

| Role | Can |
| --- | --- |
| `view` | state, stats, stream, snapshot, WebSocket, list/download captures |
| `operate` | `view`, plus capture, record, live camera settings (within limits), white balance, ROI |
| `admin` | everything: access/security, updates, calibration, defaults and limits, deleting captures, display token, server name |

Casting keeps its own rule (open unless a display token is set).

A request's role comes from:
- the level, for anonymous requests (open: admin; protected/locked: operate;
  passworded: none);
- a session cookie from a password login;
- an API key.

The handlers declare the minimum role they need, so the check isn't scattered across
path patterns.

Initially the access password grants `operate`. A `view`-only password could be added
later (not asked for).

### Sessions and CSRF

- `POST /api/login {password}` checks the hash and sets `loupecam_session`:
  random, HttpOnly, `SameSite=Strict`, `Secure` under HTTPS. Sessions are stored
  hashed in `auth.json`, so they survive restarts and self-updates. Access sessions
  last 30 days, sliding; admin sessions 12 hours, sliding. `POST /api/logout`.
  Changing a password ends all sessions.
- Passwords are hashed with Argon2id (`argon2` crate, pure Rust, so the musl build is
  fine). Failed logins back off per client IP.
- CSRF: requests that change state must send JSON (`Content-Type: application/json`)
  or a Bearer header. A cross-site HTML form can do neither, and with `SameSite=Strict`
  the cookie isn't sent cross-site anyway. Casting's file uploads use the display
  token, not cookies.
- Plain HTTP sends passwords in the clear on the LAN. The wizard says so; HTTPS comes
  in phase 3.

### API keys

Some clients can't log in with a password: Home Assistant and OctoPrint MJPEG URLs,
scripts, and the kiosk. They use named API keys with a role, sent as Bearer or
`?token=` as today. An admin creates and revokes them in the Access panel, or with
`loupecam access key create --name kiosk --role operate` on the host. Keys are stored
hashed; the full key is shown once. The kiosk (pi-kiosk plan) should get its key from a
file readable only by `loupecam-kiosk`. Not loopback trust: Chromium on the same Pi
renders arbitrary cast content.

### First-run setup

- **Unconfigured** = no `auth.json`, listening beyond loopback, and no `--token`. In
  that state everything except the UI shell and `/api/setup*` returns
  `409 {"error":"setup required"}`. The UI shows the wizard. The kiosk shows "finish
  setup at http://…".
- `--setup-code[=CODE]` (env `LOUPECAM_SETUP_CODE`): setup requires the code. With no
  value, a random 6-word or 8-character code is generated and logged at startup. The
  installer gets `--setup-code` and passes it through, printing the code.
- The wizard:
  1. Code (if required).
  2. Pick a level (cards; Open marked discouraged; Full auth disabled, "needs HTTPS",
     until phase 3).
  3. Set the password(s).
  4. Done: optionally create an API key for Home Assistant or similar, shown once.
- `POST /api/setup` is single-use (the first completed setup wins; later calls get
  409).
- `loupecam access reset` (on the host) deletes `auth.json`, so the next connection
  runs setup again: the "forgot the password" path.

### Settings policy (phase 2)

- Each setting is classified as **operational** (exposure, gain, white balance, ROI,
  orientation, tone, colour, size, capture format…) or **configuration** (updates,
  calibration selection, preview limits, defaults, limits). Configuration changes need
  `admin`. The exact split goes to the user for review once phases 1 and 2 meet.
- **Defaults**: an admin-saved settings snapshot. "Reset to defaults" is available to
  operators. An optional policy resets to the defaults on restart, or after idle.
- **Limits**: per-setting bounds an admin sets (e.g. the maximum exposure or fps, the
  largest size). Operator changes are clamped to them, and the UI greys out the
  out-of-range parts.

### Full auth (phase 3)

- HTTPS: `--tls-cert/--tls-key` (PEM) via rustls. Possibly ACME later. Behind a
  reverse proxy (e.g. Caddy), trust `X-Forwarded-Proto` from a configured proxy
  address.
- Passkeys (WebAuthn) need a secure context with a *valid* certificate: Chrome
  refuses WebAuthn on certificate errors, so self-signed certs won't do. Crate choice
  to research (`webauthn-rs` pulls in OpenSSL; check that it builds on musl, and look
  for pure-Rust alternatives).
- An admin user is created in the wizard, plus optional additional users with roles.

## Plan / steps

Phase 1, access core and setup: **← current**
1. `auth` module: the `auth.json` model (levels, hashes, sessions, keys), load and
   reload, Argon2id, role resolution, login backoff. Unit tests.
2. HTTP: role-based guard replacing `auth`; `/api/setup`, `/api/login`,
   `/api/logout`, `/api/access` (GET the level and your role, PATCH as admin),
   `/api/keys`. CSRF rule. 409 while unconfigured. mDNS `auth` TXT values.
3. CLI: `--setup-code`, `loupecam access reset | show | key …`, and a config-dir
   override so the CLI can reach the service's `/var/lib/loupecam/config`.
4. Web UI: setup wizard, login screen (access or admin), Access panel (level,
   passwords, keys, sign out everywhere), admin-only controls hidden or disabled by
   role.
5. Installer: no generated token; `--setup-code`; print the setup URL. README.
6. Test on odriver2 (clear its env token; run setup in the browser).

Phase 2: settings policy (classification, defaults, limits).

Phase 3: HTTPS and passkeys.

## Findings / gotchas

(none yet)

## Progress log

- [x] Levels and setup-code requirement from the user
- [x] Plan written
- [ ] Phase 1 steps 1–6
- [ ] Phase 2
- [ ] Phase 3

## Open questions for the user

1. Which settings count as "configuration" (admin-only), and what should defaults and
   limits cover? I'll propose a split in phase 2; nothing is blocked before then.

## Things not to do

- Don't edit the main tree's `http.rs` / `lib.rs` / `settings.rs`: the kiosk thread has
  uncommitted work there. Work here, and merge when both are committed.
- Don't trust loopback for the kiosk (casts run arbitrary web content on the same Pi).
- Don't put secrets in `settings.json` (served by `GET /api/settings`).
