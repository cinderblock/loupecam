# Issues

Short notes for problems found outside the task at hand. Claim an entry
(`in progress — <thread/branch>`) before working on it; remove it with the fix.

## Shared Cargo build-dir collides across worktrees of this repo

`~/.cargo/config.toml` sets `build-dir = "{cargo-cache-home}/build"` for every
project. Two checkouts of this repository (e.g. the main tree and a
`git worktree` such as `../loupecam-calibration`) have identically named local
crates, and their intermediate artifacts collide: a build in one tree linked the
other tree's `loupecam-isp` (`error[E0432]: unresolved import
loupecam_isp::correction`, although the module existed in that tree).

Workaround: in any additional worktree, build with
`CARGO_BUILD_BUILD_DIR="$PWD/target/build-dir"`. A durable fix would be
`build-dir = "{cargo-cache-home}/build/{workspace-path-hash}"` in the global
config (the user's call: it reduces cross-project sharing).
