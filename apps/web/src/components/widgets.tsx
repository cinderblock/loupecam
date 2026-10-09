import { useState, type ReactNode } from 'react'

export function Section({ title, children, defaultOpen = true, right }: { title: string; children: ReactNode; defaultOpen?: boolean; right?: ReactNode }) {
  const [open, setOpen] = useState(() => {
    const v = localStorage.getItem(`section:${title}`)
    return v === null ? defaultOpen : v === '1'
  })
  const toggle = () => {
    localStorage.setItem(`section:${title}`, open ? '0' : '1')
    setOpen(!open)
  }
  return (
    <section className="section">
      <header onClick={toggle}>
        <span className={open ? 'chevron open' : 'chevron'}>▸</span>
        <h3>{title}</h3>
        <span className="section-right" onClick={(e) => e.stopPropagation()}>
          {right}
        </span>
      </header>
      {open && <div className="section-body">{children}</div>}
    </section>
  )
}

interface SliderProps {
  label: string
  value: number
  min: number
  max: number
  step?: number
  /** Logarithmic scale (min must be > 0). */
  log?: boolean
  disabled?: boolean
  format?: (v: number) => string
  onChange: (v: number) => void
  title?: string
}

/**
 * Slider with a numeric readout. While dragging, it shows the local value so server
 * updates arriving mid-drag don't make it jump.
 */
export function Slider({ label, value, min, max, step = 0.01, log, disabled, format, onChange, title }: SliderProps) {
  const [drag, setDrag] = useState<number | null>(null)
  const toPos = (v: number) => (log ? Math.log(v / min) / Math.log(max / min) : (v - min) / (max - min))
  const fromPos = (p: number) => {
    const v = log ? min * Math.pow(max / min, p) : min + p * (max - min)
    return log ? v : Math.round(v / step) * step
  }
  const shown = drag ?? value
  const fmt = format ?? ((v: number) => (Number.isInteger(step) ? v.toFixed(0) : v.toFixed(2)))
  return (
    <label className={disabled ? 'slider disabled' : 'slider'} title={title}>
      <span className="slider-label">{label}</span>
      <input
        type="range"
        min={0}
        max={1000}
        value={Math.round(Math.min(1, Math.max(0, toPos(shown))) * 1000)}
        disabled={disabled}
        onPointerDown={() => setDrag(value)}
        onPointerUp={() => setDrag(null)}
        onChange={(e) => {
          const v = fromPos(Number(e.target.value) / 1000)
          setDrag(v)
          onChange(v)
        }}
      />
      <NumberField value={shown} format={fmt} disabled={disabled} onCommit={(v) => onChange(Math.min(max, Math.max(min, v)))} />
    </label>
  )
}

function NumberField({ value, format, disabled, onCommit }: { value: number; format: (v: number) => string; disabled?: boolean; onCommit: (v: number) => void }) {
  const [text, setText] = useState<string | null>(null)
  return (
    <input
      className="number"
      value={text ?? format(value)}
      disabled={disabled}
      onFocus={(e) => {
        setText(format(value))
        e.target.select()
      }}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => {
        const v = parseFloat(text ?? '')
        if (Number.isFinite(v)) onCommit(v)
        setText(null)
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
        if (e.key === 'Escape') {
          setText(null)
          ;(e.target as HTMLInputElement).blur()
        }
      }}
    />
  )
}

export function Toggle({ label, checked, onChange, disabled, title }: { label: string; checked: boolean; onChange: (v: boolean) => void; disabled?: boolean; title?: string }) {
  return (
    <label className="toggle" title={title}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>{label}</span>
    </label>
  )
}

export function Select<T extends string | number>({ label, value, options, onChange, disabled }: { label: string; value: T; options: [T, string][]; onChange: (v: T) => void; disabled?: boolean }) {
  return (
    <label className="select">
      <span className="slider-label">{label}</span>
      <select
        value={String(value)}
        disabled={disabled}
        onChange={(e) => {
          const o = options.find(([v]) => String(v) === e.target.value)
          if (o) onChange(o[0])
        }}
      >
        {options.map(([v, text]) => (
          <option key={String(v)} value={String(v)}>
            {text}
          </option>
        ))}
      </select>
    </label>
  )
}

export function Segmented<T extends string | number>({ value, options, onChange }: { value: T; options: [T, string][]; onChange: (v: T) => void }) {
  return (
    <div className="segmented">
      {options.map(([v, text]) => (
        <button key={String(v)} className={v === value ? 'active' : ''} onClick={() => onChange(v)}>
          {text}
        </button>
      ))}
    </div>
  )
}
