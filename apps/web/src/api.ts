import type { CaptureEntry, CaptureInfo, NormRect, Patch, Settings, UpdateStatus } from './types'

// The server may require a token (when exposed on a network). It is kept in
// localStorage and sent as a Bearer header, or as ?token= where headers are
// impossible (<img>, WebSocket).
const TOKEN_KEY = 'loupecam-token'

export function getToken(): string | null {
  const fromUrl = new URLSearchParams(location.search).get('token')
  if (fromUrl) localStorage.setItem(TOKEN_KEY, fromUrl)
  return localStorage.getItem(TOKEN_KEY)
}

export function setToken(t: string | null) {
  if (t) localStorage.setItem(TOKEN_KEY, t)
  else localStorage.removeItem(TOKEN_KEY)
}

export class AuthError extends Error {}

/** Append the token to a URL used by an element that can't send headers. */
export function withToken(url: string): string {
  const t = getToken()
  if (!t) return url
  return url + (url.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(t)
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {}
  const t = getToken()
  if (t) headers.Authorization = `Bearer ${t}`
  if (body !== undefined) headers['Content-Type'] = 'application/json'
  const r = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) })
  if (r.status === 401) throw new AuthError('token required')
  if (!r.ok) {
    const msg = await r.json().then((j) => j.error as string, () => r.statusText)
    throw new Error(msg)
  }
  return r.status === 204 ? (undefined as T) : r.json()
}

export const api = {
  patchSettings: (p: Patch<Settings>) => request<Settings>('PATCH', '/api/settings', p),
  capture: (fullResolution?: boolean) =>
    request<CaptureInfo>('POST', '/api/capture' + (fullResolution === undefined ? '' : `?fullResolution=${fullResolution}`)),
  captures: () => request<CaptureEntry[]>('GET', '/api/captures'),
  deleteCapture: (name: string) => request<void>('DELETE', `/api/captures/${encodeURIComponent(name)}`),
  captureUrl: (name: string) => withToken(`/api/captures/${encodeURIComponent(name)}`),
  whiteBalance: (region: NormRect | null) => request<{ gains: [number, number, number] }>('POST', '/api/white-balance', { region }),
  roi: (region: NormRect | null) => request<Settings>('POST', '/api/roi', { region }),
  checkUpdate: () => request<UpdateStatus>('POST', '/api/update/check'),
  installUpdate: () => request<{ installed: string; restarting: boolean }>('POST', '/api/update/install'),
}

export function wsUrl(): string {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return withToken(`${proto}//${location.host}/api/ws`)
}
