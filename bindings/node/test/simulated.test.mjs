// Tests against the simulated camera (no hardware needed). ESM on purpose: it checks
// that the CommonJS package's named exports are visible to `import`.
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { before, describe, it } from 'node:test'

import { Camera, listCameras, open, openSimulated, version } from '../index.js'

const SMALLEST = 2 // 1228×922

/** Run `fn` with a fresh simulated camera, closing it afterwards. */
async function withCamera(fn) {
  const cam = openSimulated()
  try {
    return await fn(cam)
  } finally {
    await cam.close()
  }
}

async function oneFrame(options = {}) {
  return withCamera(async (cam) => {
    const stream = await cam.start({ size: SMALLEST, ...options })
    return stream.read(5000)
  })
}

describe('camera', () => {
  it('describes itself', () =>
    withCamera(async (cam) => {
      assert.equal(cam.simulated, true)
      assert.equal(cam.model.name, 'MU1803-HS')
      assert.deepEqual(cam.model.resolutions[0], { width: 4912, height: 3684 })
      assert.equal(cam.model.bayerPattern, 'RGGB')
      assert.equal(cam.info.serial, 'SIMULATED')
      assert.equal(cam.geometry, null)
      assert.match(version(), /^\d+\.\d+\.\d+/)
    }))

  it('streams and reports geometry', () =>
    withCamera(async (cam) => {
      await cam.start({ size: SMALLEST, mode: 'raw12', exposureUs: 10_000, gain: 2 })
      assert.equal(cam.streaming, true)
      assert.equal(cam.exposureUs, 10_000)
      assert.ok(Math.abs(cam.gain - 2) < 0.2)
      const g = cam.geometry
      assert.deepEqual([g.width, g.height, g.bits], [1228, 922, 12])
      assert.ok(Math.abs((await cam.setExposure(5000)) - 5000) < 250)
      await cam.setGain(1)
      const roi = await cam.setRoi({ x: 100, y: 100, width: 400, height: 300 })
      assert.deepEqual([roi.width, roi.height], [400, 300])
    }))

  it('rejects bad arguments instead of throwing', () =>
    withCamera(async (cam) => {
      const p = cam.start({ mode: 'raw99' })
      assert.ok(p instanceof Promise)
      await assert.rejects(p, { code: 'ERR_INVALID_ARG_VALUE' })
      await assert.rejects(cam.setGain(1000), { code: 'ERR_INVALID_ARG_VALUE' })
    }))

  it('closes', async () => {
    const cam = openSimulated()
    await cam.close()
    await cam.close()
    assert.equal(cam.closed, true)
    await assert.rejects(cam.start(), { code: 'ERR_LOUPECAM', message: /closed/ })
    assert.throws(() => cam.info, { code: 'ERR_LOUPECAM' })
  })

  // `await using cam = …` calls this; the syntax itself needs Node 24.
  it('closes on async dispose', async (t) => {
    if (!Symbol.asyncDispose) return t.skip('no Symbol.asyncDispose in this Node')
    const cam = openSimulated()
    await cam.start({ size: SMALLEST })
    await cam[Symbol.asyncDispose]()
    assert.equal(cam.closed, true)
  })

  it('has a gain limit', () => assert.ok(Camera.maxGain() > 1))

  it('fails to open a missing camera', async (t) => {
    if (listCameras().length) return t.skip('a real camera is connected')
    await assert.rejects(open(), { code: 'ERR_LOUPECAM' })
  })
})

describe('stream', () => {
  it('reads raw frames', async () => {
    const f8 = await oneFrame({ mode: 'raw8' })
    assert.deepEqual([f8.width, f8.height, f8.bits], [1228, 922, 8])
    const raw8 = f8.raw()
    assert.ok(raw8 instanceof Uint8Array)
    assert.equal(raw8.length, 1228 * 922)

    const f12 = await oneFrame({ mode: 'raw12' })
    const raw12 = f12.raw()
    assert.ok(raw12 instanceof Uint16Array)
    assert.ok(raw12.every((v) => v < 4096))
  })

  it('iterates until the stream ends', () =>
    withCamera(async (cam) => {
      const stream = await cam.start({ size: SMALLEST })
      const seqs = []
      for await (const f of stream) {
        seqs.push(f.sequence)
        if (seqs.length === 3) break
      }
      assert.deepEqual(seqs, [...seqs].sort((a, b) => a - b))
      assert.ok(stream.stats.frames >= 3)
      // Restarting ends the old stream: iteration finishes, reads reject.
      await cam.start({ size: SMALLEST })
      let more = 0
      for await (const _ of stream) more++
      assert.ok(more <= 3) // whatever was queued
      await assert.rejects(stream.read(1000), { code: 'ERR_LOUPECAM_STREAM_STOPPED' })
    }))

  it('times out', () =>
    withCamera(async (cam) => {
      const stream = await cam.start({ size: SMALLEST, exposureUs: 200_000 })
      while (stream.tryRead()) {}
      await assert.rejects(stream.read(1), { code: 'ERR_LOUPECAM_TIMEOUT' })
      assert.ok(await stream.latest(5000))
    }))
})

