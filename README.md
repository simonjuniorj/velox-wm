<p align="center">
  <b>English</b> | <a href="#wersja-polska">Polski</a>
</p>

<h1 align="center">⚡ Velox WM</h1>

<p align="center">
  <b>A smooth, adaptive Wayland compositor and integrated desktop shell written in Rust with Smithay.</b><br/>
  <sub>Lightweight on VRAM &bull; Works across any display refresh rate (60Hz–240Hz+ &amp; VRR) &bull; Built to unify bar, launcher, settings &amp; notifications in one stack</sub>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-2024%20Edition-DEA584?style=flat&logo=rust&logoColor=white" alt="Rust 2024">
  <img src="https://img.shields.io/badge/Compositor-Smithay%200.7-5C6BC0?style=flat&logo=linux&logoColor=white" alt="Smithay 0.7">
  <img src="https://img.shields.io/badge/Protocol-Wayland%20%2B%20XWayland-FFBC00?style=flat&logo=wayland&logoColor=black" alt="Wayland + XWayland">
  <img src="https://img.shields.io/badge/License-GPL--3.0-blue?style=flat" alt="License: GPL-3.0">
  <img src="https://img.shields.io/badge/Status-Early%20Dev%20(v0.6.0)-orange?style=flat" alt="Status: Early Development">
  <a href="https://github.com/simonjuniorj"><img src="https://img.shields.io/badge/Author-simonjuniorj-181717?style=flat&logo=github&logoColor=white" alt="GitHub simonjuniorj"></a>
</p>

<p align="center">
  <a href="#-news--updates-aktualno%C5%9Bci">News &amp; Updates</a> &nbsp;&middot;&nbsp;
  <a href="#-known-bugs--current-limitations">Known Bugs</a> &nbsp;&middot;&nbsp;
  <a href="#-resource-usage-ram-vs-vram">RAM &amp; VRAM</a> &nbsp;&middot;&nbsp;
  <a href="#-key-features">Features</a> &nbsp;&middot;&nbsp;
  <a href="#-keyboard-shortcuts">Shortcuts</a> &nbsp;&middot;&nbsp;
  <a href="#-installation">Install</a> &nbsp;&middot;&nbsp;
  <a href="#-uninstallation">Uninstall</a> &nbsp;&middot;&nbsp;
  <a href="#-roadmap--to-do">To-Do</a> &nbsp;&middot;&nbsp;
  <a href="#-more-projects-by-simonjuniorj">Other Projects</a> &nbsp;&middot;&nbsp;
  <a href="#-contributing--bug-reporting">Contribute</a>
</p>

---

> [!WARNING]
> **Not a Finished Product — Active Early Development Notice**
> **Velox WM (`v0.6.0`)** is **under active development** (the project started just yesterday, on **2026-11-26**). Expect rapid changes, rough edges, and occasional bugs, glitches, or stutters.
> Please **report any bugs, visual glitches, memory spikes, XWayland issues, or frame stutters** in [GitHub Issues](../../issues) using the [Bug / Glitch / Stutter Report template](.github/ISSUE_TEMPLATE/bug_report.md) and attach `~/.local/state/velox-wm/velox.log`.

---

## 📰 News & Updates (Aktualności)

- **2026-11-27** 🚀 **First Public GitHub Push Prep, Transparency Pass & Shell Polish (`v0.6.0`)**:
  - Updated repository documentation with honest real-world RAM/VRAM metrics, a **Known Bugs** tracker, **GNU GPL-3.0** licensing, and links uniting **Velox WM** with our wider GitHub project ecosystem (`Cloudi`, `Yukki-Bot`, `SheepBot`, `srednia`).
  - Shipped **Minimal View** cards for overly narrow/cramped or underscaled windows (`< 340×220` or below client `min_size`, with double-click / right-click expand).
  - Added customizable Quick Access tile ordering and visibility (`notif`, `bg_processes`, `control`, `wifi`, `bright`, `vol`, `float`), `wp_fractional_scale_manager_v1` support, scaled pointer alignment fixes in `velox-settings`, Nerd/Powerline/CJK (`︱`) fallback fonts, `>_` terminal taskbar badges (including Alacritty), fullscreen-exit bounds fixes, and multi-surface `24/60fps` cadence detection for Video Battery Saver.
