<p align="center">
  <b>English</b> | <a href="#wersja-polska">Polski</a>
</p>

<h1 align="center">⚡ Velox WM</h1>

<p align="center">
  <b>A lightweight, ultra-responsive 144Hz+ Wayland compositor and integrated desktop shell built in Rust with Smithay.</b>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-2024%20Edition-DEA584?style=flat&logo=rust&logoColor=white" alt="Rust 2024">
  <img src="https://img.shields.io/badge/Compositor-Smithay%200.7-5C6BC0?style=flat&logo=linux&logoColor=white" alt="Smithay 0.7">
  <img src="https://img.shields.io/badge/Protocol-Wayland%20%2B%20XWayland-FFBC00?style=flat&logo=wayland&logoColor=black" alt="Wayland + XWayland">
  <img src="https://img.shields.io/badge/Status-Active%20Development%20(v0.6.0)-orange?style=flat" alt="Status: Active Development">
  <a href="https://github.com/simonjuniorj"><img src="https://img.shields.io/badge/Author-simonjuniorj-181717?style=flat&logo=github&logoColor=white" alt="GitHub simonjuniorj"></a>
</p>

<p align="center">
  <a href="#-news--updates-aktualno%C5%9Bci">News & Updates</a> &nbsp;&middot;&nbsp;
  <a href="#-lightweight-design--memory-footprint">RAM & Footprint</a> &nbsp;&middot;&nbsp;
  <a href="#-key-features">Features</a> &nbsp;&middot;&nbsp;
  <a href="#-keyboard-shortcuts">Shortcuts</a> &nbsp;&middot;&nbsp;
  <a href="#-installation">Install</a> &nbsp;&middot;&nbsp;
  <a href="#-uninstallation">Uninstall</a> &nbsp;&middot;&nbsp;
  <a href="#-roadmap--to-do">To-Do</a> &nbsp;&middot;&nbsp;
  <a href="#-contributing--bug-reporting">Contribute</a> &nbsp;&middot;&nbsp;
  <a href="#-more-projects-by-simonjuniorj">Other Projects</a>
</p>

---

> [!WARNING]
> **Active Early Development Notice:**
> **Velox WM (`v0.6.0`) is NOT a finished 1.0 product.** It is under active development—features, config layouts, and visuals are evolving rapidly, and things may occasionally behave unexpectedly.
> If you run into **bugs, visual glitches, frame stutters, XWayland quirks, or multi-monitor issues**, please report them on [GitHub Issues](../../issues) using the [Bug / Glitch / Stutter Report template](.github/ISSUE_TEMPLATE/bug_report.md) along with logs from `~/.local/state/velox-wm/velox.log`. Every report helps make Velox smoother and more reliable!

---

## 📰 News & Updates (Aktualności)

- **2026-11-27** 🧩 **Minimal View, Custom Quick Tiles & Fractional Scaling (`v0.6.0`)**:
  - Added **Minimal View** cards for underscaled or compact windows (`< 340×220` or below client `min_size`, with double-click / right-click expand) to prevent app overflow and UI glitches.
  - Added customizable Quick Access pill ordering and visibility (`notif`, `bg_processes`, `control`, `wifi`, `bright`, `vol`, `float`) from **Quick Settings** and **Velox Settings**.
  - Added `wp_fractional_scale_manager_v1` support, fixed scaled pointer alignment in `velox-settings`, added CJK/Nerd/Powerline (`︱`) symbol fallback fonts, added `>_` terminal taskbar badges (including Alacritty), stabilized fullscreen-exit geometry, and fixed multi-surface `24/60fps` cadence detection under `144Hz` video battery saver.
- **2026-11-27** 🔔 **Pixel-Inspired Notifications, Idle Power & App Library Rules (`v0.6.0`)**:
  - Added on-device Notification Organizer categories, conversation summaries, notification burst cooldown, snooze, priority conversations, and per-app mute/disable toggles.
  - Added right-click App Library editing, custom keyword category detection rules, idle screen dimming, DPMS display sleep, and integrated Lock Screen (`Super+L`).
