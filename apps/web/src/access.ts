import { useCallback, useEffect, useState } from 'react'
import { api } from './api'
import type { AccessInfo, Role } from './types'

/** The server's access settings and the caller's role (`GET /api/access`). */
export function useAccess() {
  const [info, setInfo] = useState<AccessInfo | null>(null)
  const [error, setError] = useState<string | null>(null)
  const refresh = useCallback(() => {
    api.access().then(
      (i) => {
        setInfo(i)
        setError(null)
      },
      (e: Error) => setError(e.message),
    )
  }, [])
  useEffect(refresh, [refresh])
  return { info, setInfo, refresh, error }
}

export type AccessState = ReturnType<typeof useAccess>

export const ROLE_LABEL: Record<Role, string> = { view: 'Viewer', operate: 'Operator', admin: 'Admin' }

export function roleLabel(role: Role | null): string {
  return role ? ROLE_LABEL[role] : 'Not logged in'
}
