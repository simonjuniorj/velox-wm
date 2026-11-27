use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelPosition {
    Top,
    Bottom,
}

impl PanelPosition {
    pub fn toggle(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::Bottom => "Bottom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShelfStyle {
    Floating,
    Full,
}

impl ShelfStyle {
    pub fn toggle(self) -> Self {
        match self {
            Self::Floating => Self::Full,
            Self::Full => Self::Floating,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Floating => "ChromeOS Pill",
            Self::Full => "Full Width",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayoutMode {
    MasterStack,
    Columns,
    Floating,
}

impl LayoutMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::MasterStack => Self::Columns,
            Self::Columns => Self::Floating,
            Self::Floating => Self::MasterStack,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::MasterStack => "Master-Stack",
            Self::Columns => "Columns",
            Self::Floating => "Floating",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::MasterStack => "Tile",
            Self::Columns => "Cols",
            Self::Floating => "Float",
        }
    }
}

pub const ACCENT_PRESETS: &[(&str, [u8; 3])] = &[
    ("Cobalt", [66, 135, 245]),
    ("Violet", [147, 94, 247]),
    ("Emerald", [36, 198, 138]),
    ("Cyan", [38, 198, 218]),
    ("Amber", [245, 166, 35]),
    ("Crimson", [235, 77, 107]),
    ("Rose", [244, 114, 182]),
    ("Mint", [52, 211, 153]),
];

pub const WALLPAPER_PRESETS: &[&str] =
    &["Aurora", "Nebula", "Ocean", "Emerald", "Sunset", "Carbon"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VrrMode {
    Auto,
    AlwaysOn,
    Off,
}

impl Default for VrrMode {
    fn default() -> Self {
        Self::Auto
    }
}

impl VrrMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Auto => Self::AlwaysOn,
            Self::AlwaysOn => Self::Off,
            Self::Off => Self::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (Video / Idle)",
            Self::AlwaysOn => "Always On",
            Self::Off => "Off",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Auto => "VRR Auto",
            Self::AlwaysOn => "VRR On",
            Self::Off => "VRR Off",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplaySleepMode {
    BlackScreen,
    DpmsSleep,
}

impl Default for DisplaySleepMode {
    fn default() -> Self {
        Self::DpmsSleep
    }
}

impl DisplaySleepMode {
    pub fn toggle(self) -> Self {
        match self {
            Self::BlackScreen => Self::DpmsSleep,
            Self::DpmsSleep => Self::BlackScreen,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DpmsSleep => "Display Sleep (DPMS)",
            Self::BlackScreen => "Black Screen Curtain",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::DpmsSleep => "DPMS Sleep",
            Self::BlackScreen => "Black Out",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResolutionModes {
    pub width: i32,
    pub height: i32,
    pub hz: Vec<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub refresh_hz: u32,
    pub scale: f32,
    pub vrr_mode: Option<VrrMode>,
    pub primary: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputTelemetry {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub current_hz: u32,
    pub preferred_hz: u32,
    pub available_hz: Vec<u32>,
    pub resolution_modes: Vec<ResolutionModes>,
    pub scale_x100: u32,
    pub primary: bool,
    pub vrr_capable: bool,
    pub vrr_requires_modeset: bool,
    pub vrr_enabled: bool,
    pub battery_saver_state: String,
    pub active_pacing_hz: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayTelemetry {
    pub outputs: Vec<OutputTelemetry>,
    pub detected_video_fps: u32,
}

impl DisplayTelemetry {
    pub fn path() -> Option<PathBuf> {
        if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR").filter(|s| !s.is_empty()) {
            return Some(PathBuf::from(rt).join("velox-display.json"));
        }
        let uid = unsafe { libc::getuid() };
        Some(PathBuf::from(format!("/tmp/velox-display-{uid}.json")))
    }

    pub fn save(&self) -> bool {
        let Some(path) = Self::path() else {
            return false;
        };
        let Some(dir) = path.parent() else {
            return false;
        };
        let _ = fs::create_dir_all(dir);
        let tmp = path.with_extension("json.tmp");
        let Ok(json) = serde_json::to_string(self) else {
            return false;
        };
        if fs::write(&tmp, json).is_err() {
            return false;
        }
        fs::rename(&tmp, &path).is_ok()
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshPlan {
    pub effective_pace_hz: u32,
    pub target_kms_hz: u32,
    pub use_vrr: bool,
    pub label: String,
}

#[allow(clippy::too_many_arguments)]
pub fn compute_effective_refresh_plan(
    base_cap_hz: u32,
    monitor_max_hz: u32,
    available_hz: &[u32],
    vrr_mode: VrrMode,
    vrr_capable: bool,
    video_battery_saver: bool,
    idle_refresh_hz: u32,
    battery_saver_modeset: bool,
    seamless_refresh_overlay: bool,
    input_idle_secs: f32,
    animating: bool,
    dragging: bool,
    detected_video_fps: u32,
    visible_content_active: bool,
) -> RefreshPlan {
    let max_hz = monitor_max_hz.max(60);
    let user_cap = if base_cap_hz > 0 {
        pick_nearest_mode_hz(base_cap_hz, available_hz).unwrap_or(base_cap_hz.min(max_hz))
    } else {
        max_hz
    };
    let steady_kms_hz = if seamless_refresh_overlay {
        max_hz
    } else {
        user_cap
    };

    if animating || dragging || input_idle_secs < 1.2 {
        let use_vrr = vrr_capable && vrr_mode == VrrMode::AlwaysOn;
        return RefreshPlan {
            effective_pace_hz: user_cap,
            target_kms_hz: steady_kms_hz,
            use_vrr,
            label: if use_vrr {
                format!("Active {user_cap}Hz (VRR)")
            } else if seamless_refresh_overlay && user_cap < steady_kms_hz {
                format!("Overlay {user_cap}Hz ({steady_kms_hz}Hz Panel)")
            } else {
                format!("Active {user_cap}Hz")
            },
        };
    }

    if visible_content_active || detected_video_fps > 0 {
        let use_vrr = vrr_capable && vrr_mode != VrrMode::Off;
        if video_battery_saver && detected_video_fps > 0 && detected_video_fps < user_cap {
            let video_hz = detected_video_fps.clamp(10, user_cap);
            if use_vrr {
                return RefreshPlan {
                    effective_pace_hz: video_hz,
                    target_kms_hz: steady_kms_hz,
                    use_vrr: true,
                    label: format!("Video {video_hz}Hz (VRR)"),
                };
            }
            if battery_saver_modeset && !seamless_refresh_overlay && input_idle_secs >= 2.0 {
                let kms_hz = pick_saver_kms_hz(video_hz, user_cap, available_hz)
                    .unwrap_or(steady_kms_hz);
                return RefreshPlan {
                    effective_pace_hz: video_hz,
                    target_kms_hz: kms_hz,
                    use_vrr: false,
                    label: format!("Video {kms_hz}Hz (KMS)"),
                };
            }
            if seamless_refresh_overlay {
                let overlay_hz = video_hz.clamp(24, 60).min(user_cap);
                return RefreshPlan {
                    effective_pace_hz: overlay_hz,
                    target_kms_hz: steady_kms_hz,
                    use_vrr: false,
                    label: format!("Video {overlay_hz}Hz (Overlay)"),
                };
            }
        }
        // On fixed-refresh outputs without video battery saver (or unquantized motion), present
        // incoming frames at full cap VBlank pacing with zero artificial sleep.
        let always_vrr = vrr_capable && vrr_mode == VrrMode::AlwaysOn;
        return RefreshPlan {
            effective_pace_hz: user_cap,
            target_kms_hz: steady_kms_hz,
            use_vrr: always_vrr,
            label: if detected_video_fps > 0 {
                if seamless_refresh_overlay && user_cap < steady_kms_hz {
                    format!("Video {user_cap}Hz (Overlay)")
                } else {
                    format!("Video Sync {user_cap}Hz")
                }
            } else if always_vrr {
                format!("Motion {user_cap}Hz (VRR)")
            } else if seamless_refresh_overlay && user_cap < steady_kms_hz {
                format!("Overlay {user_cap}Hz")
            } else {
                format!("Smooth {user_cap}Hz")
            },
        };
    }

    if video_battery_saver && idle_refresh_hz > 0 && input_idle_secs >= 3.0 {
        let idle_hz = idle_refresh_hz.clamp(10, user_cap);
        let use_vrr = vrr_capable && vrr_mode != VrrMode::Off;
        let target_kms_hz = if use_vrr || seamless_refresh_overlay {
            steady_kms_hz
        } else if battery_saver_modeset && input_idle_secs >= 5.0 {
            pick_saver_kms_hz(idle_hz, user_cap, available_hz).unwrap_or(steady_kms_hz)
        } else {
            steady_kms_hz
        };
        return RefreshPlan {
            effective_pace_hz: idle_hz,
            target_kms_hz,
            use_vrr,
            label: if use_vrr {
                format!("Idle {idle_hz}Hz (VRR)")
            } else if target_kms_hz < steady_kms_hz {
                format!("Idle {target_kms_hz}Hz (KMS)")
            } else if seamless_refresh_overlay {
                format!("Idle {idle_hz}Hz (Overlay)")
            } else {
                format!("Idle {idle_hz}Hz (Saver)")
            },
        };
    }

    let use_vrr = vrr_capable && vrr_mode == VrrMode::AlwaysOn;
    RefreshPlan {
        effective_pace_hz: user_cap,
        target_kms_hz: steady_kms_hz,
        use_vrr,
        label: if use_vrr {
            format!("VRR {user_cap}Hz")
        } else if seamless_refresh_overlay && user_cap < steady_kms_hz {
            format!("Overlay {user_cap}Hz")
        } else {
            format!("Native {user_cap}Hz")
        },
    }
}

pub fn pick_nearest_mode_hz(target: u32, available_hz: &[u32]) -> Option<u32> {
    if available_hz.is_empty() || target == 0 {
        return None;
    }
    available_hz
        .iter()
        .copied()
        .min_by_key(|&hz| (hz.abs_diff(target), std::cmp::Reverse(hz)))
}

fn pick_saver_kms_hz(desired_min_hz: u32, cap_hz: u32, available_hz: &[u32]) -> Option<u32> {
    available_hz
        .iter()
        .copied()
        .filter(|&hz| hz >= desired_min_hz && hz < cap_hz)
        .min()
        .or_else(|| {
            available_hz
                .iter()
                .copied()
                .filter(|&hz| hz < cap_hz)
                .min_by_key(|&hz| hz.abs_diff(desired_min_hz))
        })
}

pub const DEFAULT_TASKBAR_QUICK_TILES: &[&str] = &[
    "float",
    "vol",
    "bright",
    "wifi",
    "control",
    "bg_processes",
    "notif",
];

pub const DEFAULT_QUICK_PANEL_TILES: &[&str] = &[
    "wifi",
    "bluetooth",
    "layout",
    "edge_snap",
    "dim_inactive",
    "video_saver",
];

fn default_taskbar_quick_tiles() -> Vec<String> {
    DEFAULT_TASKBAR_QUICK_TILES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

fn default_quick_panel_tiles() -> Vec<String> {
    DEFAULT_QUICK_PANEL_TILES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

fn default_true() -> bool {
    true
}

pub fn quick_tile_label(id: &str) -> &'static str {
    match id {
        "float" => "Float / Layout",
        "vol" => "Volume",
        "bright" => "Brightness",
        "wifi" => "Wi-Fi / BT",
        "control" => "Tile Control",
        "bg_processes" => "Bg Processes",
        "notif" => "Notifications",
        "bluetooth" => "Bluetooth",
        "layout" => "Layout Mode",
        "edge_snap" => "Edge Snap",
        "dim_inactive" => "Dim Inactive",
        "video_saver" => "Video Saver",
        _ => "Tile",
    }
}

/// Runtime configuration for velox-wm, atomically persisted to `~/.config/velox-wm/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub background_color: [f32; 4],
    pub wallpaper_preset: String,
    pub wallpaper_path: Option<String>,
    pub use_custom_wallpaper: bool,
    pub accent_theme: String,
    pub panel_position: PanelPosition,
    pub shelf_style: ShelfStyle,
    pub panel_height: i32,
    pub panel_opacity: u8,
    pub corner_radius: i32,
    pub border_width: i32,
    pub terminal_cmd: String,
    pub browser_cmd: String,
    pub files_cmd: String,
    pub launcher_cmd: String,
    pub settings_cmd: String,
    pub autostart: Vec<String>,
    pub auto_launch_terminal_on_login: bool,
    pub gap_px: i32,
    pub master_ratio: f32,
    pub layout_mode: LayoutMode,
    pub auto_tile: bool,
    pub edge_snap: bool,
    pub windows_shortcuts: bool,
    pub show_system_processes: bool,
    pub output_refresh_hz: u32,
    pub vrr_mode: VrrMode,
    pub video_battery_saver: bool,
    pub idle_refresh_hz: u32,
    pub battery_saver_modeset: bool,
    pub seamless_refresh_overlay: bool,
    pub show_refresh_overlay: bool,
    pub confirm_before_display_apply: bool,
    pub monitors: Vec<MonitorConfig>,
    pub freeze_hidden_windows: bool,
    pub game_pointer_lock: bool,
    pub lock_fullscreen_taskbar: bool,
    pub dim_unfocused_windows: bool,
    pub unfocused_dim_opacity: f32,
    pub unfocused_window_opacity: f32,
    pub dim_exclude_fullscreen: bool,
    pub auto_clean_ram_cache: bool,
    pub ram_cache_threshold_pct: u8,
    pub app_prewarm_cache: bool,
    pub notifications_dnd: bool,
    pub allow_notification_snooze: bool,
    pub hide_silent_notifications_in_status_bar: bool,
    pub notification_cooldown_enabled: bool,
    pub notification_organizer_enabled: bool,
    pub organized_silenced_categories: Vec<String>,
    pub notification_summaries_enabled: bool,
    pub notification_history_enabled: bool,
    pub priority_conversations: Vec<String>,
    pub disabled_notification_apps: Vec<String>,
    pub silent_notification_apps: Vec<String>,
    pub app_category_overrides: BTreeMap<String, String>,
    pub idle_dim_secs: u32,
    pub idle_sleep_secs: u32,
    pub idle_sleep_mode: DisplaySleepMode,
    pub idle_inhibit_on_video: bool,
    pub lock_on_sleep: bool,
    pub check_github_updates: bool,
    pub github_update_repo: String,
    pub saved_volume_pct: Option<u8>,
    pub saved_muted: Option<bool>,
    pub saved_mic_volume_pct: Option<u8>,
    pub saved_mic_muted: Option<bool>,
    pub saved_brightness_pct: Option<u8>,
    pub saved_power_profile: Option<String>,
    pub focus_follows_mouse: bool,
    pub animations_enabled: bool,
    pub animation_speed: f32,
    pub comms_output_device: Option<String>,
    pub media_output_device: Option<String>,
    pub auto_route_audio_roles: bool,
    pub disabled_audio_sinks: Vec<String>,
    pub disabled_audio_sources: Vec<String>,
    pub min_window_size: (i32, i32),
    #[serde(default = "default_taskbar_quick_tiles")]
    pub taskbar_quick_tiles: Vec<String>,
    #[serde(default)]
    pub hidden_quick_tiles: Vec<String>,
    #[serde(default = "default_quick_panel_tiles")]
    pub quick_panel_tiles: Vec<String>,
    #[serde(default = "default_true")]
    pub minimal_view_enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            background_color: [0.0157, 0.0275, 0.0784, 1.0],
            wallpaper_preset: "Aurora".to_string(),
            wallpaper_path: None,
            use_custom_wallpaper: true,
            accent_theme: "Cobalt".to_string(),
            panel_position: PanelPosition::Bottom,
            shelf_style: ShelfStyle::Floating,
            panel_height: 44,
            panel_opacity: 224,
            corner_radius: 12,
            border_width: 2,
            terminal_cmd: "alacritty".to_string(),
            browser_cmd: "zen-browser".to_string(),
            files_cmd: "thunar".to_string(),
            launcher_cmd: "fuzzel".to_string(),
            settings_cmd: "velox-settings".to_string(),
            autostart: vec![
                "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1".to_string(),
            ],
            auto_launch_terminal_on_login: false,
            gap_px: 10,
            master_ratio: 0.55,
            layout_mode: LayoutMode::MasterStack,
            auto_tile: true,
            edge_snap: true,
            windows_shortcuts: true,
            show_system_processes: false,
            output_refresh_hz: 0,
            vrr_mode: VrrMode::Off,
            video_battery_saver: true,
            idle_refresh_hz: 24,
            battery_saver_modeset: false,
            seamless_refresh_overlay: true,
            show_refresh_overlay: true,
            confirm_before_display_apply: true,
            monitors: Vec::new(),
            freeze_hidden_windows: true,
            game_pointer_lock: true,
            lock_fullscreen_taskbar: true,
            dim_unfocused_windows: true,
            unfocused_dim_opacity: 0.22,
            unfocused_window_opacity: 1.0,
            dim_exclude_fullscreen: true,
            auto_clean_ram_cache: true,
            ram_cache_threshold_pct: 78,
            app_prewarm_cache: true,
            notifications_dnd: false,
            allow_notification_snooze: true,
            hide_silent_notifications_in_status_bar: false,
            notification_cooldown_enabled: true,
            notification_organizer_enabled: true,
            organized_silenced_categories: vec![
                "Promotions".to_string(),
                "News".to_string(),
            ],
            notification_summaries_enabled: true,
            notification_history_enabled: true,
            priority_conversations: Vec::new(),
            disabled_notification_apps: Vec::new(),
            silent_notification_apps: Vec::new(),
            app_category_overrides: BTreeMap::new(),
            idle_dim_secs: 300,
            idle_sleep_secs: 900,
            idle_sleep_mode: DisplaySleepMode::DpmsSleep,
            idle_inhibit_on_video: true,
            lock_on_sleep: false,
            check_github_updates: true,
            github_update_repo: String::new(),
            saved_volume_pct: None,
            saved_muted: None,
            saved_mic_volume_pct: None,
            saved_mic_muted: None,
            saved_brightness_pct: None,
            saved_power_profile: None,
            focus_follows_mouse: false,
            animations_enabled: true,
            animation_speed: 0.28,
            comms_output_device: None,
            media_output_device: None,
            auto_route_audio_roles: true,
            disabled_audio_sinks: Vec::new(),
            disabled_audio_sources: Vec::new(),
            min_window_size: (64, 64),
            taskbar_quick_tiles: default_taskbar_quick_tiles(),
            hidden_quick_tiles: Vec::new(),
            quick_panel_tiles: default_quick_panel_tiles(),
            minimal_view_enabled: true,
        }
    }
}

impl Config {
    pub fn config_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|s| !s.is_empty())
                    .map(|h| PathBuf::from(h).join(".config"))
            })
            .map(|p| p.join("velox-wm"))
    }

    pub fn config_path() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("config.toml"))
    }