- **2026-11-26** ⚡ **Velox Shell, Audio Roles, VRR & Seamless Reload Baseline (`v0.6.0`)**:
  - Introduced the ChromeOS-inspired shelf, Start Menu launcher, standalone zero-toolkit `velox-settings` GUI, `pactl` Comms vs Media audio routing, Game Focus pointer confinement, and snapshot-based seamless LightDM reload (`install.sh --reload` / `exit 42`).

---

## 🪶 Lightweight Design & Memory Footprint

Most Linux Wayland setups assemble 6–10 separate processes (compositor + bar + launcher + notification daemon + lock screen + GTK/Qt/Electron settings apps + tray applets), inflating baseline RAM usage and IPC overhead.

Velox WM keeps **everything unified in two lean native Rust binaries**:

- **In-process shell rasterizer (`velox-wm`):** Draws the taskbar/shelf, Start Menu, Quick Settings, Audio Mixer, Notification Center, Task View, Alt+Tab switcher, Lock Screen, and Minimal View cards directly into Wayland/GL surfaces—zero external panel or widget daemon processes required.
- **Zero-toolkit Settings GUI (`velox-settings`):** Rendered directly via `smithay-client-toolkit` + `wl_shm` + `fontdue`, linking only `libc`, `libgcc_s`, and `libxkbcommon` (zero GTK, Qt, Electron, or WebKit dependencies).
- **Active memory hygiene (`src/mem_cache.rs`):** Reads `/proc/self/status` (`VmRSS`) live, trims glyph & pixel caches automatically, releases unused glibc heap arenas back to the Linux kernel via `libc::malloc_trim(0)`, and throttles commits for hidden/minimized/occluded windows (`freeze_hidden_windows`).

### Footprint & RAM Usage

| Component | Release Binary Size | Live RAM Usage & Architecture Notes |
| --- | --- | --- |
| `velox-wm` (Compositor + Shell + XWayland) | `16.6 MiB` (`17,384,304` bytes) | Live idle RSS: **`<!-- TODO: paste your measured idle MB from Velox Settings -> RAM & Cache or ps -->` MB** (check exact live RSS inside **Velox Settings → RAM & Cache** or via `ps -C velox-wm -o pid,rss,vsz,comm`) |
| `velox-settings` (GUI Control Center) | `4.2 MiB` (`4,418,512` bytes) | Launches on demand; zero toolkit runtime dependencies |
| `velox-session` (Reload supervisor) | `612` bytes | Lightweight POSIX shell supervisor handling seamless exit-`42` reloads |

---

## 🚀 Key Features

### Window Management & Smooth Rendering
- **3 Workspace Modes:** Switch instantly between **Master-Stack (`Tile`)**, **Columns (`Cols`)**, and **Floating (`Float`)** across 9 virtual workspaces (`Super+1..9`).
- **Deterministic Unfullscreen & Animations:** Stable workspace window ordering, GPU animations, Aero/Edge Drag Snap Zones, floating geometry preservation, and pre-ack unfullscreen hold so exiting fullscreen returns windows cleanly inside monitor bounds.
- **Minimal View for Cramped / Underscaled Windows:** Shrinks tiny or over-constrained windows cleanly into interactive Minimal Cards instead of clipping client UIs; double-click or right-click to expand back cleanly.
- **Fractional Scaling & XWayland:** Supports `wp_fractional_scale_manager_v1`, `wl_viewport`, `wlr`/`ext` data control clipboards, clipboard/primary sync, and full `X11Wm` XWayland support for Steam/Proton games and legacy apps.
- **Inactive Window Dimming & Opacity:** Highlight focus cleanly across monitors while excluding fullscreen games and videos.

### ChromeOS-Style Shelf & Customizable Quick Access Panel
- **Top or Bottom ChromeOS Pill / Full-Width Shelf:** Customizable panel height, opacity, corner radius, 8 accent presets (`Cobalt`, `Violet`, `Emerald`, `Cyan`, `Amber`, `Crimson`, `Rose`, `Mint`), and procedural or custom wallpapers (`~/.config/velox-wm/wallpaper.jpg`/`.png`).
- **Reorderable & Toggleable Tray Pills:** Move or hide any Quick Access pill (`notif`, `bg_processes`, `control`, `wifi`, `bright`, `vol`, `float`) right inside **Quick Settings → Customize Tiles** or **Velox Settings → Personalize** while popups follow moved tiles.
- **Unicode, Nerd Font, Emoji & Terminal Recognition:** Fallback font loading including Nerd Fonts, Powerline glyphs, CJK symbols (`︱`), and system emoji; terminal emulators (`alacritty`, `foot`, `kitty`, `wezterm`, `ghostty`, etc.) display `>_` taskbar badges and `"Term"` tags even when `.desktop` declares `Terminal=false`, plus right-click Copy/Paste/Open Another Window actions.

