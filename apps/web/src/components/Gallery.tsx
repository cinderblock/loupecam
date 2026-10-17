import { useCallback, useEffect, useState } from 'react'
import { api } from '../api'
import type { CaptureEntry } from '../types'
import { formatBytes } from '../format'

/** Saved captures, newest first. `version` changes trigger a refresh. */
export function Gallery({ version }: { version: number | string }) {
  const [items, setItems] = useState<CaptureEntry[]>([])
  const refresh = useCallback(() => {
    api.captures().then(setItems, () => {})
  }, [])
  useEffect(refresh, [refresh, version])
  if (!items.length) return <p className="muted">No captures yet.</p>
  return (
    <ul className="gallery">
      {items.slice(0, 50).map((c) => (
        <li key={c.name}>
          <a href={api.captureUrl(c.name, c.location)} target="_blank" rel="noreferrer" title={c.name.endsWith('.mp4') ? 'Play' : 'Open'}>
            {c.name.endsWith('.mp4') ? '▶ ' : ''}
            {c.name}
            {c.location === 'usb' && <span className="badge" title="On the USB stick">USB</span>}
          </a>
          <span className="muted">{formatBytes(c.bytes)}</span>
          <a href={api.captureUrl(c.name, c.location)} download={c.name} title="Download">
            ⤓
          </a>
          <button
            className="icon"
            title="Delete"
            onClick={async () => {
              if (confirm(`Delete ${c.name}?`)) {
                await api.deleteCapture(c.name, c.location).catch(() => {})
                refresh()
              }
            }}
          >
            ✕
          </button>
        </li>
      ))}
    </ul>
  )
}
