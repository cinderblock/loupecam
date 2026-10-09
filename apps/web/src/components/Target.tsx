import { useEffect, useRef, useState } from 'react'
import { wsUrl } from '../api'
import type { TargetPattern } from '../types'

/**
 * Screen calibration target: open this page (/target) full screen on a display placed
 * under the optics. The calibration wizard tells it what to show (an even white field,
 * or a grid of known pitch) and waits until it has been drawn.
 */
export function TargetPage() {
  const canvas = useRef<HTMLCanvasElement>(null)
  const [pattern, setPattern] = useState<TargetPattern>({ kind: 'idle' })
  const [connected, setConnected] = useState(false)
  const pendingAck = useRef<number | null>(null)
  const ws = useRef<WebSocket | null>(null)

  useEffect(() => {
    let alive = true
    let timer: number | undefined
    const connect = () => {
      const sock = new WebSocket(wsUrl())
      ws.current = sock
      sock.onopen = () => {
        setConnected(true)
        sock.send(JSON.stringify({ type: 'target' }))
      }
      sock.onmessage = (ev) => {
        if (typeof ev.data !== 'string') return
        const msg = JSON.parse(ev.data)
        if (msg.type === 'targetShow') {
          pendingAck.current = msg.id
          setPattern(msg.pattern as TargetPattern)
        }
      }
      sock.onclose = () => {
        setConnected(false)
        if (alive) timer = window.setTimeout(connect, 1000)
      }
    }
    connect()
    return () => {
      alive = false
      window.clearTimeout(timer)
      ws.current?.close()
    }
  }, [])

  // Draw in physical (device) pixels so grid periods are exact.
  useEffect(() => {
    const c = canvas.current
    if (!c) return
    const draw = () => {
      const dpr = window.devicePixelRatio || 1
      const w = (c.width = Math.round(window.innerWidth * dpr))
      const h = (c.height = Math.round(window.innerHeight * dpr))
      const g = c.getContext('2d')!
      g.imageSmoothingEnabled = false
      if (pattern.kind === 'solid') {
        const v = Math.round(Math.min(1, Math.max(0, pattern.level)) * 255)
        g.fillStyle = `rgb(${v},${v},${v})`
        g.fillRect(0, 0, w, h)
      } else if (pattern.kind === 'grid') {
        g.fillStyle = '#000'
        g.fillRect(0, 0, w, h)
        g.fillStyle = '#fff'
        for (let x = 0; x < w; x += pattern.period) g.fillRect(x, 0, pattern.line, h)
        for (let y = 0; y < h; y += pattern.period) g.fillRect(0, y, w, pattern.line)
      } else {
        g.fillStyle = '#16181b'
        g.fillRect(0, 0, w, h)
      }
      // Acknowledge once the frame has actually been presented.
      const id = pendingAck.current
      if (id !== null) {
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            ws.current?.send(JSON.stringify({ type: 'targetAck', id }))
            if (pendingAck.current === id) pendingAck.current = null
          }),
        )
      }
    }
    draw()
    window.addEventListener('resize', draw)
    return () => window.removeEventListener('resize', draw)
  }, [pattern])

  const dpr = window.devicePixelRatio || 1
  return (
    <div className="target-page">
      <canvas ref={canvas} />
      {pattern.kind === 'idle' && (
        <div className="target-info">
          <h2>LoupeCam screen target</h2>
          <p>Place this screen under the scope and keep this page open. The calibration wizard (in LoupeCam on any device) will drive it.</p>
          <p className="muted">
            {connected ? 'Connected.' : 'Connecting…'} Screen: {Math.round(screen.width * dpr)} × {Math.round(screen.height * dpr)} physical pixels (scale {dpr}).
          </p>
          <button onClick={() => document.documentElement.requestFullscreen?.()}>Full screen</button>
        </div>
      )}
    </div>
  )
}
