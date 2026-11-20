# Game library index validation

The regression suite uses temporary SQLite databases and generated images. It covers concurrent and overlapping imports, CUE dependencies, direct track selection, broken manifests, cancellation, independent save-state identities, recent-history migration, and unplugging and reconnecting a directory.

A verified game retained its ID, boot preference and playtime when only its modification time changed. Replacing the file with another disc created a new game. For a never-launched disc that had been renamed and independently rescanned, explicit relocation preserved its custom title. The two prior session totals of 180 and 240 seconds produced one visible location and 420 seconds. A subsequent checkpoint against the second disc's original session increased the same card to 440 seconds.

## Reproduction

Run from the workspace root with Rust 1.100.0.

```sh
cargo test --locked --all-features --lib library::index_tests
cargo test --locked --all-features --lib library::index_tests::benchmark_thousand_images -- --ignored --nocapture --test-threads=1
```

On Linux 7.3.4 x86-64, AMD Ryzen 9 8945H, the debug-profile benchmark created 1,000 sparse files in a temporary directory. It used equal numbers of 96 KiB, 1 MiB, 32 MiB and 600 MiB cooked images, each with distinct sample data. Total logical media size was 165,961,728,000 bytes. The first pass started with an empty index. The second reused the database and filesystem cache. The benchmark includes file enumeration and per-entry database commits.

| Pass | First 12 results available | Full scan | Read bytes¹ | Query 1,000 rows |
| --- | ---: | ---: | ---: | ---: |
| Cold index | 11.7 ms | 755 ms | 16,682,827 | 11.6 ms |
| Warm index | 5.8 ms | 159 ms | 454,867 | 9.7 ms |

¹ Linux process `rchar` counts bytes returned by reads, including SQLite. All 1,000 warm entries were cache hits. The database contained zero strong fingerprints after both passes. Peak process RSS was 20,672 KiB. First-results timing measures when twelve entries could be queried. UI rendering and scrolling are measured separately. Sparse media and a local filesystem characterize scanner overhead; optical, network-mounted and removable storage have different latency.
