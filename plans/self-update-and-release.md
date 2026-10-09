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
- "Signing" for releases means the update signature (done). OS code signing is
  deferred: an EV certificate later, not now (user, 2026-10-09). The inert workflow
  hooks stay.
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
6. [x] Release workflow. v0.1.0 failed (stale desktop bun.lock): its draft was deleted
   and the tag kept. v0.1.1 needed two fixes (lockfile; macOS bash 3.2 has no globstar)
   and was then rebuilt via workflow_dispatch: **published with every artifact signed,
   latest.json covering 4 platforms**.
7. [x] End-to-end against real releases (0.1.1 → 0.1.2), on Windows:
   - CLI `update --yes`: the running exe was replaced; bytes identical to the release
     asset; no leftover staging file.
   - `serve` with `updates.autoInstall` set via the API: the update installed ~5 s after
     the check, the camera was released, it re-exec'd, and it was back as 0.1.2 and
     streaming ~1 s later.
   - Desktop (per-user NSIS install): with auto-install off, the "Update available"
     prompt appeared 20 s after launch with the right versions and notes. With
     `desktop.json` autoInstall=true it upgraded itself to 0.1.2 and relaunched.
     desktop.json was removed afterwards (back to the default, off).
   - Not verified: pressing "Install and restart" in the prompt. UI Automation could
     not press the TaskDialog button (it is exposed as a pane), so it needs one manual
     click. Not tested on macOS/Linux hardware (CI built and signed those).

8. [x] Release = push `vX.Y.Z` (version taken from the tag; `-pre` → pre-release).
   Verified with v0.1.3 while Cargo.toml still said 0.1.2.
9. [x] OS code signing wired, gated on config: Windows via the repo variable
   `WINDOWS_SIGN_COMMAND` (provider-agnostic; Azure Artifact Signing documented).
   Tauri signCommand tested locally with a stand-in: it signs the app exe, the NSIS
   plugins, the uninstaller and the installer. macOS via `APPLE_*` secrets (Tauri for the
   app/dmg; `macos-sign.sh` for the CLI). **Inactive by design: EV signing is planned for later.**
10. [x] SHA256SUMS + build-provenance attestations on every release (v0.1.3 verified;
    a tampered file is rejected).

## Findings / gotchas

- GitHub returns 404 for `releases/latest` when there are no releases yet. This is
  treated as "up to date".
- The Linux CLI is built for musl (fully static) so it runs on older glibc distros
  (Raspberry Pi OS bookworm has glibc 2.36; ubuntu-latest builds would need 2.39).
- The signed test fixture is marked `-text` so git line-ending conversion can't
  invalidate its signature.
- macOS runners use bash 3.2: no `shopt -s globstar`. Use `find`.
- A stale bun.lock (after editing package.json) fails `--frozen-lockfile`. CI now
  checks the desktop lockfile too.
- clap printed `loupecam-cli 0.1.1` for `--version` (package name). Fixed with
  `#[command(name = "loupecam")]`, effective from the next release.
- Windows checkouts commit scripts as 100644 (no +x). The workflow calls them via
  `bash`; the modes were also fixed with `git update-index --chmod=+x`.
- On a dispatch re-run, the attestation's sourceRepositoryRef is `refs/heads/master`;
  on a tag push it is the tag.
- latest.json is built in a final job: tauri-action's per-job merging races with a
  build matrix.

## Progress log

- 2026-10-09: repo public, v0.1.1 and v0.1.2 published, all update paths verified on Windows.

## Open questions for the user

1. Back up `~/.tauri/loupecam.key` + `.password` (losing them strands every install).

## Things not to do

- Never commit the private signing key.
- Never force-push.
