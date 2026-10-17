# SDK: crates.io, Python, Node.js and C bindings

## Goal

Make the camera driver usable as a library outside this app:

- the Rust crates published on crates.io
- a Python package (PyPI)
- a Node.js package (npm)
- a C API (shared library plus header), which C, C++, C#, Go, MATLAB, LabVIEW and
  others can call

Publishing happens from CI on release tags, never from a dev machine.

## Environment / context

- Repo `cinderblock/loupecam` (public), Windows dev machine, Rust 1.99, Python 3.12,
  Node 24, Bun 1.4, uv 0.10 (`uvx maturin`).
- Releases: pushing a `vX.Y.Z` tag runs `.github/workflows/release.yml`.
  `.github/scripts/set-version.sh` takes the version from the tag. The latest tag is
  v0.2.0.
- Names checked free on 2026-10-17: crates `loupecam`, `loupecam-protocol` and
  `loupecam-isp`; PyPI `loupecam`; npm `loupecam`.
- Latest versions on 2026-10-17: pyo3 0.30.1, numpy (rust-numpy) 0.30.0, napi 3.14,
  napi-derive 3.6, @napi-rs/cli 3.x, cbindgen 0.29.
- The simulated camera (`loupecam::sim::SimCamera`) lets every binding's tests run in
  CI without hardware.

## Decisions already made (don't re-ask)

- Publish from CI, not from a dev machine (user, 2026-10-17).
- Bindings: Python, Node.js "etc." (user, 2026-10-17). "etc." is covered by the C API.
- crates.io publishing is at my discretion ("if you think it is important"). I'm doing
  it, because bindings make the Rust crates the base layer anyway, and Rust users
  shouldn't need a git dependency.

## Design

### What is published

| Registry | Package | Contents |
| --- | --- | --- |
| crates.io | `loupecam-protocol`, `loupecam`, `loupecam-isp` | The library crates. The server, CLI, kiosk, updater and desktop app are `publish = false`. |
| PyPI | `loupecam` | abi3 wheels (CPython 3.9+), built with maturin. |
| npm | `loupecam` (+ napi-rs per-platform packages `loupecam-<platform>`) | The napi-rs addon with TypeScript types. |
| GitHub release | `loupecam-capi-<target>.zip` | Shared library, import library and `loupecam.h`. |

### Shared Rust layer

The bindings shouldn't each reinvent this, so it goes in the library crates:

- `loupecam::AnyCamera`: a real camera or the simulator behind one type. This is the
  server's private `Cam` enum, made public, and the server uses it too.
- `loupecam_isp::DevelopParams::for_model(&Model)`: per-model defaults. These are the
  Bayer pattern, the row flip for bottom-up sensors, the model's colour matrix and its
  daylight white balance. The CLI has these inline today.
- The model gets `ccm` and `daylight_wb` fields, so the defaults sit with the model
  data rather than in a function named after the MU1803.

### API shape (the same in every language)

- `list()` returns connected cameras. `open(index or id)` opens one, and `simulated()`
  opens the simulator.
- Camera:
  - `info` and `model`
  - `start(size, pixelMode, exposure, gain, speed, binning, roi)` returns a stream
  - `setExposure`, `setGain`, `setRoi`, `stop` and `close`
- Stream: `read(timeout)`, `latest(timeout)` and `stats`.
- Frame:
  - `width`, `height`, `bits` and `sequence`
  - the raw samples (a numpy array, a Buffer or a pointer)
  - `develop(options)` returns an RGB image, and `encode`/`save` write PNG, TIFF or JPEG
  - `whiteBalance()` gives grey-world gains
- Blocking calls release the GIL in Python and return Promises in Node.

### Layout

- `bindings/python/`: crate `loupecam-python` plus a maturin `pyproject.toml`. It has
  a mixed Rust/Python package (`loupecam/_native`) with `.pyi` stubs and pytest tests
  against the simulator.