describe('frame', () => {
  let frame
  before(async () => {
    frame = await oneFrame({ mode: 'raw12' })
  })

  it('develops', async () => {
    const img = await frame.develop()
    assert.deepEqual([img.width, img.height, img.channels, img.depth], [1228, 922, 3, 8])
    assert.ok(img.data instanceof Uint8Array)
    assert.equal(img.data.length, 1228 * 922 * 3)
    // The model's daylight balance keeps the simulator's grey specimen near neutral.
    let r = 0, g = 0, b = 0
    for (let i = 0; i < img.data.length; i += 3) {
      r += img.data[i]
      g += img.data[i + 1]
      b += img.data[i + 2]
    }
    assert.ok(Math.abs(r - g) < 0.15 * g && Math.abs(b - g) < 0.15 * g)

    const half = await frame.develop({ demosaic: 'superpixel' })
    assert.deepEqual([half.width, half.height], [614, 461])
    const deep = await frame.develop({ depth: 16, demosaic: 'mhc', whiteBalance: 'auto' })
    assert.ok(deep.data instanceof Uint16Array)
    const mono = await frame.develop({ monochrome: true, rotation: 90 })
    assert.deepEqual([mono.width, mono.height, mono.channels], [922, 1228, 1])
    await frame.develop({ whiteBalance: [1.5, 1, 1.7], colorMatrix: false, tone: 2.2 })
  })

  it('rejects bad develop options', async () => {
    await assert.rejects(frame.develop({ rotation: 45 }), { code: 'ERR_INVALID_ARG_VALUE' })
    await assert.rejects(frame.develop({ demosaic: 'magic' }), { code: 'ERR_INVALID_ARG_VALUE' })
    await assert.rejects(frame.develop({ whiteBalance: [1, 2] }), { code: 'ERR_INVALID_ARG_VALUE' })
  })

  it('encodes and saves', async () => {
    const png = await frame.encode()
    assert.ok(Buffer.isBuffer(png))
    assert.equal(png.subarray(0, 4).toString('latin1'), '\x89PNG')
    const jpg = await frame.encode('jpeg', { quality: 80 })
    assert.equal(jpg[0], 0xff)
    assert.equal(jpg[1], 0xd8)
    const raw = await frame.encode('tiff', { raw: true })
    assert.ok(['II', 'MM'].includes(raw.subarray(0, 2).toString('latin1')))
    await assert.rejects(frame.encode('jpeg', { raw: true }), { code: 'ERR_INVALID_ARG_VALUE' })

    const dir = await mkdtemp(join(tmpdir(), 'loupecam-'))
    try {
      const path = join(dir, 'shot.tif')
      await frame.save(path, { depth: 16 })
      assert.ok((await stat(path)).size > 1228 * 922 * 6)
      await frame.save(join(dir, 'shot.png'))
      assert.deepEqual(await readFile(join(dir, 'shot.png')), png)
      await assert.rejects(frame.save(join(dir, 'shot.bmp')), { code: 'ERR_INVALID_ARG_VALUE' })
    } finally {
      await rm(dir, { recursive: true, force: true })
    }
  })

  it('measures', () => {
    const [r, g, b] = frame.whiteBalance()
    assert.equal(g, 1)
    assert.ok(r > 1 && b > 1) // the simulator's red and blue respond weakly
    const s = frame.stats({ x: 0, y: 0, width: 200, height: 200 })
    assert.equal(s.histogram.length, 256)
    assert.ok(s.luma > 0 && s.luma < 1)
    assert.throws(() => frame.stats(null, [1, 2]), { code: 'ERR_INVALID_ARG_VALUE' })
  })
})

