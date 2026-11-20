# Desktop game library acceptance

Validated on Linux x86-64, kernel 7.3.4-1-cachyos, AMD Ryzen 9 8945H, 32 GB RAM and Rust 1.100.0. Automated runs use temporary databases and artwork directories. Local commercial media used for field checks is supplied separately.

## Offline import and launch

Added a generated, BIOS-independent ISO, the multi-track US Castlevania CUE and a US Resident Evil 2 CHD to an empty library. Selecting the CUE's BIN files in the same batch produced three launch entries. A second import returned three existing entries, and reopening the database retained all three with zero seconds and no last-played timestamp. The scanner extracted SLUS-00067 and SLUS-00421 from the two retail discs. Both were outside the bundled catalog, and the cards retained their filenames.

The runner integration test starts the generated ISO, attempts to reopen a missing image while the game is running, pauses, steps and exits. It verifies the original media receipt and the single persistent game association. Index coverage also includes grouping, relocation, removal, reimport, changed media and incremental scanning. See [index acceptance and 1,000-image measurements](library-index-validation.md), [metadata and artwork validation](library-metadata-validation.md) and [desktop interaction and scrolling](library-ui-validation.md).

## Playtime

| Case | Observed result |
| --- | --- |
| 60 s forward, 20 s paused, 30 s forward, 5 s rewind, 10 s forward | 100 s. Running intervals include the portion without controller input. |
| 600 s without runner samples, then 2 s forward | Only the two new seconds were added. The injected monotonic clock exercises the gap detector. |
| Child process exits inside an unfinished checkpoint | SQLite recovered the committed 30 s. The uncommitted 47 s checkpoint was absent. |
| Replay 30 s, then submit 42 s followed by the older 30 s | One session row, 42 s total. |
| SQLite rejects inserts, then storage recovers | Writer reports the failure, retries its cumulative checkpoint, and stores 45 s once. |
| Confirm a two-disc group | 180 s and 240 s appear together as 7 minutes. |

Statistics are enabled for interactive desktop disc sessions. The writer gets an absolute checkpoint at the start or end of a forward interval and every 30 seconds of forward play. Durations use the monotonic clock and a sample gap over five seconds starts a fresh interval. Current media, memory-card files and save-state checks use the existing emulator paths. Older save states embed the original image path and may still require that path after a move.

## Reproduction

```sh
cargo test --locked --all-features --lib runner::playtime
cargo test --locked --all-features --test standalone library_disc_launch_shutdown
EIDORYN_TEST_CUE="/path/to/game.cue" EIDORYN_TEST_CHD="/path/to/another-game.chd" \
  cargo test --locked --all-features --test standalone offline_library_mixed_media_smoke -- --ignored --nocapture
cargo test --locked --workspace --all-features
cargo test --locked --workspace --no-default-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

The all-features workspace run reported 401 passing tests. The no-default-features workspace run reported 288 passing tests. The locked desktop build, 120-frame headless demo, workspace Clippy with warnings denied, rustfmt and rustdoc also passed. Opt-in media, network and benchmark runs are recorded separately above.

Power-management coverage uses an injected gap. A physical suspend/resume cycle and native Windows and macOS desktop runs were outside this Linux validation.
