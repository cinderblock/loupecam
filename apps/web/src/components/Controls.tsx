import type { DeviceSummary, LiveStats, Patch, Settings } from '../types'
import { formatExposure } from '../format'
import { Section, Segmented, Select, Slider, Toggle } from './widgets'

interface Props {
  s: Settings
  device: DeviceSummary | null
  stats: LiveStats | null
  patch: (p: Patch<Settings>) => void
  onWhiteBalanceOnce: () => void
  onClearRoi: () => void
}

/** The vendor software's default (colour temperature 6503 K). */
const DAYLIGHT: [number, number, number] = [1.53, 1.0, 1.71]

export function Controls({ s, device, stats, patch, onWhiteBalanceOnce, onClearRoi }: Props) {
  const maxGain = device?.maxGain ?? 16
  const resolutions = device?.resolutions ?? [[0, 0]]
  return (
    <>
      <Section title="Exposure" right={<Toggle label="Auto" checked={s.autoExposure} onChange={(v) => patch({ autoExposure: v })} />}>
        <Slider
          label="Time"
          value={stats?.exposureUs || s.exposureUs}
          min={50}
          max={2_000_000}
          log
          disabled={s.autoExposure}
          format={formatExposure}
          onChange={(v) => patch({ exposureUs: Math.round(v) })}
        />
        <Slider label="Gain" value={s.gain} min={1} max={maxGain} log disabled={s.autoExposure} format={(v) => `${v.toFixed(2)}×`} onChange={(v) => patch({ gain: v })} />
        {s.autoExposure && (
          <>
            <Slider label="Target" value={s.aeTarget} min={0.02} max={0.6} log format={(v) => `${(v * 100).toFixed(0)}%`} onChange={(v) => patch({ aeTarget: v })} title="Mean brightness auto exposure aims for (linear; 18% = mid grey)" />
            <Slider label="Max time" value={s.aeMaxExposureUs} min={1000} max={2_000_000} log format={formatExposure} onChange={(v) => patch({ aeMaxExposureUs: Math.round(v) })} />
            <Slider label="Max gain" value={s.aeMaxGain} min={1} max={maxGain} log format={(v) => `${v.toFixed(1)}×`} onChange={(v) => patch({ aeMaxGain: v })} />
          </>
        )}
        <Select
          label="Anti-flicker"
          value={s.antiFlicker}
          options={[
            ['off', 'Off'],
            ['50hz', '50 Hz'],
            ['60hz', '60 Hz'],
          ]}
          onChange={(v) => patch({ antiFlicker: v })}
        />
      </Section>

      <Section title="Sensor">
        <Select
          label="Resolution"
          value={s.sizeIndex}
          options={resolutions.map(([w, h], i) => [i, `${w} × ${h}`] as [number, string])}
          onChange={(v) => patch({ sizeIndex: v, roi: null })}
        />
        <Segmented
          value={s.bitDepth}
          options={[
            [8, '8-bit'],
            [12, '12-bit'],
          ]}
          onChange={(v) => patch({ bitDepth: v })}
        />
        <Select
          label="Readout speed"
          value={s.speed}
          options={[
            [0, 'Slowest (least noise)'],
            [1, 'Slow'],
            [2, 'Fast'],
            [3, 'Fastest'],
          ]}
          onChange={(v) => patch({ speed: v })}
        />
        {s.sizeIndex > 0 && <Toggle label="Binning (sum pixels instead of skipping)" checked={s.binning} onChange={(v) => patch({ binning: v })} />}
        <div className="row">
          <span className="muted">ROI: {s.roi ? `${s.roi.width}×${s.roi.height} at ${s.roi.x},${s.roi.y}` : 'full frame'}</span>
          {s.roi && <button onClick={onClearRoi}>Clear ROI</button>}
        </div>
      </Section>

      <Section
        title="White balance"
        right={
          <Segmented
            value={s.whiteBalance.mode}
            options={[
              ['manual', 'Manual'],
              ['auto', 'Auto'],
            ]}
            onChange={(v) => patch({ whiteBalance: { mode: v } })}
          />
        }
      >
        {(['Red', 'Green', 'Blue'] as const).map((name, i) => (
          <Slider
            key={name}
            label={name}
            value={s.whiteBalance.gains[i]}
            min={0.25}
            max={4}
            log
            disabled={s.whiteBalance.mode === 'auto'}
            format={(v) => v.toFixed(2)}
            onChange={(v) => {
              const g = [...s.whiteBalance.gains] as [number, number, number]
              g[i] = v
              patch({ whiteBalance: { gains: g } })
            }}
          />
        ))}
        <div className="row wrap">
          <button onClick={() => patch({ whiteBalance: { mode: 'manual', gains: DAYLIGHT } })} title="The vendor software's default">
            Daylight
          </button>
          <button onClick={onWhiteBalanceOnce} title="Grey-world balance over the whole frame">
            Once
          </button>
        </div>
        <p className="hint">Tip: pick the white-balance tool and drag over something white or grey.</p>
      </Section>

      <Section title="Color">
        <Toggle label="Color correction matrix" checked={s.color.correction} onChange={(v) => patch({ color: { correction: v } })} />
        <Slider label="Saturation" value={s.color.saturation} min={0} max={2} onChange={(v) => patch({ color: { saturation: v } })} />
        <Slider label="Hue" value={s.color.hue} min={-180} max={180} step={1} format={(v) => `${v.toFixed(0)}°`} onChange={(v) => patch({ color: { hue: v } })} />
        <div className="row wrap">
          <Toggle label="Monochrome" checked={s.color.monochrome} onChange={(v) => patch({ color: { monochrome: v } })} />
          <Toggle label="Negative" checked={s.color.negative} onChange={(v) => patch({ color: { negative: v } })} />
        </div>
      </Section>

      <Section title="Tone">
        <Select
          label="Curve"
          value={typeof s.tone.curve === 'number' ? 'gamma' : s.tone.curve}
          options={[
            ['srgb', 'sRGB'],
            ['linear', 'Linear'],
            ['gamma', 'Gamma'],
          ]}
          onChange={(v) => patch({ tone: { curve: v === 'gamma' ? 2.2 : v } })}
        />
        {typeof s.tone.curve === 'number' && <Slider label="Gamma" value={s.tone.curve} min={0.5} max={4} onChange={(v) => patch({ tone: { curve: v } })} />}
        <Slider label="Brightness" value={s.tone.brightness} min={-0.5} max={0.5} onChange={(v) => patch({ tone: { brightness: v } })} />
        <Slider label="Contrast" value={s.tone.contrast} min={-1} max={1} onChange={(v) => patch({ tone: { contrast: v } })} />
        <Slider label="Black point" value={s.tone.blackPoint} min={0} max={0.5} format={(v) => `${(v * 100).toFixed(1)}%`} onChange={(v) => patch({ tone: { blackPoint: Math.min(v, s.tone.whitePoint - 0.01) } })} />
        <Slider label="White point" value={s.tone.whitePoint} min={0.05} max={1} format={(v) => `${(v * 100).toFixed(1)}%`} onChange={(v) => patch({ tone: { whitePoint: Math.max(v, s.tone.blackPoint + 0.01) } })} />
        <Slider label="Black level" value={s.tone.blackLevel} min={0} max={512} step={1} onChange={(v) => patch({ tone: { blackLevel: Math.round(v) } })} title="Sensor offset subtracted before processing (12-bit units)" />
      </Section>

      <Section title="Orientation" defaultOpen={false}>
        <div className="row wrap">
          <Toggle label="Flip ↔" checked={s.orientation.flipHorizontal} onChange={(v) => patch({ orientation: { flipHorizontal: v } })} />
          <Toggle label="Flip ↕" checked={s.orientation.flipVertical} onChange={(v) => patch({ orientation: { flipVertical: v } })} />
        </div>
        <Segmented
          value={s.orientation.rotation}
          options={[
            [0, '0°'],
            [90, '90°'],
            [180, '180°'],
            [270, '270°'],
          ]}
          onChange={(v) => patch({ orientation: { rotation: v } })}
        />
      </Section>

      <Section title="Preview" defaultOpen={false}>
        <Select
          label="Max width"
          value={s.preview.maxWidth}
          options={[
            [640, '640 px'],
            [1280, '1280 px'],
            [1920, '1920 px'],
            [2560, '2560 px'],
            [4912, 'Full'],
          ]}
          onChange={(v) => patch({ preview: { maxWidth: v } })}
        />
        <Slider label="Quality" value={s.preview.jpegQuality} min={30} max={98} step={1} onChange={(v) => patch({ preview: { jpegQuality: Math.round(v) } })} />
        <Slider label="Max fps" value={s.preview.maxFps} min={1} max={60} step={1} onChange={(v) => patch({ preview: { maxFps: Math.round(v) } })} />
        <Select
          label="Demosaic"
          value={s.preview.demosaic}
          options={[
            ['superpixel', 'Superpixel (fast, half-res)'],
            ['bilinear', 'Bilinear'],
            ['mhc', 'Malvar-He-Cutler (sharp)'],
          ]}
          onChange={(v) => patch({ preview: { demosaic: v } })}
        />
      </Section>
    </>
  )
}

