import { useEffect, useRef, useState } from 'react'
import type { NormRect } from '../types'

export type Tool = 'pan' | 'roi' | 'wb'

export interface Overlays {
  crosshair: boolean
  grid: boolean
  scaleBar: boolean
}

interface Props {
  frame: ImageBitmap | null
  tool: Tool
  overlays: Overlays
  /** Micrometres per preview pixel, for the scale bar (null = unknown). */
  umPerPixel: number | null
  /** Called with a region drawn with the ROI or WB tool. */
  onRegion: (tool: Tool, r: NormRect) => void
  message?: string
}

interface View {
  /** Image pixels per screen pixel is 1/zoom; null = fit to window. */
  zoom: number | null
  /** Image coordinate at the canvas centre. */
  cx: number
  cy: number
}

/** Pick a "nice" scale-bar length (1, 2, 5 × 10ⁿ µm) near `target` µm. */
function niceLength(target: number): number {
  const p = Math.pow(10, Math.floor(Math.log10(target)))
  const m = target / p
  return (m >= 5 ? 5 : m >= 2 ? 2 : 1) * p
}

export function Viewer({ frame, tool, overlays, umPerPixel, onRegion, message }: Props) {
  const canvas = useRef<HTMLCanvasElement>(null)
  const [view, setView] = useState<View>({ zoom: null, cx: 0, cy: 0 })
  const [size, setSize] = useState({ w: 1, h: 1 })
  const drag = useRef<{ x: number; y: number; cx: number; cy: number } | null>(null)
  const [sel, setSel] = useState<{ x0: number; y0: number; x1: number; y1: number } | null>(null)

  // Track canvas size (in device pixels).
  useEffect(() => {
    const c = canvas.current!
    const ro = new ResizeObserver(() => {
      const r = c.getBoundingClientRect()
      const dpr = window.devicePixelRatio || 1
      setSize({ w: Math.max(1, Math.round(r.width * dpr)), h: Math.max(1, Math.round(r.height * dpr)) })
    })
    ro.observe(c)
    return () => ro.disconnect()
  }, [])

  const fw = frame?.width ?? 1
  const fh = frame?.height ?? 1
  const fitZoom = Math.min(size.w / fw, size.h / fh)
  const zoom = view.zoom ?? fitZoom
  const cx = view.zoom === null ? fw / 2 : view.cx
  const cy = view.zoom === null ? fh / 2 : view.cy
  // Screen (device px) <-> image px.
  const toImage = (sx: number, sy: number) => ({ x: (sx - size.w / 2) / zoom + cx, y: (sy - size.h / 2) / zoom + cy })

  useEffect(() => {
    const c = canvas.current!
    c.width = size.w
    c.height = size.h
    const g = c.getContext('2d')!
    g.fillStyle = '#111'
    g.fillRect(0, 0, size.w, size.h)
    if (!frame) return
    g.imageSmoothingEnabled = zoom < 2
    const ox = size.w / 2 - cx * zoom
    const oy = size.h / 2 - cy * zoom
    g.drawImage(frame, ox, oy, fw * zoom, fh * zoom)
    const dpr = window.devicePixelRatio || 1
    g.lineWidth = dpr
    if (overlays.grid) {
      g.strokeStyle = 'rgba(255,255,255,0.35)'
      g.beginPath()
      for (let i = 1; i < 3; i++) {
        g.moveTo(ox + (fw * zoom * i) / 3, oy)
        g.lineTo(ox + (fw * zoom * i) / 3, oy + fh * zoom)
        g.moveTo(ox, oy + (fh * zoom * i) / 3)
        g.lineTo(ox + fw * zoom, oy + (fh * zoom * i) / 3)
      }
      g.stroke()
    }
    if (overlays.crosshair) {
      const mx = ox + (fw * zoom) / 2
      const my = oy + (fh * zoom) / 2
      g.strokeStyle = 'rgba(255,60,60,0.9)'
      g.beginPath()
      g.moveTo(mx - 30 * dpr, my)
      g.lineTo(mx + 30 * dpr, my)
      g.moveTo(mx, my - 30 * dpr)
      g.lineTo(mx, my + 30 * dpr)
      g.stroke()
      g.beginPath()
      g.arc(mx, my, 12 * dpr, 0, Math.PI * 2)
      g.stroke()
    }
    if (overlays.scaleBar && umPerPixel) {
      const umPerScreen = umPerPixel / zoom
      const len = niceLength(umPerScreen * size.w * 0.15)
      const px = len / umPerScreen
      const x = size.w - px - 24 * dpr
      const y = size.h - 28 * dpr
      g.fillStyle = 'rgba(0,0,0,0.55)'
      g.fillRect(x - 8 * dpr, y - 22 * dpr, px + 16 * dpr, 34 * dpr)
      g.fillStyle = '#fff'
      g.fillRect(x, y, px, 4 * dpr)
      g.font = `${12 * dpr}px system-ui, sans-serif`
      g.textAlign = 'center'
      g.fillText(len >= 1000 ? `${len / 1000} mm` : `${len} µm`, x + px / 2, y - 6 * dpr)
    }
    if (sel) {
      const a = { x: Math.min(sel.x0, sel.x1), y: Math.min(sel.y0, sel.y1) }
      const w = Math.abs(sel.x1 - sel.x0)
      const h = Math.abs(sel.y1 - sel.y0)
      g.strokeStyle = tool === 'wb' ? '#ffd23f' : '#3fa9ff'
      g.setLineDash([6 * dpr, 4 * dpr])
      g.strokeRect(ox + a.x * zoom, oy + a.y * zoom, w * zoom, h * zoom)
      g.setLineDash([])
    }
  }, [frame, size, zoom, cx, cy, overlays, umPerPixel, sel, tool, fw, fh])

  const dev = (e: { clientX: number; clientY: number }) => {
    const r = canvas.current!.getBoundingClientRect()
    const dpr = window.devicePixelRatio || 1
    return { x: (e.clientX - r.left) * dpr, y: (e.clientY - r.top) * dpr }
  }

  return (
    <div className="viewer">
      <canvas
        ref={canvas}
        className={`tool-${tool}`}
        onWheel={(e) => {
          if (!frame) return
          const p = dev(e)
          const before = toImage(p.x, p.y)
          const nz = Math.min(32, Math.max(fitZoom / 4, zoom * Math.pow(1.0015, -e.deltaY)))
          // Keep the point under the cursor fixed.
          setView({ zoom: nz, cx: before.x - (p.x - size.w / 2) / nz, cy: before.y - (p.y - size.h / 2) / nz })
        }}
        onPointerDown={(e) => {
          ;(e.target as Element).setPointerCapture(e.pointerId)
          const p = dev(e)
          if (tool === 'pan' || e.button === 1) {
            drag.current = { x: p.x, y: p.y, cx, cy }
          } else {
            const i = toImage(p.x, p.y)
            setSel({ x0: i.x, y0: i.y, x1: i.x, y1: i.y })
          }
        }}
        onPointerMove={(e) => {
          const p = dev(e)
          if (drag.current) {
            const d = drag.current
            setView({ zoom, cx: d.cx - (p.x - d.x) / zoom, cy: d.cy - (p.y - d.y) / zoom })
          } else if (sel) {
            const i = toImage(p.x, p.y)
            setSel({ ...sel, x1: i.x, y1: i.y })
          }
        }}
        onPointerUp={() => {
          drag.current = null
          if (sel && frame) {
            const clamp = (v: number, m: number) => Math.min(1, Math.max(0, v / m))
            const r: NormRect = {
              x: clamp(Math.min(sel.x0, sel.x1), fw),
              y: clamp(Math.min(sel.y0, sel.y1), fh),
              width: 0,
              height: 0,
            }
            r.width = clamp(Math.max(sel.x0, sel.x1), fw) - r.x
            r.height = clamp(Math.max(sel.y0, sel.y1), fh) - r.y
            if (r.width > 0.005 && r.height > 0.005) onRegion(tool, r)
            setSel(null)
          }
        }}
        onDoubleClick={() => setView({ zoom: null, cx: 0, cy: 0 })}
      />
      {message && <div className="viewer-message">{message}</div>}
      <div className="viewer-zoom">
        <button onClick={() => setView({ zoom: null, cx: 0, cy: 0 })} title="Fit (F)">
          Fit
        </button>
        <button onClick={() => setView({ zoom: 1, cx, cy })} title="Actual pixels (1)">
          1:1
        </button>
        <span>{Math.round(zoom * 100)}%</span>
      </div>
    </div>
  )
}
