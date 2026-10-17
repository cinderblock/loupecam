export function formatExposure(us: number): string {
  if (us >= 1e6) return `${(us / 1e6).toFixed(2)} s`
  if (us >= 1000) return `${(us / 1000).toFixed(us >= 10000 ? 1 : 2)} ms`
  return `${us.toFixed(0)} µs`
}

/** `m:ss`, or `h:mm:ss`. */
export function formatDuration(s: number): string {
  const t = Math.floor(s)
  const hms = [Math.floor(t / 3600), Math.floor(t / 60) % 60, t % 60]
  return (hms[0] ? `${hms[0]}:` : '') + hms.slice(1).map((v) => String(v).padStart(2, '0')).join(':')
}

export function formatBytes(n: number): string {
  if (n > 1e9) return `${(n / 1e9).toFixed(1)} GB`
  if (n > 1e6) return `${(n / 1e6).toFixed(1)} MB`
  if (n > 1e3) return `${(n / 1e3).toFixed(0)} kB`
  return `${n} B`
}