export function CaptureSettingsPanel({ s, patch }: { s: Settings; patch: (p: Patch<Settings>) => void }) {
  const c = s.capture
  return (
    <>
      <Segmented
        value={c.format}
        options={[
          ['png', 'PNG'],
          ['tiff', 'TIFF'],
          ['jpeg', 'JPEG'],
        ]}
        onChange={(v) => patch({ capture: { format: v } })}
      />
      {c.format === 'jpeg' ? (
        <Slider label="Quality" value={c.jpegQuality} min={50} max={100} step={1} onChange={(v) => patch({ capture: { jpegQuality: Math.round(v) } })} />
      ) : (
        <Toggle label="16 bits per channel" checked={c.sixteenBit} onChange={(v) => patch({ capture: { sixteenBit: v } })} />
      )}
      <Toggle label="Full resolution (12-bit)" checked={c.fullResolution} onChange={(v) => patch({ capture: { fullResolution: v } })} />
      <Toggle label="Also save raw sensor data (TIFF)" checked={c.saveRaw} onChange={(v) => patch({ capture: { saveRaw: v } })} />
      <Select
        label="Demosaic"
        value={c.demosaic}
        options={[
          ['mhc', 'Malvar-He-Cutler (best)'],
          ['bilinear', 'Bilinear'],
          ['superpixel', 'Superpixel (half-res)'],
        ]}
        onChange={(v) => patch({ capture: { demosaic: v } })}
      />
    </>
  )
}
