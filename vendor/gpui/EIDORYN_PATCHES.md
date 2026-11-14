# Eidoryn's GPUI 0.2.2 patch

This directory contains the published `gpui` 0.2.2 crate from crates.io,
licensed under Apache-2.0. Its archive SHA-256 is
`979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707`.
The original `.cargo_vcs_info.json` and `LICENSE-APACHE` are preserved.
The registry cache marker, standalone lockfile, and example programs/assets
are omitted; example targets are removed from the normalized `Cargo.toml`.
The manifest inherits the application's workspace resolver.

The root `[patch.crates-io]` selects this source for all platforms, including
release packages. GPUI is a workspace member so Cargo can run its native unit
tests with the application's lockfile; the default workspace member remains the
application. CI runs application tests with `--workspace --exclude gpui`, then
runs the focused native regression below. No Cargo cache edits or build-time
source rewriting are needed.

## Local changes

- `src/platform/windows/window.rs`: pass `STGMEDIUM.hGlobal` directly as an
  `HDROP` to `DragQueryFileW`. The upstream code dereferenced it as `*mut HDROP`,
  causing invalid memory access when a file entered the application window.
  Extraction checks `TYMED_HGLOBAL` and null handles, and always calls
  `ReleaseStgMedium`, including for an early return or a custom release owner.
  See [Microsoft's Shell data-transfer contract](https://learn.microsoft.com/en-us/windows/win32/shell/datascenarios).
- `src/platform/windows/window/file_drop_tests.rs`: native Windows regression
  tests using Shell data objects and real global-memory handles, including
  Unicode paths, multiple files, and storage-medium ownership.
- `src/taffy.rs`: make two `f32` literals explicit to avoid Rust 1.100's
  `float_literal_f32_fallback` warning exposed by using a local dependency.
- `src/platform/linux/x11/clipboard.rs`: normalize comment indentation for the
  workspace's rustfmt version; no behavior change.
- `src/platform/mac/shaders.metal`: normalize seven lines of mixed indentation
  for `git diff --check`; no shader changes.

Run the native tests on Windows from the repository root:

```powershell
cargo test --locked --release -p gpui --lib --no-default-features --features font-kit,windows-manifest,test-support file_drop_tests -- --test-threads=1
```

The tests exercise the native extraction path used by `IDropTarget::DragEnter`;
GPUI's simulated UI events alone do not exercise this Windows COM boundary.
On Windows x86_64, restoring the upstream handle dereference makes the
Unicode/multiple-file test exit with `0xc0000005 (STATUS_ACCESS_VIOLATION)`;
the corrected conversion passes all five native cases.
When upgrading GPUI, check that upstream has equivalent handle and lifetime
fixes before removing this patch.