- **2026-11-27** 🔔 **Pixel-Inspired Notifications, Idle Power & Editable App Library (`v0.6.0`)**:
  - Added on-device Notification Organizer categories, conversation recaps, burst Cooldown audio ducking, snooze, priority conversations, and per-app mute/disable toggles.
  - Added right-click App Library quick editing, custom keyword category rules, idle screen dimming, DPMS display sleep, and integrated Lock Screen (`Super+L`).
- **2026-11-26** 🌱 **Project Born — Joining the `simonjuniorj` Ecosystem (`v0.6.0`)**:
  - **Velox WM was created yesterday (`2026-11-26`)!** Expanding beyond our earlier Discord bots and C# utilities into low-level Linux desktop engineering: built a Rust/Smithay Wayland compositor with ChromeOS-inspired shelf, Start Menu, zero-toolkit `velox-settings` GUI, PipeWire audio role routing, Game Focus pointer confinement, and LightDM snapshot reload (`exit 42`).

---

## ⚠️ Known Bugs & Current Limitations

We believe in being completely upfront about what works cleanly and what still needs fixing:

1. **System RAM Growth Over Time (`~280–350 MB` typical → up to `~800 MB` spike):**
   On a real multi-app desktop session, `velox-wm` typically sits around **280–350 MB RSS**, and under prolonged usage or certain workloads has been observed climbing toward **~800 MB** for a still-unidentified reason (likely retained pixel/surface buffers or XWayland state accumulation). Investigation and heap profiling are high priority.
2. **Pre-1.0 Visual Glitches & Occasional Stutters:**
   Rapid fullscreen transitions, monitor hotplug/refresh changes, or fast drag-resizing on mixed refresh-rate monitors can occasionally stutter or misalign a frame depending on your GPU driver and client toolkit.
3. **XWayland App & Game Focus Quirks:**
   Some X11/Proton fullscreen windows aggressively fight `ConfigureRequest` sizing or Alt+Tab stacking; report specific titles so focus/geometry rules can be refined.
4. **No `xdg-desktop-portal` Screen Capture Yet:**
   PipeWire screen sharing (OBS, Discord, browser screen share) and native screenshot/recording portals are planned on the roadmap and not implemented yet.
5. **External Notification Bridge Process:**
   The integrated `org.freedesktop.Notifications` D-Bus listener currently spawns a small `python3` + PyGObject (`gi.repository.Gio`) bridge until a pure-Rust `zbus` daemon replaces it.

---

## 🪶 Resource Usage (RAM vs VRAM)

Instead of pretending Velox uses an unrealistic 20 MB while bundling a full desktop shell, launcher, notifications, lock screen, and settings GUI in-process, here is the real footprint observed on actual hardware:

- **Lightweight on VRAM:** No heavy blur passes, reflection shaders, Chromium/Electron renderers, or bloated toolkit GPU textures—compositing sticks to lean DRM/KMS/EGL buffers and direct surface composition, keeping **VRAM usage minimal** across high-res and multi-monitor setups (great for gaming and integrated GPUs).
- **Unified Process Footprint (No Daemon Soup):** Replaces separate compositor + panel (`waybar`/`eww`) + launcher (`rofi`/`fuzzel`) + notification daemon (`dunst`/`mako`) + lock screen + GTK/Qt settings apps with two binaries (`velox-wm` & on-demand `velox-settings`).
- **Active Memory Hygiene (`src/mem_cache.rs`):** Built-in `/proc/self/status` `VmRSS` tracking, automatic glyph/pixel cache trimming, `libc::malloc_trim(0)` heap release, and commit throttling for minimized/hidden/occluded windows (`freeze_hidden_windows`).

