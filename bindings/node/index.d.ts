// The public typings: the native module's (generated into native.d.ts from the Rust
// source), plus what index.js adds.
export * from './native'

import type { Frame } from './native'

declare module './native' {
  interface Stream {
    /** Frames until the stream ends (stopped, restarted or closed). */
    [Symbol.asyncIterator](): AsyncGenerator<Frame, void, undefined>
  }
  interface Camera {
    /** `await using cam = await open()` closes the camera at the end of the scope. */
    [Symbol.asyncDispose](): Promise<void>
  }
}
