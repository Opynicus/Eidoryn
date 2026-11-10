# Linux desktop integration

The GPUI header embeds `assets/eidoryn.png`; no external asset path is required.
The application title is **Eidoryn**, the executable is `eidoryn`, and the Wayland
application ID is `dev.eidoryn.PSX`.

To install a launcher and its icon for the current user after a release build:

```sh
install -Dm755 target/release/eidoryn "$HOME/.local/bin/eidoryn"
install -Dm644 assets/eidoryn.png "$HOME/.local/share/icons/dev.eidoryn.PSX.png"
install -Dm644 packaging/dev.eidoryn.PSX.desktop "$HOME/.local/share/applications/dev.eidoryn.PSX.desktop"
```

Ensure `$HOME/.local/bin` is on the desktop session's `PATH`. Default save paths
are relative to the process working directory; use explicit `--card1` / `--card2`
paths when launching from multiple locations. These commands are optional; the
repository does not install files automatically.
