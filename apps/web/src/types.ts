// Mirrors the JSON of crates/amscope-server (settings.rs, service.rs, captures.rs).

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
}

export interface CaptureEntry {
  name: string
  bytes: number
  modified: string
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