### Start Menu & Editable App Library
- **Smart Category Filter:** Automatic or one-click **"Search in All Categories"** fallback when a query has no matches in the active category.
- **Right-Click Context Menus & Full Overrides:** Right-click any app in the Start Menu or **Velox Settings → Apps & Startup** to launch, switch categories, toggle **Run in Terminal**, cycle **Game Detection (`Auto` / `Force ON` / `Force OFF`)**, hide/unhide, or jump directly to full property editing in `~/.config/velox-wm/app_library.toml` (including custom apps and keyword rules).

### Pixel-OS-Inspired Notifications
- **Built-in D-Bus Notification Daemon:** Toast popups + Notification Center (`Super+U`) with inline reply drafts, suggested actions (OTP copy, URL launch), snooze, burst Cooldown audio ducking, priority conversations, per-app Disable/Silent controls, and persistent JSON history.
- **On-Device Organizer & Recaps:** Local keyword bundling and conversation recaps processed completely offline without sending notifications to external servers.

### Display, Multi-Monitor, Video Battery Saver & Power Management
- **Per-Monitor Settings:** Configure resolution, refresh rate (`60Hz`–`144Hz+`), fractional scale (`0.75`–`3.0`), primary monitor, and VRR mode (`Auto` / `AlwaysOn` / `Off`) with a 15-second keep-or-revert safety modal.
- **Zero-Black-Screen Video & Idle Battery Saver:** Multi-surface `CommitCadenceTracker` detects `24/25/30/50/60 fps` video (including `144Hz`-quantized `60fps` streams while excluding games) alongside idle refresh reduction and an optional pass-through Refresh HUD overlay.
- **Idle Screen Dim, Sleep & Lock Screen:** Automatic screen dimming and DPMS/display sleep timers (with video-playback inhibit and lock-on-sleep) plus a built-in PAM/`unix_chkpwd` Lock Screen (`Super+L` / `Ctrl+Alt+L`).

### Gaming Focus, Audio Roles & Seamless Reload
- **Game Focus Mode:** Pointer confinement inside focused games across multi-monitor setups (`Alt+Tab` to switch cleanly) plus optional fullscreen taskbar lock.
- **Audio & Roles (`pactl`):** Master output & microphone sliders, per-device disable toggles, per-stream controls, and automatic **Comms vs Media** device routing.
- **Seamless Reload Without Losing Layout State:** `install.sh --reload` or `Super+Ctrl+Shift+R` shows an informative pre-reload warning modal, saves a window/workspace/notification snapshot, re-execs `velox-wm` via `velox-session` (`exit 42`), and restores your layout cleanly.

---

## ⌨️ Keyboard Shortcuts

Default bindings (`windows_shortcuts = true`; configurable in **Velox Settings → Multitasking & Keys**):

