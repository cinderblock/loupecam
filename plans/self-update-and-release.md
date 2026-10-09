# Publishing, releases, self-update and opt-in auto-update

## Goal

Publish LoupeCam publicly at `github.com/cinderblock/loupecam` and make every
distribution update itself: the `loupecam` CLI/headless server (Windows, Linux x64,
Linux arm64, macOS) and the Tauri desktop app. Updating on request must always be
possible. Automatic updating, with regular checks, is opt-in.

## Decisions already made (don't re-ask)

- The user authorised creating the public repo under `cinderblock` and deploying there
  (2026-10-09). That covers the repo, the release workflow, its secrets, and tagged
  releases.
- Licence: MIT/Apache-2.0 dual was added as the recommendation; the user published
  without objecting. (Still reported as changeable.)
- **One signing key for everything**: a minisign key from `tauri signer generate`.
  Tauri's updater requires it, and the CLI verifies its own downloads with the same
  public key (`minisign-verify`), so a compromised GitHub account alone cannot push
  code to installs.
- The update source is GitHub Releases: `releases/latest` via the GitHub API for the
  CLI, and `latest.json` for Tauri.

## Design

- `crates/loupecam-update`: a shared, synchronous library. Check the latest release,
  pick the asset for this target, download it plus its `.sig`, verify it against the
  embedded public key, extract it, and swap the running binary (`self-replace`).
- CLI: `loupecam update [--check] [--yes]`.
- `loupecam serve`: the server checks for updates periodically (default every 6 h). If
  `autoUpdate` is enabled in settings, it installs and restarts itself (re-exec with the
  same args; under a service manager, a plain exit and restart). The UI shows a banner
  with "Update now", backed by `GET /api/update` and `POST /api/update`.
- Desktop: `tauri-plugin-updater`. It checks at start and every 6 h, offers the update
  through a dialog, and installs silently when its opt-in auto-update setting is on.
  File menu: "Check for Updates…" and an "Automatically Update" checkbox. The embedded
  server's own updater is disabled there.
- Release workflow on `v*` tags: CLI archives per target plus `.sig`; Tauri bundles plus
  updater artifacts and `latest.json` (via `tauri-action`).

## Environment

- gh is authenticated as `cinderblock` (scopes repo, workflow). Git uses SSH.
- The signing private key lives at `~/.tauri/loupecam.key` (the user must back it up) and
  in the repo secret `TAURI_SIGNING_PRIVATE_KEY` (+ `_PASSWORD`).

## Plan / steps

1. [x] Created the repo (public) and pushed. First CI run green on every OS, including the arm64 runner.
2. [x] Signing key `~/.tauri/loupecam.key` (+ `.password`, `.pub`); secrets set.
3. [x] `loupecam-update` crate + `loupecam update`. Verified with a real signature
   fixture; tampering is rejected.
4. [x] Server: checks every 6 h, `updates.autoInstall` (default off), API, UI banner/panel.
   Tested locally (check, "no update available", setting toggle).
5. [x] Desktop: tauri-plugin-updater, File menu "Check for Updates…" and "Automatically
   Install Updates" (desktop.json), stops the server before restart.
6. [ ] **current** Release workflow pushed; tag v0.1.0 building.
7. [ ] End-to-end: install v0.1.0, release v0.1.1, update both the CLI and the desktop app.

## Findings / gotchas

- GitHub returns 404 for `releases/latest` when there are no releases yet. This is
  treated as "up to date".
- The Linux CLI is built for musl (fully static) so it runs on older glibc distros
  (Raspberry Pi OS bookworm has glibc 2.36; ubuntu-latest builds would need 2.39).
- The signed test fixture is marked `-text` so git line-ending conversion can't
  invalidate its signature.
- latest.json is built in a final job: tauri-action's per-job merging races with a
  build matrix.

## Progress log

## Open questions for the user

## Things not to do

- Never commit the private signing key.
- Never force-push.
