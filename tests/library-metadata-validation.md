# PS1 release metadata and cover validation

Verified on 2026-11-20 with Rust 1.100.0, CachyOS Linux x86-64 and an AMD Ryzen 9 8945H. Database and image tests use temporary directories.

## Remote release metadata

A clean library can index a disc offline using its serial, region and filename. Wikidata supplies titles, aliases and publication dates when a PS1 release can be identified. The raw JSON for the last successful match and its parsed candidates share a SQLite transaction in the user's library. A failed or empty refresh preserves the previous match. Tests cover cache reuse after restart, manual corrections, upgrades from earlier databases, partial date precision, multiple candidates, and out-of-order requests. [Wikidata PlayStation IDs](https://www.wikidata.org/wiki/Property:P2606) · [Publication dates](https://www.wikidata.org/wiki/Property:P577)

A live check sent the five distinct US disc serials from the mixed-media acceptance sample through the same SPARQL query as the desktop scraper. It matched Crash Bandicoot (`SCUS-94900`) with a release date of 1996-09-09. The query returned no bindings for Castlevania (`SLUS-00067`), Resident Evil 2 discs 1 and 2 (`SLUS-00421`, `SLUS-00592`) and Tekken 3 (`SLUS-00402`). Those results describe this sample and this exact-serial query. A metadata miss can still have a matching cover.

## Covers

Import immediately indexes the disc offline. By default a committed new game also queues a background lookup for its release and cover. Hot rescans and verified copies do not enqueue work. **Refresh library data** reads all visible game IDs from SQLite, including games outside a search, and revalidates artwork and release data. Metadata and cover jobs have separate configurable concurrency limits (1–20 each, initially 6 and 3) and provider-specific request limits. Games with the same serial share a metadata lookup. A failed cover refresh leaves the last valid image available. **Get cover** requests an individual cover and **Fetch missing covers while browsing the library** can backfill older entries. The library settings allow both automatic operations to be turned off for offline use. The first Libretro lookup downloads the upstream PS1 Redump DAT and indexes playlist names by serial and region in the user's SQLite database. The index is refreshed on demand after 30 days. A unique matching playlist name identifies the regional front cover in Libretro's official PlayStation thumbnail repository. [Libretro catalog](https://github.com/libretro/libretro-database/tree/master/metadat/redump) · [Libretro thumbnails](https://github.com/libretro-thumbnails/Sony_-_PlayStation)

ScreenScraper uses the PS1 serial and regional front-cover media. It requires developer credentials and the account may need authorization for serial-only requests. IGDB uses a catalog title, an exact PS1 match and an explicitly regional cover. It requires a client ID and access token. The custom HTTPS template accepts a serial placeholder and supports existing PSX Covers settings. [ScreenScraper API](https://www.screenscraper.fr/webapi2.php) · [IGDB API](https://api-docs.igdb.com/) · [PSX Covers downloader](https://github.com/xlenore/psx-covers/blob/main/README.md)

Tests verified regional and ambiguous matches, JPEG and PNG decoding, atomic replacement, manual cover precedence, offline thumbnail recovery, cancellation, response limits, negative caching and host-wide backoff. Live Libretro validation downloaded the catalog and a PNG into a temporary user library, then checked the original and its generated thumbnail. ScreenScraper and IGDB were tested with synthetic responses. Live validation for those two services requires credentials. The live batch test ran a full refresh, an incremental lookup and a second full refresh against Wikidata and Libretro. It verified cache reuse during the incremental lookup, reuse of the same content-addressed image after full refresh, and preservation of the user's title.

```sh
cargo test --locked --lib --all-features library::metadata::
cargo test --locked --lib --all-features library::covers::
cargo test --locked --lib --all-features local_ps1_image_sample -- --ignored
cargo test --locked --lib --all-features loopback_http_response -- --ignored
cargo test --locked --lib --all-features live_wikidata_serial_metadata -- --ignored
cargo test --locked --lib --all-features live_libretro_cover_smoke -- --ignored
cargo test --locked --lib --all-features live_psx_covers_smoke -- --ignored
cargo test --locked --lib --all-features live_library_batch_full_and_incremental_smoke -- --ignored
```

The local disc test uses six locally supplied CUE/CHD images to verify the five serials above and offline identification with a fresh library. The loopback test requires a local socket. The live tests require internet access and sample individual releases.
