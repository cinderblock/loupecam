import { useCallback, useEffect, useState, type FormEvent, type ReactNode } from 'react'
import { type AccessState, ROLE_LABEL, roleLabel } from '../access'
import { api, setToken } from '../api'
import type { AccessInfo, AccessLevel, ConfigLock, KeyInfo, Levels, Role } from '../types'

interface Preset {
  id: string
  title: string
  access: AccessLevel
  config: ConfigLock
  text: string
  warning?: string
}

/** The security levels offered, from least to most protected. */
const PRESETS: Preset[] = [
  {
    id: 'open',
    title: 'Open',
    access: 'open',
    config: 'open',
    text: 'Anyone who can reach this server can watch, use and configure the camera.',
    warning: 'Not recommended: only for a private network you fully trust.',
  },
  {
    id: 'protected',
    title: 'Configuration protected',
    access: 'open',
    config: 'password',
    text: 'Anyone can watch and use the camera. Changing the configuration (access, updates, calibration) and deleting captures needs the admin password.',
  },
  {
    id: 'locked',
    title: 'Configuration locked',
    access: 'open',
    config: 'locked',
    text: 'Anyone can watch and use the camera. The configuration can only be changed on the server itself, with `loupecam access`.',
  },
  {
    id: 'passworded',
    title: 'Passworded',
    access: 'password',
    config: 'password',
    text: 'A password is needed to use the camera at all. A separate admin password, which also lets you in, changes the configuration.',
  },
  {
    id: 'passwordedLocked',
    title: 'Passworded, configuration locked',
    access: 'password',
    config: 'locked',
    text: 'A password is needed to use the camera at all. The configuration can only be changed on the server itself.',
  },
]

function presetOf(access: AccessInfo['access'], config: ConfigLock): Preset | undefined {
  return PRESETS.find((p) => p.access === access && p.config === config)
}

/** Text with `backticked` spans shown as code. */
function withCode(text: string): ReactNode {
  return text.split('`').map((part, i) => (i % 2 ? <code key={i}>{part}</code> : part))
}

function LevelCards({ value, onChange }: { value: string; onChange: (p: Preset) => void }) {
  return (
    <div className="levels">
      {PRESETS.map((p) => (
        <label key={p.id} className={value === p.id ? 'level selected' : 'level'}>
          <input type="radio" name="level" checked={value === p.id} onChange={() => onChange(p)} />
          <span>
            <strong>{p.title}</strong>
            <span className="hint">{withCode(p.text)}</span>
            {p.warning && <span className="warn">{p.warning}</span>}
          </span>
        </label>
      ))}
      <label className="level disabled" title="Coming in a later version">
        <input type="radio" name="level" disabled />
        <span>
          <strong>Accounts with passkeys</strong>
          <span className="hint">Users sign in with passkeys, and an admin user manages them. Needs HTTPS; not available yet.</span>
        </span>
      </label>
    </div>
  )
}

/** A password with a confirmation field. `optional`: blank means keep the current one. */
function PasswordPair({ label, value, onChange, optional }: { label: string; value: [string, string]; onChange: (v: [string, string]) => void; optional?: boolean }) {
  return (
    <div className="password-pair">
      <span>{label}</span>
      <input type="password" autoComplete="new-password" placeholder={optional ? 'Unchanged' : 'Password'} value={value[0]} onChange={(e) => onChange([e.target.value, value[1]])} />
      <input type="password" autoComplete="new-password" placeholder="Again" value={value[1]} onChange={(e) => onChange([value[0], e.target.value])} />
    </div>
  )
}

const MIN_PASSWORD = 8

/** Checks the password fields; returns an error to show, or the levels to send. */
function levelsFrom(p: Preset, access: [string, string], admin: [string, string], keepExisting: { access: boolean; admin: boolean }): string | Levels {
  const check = (v: [string, string], what: string, keep: boolean): string | null => {
    if (!v[0] && !v[1] && keep) return null
    if (v[0].length < MIN_PASSWORD) return `The ${what} password needs at least ${MIN_PASSWORD} characters.`
    if (v[0] !== v[1]) return `The two ${what} passwords don't match.`
    return null
  }
  const needAccess = p.access === 'password'
  const needAdmin = p.config === 'password'
  const err = (needAccess && check(access, 'access', keepExisting.access)) || (needAdmin && check(admin, 'admin', keepExisting.admin))
  if (err) return err
  if (needAccess && needAdmin && access[0] && access[0] === admin[0]) return 'Use different access and admin passwords.'
  return {
    access: p.access,
    config: p.config,
    accessPassword: needAccess && access[0] ? access[0] : undefined,
    adminPassword: needAdmin && admin[0] ? admin[0] : undefined,
  }
}

function PlainHttpWarning() {
  if (location.protocol !== 'http:' || ['localhost', '127.0.0.1', '[::1]'].includes(location.hostname)) return null
  return <p className="warn">This connection isn't encrypted (plain HTTP), so passwords cross the network readable by anyone on it.</p>
}

