# Optional calibration wizard

## Goal

An optional, guided calibration page in the web UI where the app drives the camera
(and optionally an on-screen target) to measure what the uncalibrated defaults only
estimate. **The app must keep working fully without it.** Every result is a separate,
switchable profile.

## Context

- The user's setup: a relatively low-power **trinocular (zoom stereo) scope for PCB
  rework** with an MU1803-HS. Low magnification means a monitor under the scope is a
  usable target. Lighting may be a movable ring light.
- The app should also suit other setups (compound scopes, C-mount lens).
- Today's defaults: black level 0, analog gain stages assumed 1/2/3/4×, vendor CCM, no
  flat field, scale from objective × adapter sliders, factory defect map not decoded.

## Decisions already made (don't re-ask)

- Optional; uncalibrated behaviour is unchanged. (User, 2026-10-09)
- A screen-as-target option is wanted alongside physical targets. (User, 2026-10-09)

## Design

### Calibration store (server)

`<config dir>/loupecam/calibration/<camera serial>/`:
- `profile.json`: black level per channel, measured analog gain stages, hot-pixel list,
  scale presets `[{name, umPerPixel, sizeIndex}]`, optional CCM, and a timestamp/version
  for each section.
- `flatfield-<size>.bin`: low-resolution per-channel gain map.

Each section has an `enabled` flag. Settings gain `calibration: {flatField: bool,
scalePreset: string|null, …}`.

### Steps (each independent, any order, all optional)

1. **Dark** (lens capped or light off): black level per channel (median of a short
   exposure), hot/stuck pixels (long exposure, > median + kσ, stable across 2 frames).
2. **Gain and linearity** (any steady, even light: white paper, defocused, or the screen
   target showing flat grey): an exposure sweep at unity gain checks linearity and finds
   ~40 % exposure; then each analog stage code at digital 64 at fixed exposure gives the
   measured stage ratios. These replace the 1/2/3/4× estimates in `Gain::for_multiplier`.
3. **Flat field** (the same even field, per resolution; optionally per zoom preset): the
   average of N frames, smoothed and downsampled per channel, gives a gain map applied in
   the ISP "prepare" pass (bilinear interpolation). Toggle in settings, off by default.
4. **Scale** (per zoom preset): a ruler/micrometer (the user enters the line spacing),
   or the **screen target** showing a grid of known pitch (the user enters the screen's
   pixel pitch or diagonal + resolution once). The period is found by FFT /
   autocorrelation of row and column profiles. This replaces the objective × adapter
   sliders (kept as a manual fallback).
5. **Colour** (optional, physical 24-patch colour chart): the user drags a box around the
   chart; patch centres are sampled; a 3×3 CCM is fitted by least squares to the
   reference linear values. Not done with the screen: display primaries would skew the
   matrix.

### Screen target page

`/target`: a full-screen page on any device with a browser (it can be a different
machine), paired over the WebSocket. The calibration runner tells it what to show
(solid grey level, grid of N px, focus star), waits for an ack, then captures. Only used
where a step offers it.

### Runner

A server task that drives the camera actor: temporarily overrides settings (AE/AWB off,
fixed exposure/gain/size), captures averaged frames (a new actor command), computes
results, restores the settings, and reports progress over the WebSocket. The UI is a
step list with instructions, live preview, progress, and per-step results/accept.

## Plan / steps

1. [x] Actor `measure` command (averaged raw 12-bit frames at fixed settings, stream restored).
2. [x] Calibration store (per serial) + API + settings toggles.
3. [x] Dark + gain steps; `GainTable` in the protocol crate.
4. [x] Flat field + ISP support (and defect replacement).
5. [x] Scale presets (lines or screen grid) used by the scale bar.
6. [x] Screen target page + WebSocket pairing.
7. [x] Colour chart step (24-patch, orientation-tolerant fit).
8. [x] Merged into local `master` (fast-forward after merging master, with the mDNS and
   firewall work, into `calibration`; one lib.rs conflict and the mDNS test fixed).
   Pushed with the user's OK and **released in v0.2.0** (2026-10-09).
9. [ ] **current** Real-scene validation once the tape is off: dark (lens covered), flat field on
   white paper, scale on a ruler, colour chart if available; check the `/target` page
   in a real browser on a real screen.

## Findings / gotchas

- **Measured analog gain stages: 1.00 / 1.99 / 2.97 / 3.64×** (three runs within
  ±0.3 %). The top stage was assumed 4×. These are now the built-in defaults.
- Shop-light (LED) flicker made short-exposure gain runs disagree by ~7 %. Fixed with
  50 ms exposures (whole cycles at 100 and 120 Hz) when headroom allows, plus interleaved
  unity references.
- Black level of this sensor ≈ 5 (12-bit) with the vendor's pedestal setting.
- The dark step correctly refuses the taped lens ("not dark: median 1541 vs black 5").
- Flat field on the taped, cloudy scene: block std 10.1 → 2.0 when applied (it was
  measured on that scene, so this proves the path, not the optics).
- The screen-target protocol was verified with a stand-in WebSocket client (grid for
  scale, solid white for gain, idle after). The real page could not be exercised in a
  browser here: the preview tool stopped responding and headless Edge produces no output.
- `~/.cargo/config.toml` shares one `build-dir` across all projects. Two worktrees of
  this repo then link each other's crates. Build here with
  `CARGO_BUILD_BUILD_DIR="$PWD/target/build-dir"` (logged in ISSUES.md on master).

## Open questions for the user

## Things not to do

- Don't make any calibration mandatory or change defaults when uncalibrated.
- Don't calibrate colour from a display.
