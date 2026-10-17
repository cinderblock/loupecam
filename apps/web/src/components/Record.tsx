import { useEffect, useState } from 'react'
import { api } from '../api'
import { formatBytes, formatDuration } from '../format'
import type { DeviceSummary, Patch, RecordPreset, RecordSettings, RecordStatus, Settings } from '../types'
import { Segmented, Select, Slider, Toggle } from './widgets'

type Presets = Partial<Record<RecordPreset, RecordSettings>>

const PRESETS: [Exclude<RecordPreset, 'custom'>, string, string][] = [
  ['smooth', 'Smooth', 'High frame rate at the current stream size'],
  ['detailed', 'Detailed', 'A larger stream size, up to 1920×1080, at a lower frame rate'],
  ['timelapse', 'Timelapse', 'A frame every few seconds, played back fast'],
]

const SIZES: [number, number][] = [
  [640, 480],
  [1280, 960],
  [1920, 1080],
  [2560, 1920],
  [3840, 2160],
]

/** The usual size limits, plus the current one if it's something else. */
function sizeOptions(r: RecordSettings): [string, string][] {
  const sizes = SIZES.some(([w, h]) => w === r.maxWidth && h === r.maxHeight) ? SIZES : [...SIZES, [r.maxWidth, r.maxHeight] as [number, number]]
  return sizes.map(([w, h]) => [`${w}x${h}`, `${w} × ${h}`])
}

/** The toolbar's record / stop button, with the elapsed time while recording. */
export function RecordButton({ status, disabled, onError }: { status: RecordStatus | null; disabled: boolean; onError: (m: string) => void }) {
  const [busy, setBusy] = useState(false)
  const active = status?.active
  const toggle = async () => {
    setBusy(true)
    try {
      if (active) await api.stopRecording()
      else await api.startRecording()
    } catch (e) {
      onError(`Recording: ${(e as Error).message}`)
    } finally {
      setBusy(false)
    }
  }
  return (
    <button className={active ? 'record active' : 'record'} disabled={busy || (disabled && !active)} onClick={toggle} title={active ? 'Stop recording' : 'Start recording (settings under Recording)'}>
      <span className="record-dot" />
      {active ? `${formatDuration(active.seconds)} · Stop` : busy ? 'Starting…' : 'Record'}
    </button>
  )
}

/** Record settings: presets plus the individual knobs. */
export function RecordPanel({ s, device, status, patch }: { s: Settings; device: DeviceSummary | null; status: RecordStatus | null; patch: (p: Patch<Settings>) => void }) {
  const [presets, setPresets] = useState<Presets>({})
  useEffect(() => {
    api.record().then((r) => setPresets(r.presets), () => {})
  }, [])
  const r = s.record
  // Editing a knob makes the settings custom.
  const set = (p: Partial<RecordSettings>) => patch({ record: { ...p, preset: 'custom' } })
  const recording = !!status?.active
  const noEncoder = status && status.encoders.length === 0
  const resolutions = device?.resolutions ?? []
  return (
    <>
      <Segmented
        value={r.preset}
        options={PRESETS.map(([p, label]) => [p, label] as [RecordPreset, string])}
        onChange={(p) => {
          const next = presets[p]
          if (next) patch({ record: next })
        }}
      />
      <p className="hint">{PRESETS.find(([p]) => p === r.preset)?.[2] ?? 'Custom settings'}</p>
      <Segmented
        value={r.mode}
        options={[
          ['video', 'Video'],
          ['timelapse', 'Timelapse'],
        ]}
        onChange={(v) => set({ mode: v })}
      />
      {r.mode === 'video' ? (
        <Slider label="Max fps" value={r.maxFps} min={1} max={60} step={1} onChange={(v) => set({ maxFps: Math.round(v) })} />
      ) : (
        <>
          <Slider label="Every" value={r.timelapseIntervalS} min={0.5} max={3600} log format={(v) => (v >= 60 ? `${(v / 60).toFixed(1)} min` : `${v.toFixed(1)} s`)} onChange={(v) => set({ timelapseIntervalS: v })} />
          <Slider label="Playback" value={r.timelapseFps} min={1} max={60} step={1} format={(v) => `${v.toFixed(0)} fps`} onChange={(v) => set({ timelapseFps: Math.round(v) })} />
          <Toggle label="Also save each frame as a still" checked={r.timelapseStills} onChange={(v) => set({ timelapseStills: v })} />
        </>
      )}
      <Select
        label="Max size"
        value={`${r.maxWidth}x${r.maxHeight}`}
        options={sizeOptions(r)}
        onChange={(v) => {
          const [w, h] = v.split('x').map(Number)
          set({ maxWidth: w, maxHeight: h })
        }}
      />
      <Select
        label="Stream"
        value={r.streamSizeIndex ?? -1}
        options={[[-1, 'As it is'] as [number, string]].concat(resolutions.map(([w, h], i) => [i, `Switch to ${w} × ${h}`] as [number, string]))}
        onChange={(v) => set({ streamSizeIndex: v < 0 ? null : v })}
      />
      <Slider label="Bitrate" value={r.bitrateKbps} min={500} max={50_000} log format={(v) => `${(v / 1000).toFixed(1)} Mb/s`} onChange={(v) => set({ bitrateKbps: Math.round(v) })} />
      {recording && <p className="hint">Changes apply to the next recording.</p>}
      {noEncoder && (
        <p className="hint warn">
          This server has no H.264 encoder, so it can't record video. Timelapses can still save stills.
        </p>
      )}
      {status?.error && <p className="hint warn">Last recording: {status.error}</p>}
      {status?.last && !status.active && (
        <p className="hint">
          Last: {status.last.name ?? `${status.last.stills} stills`} · {formatDuration(status.last.seconds)} · {status.last.frames} frames
          {status.last.bytes ? ` · ${formatBytes(status.last.bytes)}` : ''}
        </p>
      )}
    </>
  )
}
