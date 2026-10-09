import { useEffect, useState } from 'react'
import { api } from '../api'
import type { CalibrationProfile, CalibrationStatus, DeviceSummary, Light, Patch, ScaleSource, Settings, StepRequest } from '../types'
import { Segmented, Toggle } from './widgets'

interface Props {
  s: Settings
  device: DeviceSummary | null
  status: CalibrationStatus | null
  patch: (p: Patch<Settings>) => void
  /** Switch the viewer to the chart-box tool; the region comes back via onChartRegion. */
  startChartBox: () => void
  flash: (m: string) => void
}

const when = (iso: string) => new Date(iso).toLocaleString()

const LINE_PRESETS: [string, number][] = [
  ['1 mm (ruler)', 1000],
  ['0.5 mm', 500],
  ['0.1 mm', 100],
  ['10 µm (micrometer)', 10],
]

/** Optional calibration: every step is independent and the app works without any. */
export function CalibrationPanel({ s, device, status, patch, startChartBox, flash }: Props) {
  const [profile, setProfile] = useState<CalibrationProfile | null>(null)
  const [targetConnected, setTargetConnected] = useState(false)
  const [gainLight, setGainLight] = useState<Light>('physical')
  const [flatLight, setFlatLight] = useState<Light>('physical')
  const [presetName, setPresetName] = useState('')
  const [scaleKind, setScaleKind] = useState<'lines' | 'screen'>('lines')
  const [spacing, setSpacing] = useState(1000)
  const [diagonal, setDiagonal] = useState(24)
  const [screenRes, setScreenRes] = useState('1920x1080')
  const [period, setPeriod] = useState(16)

  const lastAt = status?.last?.at
  useEffect(() => {
    api.calibration().then(
      (r) => {
        setProfile(r.profile)
        setTargetConnected(r.targetConnected)
      },
      () => {},
    )
  }, [lastAt, device?.serial])

  const running = status?.running ?? null
  const run = (req: StepRequest) => api.runCalibration(req).catch((e) => flash((e as Error).message))
  const forget = (section: string) =>
    api.deleteCalibration(section).then(
      () => api.calibration().then((r) => setProfile(r.profile)),
      (e) => flash((e as Error).message),
    )
  const c = s.calibration
  const sizeName = (i: number | string) => {
    const r = device?.resolutions[Number(i)]
    return r ? `${r[0]}×${r[1]}` : `size ${i}`
  }
  const [rw, rh] = screenRes.split(/[x×]/).map(Number)
  const pitchUm = rw > 0 && rh > 0 ? (diagonal * 25400) / Math.hypot(rw, rh) : 0

  const lightChoice = (v: Light, set: (l: Light) => void) => (
    <Segmented
      value={v}
      options={[
        ['physical', 'Scope light'],
        ['screen', 'Screen target'],
      ]}
      onChange={set}
    />
  )

  return (
    <div className="calibration">
      <p className="hint">Optional. LoupeCam works without calibration; each step improves one thing and can be switched off or forgotten.</p>

      {running ? (
        <div className="calib-running">
          <div className="row">
            <span>
              {running}: {status?.message}
            </span>
            <button onClick={() => api.cancelCalibration()}>Cancel</button>
          </div>
          <progress value={status?.progress ?? 0} max={1} />
        </div>
      ) : (
        status?.last && <p className={status.last.ok ? 'calib-ok' : 'calib-err'}>{status.last.ok ? '✓' : '✕'} {status.last.summary}</p>
      )}

      <p className="hint">
        Screen target: {targetConnected ? 'connected.' : 'not open.'}{' '}
        <a href="/target" target="_blank" rel="noreferrer">
          Open the target page
        </a>{' '}
        on a screen placed under the scope (any device can open <code>{location.origin}/target</code>).
      </p>

      <h4>1. Black level &amp; defective pixels</h4>
      <p className="hint">Cover the lens, or turn the light off. Takes about 20 s.</p>
      <div className="row wrap">
        <button disabled={!!running} onClick={() => run({ step: 'dark' })}>
          Measure
        </button>
        {profile?.dark && <button onClick={() => forget('dark')}>Forget</button>}
      </div>
      {profile?.dark && (
        <>
          <p className="hint">
            Black {profile.dark.blackLevel.map((b) => b.toFixed(0)).join(' / ')}; defects {Object.entries(profile.dark.defects).map(([k, v]) => `${sizeName(k)}: ${v.length}`).join(', ')} ({when(profile.dark.measuredAt)})
          </p>
          <div className="row wrap">
            <Toggle label="Subtract black level" checked={c.blackLevel} onChange={(v) => patch({ calibration: { blackLevel: v } })} />
            <Toggle label="Fix defective pixels" checked={c.defects} onChange={(v) => patch({ calibration: { defects: v } })} />
          </div>
        </>
      )}

      <h4>2. Gain</h4>
      <p className="hint">Point at something plain and evenly lit (paper, a blank slide), or use the screen target. Measures the sensor's real analog gain steps.</p>
      {lightChoice(gainLight, setGainLight)}
      <div className="row wrap">
        <button disabled={!!running} onClick={() => run({ step: 'gain', light: gainLight })}>
          Measure
        </button>
        {profile?.gain && <button onClick={() => forget('gain')}>Forget</button>}
      </div>
      {profile?.gain && (
        <>
          <p className="hint">
            Stages {profile.gain.stages.map(([, m]) => `${m.toFixed(2)}×`).join(', ')}; linear within {(profile.gain.linearityError * 100).toFixed(1)} % ({when(profile.gain.measuredAt)})
          </p>
          <Toggle label="Use measured gain" checked={c.gainStages} onChange={(v) => patch({ calibration: { gainStages: v } })} />
        </>
      )}

      <h4>3. Flat field</h4>
      <p className="hint">
        Removes vignetting, dust shadows and colour shading. Defocus fully on a plain white surface lit the way you work. Measured for the current resolution ({sizeName(s.sizeIndex)}); re-measure if you move the light.
      </p>
      {lightChoice(flatLight, setFlatLight)}
      <div className="row wrap">
        <button disabled={!!running} onClick={() => run({ step: 'flatField', light: flatLight })}>
          Measure for {sizeName(s.sizeIndex)}
        </button>
      </div>
      {profile && Object.keys(profile.flatField).length > 0 && (
        <>
          <ul className="calib-list">
            {Object.entries(profile.flatField).map(([k, f]) => (
              <li key={k}>
                {sizeName(k)} ({when(f.measuredAt)}) <button onClick={() => forget(`flatField-${k}`)}>Forget</button>
              </li>
            ))}
          </ul>
          <Toggle label="Apply flat field" checked={c.flatField} onChange={(v) => patch({ calibration: { flatField: v } })} />
        </>
      )}

      <h4>4. Scale</h4>
      <p className="hint">Calibrates the scale bar for one zoom setting. Repeat for each setting you use, and give each a name.</p>
      <label className="select">
        <span className="slider-label">Name</span>
        <input value={presetName} placeholder="e.g. 2× zoom" onChange={(e) => setPresetName(e.target.value)} />
      </label>
      <Segmented
        value={scaleKind}
        options={[
          ['lines', 'Ruler / micrometer'],
          ['screen', 'Screen grid'],
        ]}
        onChange={setScaleKind}
      />
      {scaleKind === 'lines' ? (
        <>
          <p className="hint">Focus on evenly spaced lines filling the view (several lines visible).</p>
          <label className="select">
            <span className="slider-label">Line spacing</span>
            <select value={spacing} onChange={(e) => setSpacing(Number(e.target.value))}>
              {LINE_PRESETS.map(([n, v]) => (
                <option key={v} value={v}>
                  {n}
                </option>
              ))}
            </select>
          </label>
        </>
      ) : (
        <>
          <p className="hint">Focus on the screen target. Enter the screen's size so the grid pitch is known.</p>
          <label className="select">
            <span className="slider-label">Diagonal (in)</span>
            <input type="number" step="0.1" value={diagonal} onChange={(e) => setDiagonal(Number(e.target.value))} />
          </label>
          <label className="select">
            <span className="slider-label">Resolution</span>
            <input value={screenRes} onChange={(e) => setScreenRes(e.target.value)} />
          </label>
          <label className="select">
            <span className="slider-label">Grid every (px)</span>
            <input type="number" min={4} value={period} onChange={(e) => setPeriod(Number(e.target.value))} />
          </label>
          <p className="hint">Pixel pitch {pitchUm ? `${pitchUm.toFixed(1)} µm` : '?'}; lines every {pitchUm ? `${(pitchUm * period / 1000).toFixed(2)} mm` : '?'}.</p>
        </>
      )}
      <div className="row wrap">
        <button
          disabled={!!running || !presetName.trim() || (scaleKind === 'screen' && !pitchUm)}
          onClick={() => {
            const source: ScaleSource = scaleKind === 'lines' ? { source: 'lines', spacingUm: spacing } : { source: 'screen', pixelPitchUm: pitchUm, periodPx: period }
            run({ step: 'scale', name: presetName.trim(), ...source })
          }}
        >
          Measure
        </button>
      </div>
      <div className="calib-list">
        <label className="toggle">
          <input type="radio" checked={c.scalePreset === null} onChange={() => patch({ calibration: { scalePreset: null } })} />
          <span>None (use objective × adapter below)</span>
        </label>
        {profile?.scale.map((p) => (
          <div key={p.name} className="row">
            <label className="toggle">
              <input type="radio" checked={c.scalePreset === p.name} onChange={() => patch({ calibration: { scalePreset: p.name } })} />
              <span>
                {p.name}: {p.umPerSensorPixel.toFixed(3)} µm/px
              </span>
            </label>
            <button className="icon" title="Forget" onClick={() => forget(`scale-${p.name}`)}>
              ✕
            </button>
          </div>
        ))}
      </div>

      <h4>5. Colour</h4>
      <p className="hint">Place a 24-patch colour chart in view, in focus and lit as you work. Then draw a box tightly around the patches.</p>
      <div className="row wrap">
        <button disabled={!!running} onClick={startChartBox}>
          Draw box around chart
        </button>
        {profile?.color && <button onClick={() => forget('color')}>Forget</button>}
      </div>
      {profile?.color && (
        <>
          <p className="hint">
            Fitted, mean error {profile.color.error.toFixed(1)} ({when(profile.color.measuredAt)})
          </p>
          <Toggle label="Use measured colour" checked={c.color} onChange={(v) => patch({ calibration: { color: v } })} />
        </>
      )}
    </div>
  )
}