| Component | Binary Size (`release`) | Observed Resource Footprint |
| --- | --- | --- |
| `velox-wm` (Compositor + Shell + XWayland) | `16.6 MiB` (`17,384,304` bytes) | **VRAM:** Very low / minimal compositor overhead &bull; **RAM:** `~280–350 MB` RSS typical (known creep bug can reach `~800 MB` — see [Known Bugs](#-known-bugs--current-limitations)) |
| `velox-settings` (Zero-Toolkit GUI) | `4.2 MiB` (`4,418,512` bytes) | On-demand only; software `wl_shm` UI linking just `libc`, `libgcc_s`, and `libxkbcommon` (zero GTK/Qt/Electron) |
| `velox-session` (Reload supervisor) | `612` bytes | POSIX shell wrapper supervising LightDM seamless reload (`exit 42`) |

---

## 🚀 Key Features

### Window Management & Adaptive Display Refresh
- **Three Layout Modes:** Switch live between **Master-Stack (`Tile`)**, **Columns (`Cols`)**, and **Floating (`Float`)** across 9 virtual workspaces (`Super+1..9`).
- **Any Refresh Rate + VRR (`60Hz`–`240Hz+`):** Not locked to a single rate—configure resolution, exact target Hz (`60`, `75`, `120`, `144`, `165`, etc.), fractional scaling (`0.75`–`3.0`), primary output, and VRR (`Auto` / `AlwaysOn` / `Off`) per monitor with a 15-second keep-or-revert safety modal.
- **Zero-Black-Screen Video & Idle Battery Saver:** Multi-surface `CommitCadenceTracker` detects `24/25/30/50/60 fps` video playback (including `60fps` streams on higher-refresh panels while excluding games) alongside idle refresh reduction and an optional pass-through Refresh & Battery Saver HUD overlay.
- **Minimal View for Compact / Underscaled Windows:** Cramped windows (`< 340×220` or smaller than client `min_size`, or toggled via taskbar context menu) cleanly switch into an interactive Minimal Card instead of breaking client layouts; double-click or right-click to expand back.
- **Fractional Scaling & XWayland:** Supports `wp_fractional_scale_manager_v1`, `wl_viewport`, `wlr`/`ext` data-control clipboards, Wayland/X11 selection sync, and full `X11Wm` XWayland support.
- **Aero Snap, Deterministic Unfullscreen & Inactive Dimming:** Edge/corner drag-snap zones, preserved pre-fullscreen floating bounds, deterministic workspace tiling order, and focus dimming (excluding fullscreen games and video).

### ChromeOS-Style Shelf & Customizable Quick Access Panel
- **Customizable Shelf & Visuals:** Top or bottom position, floating pill or full-width bar, customizable height/opacity/corner radius, 8 accent presets, and procedural or custom wallpapers (`~/.config/velox-wm/wallpaper.jpg`/`.png`).
- **Reorderable & Toggleable Tray Pills:** Move or hide Quick Access pills (`notif`, `bg_processes`, `control`, `wifi`, `bright`, `vol`, `float`) from **Quick Settings → Customize Tiles** or **Velox Settings → Personalize** with popups dynamically anchored to moved tiles.
- **Unicode, Nerd Font, Emoji & Terminal Recognition:** Fallback font stack supporting Nerd Fonts, Powerline glyphs, CJK vertical bars (`︱`), and emoji; terminal emulators (`alacritty`, `foot`, `kitty`, `wezterm`, `ghostty`, etc.) show a `>_` taskbar badge and `"Term"` classification even when `.desktop` sets `Terminal=false`, plus built-in right-click Copy/Paste/Open Another Window controls.

### Start Menu & Editable App Library
- **Smart Category Switching:** Automatic or one-click **"Search in All Categories"** fallback when a query yields zero matches in the selected category.
- **Right-Click Context Menus & Custom Rules:** Right-click any app in the Start Menu or **Velox Settings → Apps & Startup** to launch, change categories, toggle **Run in Terminal**, cycle **Game Detection (`Auto` / `Force ON` / `Force OFF`)**, hide/unhide, or jump directly to full metadata editing saved live in `~/.config/velox-wm/app_library.toml` (including custom apps and keyword detection rules).

### Pixel-OS-Inspired Notifications
- **Built-in D-Bus Notification Center:** Toast alerts + Notification Drawer (`Super+U`) with inline reply drafts, suggested actions (OTP copy, URL launch), snooze, burst **Cooldown** audio ducking (up to 1 minute), priority conversations, per-app Mute/Disable toggles, and persistent history (`~/.local/share/velox-wm/notification-history.json`).
- **On-Device Organizer & Recaps:** Local keyword bundling and quick conversation recaps processed completely on-device without external telemetry.

### Gaming Focus, Audio Roles, Idle Power & Seamless Reload
- **Game Focus Mode:** Automatically confines pointers inside focused games across multi-monitor setups (`Alt+Tab` to switch windows cleanly) plus optional fullscreen taskbar locking.
- **Audio & Roles (`pactl`):** Default output/mic controls, device disable toggles, per-stream sliders/mute/device routing, and automatic **Comms vs Media** device routing.
- **Idle Screen Dim, Sleep & Lock Screen:** Automatic inactivity dimming and DPMS/display sleep timers (with video-playback inhibit & lock-on-sleep) plus built-in PAM/`unix_chkpwd` Lock Screen (`Super+L` / `Ctrl+Alt+L`).
- **Seamless Reload Without Losing Layout Placement:** `install.sh --reload` or `Super+Ctrl+Shift+R` opens an informative confirmation dialog, saves a workspace/window/notification snapshot, re-execs `velox-wm` cleanly (`exit 42`), and notifies on resume.

---

## ⌨️ Keyboard Shortcuts

Default bindings (`windows_shortcuts = true`; toggleable in **Velox Settings → Multitasking & Keys**):

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
| `Alt+Tab` / `Alt+Shift+Tab` | Window Switcher (Cross-Monitor Aware) |
| `Super+Left` / `Super+Right` | Snap Left / Right Half |
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
| `Super+Shift+E` / `Alt+Shift+E` | Exit Velox Session to Display Manager |
| `Super + Left Drag` / `Super + Right Drag` | Move Window / Resize Window |

---

## 📦 Dependencies & Prerequisites

### Build Requirements
- Rust toolchain (`cargo` / `rustc` supporting **Rust 2024 edition**)
- System libraries & headers: `libseat`, `libinput`, `libudev`, `libdrm`, `libgbm`, `mesa` (EGL / GLES), `libxkbcommon`

On Arch Linux:
```bash
sudo pacman -S --needed rust mesa libdrm libinput libseat libxkbcommon systemd-libs
```

### Runtime Companions (Optional / Recommended)
- **XWayland apps & Steam/Proton games:** `xorg-xwayland`
- **Audio mixer & Comms/Media roles:** `pipewire-pulse` / `pulseaudio` (`pactl`)
- **Wi-Fi & Bluetooth toggles:** `networkmanager` (`nmcli`), `bluez-utils` (`bluetoothctl`)
- **D-Bus notifications & clipboard:** `python`, `python-gobject`, `wl-clipboard` (`wl-copy`)
- **Brightness & power:** built-in `/sys/class/backlight` + optional `ddcutil`, `power-profiles-daemon` (`powerprofilesctl`), `polkit-gnome`, `xdg-utils`, `curl`
- **Display manager:** LightDM (or any DM reading `/usr/share/wayland-sessions/`)

---

## ⚙️ Installation

1. Clone the repository:
   ```bash
   git clone https://github.com/simonjuniorj/velox-wm.git
   cd velox-wm
   ```

2. Build and install using [`install.sh`](install.sh):
   ```bash
   ./install.sh
   ```
   This builds release binaries (`--offline` first with automatic online fallback) and installs:
   - `/usr/local/bin/velox-wm`
   - `/usr/local/bin/velox-settings`
   - `/usr/local/bin/velox-session`
   - `/usr/share/wayland-sessions/velox-wm.desktop`
   - `/usr/share/applications/velox-settings.desktop`

3. **Start Using Velox WM:** Log out to LightDM and select **Velox WM**, or test nested safely inside an existing Wayland/X11 session:
   ```bash
   cargo run --release --bin velox-wm -- --nested
   ```

4. **Hot-Reload After Rebuilding:**
   ```bash
   ./install.sh --reload      # Opens pre-reload confirmation dialog & creates snapshot
   ./install.sh --reload-now  # Immediate snapshot + reload without modal
   ```
   *(Optional: set `github_update_repo = "simonjuniorj/velox-wm"` in `~/.config/velox-wm/config.toml` so Velox's built-in updater checks GitHub releases.)*

---

## 🗑️ Uninstallation

Run the included [`uninstall.sh`](uninstall.sh) script:
```bash
./uninstall.sh
```
User configs under `~/.config/velox-wm` are kept intact by default. Pass `--purge-data` to remove user configs, logs, caches, and notification history as well:
```bash
./uninstall.sh --purge-data
```

Or remove manually:
```bash
sudo rm -f \
  /usr/local/bin/velox-wm \
  /usr/local/bin/velox-settings \
  /usr/local/bin/velox-session \
  /usr/share/wayland-sessions/velox-wm.desktop \
  /usr/share/applications/velox-settings.desktop
```

---

## 📁 Configuration & Data Files

| Path | Purpose |
| --- | --- |
| `~/.config/velox-wm/config.toml` | Appearance, tiling, per-monitor refresh/scale/VRR, quick-tile order, notifications, audio roles, shortcuts |
| `~/.config/velox-wm/app_library.toml` | Custom apps, per-app overrides (category, badge, terminal, hidden, game detection), category rules |
| `~/.config/velox-wm/wallpaper.jpg` / `.png` | Custom wallpaper when `use_custom_wallpaper = true` |
| `~/.local/share/velox-wm/notification-history.json` | Persisted notification history & seen-app notification settings |
| `~/.local/state/velox-wm/velox.log` | Compositor runtime logs (attach when reporting bugs, glitches, or stutters) |

---

## 🗺️ Roadmap & To-Do

- [ ] **Fix RAM Creep Bug (`~350 MB` → `~800 MB`):** Profile buffer/texture/XWayland retention over long sessions and lock idle memory strictly down.
- [ ] **Per-Monitor Independent Workspaces & Hotplug Profiles:** Switch workspaces per physical monitor independently and save docked/undocked display profiles.
- [ ] **Full Keybind Remapper & Trackpad Gestures:** Graphical shortcut editor in `velox-settings` + 3/4-finger libinput swipe gestures for Task View, workspaces, and Show Desktop.
- [ ] **`xdg-desktop-portal-velox` & Screen Sharing / Screenshots:** PipeWire screen capture for OBS, Discord, and browsers plus built-in region screenshots/recording.
- [ ] **Pure-Rust Notification D-Bus Bridge (`zbus`):** Replace the `python3`/PyGObject helper with an in-process Rust D-Bus server to shave memory & startup overhead.
- [ ] **System Tray (`StatusNotifierItem` / AppIndicator):** Native third-party app tray icons inside the Quick Access panel.
- [ ] **Lightweight GPU Shaders & Night Light:** Opt-in subtle panel blur, workspace slide transitions, and Night Light color temperature schedules.
- [ ] **CI & Distro Packaging:** GitHub Actions build/test checks, Arch AUR `PKGBUILD`, Debian/Fedora packaging, and GitHub release tarballs for auto-update checks.
- [ ] **Gaming & Scaling Polish:** Per-app XWayland DPI controls, HDR display reporting, and custom cursor theme/size pickers in `velox-settings`.

---

## 🔗 More Projects by [simonjuniorj](https://github.com/simonjuniorj)

Other public repositories available on my GitHub profile:

| Project | Stack | Description |
| --- | --- | --- |
| 🤖 **[Cloudi](https://github.com/simonjuniorj/Cloudi)** *(Discontinued)* | JavaScript &bull; Discord.js v14 &bull; MongoDB | Official advanced moderation, SFW media safety scanning, anti-phishing, ticketing & logging Discord bot created for the Szkoła w Chmurze server network |
| 🐾 **[Yukki-Bot](https://github.com/simonjuniorj/Yukki-Bot)** *(Open Source)* | JavaScript &bull; Discord.js v13 | Multipurpose Discord bot with rich commands and zero database setup required |
| 🎵 **[SheepBot](https://github.com/simonjuniorj/SheepBot)** | JavaScript &bull; Discord.js v12 | Music bot for Discord servers |
| 🧮 **[srednia](https://github.com/simonjuniorj/srednia)** | C# | Lightweight utility application for calculating arithmetic means |

👉 Browse everything at **[github.com/simonjuniorj](https://github.com/simonjuniorj?tab=repositories)**

---

## 🤝 Contributing & Bug Reporting

PRs, memory-leak traces, frame-pacing diagnostics, and UI fixes are warmly appreciated:
- See [CONTRIBUTING.md](CONTRIBUTING.md) for the full module architecture map, `--nested` testing guide, and `cargo test --offline` checks.
- Hit a bug, memory spike, visual glitch, or frame stutter? Open an issue via [Bug / Glitch / Stutter Report](.github/ISSUE_TEMPLATE/bug_report.md) or propose features via [Feature Request](.github/ISSUE_TEMPLATE/feature_request.md).

---

## 📄 License

Licensed under the **GNU General Public License v3.0** — see the [LICENSE](LICENSE) file for full details.

---

<div id="wersja-polska"></div>

## 🇵🇱 Wersja Polska — Velox WM

**Velox WM** to płynny, adaptacyjny kompozytor **Wayland** oraz zintegrowane środowisko pulpitu napisane w języku **Rust** (`smithay`), działające na dowolnym odświeżaniu monitora (**60Hz, 75Hz, 120Hz, 144Hz, 240Hz+** oraz **VRR**). Zamiast łączyć kilkanaście osobnych programów, łączy pasek, launcher, powiadomienia, ekran blokady i ustawienia w jednej spójnej całości przy bardzo niskim zużyciu pamięci **VRAM**.

> ⚠️ **Wczesny etap rozwoju / Produkt nieukończony:**  
> Projekt powstał **wczoraj (2026-11-26)** i znajduje się we wczesnej fazie rozwoju (`v0.6.0`). Mogą zdarzać się błędy, artefakty graficzne, przycięcia lub wycieki pamięci. Jeśli coś nie działa jak należy — **zgłoś błąd, glitch lub stutter w zakładce [Issues](../../issues)** i dołącz log `~/.local/state/velox-wm/velox.log`.

### 📰 Aktualności
- **2026-11-27** 🚀 **Publikacja i rozbudowa dokumentacji, sekcja Znanych Błędów, GPL-3.0 oraz polskie poprawki (`v0.6.0`)**: Dodano **Minimal View** dla zbyt małych okien, personalizację kafelków na pasku zadań, `wp_fractional_scale_manager_v1`, naprawę pozycji kursora przy skalowaniu w `velox-settings`, obsługę symboli `︱`/Nerd/Emoji, badge `>_` dla terminali (w tym Alacritty), poprawki powrotu z pełnego ekranu oraz wykrywanie wideo 24/60fps w Video Battery Saver.
- **2026-11-27** 🔔 **Powiadomienia Pixel, tryb bezczynności i reguły App Library (`v0.6.0`)**: Dodano lokalny organizer i podsumowania powiadomień, tryb Cooldown, Snooze, menu kontekstowe pod PPM w bibliotece aplikacji, przyciemnianie/usypianie ekranu oraz ekran blokady (`Super+L`).
- **2026-11-26** 🌱 **Narodziny projektu (`v0.6.0`)**: Start **Velox WM** jako zupełnie nowego projektu systemowego uzupełniającego dotychczasowe repozytoria na moim GitHubie ([Cloudi](https://github.com/simonjuniorj/Cloudi), [Yukki-Bot](https://github.com/simonjuniorj/Yukki-Bot), [SheepBot](https://github.com/simonjuniorj/SheepBot), [srednia](https://github.com/simonjuniorj/srednia)).

### ⚠️ Znane błędy i zużycie zasobów
- **Realne zużycie RAM (~280–350 MB, znany błąd wzrostu do ~800 MB):** Na ten moment podczas normalnej pracy `velox-wm` zajmuje zazwyczaj **280–350 MB RAM**, jednak przy dłuższej sesji występuje **znany problem wzrostu pamięci nawet do ~800 MB** z nieustalonej jeszcze przyczyny (w trakcie diagnozy).
- **Bardzo lekkie zużycie VRAM:** Kompozytor nie używa ciężkich shaderów rozmycia ani procesów Chromium/Electron/GTK/Qt, więc obciążenie pamięci karty graficznej (**VRAM**) pozostaje minimalne.
- **Brak jeszcze portalu udostępniania ekranu (`xdg-desktop-portal`):** Nagrywanie / udostępnianie ekranu w Discord/OBS na Waylandzie jest na liście **To-Do**.

### 🔗 Inne moje projekty na GitHubie
- 🤖 **[Cloudi](https://github.com/simonjuniorj/Cloudi)** *(Zarchiwizowany)* — Zaawansowany bot Discord dla Szkoły w Chmurze (`Discord.js v14`, `MongoDB`)
- 🐾 **[Yukki-Bot](https://github.com/simonjuniorj/Yukki-Bot)** *(Open Source)* — Wielofunkcyjny bot Discord niewymagający bazy danych (`Discord.js v13`)
- 🎵 **[SheepBot](https://github.com/simonjuniorj/SheepBot)** — Muzyczny bot Discord (`Discord.js v12`)
- 🧮 **[srednia](https://github.com/simonjuniorj/srednia)** — Prosta aplikacja w C# do obliczania średniej arytmetycznej

Licencja: **GNU GPL v3.0** ([LICENSE](LICENSE)).
