import { useEffect, useState } from 'react'
import type { CastInfo } from '../types'

// The display page the kiosk opens in a browser for casts it can't render itself
// (PDFs, web pages). It deliberately never uses the camera token: cast content is
// arbitrary, so this page only gets the display token (if one is set), from
// ?displayToken=.
const displayToken = new URLSearchParams(location.search).get('displayToken')

function withDisplayToken(url: string): string {
  return displayToken ? url + (url.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(displayToken) : url
}

export function CastPage() {
  const [cast, setCast] = useState<CastInfo | null | undefined>(undefined)
  const [error, setError] = useState(false)

  useEffect(() => {
    const es = new EventSource(withDisplayToken('/api/display/events'))
    es.addEventListener('display', (e) => {
      setCast((JSON.parse((e as MessageEvent).data) as { cast: CastInfo | null }).cast)
      setError(false)
    })
    es.onerror = () => setError(true)
    return () => es.close()
  }, [])

  const back = () => {
    const headers: Record<string, string> = displayToken ? { Authorization: `Bearer ${displayToken}` } : {}
    fetch('/api/display', { method: 'DELETE', headers }).catch(() => {})
  }

  // `id` busts caches and remounts the frame when the same kind is re-cast.
  const content = cast && withDisplayToken(`/api/display/content?v=${cast.id}`)
  return (
    <div className="cast">
      {cast === undefined ? (
        <p className="cast-empty muted">{error ? 'Cannot reach the camera server…' : 'Connecting…'}</p>
      ) : cast === null ? (
        <p className="cast-empty muted">Nothing is being cast.</p>
      ) : cast.kind === 'image' || cast.kind === 'svg' ? (
        <img key={cast.id} className="cast-image" src={content!} alt={cast.name ?? 'Cast image'} />
      ) : cast.kind === 'pdf' ? (
        <iframe key={cast.id} className="cast-frame" src={content! + '#view=FitH'} title={cast.name ?? 'Cast document'} />
      ) : cast.kind === 'html' ? (
        <iframe key={cast.id} className="cast-frame" src={content!} sandbox="allow-scripts" title={cast.name ?? 'Cast page'} />
      ) : (
        <iframe key={cast.id} className="cast-frame" src={cast.url!} title={cast.url!} />
      )}
      <button className="cast-back" onClick={back} title="Stop casting and show the camera">
        ✕ Camera
      </button>
    </div>
  )
}
