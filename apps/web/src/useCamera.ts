import { useCallback, useEffect, useRef, useState } from 'react'
import { api, AuthError, getToken, wsUrl } from './api'
import type { LiveStats, Patch, Settings, State, UpdateStatus } from './types'

export interface Camera {
  state: State | null
  stats: LiveStats | null
  update: UpdateStatus | null
  /** Latest preview frame, decoded. */
  frame: ImageBitmap | null
  connected: boolean
  authNeeded: boolean
  patch: (p: Patch<Settings>) => void
}

/** Merge a patch into settings locally (optimistic update). */
function merge<T>(base: T, patch: unknown): T {
  if (patch === null || typeof patch !== 'object' || Array.isArray(patch)) return patch as T
  const out = { ...(base as Record<string, unknown>) }
  for (const [k, v] of Object.entries(patch as Record<string, unknown>)) {
    out[k] = v === null ? null : merge(out[k], v)
  }
  return out as T
}

/**
 * Live connection to the camera server: state and stats over a WebSocket, preview
 * frames as binary messages on the same socket, reconnecting with backoff.
 */
export function useCamera(onError: (message: string) => void): Camera {
  const onErrorRef = useRef(onError)
  useEffect(() => {
    onErrorRef.current = onError
  })
  const [state, setState] = useState<State | null>(null)
  const [stats, setStats] = useState<LiveStats | null>(null)
  const [update, setUpdate] = useState<UpdateStatus | null>(null)
  const [frame, setFrame] = useState<ImageBitmap | null>(null)
  const [connected, setConnected] = useState(false)
  const [authNeeded, setAuthNeeded] = useState(false)
  const decoding = useRef(false)
  const pending = useRef<Patch<Settings> | null>(null)
  const inflight = useRef(false)

  useEffect(() => {
    let ws: WebSocket | null = null
    let retry = 0
    let timer: number | undefined
    let alive = true

    const connect = async () => {
      // A WebSocket can't report 401, so probe first.
      const probe = await fetch('/api/state', { headers: getToken() ? { Authorization: `Bearer ${getToken()}` } : {} }).catch(() => null)
      if (!alive) return
      if (probe?.status === 401) {
        setAuthNeeded(true)
        return
      }
      setAuthNeeded(false)
      ws = new WebSocket(wsUrl())
      ws.binaryType = 'blob'
      ws.onopen = () => {
        retry = 0
        setConnected(true)
        ws?.send(JSON.stringify({ type: 'preview', enabled: true }))
      }
      ws.onmessage = (ev) => {
        if (typeof ev.data === 'string') {
          const msg = JSON.parse(ev.data)
          if (msg.type === 'state') setState(msg as State)
          else if (msg.type === 'stats') setStats(msg as LiveStats)
          else if (msg.type === 'update') setUpdate(msg as UpdateStatus)
          return
        }
        // Skip frames while the previous one is still decoding.
        if (decoding.current) return
        decoding.current = true
        createImageBitmap(ev.data as Blob)
          .then((bmp) =>
            setFrame((old) => {
              old?.close()
              return bmp
            }),
          )
          .catch(() => {})
          .finally(() => {
            decoding.current = false
          })
      }
      ws.onclose = () => {
        setConnected(false)
        if (!alive) return
        timer = window.setTimeout(connect, Math.min(5000, 250 * 2 ** retry++))
      }
    }
    connect()
    return () => {
      alive = false
      window.clearTimeout(timer)
      ws?.close()
    }
  }, [authNeeded])

  // Send queued patches one request at a time; edits made meanwhile are merged and
  // sent next, so a dragged slider costs one request per round trip.
  const flush = useCallback(async () => {
    if (inflight.current) return
    inflight.current = true
    try {
      while (pending.current) {
        const p = pending.current
        pending.current = null
        try {
          await api.patchSettings(p)
        } catch (e) {
          if (e instanceof AuthError) setAuthNeeded(true)
          else onErrorRef.current(String((e as Error).message))
        }
      }
    } finally {
      inflight.current = false
    }
  }, [])

  const patch = useCallback(
    (p: Patch<Settings>) => {
      setState((s) => (s ? { ...s, settings: merge(s.settings, p) } : s))
      pending.current = pending.current ? merge(pending.current, p) : p
      flush()
    },
    [flush],
  )

  return { state, stats, update, frame, connected, authNeeded, patch }
}
