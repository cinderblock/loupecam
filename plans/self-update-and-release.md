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

1. [ ] Create the repo, push `master`, see CI run.
2. [ ] Generate the signing key and set the secrets.
3. [ ] `loupecam-update` crate + CLI `update`.
4. [ ] Server periodic check, auto-update setting, API, UI banner.
5. [ ] Desktop updater plugin, menu, auto setting.
6. [ ] Release workflow. Tag v0.1.0, check the assets.
7. [ ] End-to-end: install v0.1.0, release v0.1.1, update both the CLI and the desktop app.

## Findings / gotchas

## Progress log

## Open questions for the user

## Things not to do

- Never commit the private signing key.
- Never force-push.
