import { useCallback, useEffect, useState } from 'react'
import { api, setToken } from './api'
import { CaptureSettingsPanel, Controls } from './components/Controls'
import { Gallery } from './components/Gallery'
import { Histogram } from './components/Histogram'
import { Section, Slider } from './components/widgets'
import { formatExposure } from './format'
import { Viewer, type Overlays, type Tool } from './components/Viewer'
import { UpdateBanner, UpdatesPanel } from './components/Updates'
import { CalibrationPanel } from './components/Calibration'
import type { NormRect, ScalePreset } from './types'
import { useCamera } from './useCamera'

function useStored<T>(key: string, initial: T): [T, (v: T) => void] {
  const [v, setV] = useState<T>(() => {
    const s = localStorage.getItem(key)
    return s === null ? initial : (JSON.parse(s) as T)
  })
  const set = useCallback(
    (nv: T) => {
      localStorage.setItem(key, JSON.stringify(nv))
      setV(nv)
    },
    [key],
  )
  return [v, set]
}

export default function App() {
  const [tool, setTool] = useState<Tool>('pan')
  const [overlays, setOverlays] = useStored<Overlays>('overlays', { crosshair: false, grid: false, scaleBar: true })
  // Optical magnification between specimen and sensor: objective × camera adapter.
  const [objective, setObjective] = useStored('objective', 10)
  const [adapter, setAdapter] = useStored('adapter', 0.5)
  const [busy, setBusy] = useState(false)
  const [toast, setToast] = useState<string | null>(null)
  const [galleryVersion, setGalleryVersion] = useState(0)

  const flash = useCallback((m: string) => {
    setToast(m)
    window.setTimeout(() => setToast((t) => (t === m ? null : t)), 4000)
  }, [])
  const cam = useCamera(flash)
  // Calibrated scale presets (refreshed when a calibration step finishes).
  const [scalePresets, setScalePresets] = useState<ScalePreset[]>([])
  const calibAt = cam.calibration?.last?.at
  const serial = cam.state?.device?.serial
  useEffect(() => {
    if (!serial) return
    api.calibration().then((r) => setScalePresets(r.profile.scale), () => {})
  }, [calibAt, serial])

  const capture = useCallback(async () => {
    if (busy) return
    setBusy(true)
    try {
      const c = await api.capture()
      flash(`Saved ${c.name} (${c.width}×${c.height}, ${formatExposure(c.exposureUs)})`)
      setGalleryVersion((v) => v + 1)
    } catch (e) {
      flash(`Capture failed: ${(e as Error).message}`)
    } finally {
      setBusy(false)
    }
  }, [busy, flash])

  const onRegion = async (t: Tool, r: NormRect) => {
    try {
      if (t === 'wb') {
        const { gains } = await api.whiteBalance(r)
        flash(`White balance set: ${gains.map((g) => g.toFixed(2)).join(' / ')}`)
      } else if (t === 'roi') {
        await api.roi(r)
      } else if (t === 'chart') {
        await api.runCalibration({ step: 'color', region: r })
      }
      setTool('pan')
    } catch (e) {
      flash((e as Error).message)
    }
  }

  // Keyboard shortcuts (ignored while typing in a field).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).closest('input, select, textarea')) return
      if (e.key === ' ' || e.key === 'Enter') {
        e.preventDefault()
        capture()
      } else if (e.key === 'Escape') setTool('pan')
      else if (e.key === 'r') setTool('roi')
      else if (e.key === 'w') setTool('wb')
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [capture])

  if (cam.authNeeded) {
    return (
      <div className="auth">
        <h2>LoupeCam</h2>
        <p>This camera server requires an access token.</p>
        <form
          onSubmit={(e) => {
            e.preventDefault()
            const t = new FormData(e.currentTarget).get('token') as string
            setToken(t)
            location.reload()
          }}
        >
          <input name="token" type="password" placeholder="Token" autoFocus />
          <button type="submit">Connect</button>
        </form>
      </div>
    )
  }

  const st = cam.state
  const s = st?.settings
  const dev = st?.device ?? null
  const status = !cam.connected ? 'Connecting to server…' : st?.status.state === 'searching' ? `Looking for camera… ${st.status.detail ?? ''}` : st?.status.state === 'busy' ? st.status.detail : null

  let umPerPixel: number | null = null
  if (dev && s && cam.stats && cam.frame) {
    const fullW = dev.resolutions[0][0]
    const sizeW = dev.resolutions[s.sizeIndex]?.[0] ?? fullW
    const rotated = s.orientation.rotation === 90 || s.orientation.rotation === 270
    const frameAcross = rotated ? cam.stats.height : cam.stats.width
    // A calibrated scale preset, or the nominal pixel size through objective × adapter.
    const preset = scalePresets.find((p) => p.name === s.calibration.scalePreset)
    const umPerSensorPixel = preset ? preset.umPerSensorPixel : dev.pixelSizeUm / (objective * adapter)
    umPerPixel = umPerSensorPixel * (fullW / sizeW) * (frameAcross / cam.frame.width)
  }

  return (
    <div className="app">
      <header className="topbar">
        <strong>LoupeCam</strong>
        <span className="muted">{dev ? `${dev.model} · ${dev.serial}` : 'no camera'}</span>
        <span className="spacer" />
        {cam.stats && (
          <span className="muted mono">
            {cam.stats.width}×{cam.stats.height} · {cam.stats.fps.toFixed(1)} fps · {formatExposure(cam.stats.exposureUs)} · {cam.stats.gain.toFixed(2)}×
          </span>
        )}
        <span className={cam.connected && st?.status.state === 'streaming' ? 'dot ok' : 'dot'} />
      </header>
      {cam.update && <UpdateBanner u={cam.update} onError={flash} />}
      <main className="main">
        <div className="stage">
          <div className="toolbar">
            {(
              [
                ['pan', 'Pan / zoom', 'Esc'],
                ['roi', 'Draw ROI', 'R'],
                ['wb', 'White-balance region', 'W'],
              ] as [Tool, string, string][]
            ).map(([t, label, key]) => (
              <button key={t} className={tool === t ? 'active' : ''} onClick={() => setTool(t)} title={`${label} (${key})`}>
                {label}
              </button>
            ))}
            <span className="spacer" />
            {(['crosshair', 'grid', 'scaleBar'] as (keyof Overlays)[]).map((k) => (
              <button key={k} className={overlays[k] ? 'active' : ''} onClick={() => setOverlays({ ...overlays, [k]: !overlays[k] })}>
                {k === 'scaleBar' ? 'Scale bar' : k[0].toUpperCase() + k.slice(1)}
              </button>
            ))}
            <button className="primary" disabled={busy || !dev} onClick={capture} title="Capture (Space)">
              {busy ? 'Capturing…' : 'Capture'}
            </button>
          </div>
          <Viewer frame={cam.frame} tool={tool} overlays={overlays} umPerPixel={umPerPixel} onRegion={onRegion} message={status ?? undefined} />
        </div>
        <aside className="sidebar">
          {cam.stats && (
            <div className="histo-wrap">
              <Histogram bins={cam.stats.histogram} clipped={cam.stats.clipped} />
            </div>
          )}
          {s && <Controls s={s} device={dev} stats={cam.stats} patch={cam.patch} onWhiteBalanceOnce={() => api.whiteBalance(null).catch((e) => flash(e.message))} onClearRoi={() => api.roi(null).catch((e) => flash(e.message))} />}
          {s && (
            <Section title="Capture">
              <CaptureSettingsPanel s={s} patch={cam.patch} />
            </Section>
          )}
          <Section title="Scale" defaultOpen={false}>
            {s?.calibration.scalePreset ? (
              <p className="hint">Using the calibrated preset “{s.calibration.scalePreset}” (change it under Calibration).</p>
            ) : (
              <>
                <Slider label="Objective" value={objective} min={0.1} max={100} log format={(v) => `${v.toFixed(v < 10 ? 2 : 0)}×`} onChange={setObjective} />
                <Slider label="Camera adapter" value={adapter} min={0.3} max={2} format={(v) => `${v.toFixed(2)}×`} onChange={setAdapter} />
                <p className="hint">Nominal scale: pixel size ÷ (objective × adapter). For an exact scale, use Calibration → Scale.</p>
              </>
            )}
          </Section>
          {s && (
            <Section title="Calibration" defaultOpen={false}>
              <CalibrationPanel
                s={s}
                device={dev}
                status={cam.calibration}
                patch={cam.patch}
                flash={flash}
                startChartBox={() => {
                  setTool('chart')
                  flash('Drag a box tightly around the colour chart patches')
                }}
              />
            </Section>
          )}
          <Section title="Captures">
            <Gallery version={galleryVersion} />
          </Section>
          {s && (
            <Section title="About & updates" defaultOpen={false}>
              <UpdatesPanel u={cam.update} s={s} patch={cam.patch} />
            </Section>
          )}
          {dev && (
            <Section title="Camera" defaultOpen={false}>
              <dl className="info">
                <dt>Model</dt>
                <dd>{dev.model}</dd>
                <dt>Serial</dt>
                <dd>{dev.serial}</dd>
                <dt>Produced</dt>
                <dd>{dev.productionDate ?? '?'}</dd>
                <dt>Firmware</dt>
                <dd>{dev.firmwareVersion}</dd>
                <dt>Hardware</dt>
                <dd>{dev.hardwareVersion}</dd>
                <dt>FPGA</dt>
                <dd>{dev.fpgaVersion}</dd>
                <dt>Pixel</dt>
                <dd>{dev.pixelSizeUm} µm</dd>
                {cam.stats && (
                  <>
                    <dt>Frames</dt>
                    <dd>
                      {cam.stats.frames} ({cam.stats.dropped} dropped, {cam.stats.sequenceGaps} lost)
                    </dd>
                    <dt>Sensor max</dt>
                    <dd>{cam.stats.maxFps.toFixed(1)} fps</dd>
                  </>
                )}
              </dl>
            </Section>
          )}
        </aside>
      </main>
      {toast && <div className="toast">{toast}</div>}
    </div>
  )
}
