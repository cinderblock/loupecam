// Mirrors the JSON of crates/loupecam-server (settings.rs, service.rs, captures.rs).

export type DemosaicSetting = 'superpixel' | 'bilinear' | 'mhc'
export type CaptureFormat = 'png' | 'tiff' | 'jpeg'
export type AntiFlicker = 'off' | '50hz' | '60hz'
export type WbMode = 'manual' | 'auto'
export type Curve = 'srgb' | 'linear' | number

export interface Roi {
  x: number
  y: number
  width: number
  height: number
}

export interface Settings {
  sizeIndex: number
  bitDepth: 8 | 12
  exposureUs: number
  autoExposure: boolean
  aeTarget: number
  aeMaxExposureUs: number
  aeMaxGain: number
  gain: number
  speed: number
  binning: boolean
  roi: Roi | null
  antiFlicker: AntiFlicker
  whiteBalance: { mode: WbMode; gains: [number, number, number] }
  color: { correction: boolean; saturation: number; hue: number; monochrome: boolean; negative: boolean }
  tone: { curve: Curve; blackPoint: number; whitePoint: number; brightness: number; contrast: number; blackLevel: number }
  orientation: { flipHorizontal: boolean; flipVertical: boolean; rotation: 0 | 90 | 180 | 270 }
  preview: { maxWidth: number; jpegQuality: number; maxFps: number; demosaic: DemosaicSetting }
  capture: {
    format: CaptureFormat
    jpegQuality: number
    sixteenBit: boolean
    fullResolution: boolean
    demosaic: DemosaicSetting
    saveRaw: boolean
  }
  record: RecordSettings
  storage: { usb: UsbMode }
  updates: { autoInstall: boolean; checkIntervalHours: number }
  calibration: CalibrationSettings
}

/** Whether captures go to a USB stick: never, when one is inserted, or only there. */
export type UsbMode = 'off' | 'whenAvailable' | 'always'
export type Location = 'internal' | 'usb'

export interface StorageStatus {
  /** Where the server looks for a stick (null: USB storage isn't set up). */
  usbDir: string | null
  usbPresent: boolean
}

export type RecordPreset = 'smooth' | 'detailed' | 'timelapse' | 'custom'
export type RecordMode = 'video' | 'timelapse'

export interface RecordSettings {
  preset: RecordPreset
  mode: RecordMode
  maxWidth: number
  maxHeight: number
  maxFps: number
  bitrateKbps: number
  streamSizeIndex: number | null
  timelapseIntervalS: number
  timelapseFps: number
  timelapseStills: boolean
}

export interface Recording {
  name: string | null
  mode: RecordMode
  location: Location
  encoder: string | null
  width: number
  height: number
  started: string
  seconds: number
  frames: number
  bytes: number
  stills: number
}

export interface RecordStatus {
  active: Recording | null
  last: Recording | null
  error: string | null
  encoders: string[]
}

export interface CalibrationSettings {
  blackLevel: boolean
  defects: boolean
  gainStages: boolean
  flatField: boolean
  color: boolean
  scalePreset: string | null
}

export interface ScalePreset {
  name: string
  umPerSensorPixel: number
  measuredAt: string
  method: string
}

export interface CalibrationProfile {
  serial: string
  dark: { measuredAt: string; blackLevel: [number, number, number]; defects: Record<string, [number, number][]> } | null
  gain: { measuredAt: string; stages: [number, number][]; linearityError: number } | null
  flatField: Record<string, { measuredAt: string; width: number; height: number }>
  scale: ScalePreset[]
  color: { measuredAt: string; error: number } | null
}

export interface CalibrationStatus {
  running: string | null
  progress: number
  message: string
  last: { step: string; ok: boolean; summary: string; at: string } | null
}

export type Light = 'physical' | 'screen'

export type ScaleSource = { source: 'lines'; spacingUm: number } | { source: 'screen'; pixelPitchUm: number; periodPx: number }

export type StepRequest =
  | { step: 'dark' }
  | { step: 'gain'; light: Light }
  | { step: 'flatField'; light: Light; sizeIndex?: number }
  | ({ step: 'scale'; name: string } & ScaleSource)
  | { step: 'color'; region: NormRect }

export type TargetPattern = { kind: 'idle' } | { kind: 'solid'; level: number } | { kind: 'grid'; period: number; line: number }

export interface Release {
  version: string
  tag: string
  notes: string
  url: string
  publishedAt: string | null
  installable: boolean
}

export interface UpdateStatus {
  /** False when the desktop app's own updater handles updates. */
  enabled: boolean
  currentVersion: string
  checking: boolean
  installing: boolean
  available: Release | null
  lastCheck: string | null
  lastError: string | null
}

export type Status =
  | { state: 'searching'; detail: string | null }
  | { state: 'streaming' }
  | { state: 'busy'; detail: string }

export interface DeviceSummary {
  model: string
  serial: string
  firmwareVersion: string
  hardwareVersion: string
  fpgaVersion: string
  productionDate: string | null
  resolutions: [number, number][]
  pixelSizeUm: number
  maxBitDepth: number
  maxGain: number
}

export interface State {
  status: Status
  device: DeviceSummary | null
  settings: Settings
}

export interface LiveStats {
  width: number
  height: number
  fps: number
  maxFps: number
  frames: number
  dropped: number
  sequenceGaps: number
  exposureUs: number
  gain: number
  mean: [number, number, number]
  clipped: number
  histogram: number[]
}

export interface CaptureInfo {
  name: string
  rawName: string | null
  width: number
  height: number
  bytes: number
  exposureUs: number
  gain: number
  taken: string
  location: Location
}

export interface CaptureEntry {
  name: string
  bytes: number
  modified: string
  location: Location
}

export type CastKind = 'image' | 'svg' | 'pdf' | 'html' | 'url'

/** What is being cast to the display (`/api/display`). */
export interface CastInfo {
  id: number
  kind: CastKind
  mime: string
  name: string | null
  bytes: number
  url: string | null
  since: string
}

/** A rectangle in normalised display coordinates (0..1). */
export interface NormRect {
  x: number
  y: number
  width: number
  height: number
}

/** Recursive partial, for JSON merge patches. */
export type Patch<T> = { [K in keyof T]?: T[K] extends object ? (T[K] extends unknown[] ? T[K] : Patch<T[K]> | null) : T[K] }

export type Role = 'view' | 'operate' | 'admin'
export type AccessLevel = 'open' | 'password'
export type ConfigLock = 'open' | 'password' | 'locked'

/** `GET /api/access`. */
export interface AccessInfo {
  setup: 'required' | 'done' | 'notNeeded'
  setupCodeRequired: boolean
  /** `token`: only the server's `--token` grants access. */
  access: AccessLevel | 'token'
  config: ConfigLock
  /** `null`: must log in. */
  role: Role | null
  /** Logged in with a session (can log out). */
  session: boolean
}

export interface Levels {
  access: AccessLevel
  config: ConfigLock
  /** Left out (or empty) on a change: keep the current one. */
  accessPassword?: string
  adminPassword?: string
}

export interface KeyInfo {
  id: string
  name: string
  role: Role
  /** Unix seconds. */
  created: number
  lastUsed: number | null
}
