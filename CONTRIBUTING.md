# Contributing to Velox WM

Thank you for helping improve **Velox WM**! Since the project is in active early development, clean patches, bug reports (including frame-pacing stutters, XWayland quirks, and UI alignment bugs), and focused PRs are all welcome.

## Project Layout

```text
velox-wm/
├── Cargo.toml                 # Crate manifest & binary targets (velox-wm, velox-settings)
├── install.sh                 # Builds release binaries and installs session & desktop files
├── uninstall.sh               # Cleanly removes installed binaries/entries (--purge-data optional)
├── assets/
│   ├── velox-session          # LightDM session supervisor supporting seamless reload (exit 42)
│   ├── velox-wm.desktop       # Wayland session entry (/usr/share/wayland-sessions)
│   └── velox-settings.desktop # Desktop entry for Velox Settings (/usr/share/applications)
└── src/
    ├── main.rs                # Entrypoint selecting --nested (winit) or --drm (udev/KMS)
    ├── lib.rs                 # Shared library modules used by compositor and velox-settings
    ├── config.rs              # Persistent TOML config (~/.config/velox-wm/config.toml) + refresh plans
    ├── state.rs               # Core compositor state, workspaces, layouts, Minimal View & cadence tracker
    ├── shell_ui.rs            # In-process panel, Start Menu, Quick Settings, Notification Center, Lock UI
    ├── font.rs                # fontdue glyph/emoji/symbol fallback rasterizer
    ├── apps.rs                # XDG desktop scanner, category rules, overrides & launch wrapper
    ├── notifications.rs       # D-Bus notification bridge, organizer, summaries, cooldown & snooze
    ├── system_ctrl.rs         # Audio roles/streams (pactl), Wi-Fi, BT, power profiles, proc manager
    ├── mem_cache.rs           # /proc/self/status VmRSS telemetry, malloc_trim & cache cleanup
    ├── updates.rs             # Session snapshot save/restore, external IPC, update watcher
    ├── grabs.rs               # Interactive window move/resize grabs & Aero/Edge snap zones
    ├── animation.rs           # Compositor-side smooth window geometry easing
    ├── wallpaper.rs           # Procedural & custom image wallpaper loader
    ├── cursor.rs              # Cursor theme & scaled buffer loader
    ├── xwayland.rs            # XWayland shell & X11Wm lifecycle handlers
    ├── backend/
    │   ├── mod.rs             # Shared element composition, Minimal View cards & animations
    │   ├── udev.rs            # Bare-metal DRM/KMS/GBM/EGL/libinput/libseat multi-monitor loop
    │   └── winit.rs           # Nested testing backend inside existing Wayland/X11 sessions
    ├── handlers/
    │   ├── compositor.rs      # wl_compositor commit/cadence/fractional-scale sync
    │   ├── xdg_shell.rs       # XDG toplevel/popup handlers & initial layout placement
    │   └── input.rs           # Keyboard bindings, pointer routing, game lock & Minimal View clicks
    └── bin/
        └── velox_settings.rs  # Standalone lightweight GUI settings app (smithay-client-toolkit)
```

## Development Workflow

1. **Test Safely Without Replacing Your TTY Session**
   Run Velox WM inside an existing Wayland or X11 session via `--nested`:
   ```bash
   cargo run --offline --bin velox-wm -- --nested
   ```
2. **Run All Unit Tests Before Committing**
   ```bash
   cargo test --offline
   cargo build --release --offline
   ```
   (Omit `--offline` on first build if Cargo needs to fetch crates.)
3. **Keep Runtime Footprint Minimal**
   - Avoid pulling heavy GUI toolkits (`gtk`, `qt`, `webkit`, Electron) or blocking main-loop I/O into the compositor thread.
   - Offload CLI queries (`pactl`, `nmcli`, `bluetoothctl`, D-Bus bridges) to background workers or threads.
   - Keep zero compiler warnings across `velox-wm` and `velox-settings`.

## Reporting Bugs, Glitches & Stutters

When opening an issue using the **Bug / Glitch / Stutter Report** template, please include:
- Monitor configuration (resolution, target Hz, VRR state)
- Whether the issue happened on bare-metal `--drm` or `--nested`
- Whether the app was Wayland-native or XWayland
- Relevant tail output from `~/.local/state/velox-wm/velox.log`