/** First-run setup: choose how the server is secured. */
export function SetupWizard({ info, onDone }: { info: AccessInfo; onDone: (i: AccessInfo) => void }) {
  const [code, setCode] = useState('')
  const [preset, setPreset] = useState<Preset>(PRESETS[1])
  const [access, setAccess] = useState<[string, string]>(['', ''])
  const [admin, setAdmin] = useState<[string, string]>(['', ''])
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    const levels = levelsFrom(preset, access, admin, { access: false, admin: false })
    if (typeof levels === 'string') return setError(levels)
    setBusy(true)
    try {
      onDone(await api.setup(levels, info.setupCodeRequired ? code : undefined))
    } catch (e) {
      setError((e as Error).message)
      setBusy(false)
    }
  }

  return (
    <form className="setup" onSubmit={submit}>
      <h2>Set up LoupeCam</h2>
      <p>
        This server is on the network and hasn't been set up yet. Choose who may use it. You can change this later under <em>Access &amp; security</em>.
      </p>
      {info.setupCodeRequired && (
        <label className="field">
          <span>Setup code</span>
          <input autoFocus value={code} onChange={(e) => setCode(e.target.value)} placeholder="XXXX-XXXX" autoComplete="off" />
          <span className="hint">Shown in the server's log when it started (e.g. `journalctl -u loupecam`).</span>
        </label>
      )}
      <LevelCards value={preset.id} onChange={setPreset} />
      {preset.access === 'password' && <PasswordPair label="Access password" value={access} onChange={setAccess} />}
      {preset.config === 'password' && <PasswordPair label="Admin password" value={admin} onChange={setAdmin} />}
      {(preset.access === 'password' || preset.config === 'password') && <PlainHttpWarning />}
      {preset.config === 'locked' && (
        <p className="hint">
          To change the levels later, run <code>loupecam access set</code> on the server. A lost password is reset with <code>loupecam access reset</code>.
        </p>
      )}
      {error && <p className="warn">{error}</p>}
      <button className="primary" type="submit" disabled={busy}>
        {busy ? 'Saving…' : 'Finish setup'}
      </button>
    </form>
  )
}

/** A password field that logs in; `onDone` gets the new access info. */
function LoginForm({ onDone, placeholder = 'Password', autoFocus }: { onDone: (i: AccessInfo) => void; placeholder?: string; autoFocus?: boolean }) {
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  return (
    <>
      <form
        onSubmit={async (e) => {
          e.preventDefault()
          setBusy(true)
          try {
            onDone(await api.login(password))
          } catch (e) {
            setError((e as Error).message)
            setBusy(false)
          }
        }}
      >
        <input type="password" autoComplete="current-password" placeholder={placeholder} value={password} onChange={(e) => setPassword(e.target.value)} autoFocus={autoFocus} />
        <button type="submit" disabled={busy || !password}>
          {busy ? 'Checking…' : 'Log in'}
        </button>
      </form>
      {error && <p className="warn">{error}</p>}
    </>
  )
}

/** Shown when the caller has no access. */
export function Login({ info, onDone }: { info: AccessInfo; onDone: (i: AccessInfo) => void }) {
  if (info.access === 'token') {
    return (
      <div className="auth">
        <h2>LoupeCam</h2>
        <p>This camera server requires an access token.</p>
        <form
          onSubmit={(e) => {
            e.preventDefault()
            setToken(new FormData(e.currentTarget).get('token') as string)
            location.reload()
          }}
        >
          <input name="token" type="password" placeholder="Token" autoFocus />
          <button type="submit">Connect</button>
        </form>
      </div>
    )
  }
  return (
    <div className="auth">
      <h2>LoupeCam</h2>
      <p>This camera is password protected.</p>
      <LoginForm onDone={onDone} autoFocus />
    </div>
  )
}

/** Renders `children` for admins; otherwise explains how to get admin access. */
export function AdminGate({ access, children }: { access: AccessState; children: ReactNode }) {
  const info = access.info
  if (!info || info.role === 'admin') return <>{children}</>
  if (info.config === 'locked') return <p className="hint">The configuration is locked. Change it on the server with <code>loupecam access</code>.</p>
  return (
    <div className="admin-gate">
      <p className="hint">Admin password required.</p>
      <LoginForm onDone={access.setInfo} placeholder="Admin password" />
    </div>
  )
}

