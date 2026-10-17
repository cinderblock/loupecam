// Type-checked only (`bun run typecheck`): the public typings, as a user sees them.
import { open, openSimulated, type DevelopOptions, type Frame, type Image } from '..'

export async function usage(): Promise<void> {
  await using cam = openSimulated()
  const real = await open().catch(() => null)
  await real?.close()

  const stream = await cam.start({ size: 2, mode: 'raw12', exposureUs: 30_000, roi: { x: 0, y: 0, width: 400, height: 300 } })
  for await (const frame of stream) {
    const f: Frame = frame
    const raw: Uint8Array | Uint16Array = f.raw()
    const options: DevelopOptions = { demosaic: 'mhc', whiteBalance: [1.5, 1, 1.7], rotation: 90, depth: 16 }
    const img: Image = await f.develop(options)
    const png: Buffer = await f.encode('png', { quality: 90, raw: false })
    const [r, g, b] = f.whiteBalance({ x: 0, y: 0, width: 10, height: 10 })
    console.log(raw.length, img.width, png.length, r + g + b, cam.geometry?.maxFps)
    break
  }
  // @ts-expect-error: not a demosaic algorithm
  await stream.read().then((f) => f.develop({ demosaic: 'magic' }))
}
