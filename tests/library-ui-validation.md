# Desktop library acceptance

The GPUI interaction tests exercise the desktop's regular navigation, SQLite reads and writes, launch commands, BIOS prompt, and asynchronous refresh. They use temporary files and the GPUI test platform.

| Case | Observed result |
| --- | --- |
| Add a folder with a nested ISO | The selected directory appears before confirmation. A recursive scan adds the nested disc. Import leaves the runner's media receipt and recent list untouched. |
| Enter on a selected disc with no BIOS | The details page offers the firmware picker. Canceling leaves the current session active. A valid 512 KiB test firmware lets the normal boot proceed, and the successful runner receipt moves the desktop to Game view. |
| Edit a title, refresh, restore automatic title | The custom title survives refresh. Restoring the title field displays the original again. |
| Copies and grouped discs | Three locations with disc numbers 1, 1 and 2 produce a two-disc badge. |
| Search and switch layouts | Searching for a specific serial isolates one entry. Switching between grid and compact list uses the configured layout immediately. |

## Scrolling measurement before optimization

The deterministic fixture generates 1,000 local 192 × 192 PNG thumbnails with distinct paths. The window is 1280 × 900. Timings cover the UI update, layout, thumbnail loading, and the test executor reaching idle. The dataset is ready in memory before the timed grid draw. The index benchmark covers discovery and SQLite separately.

| Linux debug test, one process | Cells rendered (zero based) | Elapsed | Process bytes read |
| --- | --- | ---: | ---: |
| First grid viewport | 0–19 | 254 ms | 23,628 |
| Scroll down 1,500 px | 40–59 | 250 ms | 23,628 |
| Switch to compact list at that offset | 24–34 | — | — |

Peak process RSS was 52,096 KiB. The machine used an AMD Ryzen 9 8945H, Linux 7.3.4 and Rust 1.100.0. Byte counts come from Linux process I/O accounting and include page-cache reads. At revision `9fc73ae` each visible card owned its image cache and released decoded artwork on removal. The current grid retains up to 64 decoded thumbnails across scrolls. Measurements for 100, 500 and 2,000 games are in the [library optimization report](library-optimization-validation.md).

```sh
cargo test --locked --all-features --bin eidoryn desktop::library::tests
cargo test --locked --all-features --bin eidoryn library_grid_virtualizes -- --nocapture --test-threads=1
```
