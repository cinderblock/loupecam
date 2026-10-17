import { useCallback, useEffect, useState } from 'react'
import { api } from '../api'
import type { CaptureEntry } from '../types'
import { formatBytes } from '../format'
import { ConfirmButton } from './widgets'

/** Saved captures, newest first. `version` changes trigger a refresh. */
export function Gallery({ version, canDelete }: { version: number | string; canDelete: boolean }) {
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
          <a href={api.captureUrl(c.name)} target="_blank" rel="noreferrer" title={c.name.endsWith('.mp4') ? 'Play' : 'Open'}>
            {c.name.endsWith('.mp4') ? '▶ ' : ''}
            {c.name}
          </a>
          <span className="muted">{formatBytes(c.bytes)}</span>
          <a href={api.captureUrl(c.name)} download={c.name} title="Download">
            ⤓
          </a>
          {canDelete && (
          <ConfirmButton
            className="icon"
            title="Delete"
            question="Delete?"
            yes="Delete"
            onConfirm={async () => {
              await api.deleteCapture(c.name).catch(() => {})
              refresh()
            }}
          >
            ✕
          </ConfirmButton>
          )}
        </li>
      ))}
    </ul>
  )
}
