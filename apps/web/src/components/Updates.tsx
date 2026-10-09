import { useState } from 'react'
import { api } from '../api'
import type { Patch, Settings, UpdateStatus } from '../types'
import { Select, Toggle } from './widgets'

/** Banner shown when a newer release exists. */
export function UpdateBanner({ u, onError }: { u: UpdateStatus; onError: (m: string) => void }) {
  const [dismissed, setDismissed] = useState<string | null>(null)
  const rel = u.available
  if (!rel || dismissed === rel.version) return null
  return (
    <div className="update-banner">
      <span>
        LoupeCam <strong>{rel.version}</strong> is available (you have {u.currentVersion}).{' '}
        <a href={rel.url} target="_blank" rel="noreferrer">
          Release notes
        </a>
      </span>
      <span className="spacer" />
      {u.enabled && rel.installable ? (
        <button className="primary" disabled={u.installing} onClick={() => api.installUpdate().catch((e) => onError(e.message))}>
          {u.installing ? 'Installing…' : 'Update and restart'}
        </button>
      ) : !u.enabled ? (
        <span className="muted">Use File → Check for Updates</span>
      ) : (
        <span className="muted">No build for this platform; download manually</span>
      )}
      <button onClick={() => setDismissed(rel.version)}>Later</button>
    </div>
  )
}

export function UpdatesPanel({ u, s, patch }: { u: UpdateStatus | null; s: Settings; patch: (p: Patch<Settings>) => void }) {
  const [checking, setChecking] = useState(false)
  if (!u) return null
  return (
    <>
      <div className="row">
        <span>Version {u.currentVersion}</span>
        {u.enabled && (
          <button
            disabled={checking || u.checking}
            onClick={async () => {
              setChecking(true)
              await api.checkUpdate().catch(() => {})
              setChecking(false)
            }}
          >
            {checking || u.checking ? 'Checking…' : 'Check now'}
          </button>
        )}
      </div>
      {u.enabled ? (
        <>
          <Toggle label="Install updates automatically" checked={s.updates.autoInstall} onChange={(v) => patch({ updates: { autoInstall: v } })} title="Off: you are notified and choose when to update" />
          <Select
            label="Check every"
            value={s.updates.checkIntervalHours}
            options={[
              [1, 'hour'],
              [6, '6 hours'],
              [24, 'day'],
              [168, 'week'],
            ]}
            onChange={(v) => patch({ updates: { checkIntervalHours: v } })}
          />
          <p className="hint">
            {u.available ? `Version ${u.available.version} is available.` : u.lastCheck ? `Up to date (checked ${new Date(u.lastCheck).toLocaleString()}).` : 'Not checked yet.'}
            {u.lastError && ` Last check failed: ${u.lastError}`}
          </p>
        </>
      ) : (
        <p className="hint">The desktop app updates itself: File → Check for Updates, or turn on automatic updates there.</p>
      )}
    </>
  )
}