| Shortcut | Action |
| --- | --- |
| `Super` (tap) / `Super+S` / `Super+Space` | Toggle Start Menu & App Library |
| `Super+Return` / `Alt+Return` | Launch Terminal (`terminal_cmd`, default `alacritty`) |
| `Super+E` | Launch File Manager (`files_cmd`, default `thunar`) |
| `Super+I` | Open **Velox Settings** (`velox-settings`) |
| `Super+A` | Toggle Quick Settings |
| `Super+V` / `Alt+V` | Toggle Audio Mixer |
| `Super+U` | Toggle Notification Center |
| `Super+N` | Toggle Calendar & Focus Timer |
| `Super+B` / `Ctrl+Shift+Esc` | Active Windows & Background Process Manager |
| `Super+Tab` | Task View (All Workspaces Overview) |
| `Alt+Tab` / `Alt+Shift+Tab` | Switch Focused Window (Cross-Monitor Aware) |
| `Super+Left` / `Super+Right` | Snap Window Left / Right Half |
| `Super+Up` / `Super+Down` | Expand / Maximize or Shrink / Minimize |
| `Super+M` / `Alt+M` | Toggle Fullscreen |
| `Super+F` / `Alt+F` | Toggle Floating |
| `Super+T` | Pin Window on Top (Keep Above) |
| `Super+Q` / `Alt+F4` | Close Focused Window |
| `Super+D` | Show Desktop |
| `Super+1..9` / `Ctrl+Super+Left/Right` | Switch Workspace (`1–9` / Prev & Next) |
| `Super+Shift+1..9` | Move Focused Window to Workspace `1–9` |
| `Super+Shift+M` | Move Focused Window to Next Monitor |
| `Super+Shift+Arrow` / `Super+Shift+HJKL` | Swap Tiled Window in Direction |
| `Alt+Arrow` / `Alt+HJKL` | Focus Window in Direction (`Super+Arrow` when `windows_shortcuts = false`) |
| `Super+L` / `Ctrl+Alt+L` | Lock Screen |
| `Super+X` | Power Menu (Lock, Suspend, Reboot, Power Off, Exit) |
| `Super+Ctrl+Shift+R` | Prompt Snapshot & Seamless WM Reload |
| `Super+Shift+E` / `Alt+Shift+E` | Exit Velox Session to LightDM |
| `Super + Left Drag` / `Super + Right Drag` | Move Window / Resize Window |

---

## 📦 Dependencies & Prerequisites

### Build Dependencies
- Rust toolchain (`cargo` / `rustc` supporting **Rust 2024 edition**)
- System libraries: `libseat`, `libinput`, `libudev`, `libdrm`, `libgbm`, `mesa` (EGL/GLES), `libxkbcommon`

On Arch Linux:
```bash
sudo pacman -S --needed rust mesa libdrm libinput libseat libxkbcommon systemd-libs
```

### Runtime Companions (Optional / Recommended)
- **XWayland apps & games:** `xorg-xwayland`
- **Audio controls & role routing:** `pipewire-pulse` / `pulseaudio` (`pactl`)
- **Network & Bluetooth pills:** `networkmanager` (`nmcli`), `bluez-utils` (`bluetoothctl`)
- **D-Bus notifications & clipboard helper:** `python`, `python-gobject`, `wl-clipboard` (`wl-copy`)
- **Brightness & power:** `/sys/class/backlight` + optional `ddcutil`, `power-profiles-daemon` (`powerprofilesctl`), `polkit-gnome`, `xdg-utils`, `curl`
- **Display manager:** LightDM (or any DM supporting `/usr/share/wayland-sessions/`)

---

## ⚙️ Installation

1. Clone the repository:
   ```bash
   git clone https://github.com/simonjuniorj/velox-wm.git
   cd velox-wm
   ```
   *(Update the clone URL above if your fork or repository uses a different name.)*

2. Build and install with [`install.sh`](install.sh):
   ```bash
   ./install.sh
   ```
   This builds release binaries (`--offline` first, with automatic online fallback) and installs:
   - `/usr/local/bin/velox-wm`
   - `/usr/local/bin/velox-settings`
   - `/usr/local/bin/velox-session`
   - `/usr/share/wayland-sessions/velox-wm.desktop`
   - `/usr/share/applications/velox-settings.desktop`

3. **Start a Session:** Log out to LightDM and pick **Velox WM**, or test safely inside your current desktop with:
   ```bash
   cargo run --release --bin velox-wm -- --nested
   ```

4. **Hot-Reload an Active Velox Session After Updating:**
   ```bash
   ./install.sh --reload      # Opens snapshot & reload confirmation modal
   ./install.sh --reload-now  # Immediate snapshot + reload without modal
   ```
   *(Tip: set `github_update_repo = "simonjuniorj/velox-wm"` in `~/.config/velox-wm/config.toml` once published so the built-in release watcher tracks new tags.)*

---

## 🗑️ Uninstallation

Run the companion [`uninstall.sh`](uninstall.sh) script:
```bash
./uninstall.sh
```
By default, your personal settings in `~/.config/velox-wm` are preserved. To delete all Velox configuration, state logs, cache, and notification history as well:
```bash
./uninstall.sh --purge-data
```