- `bindings/node/`: crate `loupecam-node` plus `package.json` (@napi-rs/cli, Bun for
  installs, tests on Node with `node --test`).
- `bindings/c/`: crate `loupecam-capi` (cdylib + staticlib), with a header generated
  by cbindgen, committed, and checked as up to date in CI. A C example is compiled in
  CI.
- The bindings join the workspace. Their `[lib]` has `test = false`, because a test
  binary can't link Python or Node symbols. Python and Node tests run through their
  own tools.

### Publishing auth (CI)

- **crates.io:** trusted publishing (OIDC, `rust-lang/crates-io-auth-action`). A new
  crate can't be created over OIDC, so the first release uses a `CARGO_REGISTRY_TOKEN`
  secret. Delete the secret after that and add the trusted publisher on crates.io.
- **PyPI:** trusted publishing with a "pending publisher" works for a new project, so
  it needs no secret.
- **npm:** `npm publish --provenance`. npm's trusted publishing needs the package to
  exist, so the first release needs an `NPM_TOKEN` secret. That includes the platform
  packages, each one its own npm package.
- Each registry is gated on a repository variable (`PUBLISH_CRATES`, `PUBLISH_PYPI`,
  `PUBLISH_NPM` = `true`), like the code-signing hooks. A release works before the
  accounts are set up and simply skips that registry.

## Plan / steps

1. [x] Shared layer: `AnyCamera`, `DevelopParams::for_model`, model `ccm`/`daylight_wb`;
   the server and CLI use them (`14ba3ea`). Names and parsing for pixel modes,
   demosaics, Bayer patterns and formats (`42d6767`), `SampleFormat::bits` (`0e15fe8`).
2. [x] crates.io readiness (`128c15b`): READMEs (their examples are doctests), keywords
   and categories, versioned path deps kept in step by `set-version.sh` (tried with a
   fake tag, then restored), `publish = false` elsewhere. `cargo package` packages
   and verifies all three.
3. [x] Python binding (`b36668a`): 14 pytest tests on the simulator, `stubtest` clean,
   and `mypy --strict` on an example.
4. [x] Shared develop options crate `bindings/common` (`loupecam-bindings`,
   unpublished, `143c4c0`). The Python binding uses it.
5. [x] Node binding (`be1d803`): 14 `node --test` tests on the simulator, plus a
   TypeScript usage file type-checked with tsc 7.
6. [x] C API (`0e9f554`, `49ae26c`). `test.sh` compiles `examples/snap.c` as strict
   C99, linked dynamically and statically, runs both on the simulator, and checks the
   header as C++17. A Rust test fails if `include/loupecam.h` is stale.
7. [x] CI (`6f24b5e`): Python (3.10, 3.14), Node (22, 24) and the C API on Windows,
   Linux x64/arm64 and macOS. All 27 jobs green on run 39070379638 (after the fix in `4ca3d80`).
8. [x] Release workflow (`6f24b5e`):
   - C API zips for 6 targets
   - wheels for 8 targets plus the sdist
   - Node addons for 6 targets
   - registry publishing after the GitHub release, gated on `PUBLISH_CRATES` /
     `PUBLISH_PYPI` / `PUBLISH_NPM`, and idempotent
   actionlint is clean. **Not run yet.**
9. [x] Docs (`09b3bbd`): the README library section and layout;
   `docs/releasing.md` covers the registries and the one-time setup.
