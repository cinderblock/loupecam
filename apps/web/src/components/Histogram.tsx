import { useEffect, useRef } from 'react'

/** Luma histogram (display space), log-compressed so small populations stay visible. */
export function Histogram({ bins, clipped }: { bins: number[]; clipped: number }) {
  const ref = useRef<HTMLCanvasElement>(null)
  useEffect(() => {
    const c = ref.current!
    const dpr = window.devicePixelRatio || 1
    const w = (c.width = c.clientWidth * dpr)
    const h = (c.height = c.clientHeight * dpr)
    const g = c.getContext('2d')!
    g.clearRect(0, 0, w, h)
    if (!bins.length) return
    const max = Math.log1p(Math.max(...bins))
    const bw = w / bins.length
    g.fillStyle = '#9ab'
    bins.forEach((n, i) => {
      const bh = max > 0 ? (Math.log1p(n) / max) * h : 0
      g.fillRect(i * bw, h - bh, Math.max(1, bw - dpr), bh)
    })
    if (clipped > 0.001) {
      g.fillStyle = '#e44'
      g.fillRect(w - 3 * dpr, 0, 3 * dpr, h)
    }
  }, [bins, clipped])
  return <canvas className="histogram" ref={ref} title={clipped > 0.001 ? `${(clipped * 100).toFixed(1)}% clipped` : undefined} />
}