Manual removal alternative:
```bash
sudo rm -f \
  /usr/local/bin/velox-wm \
  /usr/local/bin/velox-settings \
  /usr/local/bin/velox-session \
  /usr/share/wayland-sessions/velox-wm.desktop \
  /usr/share/applications/velox-settings.desktop
```

---

## 📁 Configuration & Data Locations

| File / Path | Purpose |
| --- | --- |
| `~/.config/velox-wm/config.toml` | Appearance, tiling, monitors, VRR/battery saver, quick-tile order, notifications, audio roles, shortcuts |
| `~/.config/velox-wm/app_library.toml` | Custom apps, app overrides (category, badge color, terminal, hidden, game detection), category rules |
| `~/.config/velox-wm/wallpaper.jpg` / `.png` | Custom wallpaper image when `use_custom_wallpaper = true` |
| `~/.local/share/velox-wm/notification-history.json` | Persisted notification history & seen-app notification toggles |
| `~/.local/state/velox-wm/velox.log` | Compositor runtime logs (helpful when reporting bugs or stutters) |

---

## 🗺️ Roadmap & To-Do

- [ ] **Per-Monitor Independent Workspaces & Hotplug Profiles:** Switch workspaces per physical monitor independently and save automatic docked/undocked display layouts.
- [ ] **Full Keybind Remapper & Trackpad Gestures:** Graphical shortcut editor inside `velox-settings` plus 3/4-finger libinput swipe gestures for Task View, workspaces, and Show Desktop.
- [ ] **`xdg-desktop-portal-velox` & Screen Sharing / Screenshots:** PipeWire screen capture for OBS, Discord, and browsers alongside a native region screenshot & screen recorder overlay.
- [ ] **Pure-Rust Notification D-Bus Engine (`zbus`):** Replace the `python3`/PyGObject helper with an in-process Rust D-Bus server to cut RAM usage and startup latency even further.
- [ ] **System Tray (`StatusNotifierItem` / AppIndicator):** Render third-party tray icons alongside the customizable Quick Access tiles.
- [ ] **Lightweight GPU Shaders & Night Light:** Optional background blur for shell surfaces, smooth workspace slide animations, and automatic color-temperature / Night Light schedules.
- [ ] **Packaging & GitHub Release Pipeline:** Arch Linux AUR `PKGBUILD`, Debian/Fedora packaging, GitHub Actions CI (`cargo test` / `cargo build --release`), and prebuilt release tarballs for one-click updater checks.
- [ ] **Gaming & DPI Polish:** Per-app XWayland scale tuning, HDR display capability reporting, and custom cursor theme/size selection in `velox-settings`.

---

## 🤝 Contributing & Bug Reporting

PRs, performance profiling, bug fixes, and UI ideas are very welcome!
- Read [CONTRIBUTING.md](CONTRIBUTING.md) for the codebase map, `--nested` dev workflow, and `cargo test --offline` checks.
- Spotted a glitch, black/misaligned window, stutter, or crash? Open an issue with the [Bug / Glitch / Stutter template](.github/ISSUE_TEMPLATE/bug_report.md) or propose improvements with the [Feature Request template](.github/ISSUE_TEMPLATE/feature_request.md).

---

## 🔗 More Projects by [simonjuniorj](https://github.com/simonjuniorj)

Check out my other repositories and past projects on GitHub:

- 🤖 **[Cloudi](https://github.com/simonjuniorj/Cloudi)** *(Archived / Closed Source)* — Advanced Discord administration, SFW media safety scanning, anti-phishing, ticketing, verification, and server logging bot created for the Szkoła w Chmurze server network.
- 🐾 **[Yukki-Bot](https://github.com/simonjuniorj/Yukki-Bot)** *(Open Source / Free)* — Lightweight, free, database-free Discord bot ready to self-host and customize.
- 📦 **[All Repositories on GitHub → github.com/simonjuniorj](https://github.com/simonjuniorj?tab=repositories)** — Browse additional tools, scripts, and upcoming projects.

---

## 📄 License

> No `LICENSE` file has been added yet. Choose and add a license (e.g., `MIT`, `Apache-2.0`, or `GPL-3.0-or-later`) and set `license = "..."` in [Cargo.toml](Cargo.toml) before publishing publicly.

---

<div id="wersja-polska"></div>

## 🇵🇱 Wersja Polska — Velox WM

**Velox WM** to lekki, responsywny kompozytor **Wayland** (`144Hz+`) oraz zintegrowane środowisko pulpitu napisane w języku **Rust** (`smithay`), zaprojektowane z myślą o płynności działania, niskim zużyciu pamięci RAM, grach wielomonitorowych oraz pełnej kontroli bez dziesiątek ciężkich procesów w tle.

> ⚠️ **Ważna informacja / Projekt w trakcie aktywnego rozwoju:**  
> Velox WM (`v0.6.0`) **nie jest jeszcze ukończonym produktem**. Projekt jest intensywnie rozwijany, dlatego mogą występować okazjonalne błędy, artefakty graficzne, przycięcia (stutters) lub problemy z kompatybilnością niektórych aplikacji XWayland. Jeśli zauważysz jakikolwiek problem — **zgłoś go przez [GitHub Issues](../../issues)** i dołącz logi z `~/.local/state/velox-wm/velox.log`!

### 📰 Aktualności (Polskie)

- **2026-11-27** 🧩 **Minimal View, własna kolejność kafelków i skalowanie ułamkowe (`v0.6.0`)**: Dodano kompaktowy tryb **Minimal View** dla zbyt małych okien, personalizację kolejności/widoczności kafelków szybkiego dostępu na pasku zadań, obsługę `wp_fractional_scale_manager_v1`, naprawę pozycji kursora przy skalowaniu w `velox-settings`, obsługę symboli `︱`/Nerd/Emoji na pasku, badge `>_` dla terminali (w tym Alacritty), poprawki wyjścia z pełnego ekranu oraz stabilniejsze wykrywanie wideo 24/60fps na ekranach 144Hz.
- **2026-11-27** 🔔 **Powiadomienia w stylu Pixel, oszczędzanie energii i reguły App Library (`v0.6.0`)**: Dodano lokalny organizer powiadomień i podsumowania rozmów (bez wysyłania danych na zewnątrz), tryb wyciszenia serii powiadomień (Cooldown), odkładanie (Snooze), menu kontekstowe pod PPM w bibliotece aplikacji, przyciemnianie ekranu przy bezczynności, uśpienie wyświetlacza oraz ekran blokady (`Super+L`).
- **2026-11-26** ⚡ **Pierwsza wersja kompozytora 144Hz, panelu i `velox-settings` (`v0.6.0`)**: Pasek w stylu ChromeOS, Menu Start, natywna aplikacja ustawień bez GTK/Qt/Electron, routing audio Comms/Media (`pactl`), blokada kursora w grach oraz bezstratne przeładowanie sesji (`install.sh --reload` / kod wyjścia `42`).

### 🪶 Dlaczego tak lekki?
- **Brak narzutu GTK / Qt / Electron:** Zarówno powłoka (`velox-wm`), jak i panel ustawień (`velox-settings`, rozmiar release `4.2 MiB`, zaledwie 3 biblioteki systemowe: `libc`, `libgcc_s`, `libxkbcommon`) korzystają z własnego lekkiego renderera tekstów i buforów Wayland.
- **Aktywne zarządzanie RAM:** Wbudowany monitor `/proc/self/status` (`VmRSS`), automatyczne czyszczenie cache glifów i zwalnianie pamięci sterty (`malloc_trim(0)`) oraz zamrażanie ukrytych okien (`freeze_hidden_windows`).
- **Zmierzone zużycie RAM:** Sprawdź dokładnie na swojej maszynie w **Velox Settings → RAM & Cache** lub poleceniem `ps -C velox-wm -o pid,rss,vsz,comm`.

### 🔗 Inne moje projekty
- 🤖 **[Cloudi](https://github.com/simonjuniorj/Cloudi)** *(Zarchiwizowany)* — Zaawansowany bot Discord dla sieci serwerów Szkoły w Chmurze.
- 🐾 **[Yukki-Bot](https://github.com/simonjuniorj/Yukki-Bot)** *(Publiczny / Darmowy)* — Lekki bot Discord niewymagający bazy danych.
- 📦 **[Wszystkie repozytoria — github.com/simonjuniorj](https://github.com/simonjuniorj?tab=repositories)**
