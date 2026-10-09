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

1. [ ] Actor command: capture N frames (averaged mosaic + stats) with temporary settings.
2. [ ] Calibration store + API (`GET/DELETE /api/calibration[/section]`, `POST
   /api/calibration/run/{step}`), settings toggles.
3. [ ] Steps 1–2 (dark, gain) + gain stage override in the protocol crate.
4. [ ] Step 3 flat field + ISP support.
5. [ ] Step 4 scale + zoom presets in the UI (replacing the objective/adapter sliders).
6. [ ] Screen target page + pairing.
7. [ ] Step 5 colour chart.
8. [ ] Test on the hardware once the tape is off; docs.

## Findings / gotchas

## Open questions for the user

## Things not to do

- Don't make any calibration mandatory or change defaults when uncalibrated.
- Don't calibrate colour from a display.