10. [ ] **(current) Hand over**: pushed 2026-10-17 with the user's OK. The first CI
    run failed the C API on every OS and the header test on Windows; fixed in the next
    commit (see Findings), and CI is green. Left for the user: the registry setup in
    `docs/releasing.md`, then a release tag (the release jobs haven't run yet).

## Findings / gotchas

- crates.io rejects OIDC publishes that would create a new crate (403). The first
  version needs a token.
- npm trusted publishing can only be configured on an existing package.
- PyO3 0.30 + rust-numpy 0.30 work with `abi3-py310`, so one wheel per platform.
- `uv venv` picked a 32-bit CPython 3.13 on this machine and maturin refused it
  ("platform win32 i686"). Pass `--python` the x64 3.12 explicitly.
- stubtest needs `@final` on PyO3 classes (they can't be subclassed), and an
  allowlist for type-only aliases (`tests/stubtest-allowlist.txt`).
- napi-rs v3:
  - Custom error codes work for synchronous calls as `Result<T, Code>` with
    `Code: AsRef<str>`, but only spelled literally as `Result<…>`: the macro
    doesn't see through a type alias.
  - `Task` errors are always `Status`, so a rejection with a custom `code` is built
    as a JS error object (`env.create_error` + `code`) and wrapped with
    `Error::from(unknown)`.
  - Doc comments and `Option<…>` passed into a `macro_rules!` as `$m:meta` or `$t:ty`
    are opaque to napi-derive: docs render as `r"…` and the fields become
    required. Pass docs as string literals, and spell out `Option<$ty>`.
  - Node's CJS lexer only sees named exports through
    `module.exports = require('./native.js')`. Assigning a variable hides them from
    `import { … }`.
  - `await using` is a syntax error on Node 22, so the test calls
    `[Symbol.asyncDispose]()` directly.

- WSL has no `/sys/bus/usb`, so `loupecam::list()` errors there
  (`USB: /sys/bus/usb/devices/ not found`) instead of returning nothing. The
  tests treat that as "no camera".
- `cargo rustc --crate-type cdylib,staticlib -- --print native-static-libs` gives
  the system libraries for a static link. On Windows they're `x.lib` names: for
  clang, pass them as `-lx` and drop the `/defaultlib:` options.
- `napi artifacts` and `napi pre-publish` refuse to run unless every configured
  target's `.node` is present. A local dry run needs stand-ins for the other platforms.
- CI sets `CARGO_TERM_COLOR=always`, which colours even the `native-static-libs`
  note: the parsed list ended in an escape code (`cannot find -lc\e[0m`). Pass
  `--color never` wherever that line is parsed (`test.sh` and `release.yml`).
- Git Bash on the Windows runner has no `/dev/stderr` (`tee: /dev/stderr: No such
  file`). Tee to a log file instead.
- With a CRLF checkout of `bindings/c/src/lib.rs`, cbindgen copies the CRs into
  the header's doc comments, so the header test strips carriage returns from both sides.

## Progress log

- [x] Steps 1–9 (see the plan).
  - Locally on Windows, everything passes: workspace clippy and tests, 14 pytest, stubtest,
    `mypy --strict` on an example, 14 Node tests, tsc, and the C test (dynamic and static).
  - On Linux (WSL Ubuntu 24.04): the C test (dynamic and static), and the Python package
    installed through pip on CPython 3.10 (14/14 after the USB-less fix).
  - WSL has no Node, so the Node tests have only run on Windows.
- [x] Local packaging dry runs:
  - the Python sdist builds and works in a fresh venv
  - the npm flow (`create-npm-dirs` → `artifacts` → `pre-publish --dry-run` →
    `npm pack --dry-run`) produces the right package contents
- [x] CI on all OSes: green after the C API fix.
- [ ] First publish to each registry (needs the user's accounts and a release tag).

## Open questions for the user

1. Registry accounts. `docs/releasing.md` → "Package registries" lists the steps:
   - crates.io: a token secret for the first publish, then trusted publishing
   - PyPI: a pending publisher, with no secret
   - npm: a token secret for the first publish, then trusted publishing
   Then set the `PUBLISH_*` variables and tag a release.

## Things not to do

- Don't publish from this machine (the user wants CI only).
- Don't push tags or trigger a release without asking.
- Don't type registry tokens anywhere. The user adds secrets themselves.