    pub fn config_modified_nanos() -> u128 {
        Self::config_path()
            .and_then(|p| fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    pub fn load_or_create() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };

        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(raw) => match toml::from_str::<Config>(&raw) {
                    Ok(mut cfg) => {
                        // Keep legacy `auto_tile` boolean synchronized with `layout_mode`.
                        if !cfg.auto_tile && cfg.layout_mode != LayoutMode::Floating {
                            cfg.layout_mode = LayoutMode::Floating;
                        } else if cfg.auto_tile && cfg.layout_mode == LayoutMode::Floating {
                            cfg.auto_tile = false;
                        }
                        return cfg;
                    }
                    Err(err) => {
                        tracing::warn!(?err, ?path, "Failed to parse config.toml, using defaults");
                    }
                },
                Err(err) => {
                    tracing::warn!(?err, ?path, "Failed to read config.toml, using defaults");
                }
            }
        } else {
            let default = Self::default();
            let _ = default.save();
            return default;
        }

        Self::default()
    }

    /// Atomically saves preferences to `~/.config/velox-wm/config.toml` and returns true on success.
    pub fn save(&self) -> bool {
        let Some(dir) = Self::config_dir() else {
            return false;
        };
        if let Err(err) = fs::create_dir_all(&dir) {
            tracing::warn!(?err, ?dir, "Failed to create config directory");
            return false;
        }
        let path = dir.join("config.toml");
        let tmp_path = dir.join("config.toml.tmp");
        match toml::to_string_pretty(self) {
            Ok(serialized) => {
                if let Err(err) = fs::write(&tmp_path, serialized) {
                    tracing::warn!(?err, ?tmp_path, "Failed to write temp config.toml");
                    return false;
                }
                if let Err(err) = fs::rename(&tmp_path, &path) {
                    tracing::warn!(?err, ?path, "Failed to rename config.toml");
                    return false;
                }
                true
            }
            Err(err) => {
                tracing::warn!(?err, "Failed to serialize config.toml");
                false
            }
        }
    }

    pub fn accent_rgb(&self) -> [u8; 3] {
        ACCENT_PRESETS
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&self.accent_theme))
            .map(|(_, rgb)| *rgb)
            .unwrap_or(ACCENT_PRESETS[0].1)
    }

    pub fn monitor_cfg(&self, name: &str) -> Option<&MonitorConfig> {
        self.monitors.iter().find(|m| m.name == name)
    }

    pub fn monitor_cfg_mut(&mut self, name: &str) -> &mut MonitorConfig {
        if let Some(pos) = self.monitors.iter().position(|m| m.name == name) {
            return &mut self.monitors[pos];
        }
        self.monitors.push(MonitorConfig {
            name: name.to_string(),
            scale: 1.0,
            ..MonitorConfig::default()
        });
        self.monitors.last_mut().expect("pushed monitor config")
    }

    pub fn resolve_wallpaper_path(&self) -> Option<PathBuf> {
        if !self.use_custom_wallpaper {
            return None;
        }
        if let Some(ref p) = self.wallpaper_path {
            let expanded = expand_tilde(p);
            if expanded.exists() {
                return Some(expanded);
            }
        }
        let dir = Self::config_dir()?;
        for ext in ["png", "jpg", "jpeg", "webp"] {
            let candidate = dir.join(format!("wallpaper.{ext}"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }

    fn normalize_tile_list(list: &[String], defaults: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for item in list {
            if defaults.contains(&item.as_str()) && !out.iter().any(|x| x == item) {
                out.push(item.clone());
            }
        }
        for def in defaults {
            if !out.iter().any(|x| x == def) {
                out.push((*def).to_string());
            }
        }
        out
    }

    pub fn all_taskbar_tiles_ordered(&self) -> Vec<(String, bool)> {
        Self::normalize_tile_list(&self.taskbar_quick_tiles, DEFAULT_TASKBAR_QUICK_TILES)
            .into_iter()
            .map(|id| {
                let visible = !self.hidden_quick_tiles.iter().any(|h| h == &id);
                (id, visible)
            })
            .collect()
    }

    pub fn effective_taskbar_tiles(&self) -> Vec<String> {
        let mut visible: Vec<String> = self
            .all_taskbar_tiles_ordered()
            .into_iter()
            .filter(|(_, show)| *show)
            .map(|(id, _)| id)
            .collect();
        if visible.is_empty() {
            visible.push("control".to_string());
        }
        visible
    }

    pub fn move_taskbar_tile(&mut self, id: &str, delta: isize) -> bool {
        let mut order =
            Self::normalize_tile_list(&self.taskbar_quick_tiles, DEFAULT_TASKBAR_QUICK_TILES);
        let Some(pos) = order.iter().position(|t| t == id) else {
            return false;
        };
        let target = pos as isize + delta;
        if target < 0 || target >= order.len() as isize {
            return false;
        }
        order.swap(pos, target as usize);
        self.taskbar_quick_tiles = order;
        true
    }

    pub fn toggle_taskbar_tile_visible(&mut self, id: &str) -> bool {
        if let Some(pos) = self.hidden_quick_tiles.iter().position(|h| h == id) {
            self.hidden_quick_tiles.remove(pos);
            true
        } else {
            // Always keep at least one quick tile so the user can reopen customization.
            let visible_count = self
                .all_taskbar_tiles_ordered()
                .iter()
                .filter(|(_, v)| *v)
                .count();
            if visible_count > 1 {
                self.hidden_quick_tiles.push(id.to_string());
                true
            } else {
                false
            }
        }
    }

    pub fn reset_taskbar_tiles(&mut self) {
        self.taskbar_quick_tiles = default_taskbar_quick_tiles();
        self.hidden_quick_tiles.clear();
        self.quick_panel_tiles = default_quick_panel_tiles();
    }

    pub fn effective_quick_panel_tiles(&self) -> Vec<String> {
        Self::normalize_tile_list(&self.quick_panel_tiles, DEFAULT_QUICK_PANEL_TILES)
    }

    pub fn move_quick_panel_tile(&mut self, id: &str, delta: isize) -> bool {
        let mut order =
            Self::normalize_tile_list(&self.quick_panel_tiles, DEFAULT_QUICK_PANEL_TILES);
        let Some(pos) = order.iter().position(|t| t == id) else {
            return false;
        };
        let target = pos as isize + delta;
        if target < 0 || target >= order.len() as isize {
            return false;
        }
        order.swap(pos, target as usize);
        self.quick_panel_tiles = order;
        true
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip_toml() {
        let mut cfg = Config::default();
        cfg.accent_theme = "Violet".to_string();
        cfg.gap_px = 16;
        cfg.panel_position = PanelPosition::Bottom;
        cfg.shelf_style = ShelfStyle::Floating;
        cfg.layout_mode = LayoutMode::Columns;
        cfg.comms_output_device = Some("headset".to_string());

        let serialized = toml::to_string_pretty(&cfg).expect("serialize config");
        let parsed: Config = toml::from_str(&serialized).expect("deserialize config");
        assert_eq!(parsed.accent_theme, "Violet");
        assert_eq!(parsed.gap_px, 16);
        assert_eq!(parsed.panel_position, PanelPosition::Bottom);
        assert_eq!(parsed.shelf_style, ShelfStyle::Floating);
        assert_eq!(parsed.layout_mode, LayoutMode::Columns);
        assert_eq!(parsed.comms_output_device.as_deref(), Some("headset"));
        assert_eq!(parsed.accent_rgb(), [147, 94, 247]);
        assert!(parsed.windows_shortcuts);
        assert!(!parsed.show_system_processes);
        assert_eq!(parsed.vrr_mode, VrrMode::Off);
        assert!(parsed.video_battery_saver);
        assert_eq!(parsed.idle_refresh_hz, 24);
        assert!(!parsed.battery_saver_modeset);
        assert!(parsed.seamless_refresh_overlay);
        assert!(parsed.show_refresh_overlay);
        assert!(parsed.monitors.is_empty());
        assert!(parsed.disabled_audio_sinks.is_empty());
        assert!(parsed.disabled_audio_sources.is_empty());
        assert!(parsed.dim_unfocused_windows);
        assert!(!parsed.focus_follows_mouse);
        assert!(parsed.freeze_hidden_windows);
        assert!(parsed.game_pointer_lock);
        assert!(parsed.lock_fullscreen_taskbar);
        assert!(parsed.auto_clean_ram_cache);
        assert!(parsed.allow_notification_snooze);
        assert!(!parsed.hide_silent_notifications_in_status_bar);
        assert!(parsed.notification_cooldown_enabled);
        assert!(parsed.notification_organizer_enabled);
        assert!(parsed.notification_summaries_enabled);
        assert!(parsed.notification_history_enabled);
        assert_eq!(parsed.idle_dim_secs, 300);
        assert_eq!(parsed.idle_sleep_secs, 900);
        assert_eq!(parsed.idle_sleep_mode, DisplaySleepMode::DpmsSleep);
        assert!(parsed.minimal_view_enabled);
        assert_eq!(parsed.taskbar_quick_tiles.len(), DEFAULT_TASKBAR_QUICK_TILES.len());
    }

    #[test]
    fn quick_tile_customization_reorders_and_toggles_cleanly() {
        let mut cfg = Config::default();
        assert!(cfg.move_taskbar_tile("notif", -1));
        assert_eq!(cfg.effective_taskbar_tiles().last().map(String::as_str), Some("bg_processes"));
        cfg.toggle_taskbar_tile_visible("wifi");
        assert!(!cfg.effective_taskbar_tiles().iter().any(|t| t == "wifi"));
        cfg.reset_taskbar_tiles();
        assert_eq!(cfg.effective_taskbar_tiles(), default_taskbar_quick_tiles());
    }

    #[test]
    fn video_battery_saver_and_vrr_plan_works() {
        let modes = [60, 120, 144];
        let active = compute_effective_refresh_plan(
            0,
            144,
            &modes,
            VrrMode::Auto,
            true,
            true,
            10,
            true,
            false,
            0.2,
            false,
            false,
            30,
            true,
        );
        assert_eq!(active.effective_pace_hz, 144);
        assert!(!active.use_vrr);

        let video_vrr = compute_effective_refresh_plan(
            0,
            144,
            &modes,
            VrrMode::Auto,
            true,
            true,
            10,
            true,
            false,
            2.5,
            false,
            false,
            30,
            true,
        );
        assert_eq!(video_vrr.effective_pace_hz, 30);
        assert_eq!(video_vrr.target_kms_hz, 144);
        assert!(video_vrr.use_vrr);

        // Fixed-refresh display with seamless refresh overlay + video battery saver: paces smoothly at video fps while keeping KMS at 144Hz.
        let video_non_vrr_overlay = compute_effective_refresh_plan(
            0,
            144,
            &modes,
            VrrMode::Off,
            false,
            true,
            24,
            false,
            true,
            8.0,
            false,
            false,
            30,
            true,
        );
        assert_eq!(video_non_vrr_overlay.effective_pace_hz, 30);
        assert_eq!(video_non_vrr_overlay.target_kms_hz, 144);
        assert!(!video_non_vrr_overlay.use_vrr);

        // Active video whose exact fps isn't matched (detected_video_fps=0) still stays full native Hz.
        let unquantized_video = compute_effective_refresh_plan(
            0,
            144,
            &modes,
            VrrMode::Auto,
            true,
            true,
            24,
            false,
            true,
            10.0,
            false,
            false,
            0,
            true,
        );
        assert_eq!(unquantized_video.effective_pace_hz, 144);

        let video_non_vrr_kms = compute_effective_refresh_plan(
            0,
            144,
            &modes,
            VrrMode::Auto,
            false,
            true,
            10,
            true,
            false,
            4.0,
            false,
            false,
            30,
            true,
        );
        assert_eq!(video_non_vrr_kms.effective_pace_hz, 30);
        assert_eq!(video_non_vrr_kms.target_kms_hz, 60);
        assert!(!video_non_vrr_kms.use_vrr);

        // Truly static screen (no input and no visible surface commits) engages idle downclock.
        let idle_vrr = compute_effective_refresh_plan(
            0,
            144,
            &modes,
            VrrMode::Auto,
            true,
            true,
            10,
            true,
            true,
            5.0,
            false,
            false,
            0,
            false,
        );
        assert_eq!(idle_vrr.effective_pace_hz, 10);
        assert_eq!(idle_vrr.target_kms_hz, 144);
        assert!(idle_vrr.use_vrr);

        // Seamless refresh overlay keeps KMS steady at 144Hz even when capping active desktop to 60Hz (0 black screen!).
        let overlay_cap = compute_effective_refresh_plan(
            60,
            144,
            &modes,
            VrrMode::Off,
            false,
            true,
            24,
            true,
            true,
            0.2,
            false,
            false,
            0,
            true,
        );
        assert_eq!(overlay_cap.effective_pace_hz, 60);
        assert_eq!(overlay_cap.target_kms_hz, 144);
    }
}
