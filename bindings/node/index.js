// The public entry point: the native module (loaded by the generated native.js for
// this platform), plus the parts better written in JavaScript.
'use strict'

// This exact form lets Node's CommonJS lexer see native.js's named exports, so
// `import { open } from 'loupecam'` works from ES modules.
module.exports = require('./native.js')

const { Camera, Stream } = module.exports

/** `for await (const frame of stream)`: frames until the stream ends. */
Stream.prototype[Symbol.asyncIterator] = async function* frames() {
  for (;;) {
    let frame
    try {
      frame = await this.read()
    } catch (e) {
      if (e && e.code === 'ERR_LOUPECAM_STREAM_STOPPED') return
      throw e
    }
    yield frame
  }
}

// `await using cam = openSimulated()` closes the camera at the end of the scope.
if (Symbol.asyncDispose) {
  Camera.prototype[Symbol.asyncDispose] = function dispose() {
    return this.close()
  }
}
