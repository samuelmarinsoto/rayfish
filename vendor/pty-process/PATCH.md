# Carried patch: OpenBSD ptsname

This is a vendored copy of `pty-process` 0.5.3 carrying one small patch that
upstream has not merged (no OpenBSD work exists in the upstream repository,
git.tozt.net/doy/pty-process, as of 2026-09).

**The patch:** `Pty::pts()` on `target_os = "openbsd"` resolves the slave
device through `ptsname(3)` directly instead of `rustix::pty::ptsname`,
because rustix has not implemented that call for OpenBSD. OpenBSD's libc
ships `ptsname(3)` (stdlib.h); it uses a static buffer and the platform has
no `_r` variant, so the vendored helper serializes access with a mutex.

Every other platform takes the unchanged upstream path.

**Drop this vendored copy** once upstream gains OpenBSD support (or once
rustix wires up `ptsname` for OpenBSD and pty-process picks that release),
and remove the `[patch.crates-io]` entry in the workspace `Cargo.toml`.