function Keys({ flash }: { flash: (m: string) => void }) {
  const [keys, setKeys] = useState<KeyInfo[]>([])
  const [name, setName] = useState('')
  const [role, setRole] = useState<Role>('view')
  const [created, setCreated] = useState<{ name: string; key: string } | null>(null)
  const refresh = useCallback(() => {
    api.keys().then(setKeys, (e) => flash(e.message))
  }, [flash])
  useEffect(refresh, [refresh])
  return (
    <>
      <p className="hint">For clients that can't log in: Home Assistant or OctoPrint stream URLs (append <code>?token=KEY</code>), scripts (<code>Authorization: Bearer KEY</code>).</p>
      <ul className="keys">
        {keys.map((k) => (
          <li key={k.id}>
            <span>{k.name}</span>
            <span className="muted">
              {ROLE_LABEL[k.role]} · {k.lastUsed ? `used ${new Date(k.lastUsed * 1000).toLocaleDateString()}` : 'never used'}
            </span>
            <button
              className="icon"
              title="Revoke"
              onClick={async () => {
                if (confirm(`Revoke the key “${k.name}”? Clients using it lose access.`)) {
                  await api.deleteKey(k.id).catch((e) => flash(e.message))
                  refresh()
                }
              }}
            >
              ✕
            </button>
          </li>
        ))}
      </ul>
      {created && (
        <div className="new-key">
          <span>
            Key for <strong>{created.name}</strong>. Copy it now; it isn't shown again:
          </span>
          <code>{created.key}</code>
          <div className="row">
            <button onClick={() => navigator.clipboard.writeText(created.key).then(() => flash('Copied'), () => flash('Copy failed; select the key and copy it'))}>Copy</button>
            <button onClick={() => setCreated(null)}>Done</button>
          </div>
        </div>
      )}
      <form
        className="row"
        onSubmit={async (e) => {
          e.preventDefault()
          try {
            const r = await api.createKey(name, role)
            setCreated({ name: r.info.name, key: r.key })
            setName('')
            refresh()
          } catch (e) {
            flash((e as Error).message)
          }
        }}
      >
        <input placeholder="Name, e.g. Home Assistant" value={name} onChange={(e) => setName(e.target.value)} />
        <select value={role} onChange={(e) => setRole(e.target.value as Role)}>
          <option value="view">Viewer</option>
          <option value="operate">Operator</option>
          <option value="admin">Admin</option>
        </select>
        <button type="submit" disabled={!name.trim()}>
          Create key
        </button>
      </form>
    </>
  )
}

/** Change the levels and passwords (admins). */
function ChangeLevels({ access, flash }: { access: AccessState; flash: (m: string) => void }) {
  const info = access.info!
  const current = presetOf(info.access, info.config)
  const [preset, setPreset] = useState<Preset>(current ?? PRESETS[1])
  const [accessPw, setAccessPw] = useState<[string, string]>(['', ''])
  const [adminPw, setAdminPw] = useState<[string, string]>(['', ''])
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const keep = { access: info.access === 'password', admin: info.config === 'password' }
  return (
    <form
      onSubmit={async (e) => {
        e.preventDefault()
        const levels = levelsFrom(preset, accessPw, adminPw, keep)
        if (typeof levels === 'string') return setError(levels)
        if (preset.config === 'locked' && !confirm('Lock the configuration? After this it can only be changed on the server itself, with `loupecam access`.')) return
        setBusy(true)
        try {
          access.setInfo(await api.changeAccess(levels))
          setAccessPw(['', ''])
          setAdminPw(['', ''])
          setError(null)
          flash('Access settings saved. Everyone else has to log in again.')
        } catch (e) {
          setError((e as Error).message)
        } finally {
          setBusy(false)
        }
      }}
    >
      <LevelCards value={preset.id} onChange={setPreset} />
      {preset.access === 'password' && <PasswordPair label="Access password" value={accessPw} onChange={setAccessPw} optional={keep.access} />}
      {preset.config === 'password' && <PasswordPair label="Admin password" value={adminPw} onChange={setAdminPw} optional={keep.admin} />}
      {(preset.access === 'password' || preset.config === 'password') && <PlainHttpWarning />}
      {error && <p className="warn">{error}</p>}
      <button className="primary" type="submit" disabled={busy}>
        {busy ? 'Saving…' : 'Save'}
      </button>
    </form>
  )
}

/** The "Access & security" section. */
export function AccessPanel({ access, flash }: { access: AccessState; flash: (m: string) => void }) {
  const info = access.info
  if (!info) return null
  const level = info.setup === 'notNeeded' ? (info.access === 'token' ? 'Token (set on the command line)' : 'Open (local only)') : (presetOf(info.access, info.config)?.title ?? `${info.access} / ${info.config}`)
  return (
    <>
      <dl className="info">
        <dt>Level</dt>
        <dd>{level}</dd>
        <dt>You</dt>
        <dd>{roleLabel(info.role)}</dd>
      </dl>
      {info.session && (
        <div className="row">
          <button onClick={() => api.logout().then(access.setInfo, (e) => flash(e.message))}>Log out</button>
        </div>
      )}
      {info.setup === 'notNeeded' ? (
        <p className="hint">{info.access === 'token' ? 'Access is set by the server’s --token option.' : 'This server only listens on this computer, so it needs no protection.'}</p>
      ) : (
        <AdminGate access={access}>
          <h4>Security level</h4>
          <ChangeLevels access={access} flash={flash} />
          <h4>API keys</h4>
          <Keys flash={flash} />
          <h4>Sessions</h4>
          <div className="row">
            <button
              onClick={async () => {
                if (confirm('Log out everyone, including you?')) access.setInfo(await api.logoutAll().catch((e) => (flash(e.message), info)))
              }}
            >
              Log out everyone
            </button>
          </div>
        </AdminGate>
      )}
    </>
  )
}
