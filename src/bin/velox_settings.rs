use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_pointer, delegate_registry, delegate_seat,
    delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            EventLoop, LoopSignal,
            timer::{TimeoutAction, Timer},
        },
        calloop_wayland_source::WaylandSource,
        protocols::xdg::shell::client::xdg_toplevel::ResizeEdge,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{BTN_LEFT, BTN_RIGHT, PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};
use xkbcommon::xkb;

use velox_wm::{
    apps::{
        AppCategory, AppLibraryStore, AppOverrideEntry, CustomAppEntry, CustomCategoryRule,
        DesktopApp, executable_exists, generate_custom_id, parse_exec_args, scan_desktop_apps,
        scan_desktop_apps_including_hidden,
    },
    config::{
        ACCENT_PRESETS, Config, DisplaySleepMode, DisplayTelemetry, LayoutMode, OutputTelemetry,
        PanelPosition, ShelfStyle, VrrMode, WALLPAPER_PRESETS, quick_tile_label,
    },
    font::{UiPainter, fill_circle, fill_rounded_rect, stroke_rounded_rect},
    notifications::{
        clear_persisted_notification_history, load_notification_store, normalize_app_key,
        NotificationHistoryEntry, ORGANIZER_SILENCEABLE_CATEGORIES,
    },
    system_ctrl::{AudioDevice, SysCommand, SystemController, SystemSnapshot},
    updates::{send_external_cmd, take_settings_cmd},
};

const SIDEBAR_W: i32 = 216;
const TITLEBAR_H: i32 = 48;
const WINDOW_RADIUS: i32 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Personalize,
    Display,
    Multitasking,
    Audio,
    Notifications,
    NetworkPower,
    AppsStartup,
    Processes,
    MemoryCache,
}

impl SettingsTab {
    const ALL: [SettingsTab; 9] = [
        SettingsTab::Personalize,
        SettingsTab::Display,
        SettingsTab::Multitasking,
        SettingsTab::Audio,
        SettingsTab::Notifications,
        SettingsTab::NetworkPower,
        SettingsTab::AppsStartup,
        SettingsTab::Processes,
        SettingsTab::MemoryCache,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Personalize => "Personalize",
            Self::Display => "Display & Speed",
            Self::Multitasking => "Multitasking & Keys",
            Self::Audio => "Audio & Roles",
            Self::Notifications => "Notifications",
            Self::NetworkPower => "Network & Power",
            Self::AppsStartup => "Apps & Startup",
            Self::Processes => "Processes",
            Self::MemoryCache => "RAM & Cache",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Personalize => "Theme, wallpaper, shelf style",
            Self::Display => "Refresh Hz & zero-blink sync",
            Self::Multitasking => "Tiling, dimming & shortcuts",
            Self::Audio => "Volume, devices & roles",
            Self::Notifications => "Organizer, AI recaps & apps",
            Self::NetworkPower => "Wi-Fi, Bluetooth & brightness",
            Self::AppsStartup => "Default apps & autostart",
            Self::Processes => "Active background apps",
            Self::MemoryCache => "RAM cleaner & launch cache",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HitRect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl HitRect {
    fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SliderTarget {
    CornerRadius,
    BorderWidth,
    PanelHeight,
    PanelOpacity,
    AnimationSpeed,
    WindowGap,
    MasterRatio,
    MasterOutVolume,
    MasterInVolume,
    SinkVolume { index: u32 },
    SourceVolume { index: u32 },
    Brightness,
    UnfocusedDimOpacity,
    UnfocusedWindowAlpha,
    RamThresholdPct,
    StreamVolume { index: u32 },
    CaptureStreamVolume { index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppTextField {
    LibSearch,
    EditName,
    EditComment,
    EditExec,
    EditKeywords,
    EditCustomCategory,
    EditBadgeHex,
    NewAppName,
    NewAppComment,
    NewAppExec,
    NewAppCustomCategory,
    NewAppKeywords,
    NewRuleCategory,
    NewRuleKeywords,
}

#[derive(Debug, Clone)]
enum HitAction {
    WindowMinimize,
    WindowMaximizeToggle,
    WindowClose,
    SelectTab(SettingsTab),
    ResetDefaults,
    SetAccent(String),
    SetWallpaperPreset(String),
    ToggleCustomWallpaper,
    SetPanelPos(PanelPosition),
    SetShelfStyle(ShelfStyle),
    SelectMonitor(String),
    RequestMonitorRes { name: String, width: i32, height: i32 },
    RequestRefreshHz { name: String, hz: u32 },
    SetMonitorScale { name: String, scale: f32 },
    SetPrimaryMonitor(String),
    RequestVrrMode { name: String, mode: VrrMode },
    ConfirmDisplayModal,
    CancelRevertDisplayModal,
    ToggleConfirmBeforeDisplay,
    ToggleSeamlessRefreshOverlay,
    ToggleShowRefreshOverlay,
    ToggleVideoBatterySaver,
    SetIdleRefreshHz(u32),
    ToggleBatterySaverModeset,
    SetIdleDimSecs(u32),
    SetIdleSleepSecs(u32),
    ToggleIdleSleepMode,
    ToggleIdleInhibitVideo,
    ToggleLockOnSleep,
    LockScreenNow,
    ToggleAnimations,
    SetLayoutMode(LayoutMode),
    ToggleAutoTile,
    ToggleEdgeSnap,
    ToggleFocusFollowsMouse,
    ToggleGamePointerLock,
    ToggleLockFullscreenTaskbar,
    ToggleDimUnfocused,
    ToggleDimExcludeFullscreen,
    ToggleFreezeHidden,
    ToggleWindowsShortcuts,
    ToggleAutoCleanRam,
    ToggleAppPrewarm,
    CleanRamCache { deep_kernel: bool },
    ToggleAutoRouteAudio,
    ToggleOutMute(String),
    ToggleInMute(String),
    SetDefaultOut(String),
    SetDefaultIn(String),
    ToggleAudioDisabled { is_input: bool, name: String, disabled: bool },
    SetCommsRole(String),
    SetMediaRole(String),
    ClearRoles,
    ToggleStreamMute { is_input: bool, index: u32 },
    CycleStreamDevice { is_input: bool, index: u32 },
    ToggleWifi,
    ToggleBluetooth,
    SetPowerProfile(String),
    SetTerminal(String),
    SetBrowser(String),
    SetFiles(String),
    SetLauncher(String),
    ToggleAutoTerminalLogin,
    ToggleAutostart(String),
    ToggleDnd,
    ToggleNotifSnooze,
    ToggleHideSilentStatusBar,
    ToggleNotifCooldown,
    ToggleNotifOrganizer,
    ToggleOrganizerSilenceCat(String),
    ToggleNotifSummaries,
    ToggleNotifHistory,
    ClearNotifHistory,
    TogglePriorityConversation(String),
    ToggleAppNotifDisabled(String),
    ToggleAppNotifSilent(String),
    FocusTextField(AppTextField),
    AppLibFilterCategory(AppCategory),
    AppLibSearchAllCategories,
    ClearAppLibSearch,
    SelectAppForEdit(String),
    EditSetCategory(AppCategory),
    EditSetBadgeColor([u8; 3]),
    EditToggleTerminal,
    EditToggleHidden,
    EditCycleGameOverride,
    SaveAppEdit,
    ResetAppOverride,
    DeleteCustomApp(String),
    ToggleNewAppForm,
    NewAppSetCategory(AppCategory),
    NewAppToggleTerminal,
    CreateCustomApp,
    ToggleNewRuleForm,
    NewRuleSetBuiltinCat(Option<AppCategory>),
    CreateCategoryRule,
    DeleteCategoryRule(usize),
    RescanAppLibraryNow,
    ToggleCheckGithubUpdates,
    CheckUpdatesNow,
    SeamlessReloadWm,
    ToggleShowSystemProcs,
    KillProcess { pids: Vec<u32>, force: bool },
    RefreshSnapshot,
    MoveTaskbarTile { tile: String, delta: isize },
    ToggleTaskbarTile(String),
    ResetTaskbarTiles,
    MoveQuickPanelTile { tile: String, delta: isize },
    ToggleMinimalViewConfig,
    OpenAppCtxMenu { app_id: String, x: i32, y: i32 },
    CloseAppCtxMenu,
    AppCtxSelectForEdit(String),
    AppCtxLaunch(String),
    AppCtxToggleTerminal(String),
    AppCtxCycleGame(String),
    AppCtxToggleHidden(String),
    SliderTrack { target: SliderTarget, rect: HitRect },
}

#[derive(Debug, Clone)]
struct HitRegion {
    rect: HitRect,
    action: HitAction,
}

fn slider_ratio(px: i32, rect: HitRect) -> f32 {
    if rect.w <= 1 {
        return 0.0;
    }
    ((px - rect.x) as f32 / rect.w as f32).clamp(0.0, 1.0)
}

fn rgba_to_argb8888(rgba: &[u8], dst: &mut [u8]) {
    for (src_px, dst_px) in rgba.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        dst_px[0] = src_px[2];
        dst_px[1] = src_px[1];
        dst_px[2] = src_px[0];
        dst_px[3] = src_px[3];
    }
}

fn format_mode_mhz(mhz: i32) -> String {
    if mhz <= 0 {
        return "Auto".to_string();
    }
    let whole = mhz / 1000;
    let frac = (mhz % 1000) / 10;
    if frac == 0 {
        format!("{whole} Hz")
    } else {
        format!("{whole}.{frac:02} Hz")
    }
}

#[derive(Debug, Clone)]
struct DisplayConfirmModal {
    monitor_name: String,
    pending_width: i32,
    pending_height: i32,
    prev_width: i32,
    prev_height: i32,
    pending_hz: u32,
    prev_hz: u32,
    pending_vrr: VrrMode,
    prev_vrr: VrrMode,
}

struct SettingsApp {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    loop_signal: LoopSignal,

    pool: SlotPool,
    window: Window,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    seat: Option<wl_seat::WlSeat>,
    xkb_context: xkb::Context,
    xkb_state: Option<xkb::State>,

    width: u32,
    height: u32,
    scale: i32,
    configured: bool,
    maximized: bool,
    needs_redraw: bool,

    pointer_x: i32,
    pointer_y: i32,
    hovered_idx: Option<usize>,
    active_drag: Option<(SliderTarget, HitRect)>,
    scroll_y: i32,
    max_scroll_y: i32,

    active_tab: SettingsTab,
    config: Config,
    desktop_apps: Vec<DesktopApp>,
    app_store: AppLibraryStore,
    active_text_field: Option<AppTextField>,
    app_filter_query: String,
    app_filter_category: AppCategory,
    selected_app_id: Option<String>,
    edit_name: String,
    edit_comment: String,
    edit_exec: String,
    edit_keywords: String,
    edit_custom_cat: String,
    edit_badge_hex: String,
    edit_badge_color: [u8; 3],
    edit_category: AppCategory,
    edit_terminal: bool,
    edit_hidden: bool,
    edit_game_override: Option<bool>,
    show_new_app_form: bool,
    new_app_name: String,
    new_app_comment: String,
    new_app_exec: String,
    new_app_custom_cat: String,
    new_app_keywords: String,
    new_app_category: AppCategory,
    new_app_terminal: bool,
    show_new_rule_form: bool,
    new_rule_category: String,
    new_rule_keywords: String,
    new_rule_builtin: Option<AppCategory>,
    notif_history: Vec<NotificationHistoryEntry>,
    notif_seen_apps: Vec<String>,
    display_telemetry: DisplayTelemetry,
    selected_monitor: Option<String>,
    display_modal: Option<DisplayConfirmModal>,
    sys_ctrl: SystemController,
    snapshot: SystemSnapshot,
    painter: UiPainter,
    rgba_buf: Vec<u8>,
    hit_regions: Vec<HitRegion>,
    app_row_rects: Vec<(HitRect, String)>,
    app_ctx_menu: Option<(String, i32, i32)>,
    last_saved_at: Option<Instant>,
    status_note: String,
    dirty_config: bool,
}

impl SettingsApp {
    fn select_app_for_edit(&mut self, id: &str) {
        if let Some(app) = self.desktop_apps.iter().find(|a| a.id == id).cloned() {
            self.selected_app_id = Some(app.id.clone());
            self.edit_name = app.name;
            self.edit_comment = app.comment;
            self.edit_exec = app.exec_args.join(" ");
            self.edit_keywords = app.keywords.join(", ");
            self.edit_custom_cat = app.custom_category.unwrap_or_default();
            self.edit_badge_color = app.badge_color;
            self.edit_badge_hex = format!(
                "#{:02X}{:02X}{:02X}",
                app.badge_color[0], app.badge_color[1], app.badge_color[2]
            );
            self.edit_category = app.category;
            self.edit_terminal = app.terminal;
            self.edit_hidden = app.hidden;
            self.edit_game_override = app.is_game_override;
        }
    }

    fn reload_app_library(&mut self, note: &str) {
        self.desktop_apps = scan_desktop_apps_including_hidden();
        if let Some(ref sel) = self.selected_app_id.clone() {
            self.select_app_for_edit(sel);
        } else if let Some(first) = self.desktop_apps.first() {
            let id = first.id.clone();
            self.select_app_for_edit(&id);
        }
        self.status_note = note.to_string();
        self.needs_redraw = true;
    }

    fn text_field_mut(&mut self, field: AppTextField) -> &mut String {
        match field {
            AppTextField::LibSearch => &mut self.app_filter_query,
            AppTextField::EditName => &mut self.edit_name,
            AppTextField::EditComment => &mut self.edit_comment,
            AppTextField::EditExec => &mut self.edit_exec,
            AppTextField::EditKeywords => &mut self.edit_keywords,
            AppTextField::EditCustomCategory => &mut self.edit_custom_cat,
            AppTextField::EditBadgeHex => &mut self.edit_badge_hex,
            AppTextField::NewAppName => &mut self.new_app_name,
            AppTextField::NewAppComment => &mut self.new_app_comment,
            AppTextField::NewAppExec => &mut self.new_app_exec,
            AppTextField::NewAppCustomCategory => &mut self.new_app_custom_cat,
            AppTextField::NewAppKeywords => &mut self.new_app_keywords,
            AppTextField::NewRuleCategory => &mut self.new_rule_category,
            AppTextField::NewRuleKeywords => &mut self.new_rule_keywords,
        }
    }

    fn draw_text_input(
        &mut self,
        x: i32,
        y: i32,
        w_box: i32,
        label: &str,
        value: &str,
        placeholder: &str,
        field: AppTextField,
    ) {
        let w = self.width as i32;
        let h = self.height as i32;
        let accent = self.config.accent_rgb();
        let focused = self.active_text_field == Some(field);
        self.painter.draw_text(
            &mut self.rgba_buf,
            w,
            h,
            x,
            y,
            w_box,
            label,
            11.0,
            [168, 186, 218, 240],
        );
        let box_y = y + 17;
        let rect = HitRect::new(x, box_y, w_box, 30);
        fill_rounded_rect(&mut self.rgba_buf, w, h, rect.x, rect.y, rect.w, rect.h, 7, [13, 18, 32, 244]);
        stroke_rounded_rect(
            &mut self.rgba_buf,
            w,
            h,
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            7,
            1,
            if focused {
                [accent[0], accent[1], accent[2], 235]
            } else {
                [56, 72, 108, 175]
            },
        );
        let display = if value.is_empty() && !focused {
            placeholder.to_string()
        } else if focused {
            format!("{value}_")
        } else {
            value.to_string()
        };
        let col = if value.is_empty() && !focused {
            [120, 134, 166, 220]
        } else {
            [240, 246, 255, 255]
        };
        self.painter.draw_text(
            &mut self.rgba_buf,
            w,
            h,
            rect.x + 10,
            rect.y + 8,
            rect.w - 20,
            &display,
            12.0,
            col,
        );
        self.hit_regions.push(HitRegion {
            rect,
            action: HitAction::FocusTextField(field),
        });
    }

    fn save_config(&mut self, note: &str) {
        if self.config.save() {
            self.last_saved_at = Some(Instant::now());
            self.status_note = format!("Saved • {note} (live applied)");
        } else {
            self.status_note = "Failed to save ~/.config/velox-wm/config.toml".to_string();
        }
        self.dirty_config = false;
        self.needs_redraw = true;
    }

    fn selected_output(&self) -> Option<OutputTelemetry> {
        if let Some(ref sel) = self.selected_monitor {
            if let Some(found) = self.display_telemetry.outputs.iter().find(|o| o.name == *sel) {
                return Some(found.clone());
            }
        }
        self.display_telemetry
            .outputs
            .iter()
            .find(|o| o.primary)
            .or_else(|| self.display_telemetry.outputs.first())
            .cloned()
    }

    fn is_primary_or_only_monitor(&self, name: &str) -> bool {
        self.display_telemetry.outputs.len() <= 1
            || self
                .display_telemetry
                .outputs
                .iter()
                .any(|o| o.name == name && o.primary)
    }

    fn effective_monitor_settings(&self, out: &OutputTelemetry) -> (i32, i32, u32, f32, VrrMode) {
        let mon_cfg = self.config.monitor_cfg(&out.name);
        let (w, h) = mon_cfg
            .filter(|m| m.width > 0 && m.height > 0)
            .map(|m| (m.width, m.height))
            .unwrap_or((out.width, out.height));
        let hz = mon_cfg
            .filter(|m| m.refresh_hz > 0)
            .map(|m| m.refresh_hz)
            .unwrap_or_else(|| {
                if self.is_primary_or_only_monitor(&out.name) {
                    self.config.output_refresh_hz
                } else {
                    0
                }
            });
        let scale = mon_cfg
            .filter(|m| (0.75..=3.0).contains(&m.scale))
            .map(|m| m.scale)
            .unwrap_or_else(|| {
                if out.scale_x100 >= 75 {
                    out.scale_x100 as f32 / 100.0
                } else {
                    1.0
                }
            });
        let vrr = mon_cfg
            .and_then(|m| m.vrr_mode)
            .unwrap_or(self.config.vrr_mode);
        (w, h, hz, scale, vrr)
    }

    fn apply_display_change(&mut self, change: DisplayConfirmModal) {
        let sync_global = change.monitor_name.is_empty()
            || self.is_primary_or_only_monitor(&change.monitor_name);
        let compat_hz = if !change.monitor_name.is_empty()
            && change.pending_width > 0
            && change.pending_height > 0
            && change.pending_hz > 0
        {
            self.display_telemetry
                .outputs
                .iter()
                .find(|o| o.name == change.monitor_name)
                .and_then(|o| {
                    o.resolution_modes
                        .iter()
                        .find(|r| r.width == change.pending_width && r.height == change.pending_height)
                })
                .map(|r| {
                    if r.hz.contains(&change.pending_hz) {
                        change.pending_hz
                    } else {
                        0
                    }
                })
                .unwrap_or(change.pending_hz)
        } else {
            change.pending_hz
        };
        if !change.monitor_name.is_empty() {
            let mon = self.config.monitor_cfg_mut(&change.monitor_name);
            if change.pending_width > 0 && change.pending_height > 0 {
                mon.width = change.pending_width;
                mon.height = change.pending_height;
            }
            mon.refresh_hz = compat_hz;
            mon.vrr_mode = Some(change.pending_vrr);
        }
        if sync_global {
            self.config.output_refresh_hz = compat_hz;
            self.config.vrr_mode = change.pending_vrr;
        }
        let hz_txt = if compat_hz == 0 {
            "Auto Max".to_string()
        } else {
            format!("{compat_hz} Hz")
        };
        let label = if change.monitor_name.is_empty() {
            "Display".to_string()
        } else {
            change.monitor_name
        };
        let res_txt = if change.pending_width > 0 && change.pending_height > 0 {
            format!(" {}x{}", change.pending_width, change.pending_height)
        } else {
            String::new()
        };
        self.save_config(&format!(
            "{label}{res_txt} @ {hz_txt} ({})",
            change.pending_vrr.short_label()
        ));
    }

    fn request_or_apply_display(&mut self, change: DisplayConfirmModal) {
        if !self.config.confirm_before_display_apply {
            self.apply_display_change(change);
            return;
        }
        self.display_modal = Some(change);
        self.needs_redraw = true;
    }

    fn apply_slider_at(&mut self, target: SliderTarget, rect: HitRect, px: i32, persist: bool) {
        let t = slider_ratio(px, rect);
        match target {
            SliderTarget::CornerRadius => {
                self.config.corner_radius = (t * 24.0).round() as i32;
                self.dirty_config = true;
            }
            SliderTarget::BorderWidth => {
                self.config.border_width = (t * 6.0).round() as i32;
                self.dirty_config = true;
            }
            SliderTarget::PanelHeight => {
                self.config.panel_height = 34 + (t * 30.0).round() as i32;
                self.dirty_config = true;
            }
            SliderTarget::PanelOpacity => {
                self.config.panel_opacity = (140.0 + t * 115.0).round().clamp(140.0, 255.0) as u8;
                self.dirty_config = true;
            }
            SliderTarget::AnimationSpeed => {
                let speed = (0.10 + t * 0.55).clamp(0.10, 0.65);
                self.config.animation_speed = (speed * 100.0).round() / 100.0;
                self.dirty_config = true;
            }
            SliderTarget::WindowGap => {
                self.config.gap_px = (t * 28.0).round() as i32;
                self.dirty_config = true;
            }
            SliderTarget::MasterRatio => {
                let ratio = (0.35 + t * 0.40).clamp(0.35, 0.75);
                self.config.master_ratio = (ratio * 100.0).round() / 100.0;
                self.dirty_config = true;
            }
            SliderTarget::MasterOutVolume => {
                let pct = (t * 100.0).round().clamp(0.0, 100.0) as u8;
                let sink = if self.snapshot.default_sink.is_empty() {
                    "@DEFAULT_SINK@".to_string()
                } else {
                    self.snapshot.default_sink.clone()
                };
                self.sys_ctrl.send(SysCommand::SetSinkVolume { name: sink, pct });
                self.config.saved_volume_pct = Some(pct);
                self.dirty_config = true;
                self.snapshot = self.sys_ctrl.snapshot();
                self.status_note = format!("Output volume {pct}% (will save)");
            }
            SliderTarget::MasterInVolume => {
                let pct = (t * 100.0).round().clamp(0.0, 100.0) as u8;
                let src = if self.snapshot.default_source.is_empty() {
                    "@DEFAULT_SOURCE@".to_string()
                } else {
                    self.snapshot.default_source.clone()
                };
                self.sys_ctrl.send(SysCommand::SetSourceVolume { name: src, pct });
                self.config.saved_mic_volume_pct = Some(pct);
                self.dirty_config = true;
                self.snapshot = self.sys_ctrl.snapshot();
                self.status_note = format!("Microphone gain {pct}% (will save)");
            }
            SliderTarget::SinkVolume { index } => {
                let pct = (t * 100.0).round().clamp(0.0, 100.0) as u8;
                if let Some((name, is_default, desc)) = self
                    .snapshot
                    .sinks
                    .iter()
                    .find(|d| d.index == index)
                    .map(|d| (d.name.clone(), d.is_default, d.description.clone()))
                {
                    self.sys_ctrl.send(SysCommand::SetSinkVolume { name, pct });
                    if is_default {
                        self.config.saved_volume_pct = Some(pct);
                        self.dirty_config = true;
                    }
                    self.snapshot = self.sys_ctrl.snapshot();
                    self.status_note = format!("{desc} volume {pct}%");
                }
            }
            SliderTarget::SourceVolume { index } => {
                let pct = (t * 100.0).round().clamp(0.0, 100.0) as u8;
                if let Some((name, is_default, desc)) = self
                    .snapshot
                    .sources
                    .iter()
                    .find(|d| d.index == index)
                    .map(|d| (d.name.clone(), d.is_default, d.description.clone()))
                {
                    self.sys_ctrl.send(SysCommand::SetSourceVolume { name, pct });
                    if is_default {
                        self.config.saved_mic_volume_pct = Some(pct);
                        self.dirty_config = true;
                    }
                    self.snapshot = self.sys_ctrl.snapshot();
                    self.status_note = format!("{desc} gain {pct}%");
                }
            }
            SliderTarget::Brightness => {
                let pct = (5.0 + t * 95.0).round().clamp(5.0, 100.0) as u8;
                self.sys_ctrl.send(SysCommand::SetBrightness { pct });
                self.config.saved_brightness_pct = Some(pct);
                self.dirty_config = true;
                self.snapshot = self.sys_ctrl.snapshot();
                self.status_note = format!("Brightness {pct}% (will save)");
            }
            SliderTarget::UnfocusedDimOpacity => {
                let dim = (t * 0.60).clamp(0.0, 0.60);
                self.config.unfocused_dim_opacity = (dim * 100.0).round() / 100.0;
                self.dirty_config = true;
                self.status_note = format!(
                    "Unfocused dim {}%",
                    (self.config.unfocused_dim_opacity * 100.0).round() as i32
                );
            }
            SliderTarget::UnfocusedWindowAlpha => {
                let alpha = (0.65 + t * 0.35).clamp(0.65, 1.0);
                self.config.unfocused_window_opacity = (alpha * 100.0).round() / 100.0;
                self.dirty_config = true;
                self.status_note = format!(
                    "Inactive opacity {}%",
                    (self.config.unfocused_window_opacity * 100.0).round() as i32
                );
            }
            SliderTarget::RamThresholdPct => {
                let pct = (55.0 + t * 37.0).round().clamp(55.0, 92.0) as u8;
                self.config.ram_cache_threshold_pct = pct;
                self.dirty_config = true;
                self.status_note = format!("Auto-clean threshold {pct}%");
            }
            SliderTarget::StreamVolume { index } => {
                let pct = (t * 100.0).round().clamp(0.0, 100.0) as u8;
                self.sys_ctrl.send(SysCommand::SetStreamVolume {
                    is_input: false,
                    index,
                    pct,
                });
                self.snapshot = self.sys_ctrl.snapshot();
            }
            SliderTarget::CaptureStreamVolume { index } => {
                let pct = (t * 100.0).round().clamp(0.0, 100.0) as u8;
                self.sys_ctrl.send(SysCommand::SetStreamVolume {
                    is_input: true,
                    index,
                    pct,
                });
                self.snapshot = self.sys_ctrl.snapshot();
            }
        }
        if persist && self.dirty_config {
            self.save_config("Preference updated");
        } else {
            self.needs_redraw = true;
        }
    }

    fn execute_action(&mut self, action: HitAction) {
        match action {
            HitAction::WindowMinimize => {
                self.window.set_minimized();
            }
            HitAction::WindowMaximizeToggle => {
                if self.maximized {
                    self.window.unset_maximized();
                } else {
                    self.window.set_maximized();
                }
            }
            HitAction::WindowClose => {
                self.loop_signal.stop();
            }
            HitAction::SelectTab(tab) => {
                self.active_tab = tab;
                self.scroll_y = 0;
                self.needs_redraw = true;
            }
            HitAction::ResetDefaults => {
                self.config = Config::default();
                self.save_config("Reset to defaults");
            }
            HitAction::SetAccent(name) => {
                self.config.accent_theme = name.clone();
                self.save_config(&format!("Accent {name}"));
            }
            HitAction::SetWallpaperPreset(preset) => {
                self.config.wallpaper_preset = preset.clone();
                self.config.use_custom_wallpaper = false;
                self.save_config(&format!("Wallpaper {preset}"));
            }
            HitAction::ToggleCustomWallpaper => {
                self.config.use_custom_wallpaper = !self.config.use_custom_wallpaper;
                self.save_config("Wallpaper source");
            }
            HitAction::SetPanelPos(pos) => {
                self.config.panel_position = pos;
                self.save_config(&format!("Taskbar {}", pos.label()));
            }
            HitAction::SetShelfStyle(style) => {
                self.config.shelf_style = style;
                self.save_config(&format!("Shelf {}", style.label()));
            }
            HitAction::SelectMonitor(name) => {
                self.selected_monitor = Some(name.clone());
                self.status_note = format!("Selected monitor {name}");
                self.needs_redraw = true;
            }
            HitAction::RequestMonitorRes { name, width, height } => {
                let (prev_w, prev_h, cur_hz, _, cur_vrr) = self
                    .display_telemetry
                    .outputs
                    .iter()
                    .find(|o| o.name == name)
                    .map(|o| self.effective_monitor_settings(o))
                    .unwrap_or((width, height, self.config.output_refresh_hz, 1.0, self.config.vrr_mode));
                if prev_w == width && prev_h == height {
                    return;
                }
                self.request_or_apply_display(DisplayConfirmModal {
                    monitor_name: name,
                    pending_width: width,
                    pending_height: height,
                    prev_width: prev_w,
                    prev_height: prev_h,
                    pending_hz: cur_hz,
                    prev_hz: cur_hz,
                    pending_vrr: cur_vrr,
                    prev_vrr: cur_vrr,
                });
            }
            HitAction::RequestRefreshHz { name, hz } => {
                let (cur_w, cur_h, prev_hz, _, cur_vrr) = self
                    .display_telemetry
                    .outputs
                    .iter()
                    .find(|o| o.name == name)
                    .map(|o| self.effective_monitor_settings(o))
                    .unwrap_or((0, 0, self.config.output_refresh_hz, 1.0, self.config.vrr_mode));
                if hz == prev_hz {
                    return;
                }
                self.request_or_apply_display(DisplayConfirmModal {
                    monitor_name: name,
                    pending_width: cur_w,
                    pending_height: cur_h,
                    prev_width: cur_w,
                    prev_height: cur_h,
                    pending_hz: hz,
                    prev_hz,
                    pending_vrr: cur_vrr,
                    prev_vrr: cur_vrr,
                });
            }
            HitAction::SetMonitorScale { name, scale } => {
                self.config.monitor_cfg_mut(&name).scale = scale;
                self.save_config(&format!("{name} scale {:.0}%", scale * 100.0));
            }
            HitAction::SetPrimaryMonitor(name) => {
                for m in &mut self.config.monitors {
                    m.primary = m.name == name;
                }
                self.config.monitor_cfg_mut(&name).primary = true;
                self.selected_monitor = Some(name.clone());
                self.save_config(&format!("{name} set as primary display"));
            }
            HitAction::RequestVrrMode { name, mode } => {
                let (cur_w, cur_h, cur_hz, _, prev_vrr) = self
                    .display_telemetry
                    .outputs
                    .iter()
                    .find(|o| o.name == name)
                    .map(|o| self.effective_monitor_settings(o))
                    .unwrap_or((0, 0, self.config.output_refresh_hz, 1.0, self.config.vrr_mode));
                if mode == prev_vrr {
                    return;
                }
                self.request_or_apply_display(DisplayConfirmModal {
                    monitor_name: name,
                    pending_width: cur_w,
                    pending_height: cur_h,
                    prev_width: cur_w,
                    prev_height: cur_h,
                    pending_hz: cur_hz,
                    prev_hz: cur_hz,
                    pending_vrr: mode,
                    prev_vrr,
                });
            }
            HitAction::ConfirmDisplayModal => {
                if let Some(modal) = self.display_modal.take() {
                    self.apply_display_change(modal);
                }
            }
            HitAction::CancelRevertDisplayModal => {
                self.display_modal = None;
                self.status_note = "Cancelled display change".to_string();
                self.needs_redraw = true;
            }
            HitAction::ToggleConfirmBeforeDisplay => {
                self.config.confirm_before_display_apply =
                    !self.config.confirm_before_display_apply;
                self.save_config(if self.config.confirm_before_display_apply {
                    "Display confirmation popup ON"
                } else {
                    "Direct instant display changes ON"
                });
            }
            HitAction::ToggleSeamlessRefreshOverlay => {
                self.config.seamless_refresh_overlay = !self.config.seamless_refresh_overlay;
                self.save_config(if self.config.seamless_refresh_overlay {
                    "Zero-black-screen seamless refresh overlay ON"
                } else {
                    "Direct KMS modeset allowed on refresh changes"
                });
            }
            HitAction::ToggleShowRefreshOverlay => {
                self.config.show_refresh_overlay = !self.config.show_refresh_overlay;
                self.save_config(if self.config.show_refresh_overlay {
                    "Refresh & battery-saver status overlay HUD ON"
                } else {
                    "Refresh & battery-saver status overlay HUD hidden"
                });
            }
            HitAction::ToggleVideoBatterySaver => {
                self.config.video_battery_saver = !self.config.video_battery_saver;
                self.save_config(if self.config.video_battery_saver {
                    "Video & Idle Battery Saver ON"
                } else {
                    "Video & Idle Battery Saver OFF"
                });
            }
            HitAction::SetIdleRefreshHz(hz) => {
                self.config.idle_refresh_hz = hz;
                let note = if hz == 0 {
                    "Idle downclock disabled".to_string()
                } else {
                    format!("Idle refresh target {hz} Hz")
                };
                self.save_config(&note);
            }
            HitAction::ToggleBatterySaverModeset => {
                self.config.battery_saver_modeset = !self.config.battery_saver_modeset;
                self.save_config(if self.config.battery_saver_modeset {
                    "Non-VRR hardware modeset saver ON"
                } else {
                    "Software-only pacing on non-VRR outputs"
                });
            }
            HitAction::SetIdleDimSecs(secs) => {
                self.config.idle_dim_secs = secs;
                self.save_config(&if secs == 0 {
                    "Idle screen dim disabled".to_string()
                } else {
                    format!("Idle screen dim after {}m", secs / 60)
                });
            }
            HitAction::SetIdleSleepSecs(secs) => {
                self.config.idle_sleep_secs = secs;
                self.save_config(&if secs == 0 {
                    "Display sleep / blackout disabled".to_string()
                } else {
                    format!("Display sleep after {}m", secs / 60)
                });
            }
            HitAction::ToggleIdleSleepMode => {
                self.config.idle_sleep_mode = self.config.idle_sleep_mode.toggle();
                self.save_config(self.config.idle_sleep_mode.label());
            }
            HitAction::ToggleIdleInhibitVideo => {
                self.config.idle_inhibit_on_video = !self.config.idle_inhibit_on_video;
                self.save_config("Video idle inhibit updated");
            }
            HitAction::ToggleLockOnSleep => {
                self.config.lock_on_sleep = !self.config.lock_on_sleep;
                self.save_config("Lock screen on sleep updated");
            }
            HitAction::LockScreenNow => {
                send_external_cmd("lock-screen");
                self.status_note = "Locking screen via Velox WM".to_string();
                self.needs_redraw = true;
            }
            HitAction::ToggleAnimations => {
                self.config.animations_enabled = !self.config.animations_enabled;
                self.save_config(if self.config.animations_enabled {
                    "Animations ON"
                } else {
                    "Animations OFF"
                });
            }
            HitAction::SetLayoutMode(mode) => {
                self.config.layout_mode = mode;
                self.config.auto_tile = mode != LayoutMode::Floating;
                self.save_config(&format!("Layout {}", mode.label()));
            }
            HitAction::ToggleAutoTile => {
                self.config.auto_tile = !self.config.auto_tile;
                self.config.layout_mode = if self.config.auto_tile {
                    LayoutMode::MasterStack
                } else {
                    LayoutMode::Floating
                };
                self.save_config("Auto-tile toggled");
            }
            HitAction::ToggleEdgeSnap => {
                self.config.edge_snap = !self.config.edge_snap;
                self.save_config("Edge snap toggled");
            }
            HitAction::ToggleFocusFollowsMouse => {
                self.config.focus_follows_mouse = !self.config.focus_follows_mouse;
                self.save_config("Focus follows mouse");
            }
            HitAction::ToggleGamePointerLock => {
                self.config.game_pointer_lock = !self.config.game_pointer_lock;
                self.save_config(if self.config.game_pointer_lock {
                    "Game cursor lock ON (use Alt+Tab to escape)"
                } else {
                    "Game cursor lock OFF"
                });
            }
            HitAction::ToggleLockFullscreenTaskbar => {
                self.config.lock_fullscreen_taskbar = !self.config.lock_fullscreen_taskbar;
                self.save_config(if self.config.lock_fullscreen_taskbar {
                    "Taskbar locked during fullscreen games ON"
                } else {
                    "Taskbar locked during fullscreen games OFF"
                });
            }
            HitAction::ToggleDimUnfocused => {
                self.config.dim_unfocused_windows = !self.config.dim_unfocused_windows;
                self.save_config(if self.config.dim_unfocused_windows {
                    "Unfocused window dim ON"
                } else {
                    "Unfocused window dim OFF"
                });
            }
            HitAction::ToggleDimExcludeFullscreen => {
                self.config.dim_exclude_fullscreen = !self.config.dim_exclude_fullscreen;
                self.save_config("Fullscreen dim exclusion toggled");
            }
            HitAction::ToggleFreezeHidden => {
                self.config.freeze_hidden_windows = !self.config.freeze_hidden_windows;
                self.save_config(if self.config.freeze_hidden_windows {
                    "Off-screen & hidden window views frozen (~1Hz app keepalive)"
                } else {
                    "Off-screen window view freeze OFF"
                });
            }
            HitAction::ToggleWindowsShortcuts => {
                self.config.windows_shortcuts = !self.config.windows_shortcuts;
                self.save_config("Windows shortcuts");
            }
            HitAction::ToggleAutoCleanRam => {
                self.config.auto_clean_ram_cache = !self.config.auto_clean_ram_cache;
                self.save_config(if self.config.auto_clean_ram_cache {
                    "Auto RAM cache cleaner ON"
                } else {
                    "Auto RAM cache cleaner OFF"
                });
            }
            HitAction::ToggleAppPrewarm => {
                self.config.app_prewarm_cache = !self.config.app_prewarm_cache;
                self.save_config(if self.config.app_prewarm_cache {
                    "App launch cache pre-warming ON"
                } else {
                    "App launch cache pre-warming OFF"
                });
            }
            HitAction::CleanRamCache { deep_kernel } => {
                self.sys_ctrl.send(SysCommand::CleanRamCache { deep_kernel });
                self.snapshot = self.sys_ctrl.snapshot();
                self.status_note = if deep_kernel {
                    "Trimming heaps & kernel page cache...".to_string()
                } else {
                    "Trimming heap & app caches...".to_string()
                };
                self.needs_redraw = true;
            }
            HitAction::ToggleAutoRouteAudio => {
                let next = !self.config.auto_route_audio_roles;
                self.config.auto_route_audio_roles = next;
                self.sys_ctrl.send(SysCommand::SetAutoRouteRoles { enabled: next });
                self.snapshot = self.sys_ctrl.snapshot();
                self.save_config("Auto role routing");
            }
            HitAction::ToggleOutMute(name) => {
                let next_muted = self
                    .snapshot
                    .sinks
                    .iter()
                    .find(|s| s.name == name || (name == "@DEFAULT_SINK@" && s.is_default))
                    .map(|s| !s.muted)
                    .unwrap_or(!self.snapshot.default_output_vol().1);
                self.sys_ctrl.send(SysCommand::ToggleSinkMute { name });
                self.config.saved_muted = Some(next_muted);
                self.snapshot = self.sys_ctrl.snapshot();
                self.save_config(if next_muted { "Output muted" } else { "Output unmuted" });
            }
            HitAction::ToggleInMute(name) => {
                let next_muted = self
                    .snapshot
                    .sources
                    .iter()
                    .find(|s| s.name == name || (name == "@DEFAULT_SOURCE@" && s.is_default))
                    .map(|s| !s.muted)
                    .unwrap_or(!self.snapshot.default_input_vol().1);
                self.sys_ctrl.send(SysCommand::ToggleSourceMute { name });
                self.config.saved_mic_muted = Some(next_muted);
                self.snapshot = self.sys_ctrl.snapshot();
                self.save_config(if next_muted { "Microphone muted" } else { "Microphone unmuted" });
            }
            HitAction::SetDefaultOut(name) => {
                self.sys_ctrl.send(SysCommand::SetDefaultSink { name });
                self.snapshot = self.sys_ctrl.snapshot();
                self.status_note = "Default output switched".to_string();
                self.needs_redraw = true;
            }
            HitAction::SetDefaultIn(name) => {
                self.sys_ctrl.send(SysCommand::SetDefaultSource { name });
                self.snapshot = self.sys_ctrl.snapshot();
                self.status_note = "Default input microphone switched".to_string();
                self.needs_redraw = true;
            }
            HitAction::ToggleAudioDisabled {
                is_input,
                name,
                disabled,
            } => {
                let list = if is_input {
                    &mut self.config.disabled_audio_sources
                } else {
                    &mut self.config.disabled_audio_sinks
                };
                if disabled {
                    if !list.contains(&name) {
                        list.push(name.clone());
                    }
                    if !is_input {
                        if self.config.comms_output_device.as_deref() == Some(name.as_str()) {
                            self.config.comms_output_device = None;
                        }
                        if self.config.media_output_device.as_deref() == Some(name.as_str()) {
                            self.config.media_output_device = None;
                        }
                    }
                } else {
                    list.retain(|n| n != &name);
                }
                self.sys_ctrl.send(SysCommand::SetAudioDeviceDisabled {
                    is_input,
                    name: name.clone(),
                    disabled,
                });
                self.snapshot = self.sys_ctrl.snapshot();
                let kind = if is_input { "Input device" } else { "Output device" };
                let note = if disabled {
                    format!("{kind} disabled")
                } else {
                    format!("{kind} enabled")
                };
                self.save_config(&note);
            }
            HitAction::SetCommsRole(name) => {
                self.config.comms_output_device = Some(name.clone());
                self.sys_ctrl.send(SysCommand::RouteRole {
                    is_comms: true,
                    target_sink: Some(name),
                });
                self.snapshot = self.sys_ctrl.snapshot();
                self.save_config("Comms audio device updated");
            }
            HitAction::SetMediaRole(name) => {
                self.config.media_output_device = Some(name.clone());
                self.sys_ctrl.send(SysCommand::RouteRole {
                    is_comms: false,
                    target_sink: Some(name),
                });
                self.snapshot = self.sys_ctrl.snapshot();
                self.save_config("Playback audio device updated");
            }
            HitAction::ClearRoles => {
                self.config.comms_output_device = None;
                self.config.media_output_device = None;
                self.sys_ctrl.send(SysCommand::RouteRole {
                    is_comms: true,
                    target_sink: None,
                });
                self.sys_ctrl.send(SysCommand::RouteRole {
                    is_comms: false,
                    target_sink: None,
                });
                self.snapshot = self.sys_ctrl.snapshot();
                self.save_config("Cleared role device overrides");
            }
            HitAction::ToggleStreamMute { is_input, index } => {
                self.sys_ctrl
                    .send(SysCommand::ToggleStreamMute { is_input, index });
                self.snapshot = self.sys_ctrl.snapshot();
                self.needs_redraw = true;
            }
            HitAction::CycleStreamDevice { is_input, index } => {
                let devices: Vec<&AudioDevice> = if is_input {
                    self.snapshot.sources.iter().filter(|d| !d.disabled).collect()
                } else {
                    self.snapshot.sinks.iter().filter(|d| !d.disabled).collect()
                };
                if !devices.is_empty() {
                    let stream_opt = if is_input {
                        self.snapshot.source_outputs.iter().find(|s| s.index == index)
                    } else {
                        self.snapshot.sink_inputs.iter().find(|s| s.index == index)
                    };
                    if let Some(stream) = stream_opt {
                        let cur_pos = devices
                            .iter()
                            .position(|d| d.index == stream.device_index)
                            .unwrap_or(0);
                        let next_dev = devices[(cur_pos + 1) % devices.len()];
                        self.sys_ctrl.send(SysCommand::MoveStream {
                            is_input,
                            index,
                            target: next_dev.name.clone(),
                        });
                        self.snapshot = self.sys_ctrl.snapshot();
                        self.needs_redraw = true;
                    }
                }
            }
            HitAction::ToggleWifi => {
                self.sys_ctrl.send(SysCommand::ToggleWifi);
                self.snapshot = self.sys_ctrl.snapshot();
                self.needs_redraw = true;
            }
            HitAction::ToggleBluetooth => {
                self.sys_ctrl.send(SysCommand::ToggleBluetooth);
                self.snapshot = self.sys_ctrl.snapshot();
                self.needs_redraw = true;
            }
            HitAction::SetPowerProfile(profile) => {
                self.sys_ctrl.send(SysCommand::SetPowerProfile {
                    profile: profile.clone(),
                });
                self.config.saved_power_profile = Some(profile.clone());
                self.snapshot = self.sys_ctrl.snapshot();
                self.save_config(&format!("Power profile: {profile}"));
            }
            HitAction::SetTerminal(cmd) => {
                self.config.terminal_cmd = cmd.clone();
                self.save_config(&format!("Terminal -> {cmd}"));
            }
            HitAction::SetBrowser(cmd) => {
                self.config.browser_cmd = cmd.clone();
                self.save_config(&format!("Browser -> {cmd}"));
            }
            HitAction::SetFiles(cmd) => {
                self.config.files_cmd = cmd.clone();
                self.save_config(&format!("Files -> {cmd}"));
            }
            HitAction::SetLauncher(cmd) => {
                self.config.launcher_cmd = cmd.clone();
                self.save_config(&format!("Launcher -> {cmd}"));
            }
            HitAction::ToggleAutoTerminalLogin => {
                self.config.auto_launch_terminal_on_login =
                    !self.config.auto_launch_terminal_on_login;
                self.save_config("Login terminal toggled");
            }
            HitAction::ToggleAutostart(cmd) => {
                if let Some(pos) = self.config.autostart.iter().position(|c| c == &cmd) {
                    self.config.autostart.remove(pos);
                } else {
                    self.config.autostart.push(cmd);
                }
                self.save_config("Autostart updated");
            }
            HitAction::ToggleDnd => {
                self.config.notifications_dnd = !self.config.notifications_dnd;
                self.save_config(if self.config.notifications_dnd {
                    "Notifications Do Not Disturb ON"
                } else {
                    "Notifications Do Not Disturb OFF"
                });
            }
            HitAction::ToggleNotifSnooze => {
                self.config.allow_notification_snooze = !self.config.allow_notification_snooze;
                self.save_config("Notification snooze updated");
            }
            HitAction::ToggleHideSilentStatusBar => {
                self.config.hide_silent_notifications_in_status_bar =
                    !self.config.hide_silent_notifications_in_status_bar;
                self.save_config("Status bar silent visibility updated");
            }
            HitAction::ToggleNotifCooldown => {
                self.config.notification_cooldown_enabled = !self.config.notification_cooldown_enabled;
                self.save_config("Notification cooldown updated");
            }
            HitAction::ToggleNotifOrganizer => {
                self.config.notification_organizer_enabled = !self.config.notification_organizer_enabled;
                self.save_config("Notification Organizer updated");
            }
            HitAction::ToggleOrganizerSilenceCat(cat) => {
                if let Some(pos) = self
                    .config
                    .organized_silenced_categories
                    .iter()
                    .position(|c| c.eq_ignore_ascii_case(&cat))
                {
                    self.config.organized_silenced_categories.remove(pos);
                } else {
                    self.config.organized_silenced_categories.push(cat);
                }
                self.save_config("Organizer silenced categories updated");
            }
            HitAction::ToggleNotifSummaries => {
                self.config.notification_summaries_enabled = !self.config.notification_summaries_enabled;
                self.save_config("Notification summaries updated");
            }
            HitAction::ToggleNotifHistory => {
                self.config.notification_history_enabled = !self.config.notification_history_enabled;
                self.save_config("Notification history updated");
            }
            HitAction::ClearNotifHistory => {
                clear_persisted_notification_history();
                self.notif_history.clear();
                self.status_note = "Cleared notification history".to_string();
                self.needs_redraw = true;
            }
            HitAction::TogglePriorityConversation(key) => {
                if let Some(pos) = self
                    .config
                    .priority_conversations
                    .iter()
                    .position(|k| k.eq_ignore_ascii_case(&key))
                {
                    self.config.priority_conversations.remove(pos);
                } else {
                    self.config.priority_conversations.push(key);
                }
                self.save_config("Priority conversations updated");
            }
            HitAction::ToggleAppNotifDisabled(app_name) => {
                let key = normalize_app_key(&app_name);
                if let Some(pos) = self
                    .config
                    .disabled_notification_apps
                    .iter()
                    .position(|a| normalize_app_key(a) == key)
                {
                    self.config.disabled_notification_apps.remove(pos);
                } else {
                    self.config.disabled_notification_apps.push(key);
                }
                self.save_config("App notification toggle updated");
            }
            HitAction::ToggleAppNotifSilent(app_name) => {
                let key = normalize_app_key(&app_name);
                if let Some(pos) = self
                    .config
                    .silent_notification_apps
                    .iter()
                    .position(|a| normalize_app_key(a) == key)
                {
                    self.config.silent_notification_apps.remove(pos);
                } else {
                    self.config.silent_notification_apps.push(key);
                }
                self.save_config("App silent notification toggle updated");
            }
            HitAction::FocusTextField(field) => {
                self.active_text_field = Some(field);
                self.status_note = "Typing active — press Enter or click another control when done".to_string();
                self.needs_redraw = true;
            }
            HitAction::AppLibFilterCategory(cat) => {
                self.app_filter_category = cat;
                self.needs_redraw = true;
            }
            HitAction::AppLibSearchAllCategories => {
                self.app_filter_category = AppCategory::All;
                self.needs_redraw = true;
            }
            HitAction::ClearAppLibSearch => {
                self.app_filter_query.clear();
                self.needs_redraw = true;
            }
            HitAction::SelectAppForEdit(id) => {
                self.active_text_field = None;
                self.select_app_for_edit(&id);
                self.needs_redraw = true;
            }
            HitAction::EditSetCategory(cat) => {
                self.edit_category = cat;
                self.needs_redraw = true;
            }
            HitAction::EditSetBadgeColor(col) => {
                self.edit_badge_color = col;
                self.edit_badge_hex = format!("#{:02X}{:02X}{:02X}", col[0], col[1], col[2]);
                self.needs_redraw = true;
            }
            HitAction::EditToggleTerminal => {
                self.edit_terminal = !self.edit_terminal;
                self.needs_redraw = true;
            }
            HitAction::EditToggleHidden => {
                self.edit_hidden = !self.edit_hidden;
                self.needs_redraw = true;
            }
            HitAction::EditCycleGameOverride => {
                self.edit_game_override = match self.edit_game_override {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                };
                self.needs_redraw = true;
            }
            HitAction::SaveAppEdit => {
                let Some(id) = self.selected_app_id.clone() else {
                    return;
                };
                if let Some(parsed) = parse_hex_rgb(&self.edit_badge_hex) {
                    self.edit_badge_color = parsed;
                }
                let kws: Vec<String> = self
                    .edit_keywords
                    .split(',')
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .map(str::to_string)
                    .collect();
                let is_custom = self
                    .desktop_apps
                    .iter()
                    .find(|a| a.id == id)
                    .is_some_and(|a| a.is_custom);
                if is_custom {
                    self.app_store.upsert_custom_app(CustomAppEntry {
                        id: id.clone(),
                        name: self.edit_name.trim().to_string(),
                        comment: self.edit_comment.trim().to_string(),
                        exec_line: self.edit_exec.trim().to_string(),
                        terminal: self.edit_terminal,
                        category: self.edit_category,
                        custom_category: if self.edit_custom_cat.trim().is_empty() {
                            None
                        } else {
                            Some(self.edit_custom_cat.trim().to_string())
                        },
                        badge_color: self.edit_badge_color,
                        keywords: kws,
                        is_game_override: self.edit_game_override,
                    });
                } else {
                    self.app_store.upsert_override(AppOverrideEntry {
                        id: id.clone(),
                        name: Some(self.edit_name.trim().to_string()),
                        comment: Some(self.edit_comment.trim().to_string()),
                        exec_line: Some(self.edit_exec.trim().to_string()),
                        terminal: Some(self.edit_terminal),
                        category: Some(self.edit_category),
                        custom_category: Some(self.edit_custom_cat.trim().to_string()),
                        badge_color: Some(self.edit_badge_color),
                        keywords: Some(kws),
                        hidden: self.edit_hidden,
                        is_game_override: self.edit_game_override,
                    });
                }
                if self.app_store.save() {
                    self.reload_app_library("Saved app library changes (live in Start Menu)");
                }
            }
            HitAction::ResetAppOverride => {
                if let Some(ref id) = self.selected_app_id.clone() {
                    self.app_store.remove_override(id);
                    if self.app_store.save() {
                        self.reload_app_library("Reset app values to detected defaults");
                    }
                }
            }
            HitAction::DeleteCustomApp(id) => {
                self.app_store.remove_custom_app(&id);
                if self.app_store.save() {
                    if self.selected_app_id.as_deref() == Some(&id) {
                        self.selected_app_id = None;
                    }
                    self.reload_app_library("Removed custom app");
                }
            }
            HitAction::ToggleNewAppForm => {
                self.show_new_app_form = !self.show_new_app_form;
                if self.show_new_app_form {
                    self.active_text_field = Some(AppTextField::NewAppName);
                }
                self.needs_redraw = true;
            }
            HitAction::NewAppSetCategory(cat) => {
                self.new_app_category = cat;
                self.needs_redraw = true;
            }
            HitAction::NewAppToggleTerminal => {
                self.new_app_terminal = !self.new_app_terminal;
                self.needs_redraw = true;
            }
            HitAction::CreateCustomApp => {
                if self.new_app_name.trim().is_empty()
                    || parse_exec_args(&self.new_app_exec).is_empty()
                {
                    self.status_note = "Enter both Name and Exec command to add an app".to_string();
                    self.needs_redraw = true;
                    return;
                }
                let id = generate_custom_id(&self.new_app_name);
                let kws = self
                    .new_app_keywords
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                self.app_store.upsert_custom_app(CustomAppEntry {
                    id: id.clone(),
                    name: self.new_app_name.trim().to_string(),
                    comment: self.new_app_comment.trim().to_string(),
                    exec_line: self.new_app_exec.trim().to_string(),
                    terminal: self.new_app_terminal,
                    category: self.new_app_category,
                    custom_category: if self.new_app_custom_cat.trim().is_empty() {
                        None
                    } else {
                        Some(self.new_app_custom_cat.trim().to_string())
                    },
                    badge_color: self.config.accent_rgb(),
                    keywords: kws,
                    is_game_override: None,
                });
                if self.app_store.save() {
                    self.new_app_name.clear();
                    self.new_app_comment.clear();
                    self.new_app_exec.clear();
                    self.new_app_custom_cat.clear();
                    self.new_app_keywords.clear();
                    self.show_new_app_form = false;
                    self.active_text_field = None;
                    self.selected_app_id = Some(id);
                    self.reload_app_library("Added custom app to library");
                }
            }
            HitAction::ToggleNewRuleForm => {
                self.show_new_rule_form = !self.show_new_rule_form;
                if self.show_new_rule_form {
                    self.active_text_field = Some(AppTextField::NewRuleCategory);
                }
                self.needs_redraw = true;
            }
            HitAction::NewRuleSetBuiltinCat(cat) => {
                self.new_rule_builtin = cat;
                self.needs_redraw = true;
            }
            HitAction::CreateCategoryRule => {
                let kws: Vec<String> = self
                    .new_rule_keywords
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                if self.new_rule_category.trim().is_empty() || kws.is_empty() {
                    self.status_note = "Enter category name and comma-separated keywords".to_string();
                    self.needs_redraw = true;
                    return;
                }
                self.app_store.category_rules.push(CustomCategoryRule {
                    category_name: self.new_rule_category.trim().to_string(),
                    keywords: kws,
                    builtin_category: self.new_rule_builtin,
                });
                if self.app_store.save() {
                    self.new_rule_category.clear();
                    self.new_rule_keywords.clear();
                    self.show_new_rule_form = false;
                    self.active_text_field = None;
                    self.reload_app_library("Added category detection rule");
                }
            }
            HitAction::DeleteCategoryRule(idx) => {
                if idx < self.app_store.category_rules.len() {
                    self.app_store.category_rules.remove(idx);
                    if self.app_store.save() {
                        self.reload_app_library("Removed category rule");
                    }
                }
            }
            HitAction::RescanAppLibraryNow => {
                let _ = scan_desktop_apps();
                self.reload_app_library("Rescanned desktop apps & overrides");
            }
            HitAction::ToggleCheckGithubUpdates => {
                self.config.check_github_updates = !self.config.check_github_updates;
                self.save_config("GitHub release check toggled");
            }
            HitAction::CheckUpdatesNow => {
                send_external_cmd("check-updates");
                self.status_note = "Checking for local & GitHub updates...".to_string();
                self.needs_redraw = true;
            }
            HitAction::SeamlessReloadWm => {
                send_external_cmd("confirm-reload");
                self.status_note =
                    "Opened snapshot & reload confirmation dialog in Velox WM".to_string();
                self.needs_redraw = true;
            }
            HitAction::ToggleShowSystemProcs => {
                let next = !self.config.show_system_processes;
                self.config.show_system_processes = next;
                self.sys_ctrl.send(SysCommand::SetShowSystemProcesses { enabled: next });
                self.sys_ctrl.send(SysCommand::RefreshNow);
                self.snapshot = self.sys_ctrl.snapshot();
                self.save_config("Background filter updated");
            }
            HitAction::KillProcess { pids, force } => {
                self.sys_ctrl.send(SysCommand::KillProcess { pids, force });
                self.snapshot = self.sys_ctrl.snapshot();
                self.status_note = if force {
                    "Force-terminated background app".to_string()
                } else {
                    "Closing background app cleanly...".to_string()
                };
                self.needs_redraw = true;
            }
            HitAction::RefreshSnapshot => {
                self.sys_ctrl.send(SysCommand::RefreshNow);
                self.snapshot = self.sys_ctrl.snapshot();
                self.needs_redraw = true;
            }
            HitAction::MoveTaskbarTile { tile, delta } => {
                if self.config.move_taskbar_tile(&tile, delta) {
                    self.save_config("Taskbar tile order updated");
                }
            }
            HitAction::ToggleTaskbarTile(tile) => {
                if self.config.toggle_taskbar_tile_visible(&tile) {
                    self.save_config("Taskbar tile visibility updated");
                }
            }
            HitAction::ResetTaskbarTiles => {
                self.config.reset_taskbar_tiles();
                self.save_config("Taskbar tiles reset to default");
            }
            HitAction::MoveQuickPanelTile { tile, delta } => {
                if self.config.move_quick_panel_tile(&tile, delta) {
                    self.save_config("Quick panel tile order updated");
                }
            }
            HitAction::ToggleMinimalViewConfig => {
                self.config.minimal_view_enabled = !self.config.minimal_view_enabled;
                self.save_config("Compact window Minimal View updated");
            }
            HitAction::OpenAppCtxMenu { app_id, x, y } => {
                self.app_ctx_menu = Some((app_id, x, y));
                self.needs_redraw = true;
            }
            HitAction::CloseAppCtxMenu => {
                self.app_ctx_menu = None;
                self.needs_redraw = true;
            }
            HitAction::AppCtxSelectForEdit(id) => {
                self.app_ctx_menu = None;
                self.select_app_for_edit(&id);
                self.active_text_field = Some(AppTextField::EditName);
                self.status_note = format!("Editing {id} properties");
                self.needs_redraw = true;
            }
            HitAction::AppCtxLaunch(id) => {
                self.app_ctx_menu = None;
                if let Some(app) = self.desktop_apps.iter().find(|a| a.id == id).cloned() {
                    let wd = std::env::var("WAYLAND_DISPLAY").ok();
                    let xd = std::env::var("DISPLAY").ok();
                    app.launch(&self.config.terminal_cmd, wd.as_deref(), xd.as_deref());
                    self.status_note = format!("Launched {}", app.name);
                }
                self.needs_redraw = true;
            }
            HitAction::AppCtxToggleTerminal(id) => {
                self.app_ctx_menu = None;
                self.select_app_for_edit(&id);
                self.edit_terminal = !self.edit_terminal;
                self.execute_action(HitAction::SaveAppEdit);
            }
            HitAction::AppCtxCycleGame(id) => {
                self.app_ctx_menu = None;
                self.select_app_for_edit(&id);
                self.edit_game_override = match self.edit_game_override {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                };
                self.execute_action(HitAction::SaveAppEdit);
            }
            HitAction::AppCtxToggleHidden(id) => {
                self.app_ctx_menu = None;
                self.select_app_for_edit(&id);
                self.edit_hidden = !self.edit_hidden;
                self.execute_action(HitAction::SaveAppEdit);
            }
            HitAction::SliderTrack { .. } => {}
        }
    }

    fn draw_toggle_pill(
        &mut self,
        x: i32,
        y: i32,
        label: &str,
        sub: &str,
        active: bool,
        action: HitAction,
    ) {
        let accent = self.config.accent_rgb();
        let w = self.width as i32;
        let h = self.height as i32;
        let card_w = (w - SIDEBAR_W - 60).max(260);
        let row_h = 54;
        let hovered = self.hovered_rect(HitRect::new(x, y, card_w, row_h));
        let bg = if hovered {
            [30, 42, 68, 235]
        } else {
            [20, 29, 49, 225]
        };
        fill_rounded_rect(&mut self.rgba_buf, w, h, x, y, card_w, row_h, 11, bg);
        stroke_rounded_rect(
            &mut self.rgba_buf,
            w,
            h,
            x,
            y,
            card_w,
            row_h,
            11,
            1,
            if active {
                [accent[0], accent[1], accent[2], 145]
            } else {
                [72, 92, 132, 95]
            },
        );
        self.painter.draw_text(
            &mut self.rgba_buf,
            w,
            h,
            x + 14,
            y + 10,
            card_w - 92,
            label,
            14.0,
            [236, 242, 255, 255],
        );
        self.painter.draw_text(
            &mut self.rgba_buf,
            w,
            h,
            x + 14,
            y + 30,
            card_w - 92,
            sub,
            11.5,
            [154, 170, 200, 235],
        );

        let sw = 48;
        let sh = 24;
        let sx = x + card_w - sw - 14;
        let sy = y + (row_h - sh) / 2;
        let track_col = if active {
            [accent[0], accent[1], accent[2], 245]
        } else {
            [56, 68, 96, 235]
        };
        fill_rounded_rect(&mut self.rgba_buf, w, h, sx, sy, sw, sh, 12, track_col);
        let knob_cx = if active { sx + sw - 12 } else { sx + 12 };
        fill_circle(
            &mut self.rgba_buf,
            w,
            h,
            knob_cx,
            sy + sh / 2,
            8,
            [245, 249, 255, 255],
        );

        self.hit_regions.push(HitRegion {
            rect: HitRect::new(x, y, card_w, row_h),
            action,
        });
    }

    fn draw_slider_row(
        &mut self,
        x: i32,
        y: i32,
        label: &str,
        value_label: &str,
        ratio: f32,
        target: SliderTarget,
    ) {
        let accent = self.config.accent_rgb();
        let w = self.width as i32;
        let h = self.height as i32;
        let card_w = (w - SIDEBAR_W - 60).max(260);
        let row_h = 58;
        fill_rounded_rect(
            &mut self.rgba_buf,
            w,
            h,
            x,
            y,
            card_w,
            row_h,
            11,
            [20, 29, 49, 225],
        );
        stroke_rounded_rect(
            &mut self.rgba_buf,
            w,
            h,
            x,
            y,
            card_w,
            row_h,
            11,
            1,
            [72, 92, 132, 95],
        );
        self.painter.draw_text(
            &mut self.rgba_buf,
            w,
            h,
            x + 14,
            y + 9,
            card_w - 130,
            label,
            13.5,
            [236, 242, 255, 255],
        );
        let val_w = self.painter.measure_text(value_label, 13.0);
        self.painter.draw_text(
            &mut self.rgba_buf,
            w,
            h,
            x + card_w - val_w - 14,
            y + 9,
            110,
            value_label,
            13.0,
            [accent[0], accent[1], accent[2], 255],
        );

        let track_x = x + 14;
        let track_y = y + 36;
        let track_w = card_w - 28;
        self.draw_inline_slider_track(track_x, track_y, track_w, ratio, target);
    }

    fn draw_inline_slider_track(
        &mut self,
        track_x: i32,
        track_y: i32,
        track_w: i32,
        ratio: f32,
        target: SliderTarget,
    ) {
        let accent = self.config.accent_rgb();
        let w = self.width as i32;
        let h = self.height as i32;
        let track_h = 8;
        fill_rounded_rect(
            &mut self.rgba_buf,
            w,
            h,
            track_x,
            track_y,
            track_w,
            track_h,
            4,
            [44, 58, 88, 240],
        );
        let fill_w = ((track_w as f32 * ratio.clamp(0.0, 1.0)).round() as i32).max(6);
        fill_rounded_rect(
            &mut self.rgba_buf,
            w,
            h,
            track_x,
            track_y,
            fill_w,
            track_h,
            4,
            [accent[0], accent[1], accent[2], 245],
        );
        let knob_x = track_x + (track_w as f32 * ratio.clamp(0.0, 1.0)).round() as i32;
        fill_circle(
            &mut self.rgba_buf,
            w,
            h,
            knob_x.clamp(track_x + 6, track_x + track_w - 6),
            track_y + track_h / 2,
            7,
            [246, 250, 255, 255],
        );

        let hit = HitRect::new(track_x, track_y - 12, track_w, 28);
        self.hit_regions.push(HitRegion {
            rect: hit,
            action: HitAction::SliderTrack { target, rect: hit },
        });
    }

    fn draw_chip_button(
        &mut self,
        x: i32,
        y: i32,
        cw: i32,
        ch: i32,
        text: &str,
        selected: bool,
        dot_rgb: Option<[u8; 3]>,
        action: HitAction,
    ) {
        let accent = self.config.accent_rgb();
        let w = self.width as i32;
        let h = self.height as i32;
        let rect = HitRect::new(x, y, cw, ch);
        let hovered = self.hovered_rect(rect);
        let bg = if selected {
            [accent[0], accent[1], accent[2], 210]
        } else if hovered {
            [42, 58, 92, 235]
        } else {
            [25, 36, 60, 225]
        };
        fill_rounded_rect(&mut self.rgba_buf, w, h, x, y, cw, ch, 9, bg);
        stroke_rounded_rect(
            &mut self.rgba_buf,
            w,
            h,
            x,
            y,
            cw,
            ch,
            9,
            1,
            if selected {
                [235, 245, 255, 190]
            } else {
                [84, 108, 150, 105]
            },
        );
        let text_x = if let Some(rgb) = dot_rgb {
            fill_circle(
                &mut self.rgba_buf,
                w,
                h,
                x + 15,
                y + ch / 2,
                6,
                [rgb[0], rgb[1], rgb[2], 255],
            );
            x + 27
        } else {
            x + 11
        };
        let ty = y + ((ch - 13) / 2).max(2);
        self.painter.draw_text(
            &mut self.rgba_buf,
            w,
            h,
            text_x,
            ty,
            cw - (text_x - x) - 8,
            text,
            12.5,
            [244, 248, 255, 255],
        );
        self.hit_regions.push(HitRegion { rect, action });
    }

    fn hovered_rect(&self, rect: HitRect) -> bool {
        rect.contains(self.pointer_x, self.pointer_y)
    }

    fn render_ui(&mut self, qh: &QueueHandle<Self>) {
        if !self.configured || self.width == 0 || self.height == 0 {
            return;
        }
        let w = self.width as i32;
        let h = self.height as i32;
        let byte_len = (w * h * 4) as usize;
        if self.rgba_buf.len() != byte_len {
            self.rgba_buf.resize(byte_len, 0);
        } else {
            self.rgba_buf.fill(0);
        }
        self.hit_regions.clear();
        self.app_row_rects.clear();

        let accent = self.config.accent_rgb();
        let radius = if self.maximized { 0 } else { WINDOW_RADIUS };

        // Outer glass-dark shell backdrop + outer border
        fill_rounded_rect(&mut self.rgba_buf, w, h, 0, 0, w, h, radius, [10, 15, 28, 246]);
        stroke_rounded_rect(
            &mut self.rgba_buf,
            w,
            h,
            0,
            0,
            w,
            h,
            radius,
            1,
            [92, 126, 192, 128],
        );

        // Sidebar panel
        fill_rounded_rect(
            &mut self.rgba_buf,
            w,
            h,
            8,
            TITLEBAR_H + 4,
            SIDEBAR_W - 14,
            h - TITLEBAR_H - 12,
            11,
            [14, 21, 38, 238],
        );

        // Sidebar tabs
        let mut tab_y = TITLEBAR_H + 12;
        for tab in SettingsTab::ALL {
            let active = self.active_tab == tab;
            let rect = HitRect::new(16, tab_y, SIDEBAR_W - 30, 44);
            let hovered = self.hovered_rect(rect);
            let bg = if active {
                [accent[0], accent[1], accent[2], 210]
            } else if hovered {
                [32, 46, 76, 225]
            } else {
                [20, 30, 52, 185]
            };
            fill_rounded_rect(&mut self.rgba_buf, w, h, rect.x, rect.y, rect.w, rect.h, 10, bg);
            self.painter.draw_text(
                &mut self.rgba_buf,
                w,
                h,
                rect.x + 12,
                rect.y + 6,
                rect.w - 20,
                tab.title(),
                12.5,
                [246, 249, 255, 255],
            );
            self.painter.draw_text(
                &mut self.rgba_buf,
                w,
                h,
                rect.x + 12,
                rect.y + 24,
                rect.w - 20,
                tab.subtitle(),
                9.5,
                if active {
                    [236, 244, 255, 240]
                } else {
                    [150, 168, 200, 220]
                },
            );
            self.hit_regions.push(HitRegion {
                rect,
                action: HitAction::SelectTab(tab),
            });
            tab_y += 49;
        }

        // Content area (scrolled vertically, clipped via top/bottom masking before titlebar & footer)
        let cx = SIDEBAR_W + 18;
        let card_w = (w - SIDEBAR_W - 60).max(260);
        let mut cy = TITLEBAR_H + 18 - self.scroll_y;
        let content_top = TITLEBAR_H + 4;

        match self.active_tab {
            SettingsTab::Personalize => {
                self.draw_section_header(cx, &mut cy, "Theme Accent Color");
                let cols = ((card_w + 10) / 138).max(2);
                let chip_w = (card_w - (cols - 1) * 10) / cols;
                for (i, (name, rgb)) in ACCENT_PRESETS.iter().enumerate() {
                    let row = (i as i32) / cols;
                    let col = (i as i32) % cols;
                    let selected = self.config.accent_theme.eq_ignore_ascii_case(name);
                    self.draw_chip_button(
                        cx + col * (chip_w + 10),
                        cy + row * 40,
                        chip_w,
                        32,
                        name,
                        selected,
                        Some(*rgb),
                        HitAction::SetAccent((*name).to_string()),
                    );
                }
                let rows = ((ACCENT_PRESETS.len() as i32 + cols - 1) / cols).max(1);
                cy += rows * 40 + 10;

                self.draw_section_header(cx, &mut cy, "Wallpaper Preset & Source");
                let wcols = ((card_w + 10) / 138).max(2);
                let wchip_w = (card_w - (wcols - 1) * 10) / wcols;
                for (i, preset) in WALLPAPER_PRESETS.iter().enumerate() {
                    let row = (i as i32) / wcols;
                    let col = (i as i32) % wcols;
                    let selected = !self.config.use_custom_wallpaper
                        && self.config.wallpaper_preset.eq_ignore_ascii_case(preset);
                    self.draw_chip_button(
                        cx + col * (wchip_w + 10),
                        cy + row * 40,
                        wchip_w,
                        32,
                        preset,
                        selected,
                        None,
                        HitAction::SetWallpaperPreset((*preset).to_string()),
                    );
                }
                let wrows = ((WALLPAPER_PRESETS.len() as i32 + wcols - 1) / wcols).max(1);
                cy += wrows * 40 + 8;

                let wp_sub = self
                    .config
                    .resolve_wallpaper_path()
                    .map(|p| format!("Found: {}", p.display()))
                    .unwrap_or_else(|| {
                        "Place ~/.config/velox-wm/wallpaper.jpg or .png to use custom image"
                            .to_string()
                    });
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Use Custom Wallpaper Image",
                    &wp_sub,
                    self.config.use_custom_wallpaper,
                    HitAction::ToggleCustomWallpaper,
                );
                cy += 64;

                self.draw_section_header(cx, &mut cy, "ChromeOS Shelf & Window Styling");
                let half = (card_w - 10) / 2;
                self.draw_chip_button(
                    cx,
                    cy,
                    half,
                    34,
                    "Shelf: Bottom",
                    self.config.panel_position == PanelPosition::Bottom,
                    None,
                    HitAction::SetPanelPos(PanelPosition::Bottom),
                );
                self.draw_chip_button(
                    cx + half + 10,
                    cy,
                    half,
                    34,
                    "Shelf: Top",
                    self.config.panel_position == PanelPosition::Top,
                    None,
                    HitAction::SetPanelPos(PanelPosition::Top),
                );
                cy += 42;
                self.draw_chip_button(
                    cx,
                    cy,
                    half,
                    34,
                    "Style: ChromeOS Pill",
                    self.config.shelf_style == ShelfStyle::Floating,
                    None,
                    HitAction::SetShelfStyle(ShelfStyle::Floating),
                );
                self.draw_chip_button(
                    cx + half + 10,
                    cy,
                    half,
                    34,
                    "Style: Full Width Bar",
                    self.config.shelf_style == ShelfStyle::Full,
                    None,
                    HitAction::SetShelfStyle(ShelfStyle::Full),
                );
                cy += 44;

                let corner_val = format!("{} px", self.config.corner_radius);
                self.draw_slider_row(
                    cx,
                    cy,
                    "Window Corner Radius",
                    &corner_val,
                    self.config.corner_radius as f32 / 24.0,
                    SliderTarget::CornerRadius,
                );
                cy += 66;
                let bw_val = format!("{} px", self.config.border_width);
                self.draw_slider_row(
                    cx,
                    cy,
                    "Focused Window Border Width",
                    &bw_val,
                    self.config.border_width as f32 / 6.0,
                    SliderTarget::BorderWidth,
                );
                cy += 66;
                let ph_val = format!("{} px", self.config.panel_height);
                self.draw_slider_row(
                    cx,
                    cy,
                    "Taskbar Height",
                    &ph_val,
                    (self.config.panel_height - 34) as f32 / 30.0,
                    SliderTarget::PanelHeight,
                );
                cy += 66;
                let po_val = format!("{} / 255", self.config.panel_opacity);
                self.draw_slider_row(
                    cx,
                    cy,
                    "Taskbar Opacity",
                    &po_val,
                    (self.config.panel_opacity.saturating_sub(140)) as f32 / 115.0,
                    SliderTarget::PanelOpacity,
                );
                cy += 68;

                self.draw_section_header(cx, &mut cy, "Taskbar Quick Access Tile Placement");
                let tiles_all = self.config.all_taskbar_tiles_ordered();
                for (tile, visible) in tiles_all {
                    let hidden = !visible;
                    let row_bg = if hidden { [16, 22, 38, 190] } else { [20, 29, 49, 232] };
                    fill_rounded_rect(&mut self.rgba_buf, w, h, cx, cy, card_w, 38, 9, row_bg);
                    self.painter.draw_text(
                        &mut self.rgba_buf,
                        w,
                        h,
                        cx + 12,
                        cy + 11,
                        card_w - 230,
                        quick_tile_label(&tile),
                        12.5,
                        if hidden { [140, 152, 180, 220] } else { [240, 245, 255, 255] },
                    );
                    self.draw_chip_button(
                        cx + card_w - 208,
                        cy + 5,
                        56,
                        28,
                        "< Left",
                        false,
                        None,
                        HitAction::MoveTaskbarTile { tile: tile.clone(), delta: -1 },
                    );
                    self.draw_chip_button(
                        cx + card_w - 146,
                        cy + 5,
                        60,
                        28,
                        "Right >",
                        false,
                        None,
                        HitAction::MoveTaskbarTile { tile: tile.clone(), delta: 1 },
                    );
                    self.draw_chip_button(
                        cx + card_w - 80,
                        cy + 5,
                        70,
                        28,
                        if hidden { "Hidden" } else { "Shown" },
                        !hidden,
                        None,
                        HitAction::ToggleTaskbarTile(tile),
                    );
                    cy += 44;
                }
                self.draw_chip_button(
                    cx,
                    cy,
                    220,
                    32,
                    "Reset Taskbar Tile Order",
                    false,
                    None,
                    HitAction::ResetTaskbarTiles,
                );
                cy += 44;

                self.draw_section_header(cx, &mut cy, "Quick Settings Top Toggle Tile Order");
                for tile in self.config.effective_quick_panel_tiles() {
                    let lbl = match tile.as_str() {
                        "wifi" => "Wi-Fi Toggle Tile",
                        "bluetooth" => "Bluetooth Toggle Tile",
                        "layout" => "Tiling Mode Toggle Tile",
                        "edge_snap" => "Edge Snap Toggle Tile",
                        "dim_inactive" => "Dim Inactive Windows Tile",
                        "video_saver" => "Video Battery Saver Tile",
                        other => other,
                    };
                    fill_rounded_rect(&mut self.rgba_buf, w, h, cx, cy, card_w, 34, 8, [18, 26, 44, 225]);
                    self.painter.draw_text(
                        &mut self.rgba_buf,
                        w,
                        h,
                        cx + 12,
                        cy + 10,
                        card_w - 140,
                        lbl,
                        12.0,
                        [232, 240, 255, 245],
                    );
                    self.draw_chip_button(
                        cx + card_w - 122,
                        cy + 4,
                        54,
                        26,
                        "Up",
                        false,
                        None,
                        HitAction::MoveQuickPanelTile { tile: tile.clone(), delta: -1 },
                    );
                    self.draw_chip_button(
                        cx + card_w - 62,
                        cy + 4,
                        54,
                        26,
                        "Down",
                        false,
                        None,
                        HitAction::MoveQuickPanelTile { tile, delta: 1 },
                    );
                    cy += 40;
                }
                cy += 12;
            }
            SettingsTab::Display => {
                self.draw_section_header(cx, &mut cy, "Detected Monitors (Click a Monitor to Configure)");
                let selected_out = self.selected_output();
                let selected_name = selected_out
                    .as_ref()
                    .map(|o| o.name.clone())
                    .unwrap_or_default();

                if !self.display_telemetry.outputs.is_empty() {
                    let multi_out = self.display_telemetry.outputs.len() > 1;
                    for out in self.display_telemetry.outputs.clone() {
                        let is_selected = out.name == selected_name;
                        let card_rect = HitRect::new(cx, cy, card_w, 56);
                        let hovered = self.hovered_rect(card_rect);
                        let bg = if is_selected {
                            [28, 42, 74, 240]
                        } else if hovered {
                            [25, 37, 64, 236]
                        } else {
                            [20, 30, 52, 232]
                        };
                        fill_rounded_rect(&mut self.rgba_buf, w, h, cx, cy, card_w, 56, 11, bg);
                        stroke_rounded_rect(
                            &mut self.rgba_buf,
                            w,
                            h,
                            cx,
                            cy,
                            card_w,
                            56,
                            11,
                            if is_selected { 2 } else { 1 },
                            if is_selected {
                                [accent[0], accent[1], accent[2], 235]
                            } else {
                                [84, 108, 150, 115]
                            },
                        );
                        let vrr_txt = if out.vrr_capable {
                            if out.vrr_enabled {
                                "VRR Active"
                            } else if out.vrr_requires_modeset {
                                "VRR Ready (HDMI)"
                            } else {
                                "VRR Ready"
                            }
                        } else {
                            "Fixed Mode (Paced)"
                        };
                        let pace_hz = if out.active_pacing_hz > 0 {
                            out.active_pacing_hz
                        } else {
                            out.current_hz
                        };
                        let badge = if out.primary { " • Primary" } else { "" };
                        let title = format!(
                            "{}{} • {}x{} ({:.0}%)",
                            out.name,
                            badge,
                            out.width,
                            out.height,
                            out.scale_x100.max(100) as f32
                        );
                        let desc = format!(
                            "KMS {} Hz • Paced {} Hz • Max {} Hz • {} • {}",
                            out.current_hz, pace_hz, out.preferred_hz, vrr_txt, out.battery_saver_state
                        );
                        self.painter.draw_text(
                            &mut self.rgba_buf,
                            w,
                            h,
                            cx + 14,
                            cy + 9,
                            card_w - 28,
                            &title,
                            14.0,
                            [240, 246, 255, 255],
                        );
                        self.painter.draw_text(
                            &mut self.rgba_buf,
                            w,
                            h,
                            cx + 14,
                            cy + 30,
                            card_w - 28,
                            &desc,
                            11.5,
                            [150, 212, 184, 245],
                        );
                        self.hit_regions.push(HitRegion {
                            rect: card_rect,
                            action: HitAction::SelectMonitor(out.name),
                        });
                        cy += 64;
                    }
                    if multi_out && !selected_name.is_empty() {
                        let is_pri = selected_out.as_ref().map(|o| o.primary).unwrap_or(false);
                        self.draw_chip_button(
                            cx,
                            cy,
                            card_w,
                            32,
                            &format!("Set {selected_name} as Primary Monitor"),
                            is_pri,
                            None,
                            HitAction::SetPrimaryMonitor(selected_name.clone()),
                        );
                        cy += 42;
                    }
                } else {
                    let mut fallback_cards = Vec::new();
                    for out in self.output_state.outputs() {
                        if let Some(info) = self.output_state.info(&out) {
                            let cur_mode = info.modes.iter().find(|m| m.current);
                            let max_mhz = info.modes.iter().map(|m| m.refresh_rate).max().unwrap_or(0);
                            let res_text = cur_mode
                                .map(|m| {
                                    format!(
                                        "{}x{} @ {} (Max {})",
                                        m.dimensions.0,
                                        m.dimensions.1,
                                        format_mode_mhz(m.refresh_rate),
                                        format_mode_mhz(max_mhz),
                                    )
                                })
                                .unwrap_or_else(|| "Active output".to_string());
                            let label = format!("{} {}", info.make, info.model).trim().to_string();
                            fallback_cards.push((
                                if label.is_empty() {
                                    format!("Output #{}", info.id)
                                } else {
                                    label
                                },
                                res_text,
                            ));
                        }
                    }
                    if fallback_cards.is_empty() {
                        fallback_cards.push((
                            "Primary Display".to_string(),
                            "Awaiting compositor output info".to_string(),
                        ));
                    }
                    for (name, desc) in fallback_cards {
                        fill_rounded_rect(&mut self.rgba_buf, w, h, cx, cy, card_w, 52, 11, [22, 34, 58, 232]);
                        stroke_rounded_rect(&mut self.rgba_buf, w, h, cx, cy, card_w, 52, 11, 1, [accent[0], accent[1], accent[2], 130]);
                        self.painter.draw_text(&mut self.rgba_buf, w, h, cx + 14, cy + 8, card_w - 28, &name, 14.0, [240, 246, 255, 255]);
                        self.painter.draw_text(&mut self.rgba_buf, w, h, cx + 14, cy + 28, card_w - 28, &desc, 11.5, [150, 212, 184, 245]);
                        cy += 60;
                    }
                }

                let (eff_w, eff_h, eff_hz, eff_scale, eff_vrr) = selected_out
                    .as_ref()
                    .map(|o| self.effective_monitor_settings(o))
                    .unwrap_or((0, 0, self.config.output_refresh_hz, 1.0, self.config.vrr_mode));
                let target_hdr = if selected_name.is_empty() {
                    "Primary Display".to_string()
                } else {
                    selected_name.clone()
                };

                if let Some(ref out) = selected_out {
                    if !out.resolution_modes.is_empty() {
                        self.draw_section_header(cx, &mut cy, &format!("Resolution — {target_hdr}"));
                        let r_cols = 3;
                        let rw = (card_w - 20) / r_cols;
                        for (i, mode) in out.resolution_modes.iter().take(12).enumerate() {
                            let row = (i as i32) / r_cols;
                            let col = (i as i32) % r_cols;
                            let lbl = format!("{}x{}", mode.width, mode.height);
                            self.draw_chip_button(
                                cx + col * (rw + 10),
                                cy + row * 40,
                                rw,
                                32,
                                &lbl,
                                eff_w == mode.width && eff_h == mode.height,
                                None,
                                HitAction::RequestMonitorRes {
                                    name: selected_name.clone(),
                                    width: mode.width,
                                    height: mode.height,
                                },
                            );
                        }
                        let shown = out.resolution_modes.len().min(12) as i32;
                        let r_rows = ((shown + r_cols - 1) / r_cols).max(1);
                        cy += r_rows * 40 + 8;
                    }
                }

                if !selected_name.is_empty() {
                    self.draw_section_header(cx, &mut cy, &format!("Display Scale — {target_hdr}"));
                    let scales: &[(f32, &str)] = &[(1.0, "100%"), (1.25, "125%"), (1.5, "150%"), (2.0, "200%")];
                    let sw = (card_w - 30) / 4;
                    let eff_scale_x100 = (eff_scale * 100.0).round() as i32;
                    for (i, (sc, lbl)) in scales.iter().enumerate() {
                        self.draw_chip_button(
                            cx + i as i32 * (sw + 10),
                            cy,
                            sw,
                            32,
                            lbl,
                            eff_scale_x100 == (*sc * 100.0).round() as i32,
                            None,
                            HitAction::SetMonitorScale {
                                name: selected_name.clone(),
                                scale: *sc,
                            },
                        );
                    }
                    cy += 42;
                }

                self.draw_section_header(
                    cx,
                    &mut cy,
                    &format!("Refresh Rate — {target_hdr} (Zero-Black-Screen Pacing)"),
                );
                let mut hz_choices: Vec<u32> = selected_out
                    .as_ref()
                    .and_then(|o| {
                        o.resolution_modes
                            .iter()
                            .find(|r| r.width == eff_w && r.height == eff_h)
                            .map(|r| r.hz.clone())
                            .or_else(|| (!o.available_hz.is_empty()).then(|| o.available_hz.clone()))
                    })
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| vec![60, 120, 144, 165, 240]);
                hz_choices.retain(|&hz| hz > 0);
                hz_choices.sort_unstable();
                hz_choices.dedup();
                let mut hz_opts: Vec<(u32, String)> = vec![(0, "Auto Max".to_string())];
                for hz in hz_choices {
                    hz_opts.push((hz, format!("{hz} Hz")));
                }
                let cols = 3;
                let cw = (card_w - 20) / cols;
                for (i, (hz, label)) in hz_opts.iter().enumerate() {
                    let row = (i as i32) / cols;
                    let col = (i as i32) % cols;
                    self.draw_chip_button(
                        cx + col * (cw + 10),
                        cy + row * 40,
                        cw,
                        32,
                        label,
                        eff_hz == *hz,
                        None,
                        HitAction::RequestRefreshHz {
                            name: selected_name.clone(),
                            hz: *hz,
                        },
                    );
                }
                let hz_rows = ((hz_opts.len() as i32 + cols - 1) / cols).max(1);
                cy += hz_rows * 40 + 8;

                self.draw_section_header(cx, &mut cy, &format!("Variable Refresh Rate (VRR) — {target_hdr}"));
                let vrr_modes = [VrrMode::Auto, VrrMode::AlwaysOn, VrrMode::Off];
                let vc = (card_w - 20) / 3;
                for (i, mode) in vrr_modes.iter().enumerate() {
                    self.draw_chip_button(
                        cx + i as i32 * (vc + 10),
                        cy,
                        vc,
                        34,
                        mode.label(),
                        eff_vrr == *mode,
                        None,
                        HitAction::RequestVrrMode {
                            name: selected_name.clone(),
                            mode: *mode,
                        },
                    );
                }
                cy += 44;

                self.draw_section_header(cx, &mut cy, "Seamless Refresh Overlay & Battery Saver");
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Zero-Black-Screen Seamless Overlay (No Modeset Blink)",
                    "Keeps DRM panel mode steady and paces video/idle refresh cleanly without black screens",
                    self.config.seamless_refresh_overlay,
                    HitAction::ToggleSeamlessRefreshOverlay,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Show Refresh & Battery-Saver Overlay Pill",
                    "Compact corner HUD showing KMS Hz, live paced Hz, Video fps & Idle saver state",
                    self.config.show_refresh_overlay,
                    HitAction::ToggleShowRefreshOverlay,
                );
                cy += 62;
                let vid_sub = if self.display_telemetry.detected_video_fps > 0 {
                    format!(
                        "Smooth {} fps video playback detected (zero-stutter sync; downclocks on static idle or VRR)",
                        self.display_telemetry.detected_video_fps
                    )
                } else {
                    "Matches refresh dynamically for smooth playback and downclocks static screens seamlessly"
                        .to_string()
                };
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Video & Idle Battery Saver",
                    &vid_sub,
                    self.config.video_battery_saver,
                    HitAction::ToggleVideoBatterySaver,
                );
                cy += 62;

                self.draw_section_header(cx, &mut cy, "Idle Downclock Refresh Target (After 3s Idle)");
                let idle_opts: &[(u32, &str)] = &[(0, "Off (Native)"), (24, "24 Hz"), (10, "10 Hz")];
                for (i, (hz, lbl)) in idle_opts.iter().enumerate() {
                    self.draw_chip_button(
                        cx + i as i32 * (vc + 10),
                        cy,
                        vc,
                        32,
                        lbl,
                        self.config.idle_refresh_hz == *hz,
                        None,
                        HitAction::SetIdleRefreshHz(*hz),
                    );
                }
                cy += 42;

                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Confirm Before Resolution / Refresh / VRR Changes",
                    "Prompt once before applying display changes (off = instant apply)",
                    self.config.confirm_before_display_apply,
                    HitAction::ToggleConfirmBeforeDisplay,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Optional Non-VRR Physical Modeset",
                    "Off by default to prevent monitor blackouts; seamless overlay handles Hz pacing safely",
                    self.config.battery_saver_modeset,
                    HitAction::ToggleBatterySaverModeset,
                );
                cy += 64;

                self.draw_section_header(cx, &mut cy, "Idle Screen Dimming & Display Sleep (Energy Saver)");
                self.painter.draw_text(
                    &mut self.rgba_buf,
                    w,
                    h,
                    cx,
                    cy,
                    card_w,
                    "Dim whole screen after inactivity (no keyboard/mouse movement):",
                    12.0,
                    [184, 202, 232, 240],
                );
                cy += 22;
                let dim_opts: &[(u32, &str)] = &[
                    (0, "Off"),
                    (60, "1 min"),
                    (300, "5 min"),
                    (600, "10 min"),
                    (900, "15 min"),
                ];
                let d5 = (card_w - 32) / 5;
                for (i, (secs, lbl)) in dim_opts.iter().enumerate() {
                    self.draw_chip_button(
                        cx + i as i32 * (d5 + 8),
                        cy,
                        d5,
                        32,
                        lbl,
                        self.config.idle_dim_secs == *secs,
                        None,
                        HitAction::SetIdleDimSecs(*secs),
                    );
                }
                cy += 42;

                self.painter.draw_text(
                    &mut self.rgba_buf,
                    w,
                    h,
                    cx,
                    cy,
                    card_w,
                    "Turn off display or switch screen black after longer inactivity:",
                    12.0,
                    [184, 202, 232, 240],
                );
                cy += 22;
                let sleep_opts: &[(u32, &str)] = &[
                    (0, "Off"),
                    (300, "5 min"),
                    (600, "10 min"),
                    (900, "15 min"),
                    (1800, "30 min"),
                ];
                for (i, (secs, lbl)) in sleep_opts.iter().enumerate() {
                    self.draw_chip_button(
                        cx + i as i32 * (d5 + 8),
                        cy,
                        d5,
                        32,
                        lbl,
                        self.config.idle_sleep_secs == *secs,
                        None,
                        HitAction::SetIdleSleepSecs(*secs),
                    );
                }
                cy += 42;

                let s2 = (card_w - 10) / 2;
                self.draw_chip_button(
                    cx,
                    cy,
                    s2,
                    34,
                    "Sleep Mode: Display Sleep (DPMS Off)",
                    self.config.idle_sleep_mode == DisplaySleepMode::DpmsSleep,
                    None,
                    HitAction::ToggleIdleSleepMode,
                );
                self.draw_chip_button(
                    cx + s2 + 10,
                    cy,
                    s2,
                    34,
                    "Sleep Mode: Black Screen Curtain",
                    self.config.idle_sleep_mode == DisplaySleepMode::BlackScreen,
                    None,
                    HitAction::ToggleIdleSleepMode,
                );
                cy += 44;

                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Prevent Idle Dim / Sleep During Video Playback",
                    "Keeps screen awake while video motion is actively detected",
                    self.config.idle_inhibit_on_video,
                    HitAction::ToggleIdleInhibitVideo,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Lock Screen Automatically on Display Sleep",
                    "Requires login password (or Win+L anytime) after screen powers off",
                    self.config.lock_on_sleep,
                    HitAction::ToggleLockOnSleep,
                );
                cy += 62;
                self.draw_chip_button(
                    cx,
                    cy,
                    card_w,
                    34,
                    "Lock Screen Now (Win+L / Ctrl+Alt+L)",
                    false,
                    None,
                    HitAction::LockScreenNow,
                );
                cy += 44;

                self.draw_section_header(cx, &mut cy, "Motion, Frame Pacing & Animations");
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Smooth Window & Popover Animations",
                    "GPU-friendly motion paced by hardware VBlank (turn off for instant snaps)",
                    self.config.animations_enabled,
                    HitAction::ToggleAnimations,
                );
                cy += 62;
                let sp_val = format!("{:.2}x", self.config.animation_speed);
                self.draw_slider_row(
                    cx,
                    cy,
                    "Animation Interpolation Speed",
                    &sp_val,
                    ((self.config.animation_speed - 0.10) / 0.55).clamp(0.0, 1.0),
                    SliderTarget::AnimationSpeed,
                );
                cy += 68;
            }
            SettingsTab::Multitasking => {
                self.draw_section_header(cx, &mut cy, "Workspace Tiling Mode");
                let m3 = (card_w - 20) / 3;
                for (i, mode) in [
                    LayoutMode::MasterStack,
                    LayoutMode::Columns,
                    LayoutMode::Floating,
                ]
                .into_iter()
                .enumerate()
                {
                    self.draw_chip_button(
                        cx + i as i32 * (m3 + 10),
                        cy,
                        m3,
                        34,
                        mode.label(),
                        self.config.layout_mode == mode,
                        None,
                        HitAction::SetLayoutMode(mode),
                    );
                }
                cy += 44;

                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Auto-Tile New Windows",
                    "Automatically arrange windows on spawn and restore floating rects on Expand/Shrink",
                    self.config.auto_tile,
                    HitAction::ToggleAutoTile,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Aero / Edge Drag Snap Zones",
                    "Drag any window to top edge to expand, or left/right edges for crisp split-screen",
                    self.config.edge_snap,
                    HitAction::ToggleEdgeSnap,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Focus Follows Mouse (Hover Without Raising)",
                    "Move pointer to focus typing/scroll without raising background windows over floating apps",
                    self.config.focus_follows_mouse,
                    HitAction::ToggleFocusFollowsMouse,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Windows-Style Shortcuts Enabled",
                    "Win tap = Start, Win+Arrows = Snap/Expand, Win+Shift+M = Next Monitor, Win+D/E/I, Alt+Tab",
                    self.config.windows_shortcuts,
                    HitAction::ToggleWindowsShortcuts,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Confine Pointer Inside Focused Games",
                    "Prevent cursor escaping to other monitors while playing; switch out with Alt+Tab or shortcuts",
                    self.config.game_pointer_lock,
                    HitAction::ToggleGamePointerLock,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Lock Taskbar in Fullscreen Games",
                    "Block accidental taskbar clicks/shortcuts in fullscreen (stays accessible when not fullscreen)",
                    self.config.lock_fullscreen_taskbar,
                    HitAction::ToggleLockFullscreenTaskbar,
                );
                cy += 64;

                self.draw_section_header(cx, &mut cy, "Focus & Unfocused Window Dimming");
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Dim Inactive Windows",
                    "Subtly dim background windows so your active window stands out cleanly across monitors",
                    self.config.dim_unfocused_windows,
                    HitAction::ToggleDimUnfocused,
                );
                cy += 62;
                let dim_lbl = format!("{}%", (self.config.unfocused_dim_opacity * 100.0).round() as i32);
                self.draw_slider_row(
                    cx,
                    cy,
                    "Unfocused Window Dim Intensity",
                    &dim_lbl,
                    (self.config.unfocused_dim_opacity / 0.60).clamp(0.0, 1.0),
                    SliderTarget::UnfocusedDimOpacity,
                );
                cy += 66;
                let alpha_lbl = format!(
                    "{}%",
                    (self.config.unfocused_window_opacity * 100.0).round() as i32
                );
                self.draw_slider_row(
                    cx,
                    cy,
                    "Unfocused Window Opacity",
                    &alpha_lbl,
                    ((self.config.unfocused_window_opacity - 0.65) / 0.35).clamp(0.0, 1.0),
                    SliderTarget::UnfocusedWindowAlpha,
                );
                cy += 66;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Keep Fullscreen Videos & Games Bright",
                    "Skip dimming fullscreen apps (e.g. YouTube or games on a second monitor)",
                    self.config.dim_exclude_fullscreen,
                    HitAction::ToggleDimExcludeFullscreen,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Freeze Off-Screen & Hidden Window Views",
                    "Pause redraws for minimized, other-workspace & behind-fullscreen windows (~1Hz app keepalive)",
                    self.config.freeze_hidden_windows,
                    HitAction::ToggleFreezeHidden,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Minimal View for Compact / Underscaled Windows",
                    "Show a clean compact card instead of glitching when windows shrink below safe size",
                    self.config.minimal_view_enabled,
                    HitAction::ToggleMinimalViewConfig,
                );
                cy += 64;

                let mr_val = format!("{}%", (self.config.master_ratio * 100.0).round() as i32);
                self.draw_slider_row(
                    cx,
                    cy,
                    "Master Pane Split Ratio",
                    &mr_val,
                    ((self.config.master_ratio - 0.35) / 0.40).clamp(0.0, 1.0),
                    SliderTarget::MasterRatio,
                );
                cy += 66;
                let gp_val = format!("{} px", self.config.gap_px);
                self.draw_slider_row(
                    cx,
                    cy,
                    "Outer & Tile Window Gaps",
                    &gp_val,
                    self.config.gap_px as f32 / 28.0,
                    SliderTarget::WindowGap,
                );
                cy += 70;

                self.draw_section_header(cx, &mut cy, "Multitasking Shortcut Reference");
                let lines = [
                    "Win (tap) -> Start Menu    |  Win+L -> Lock Screen  |  Win+X -> Power Menu",
                    "Win+Tab -> Task View       |  Win+Shift+M -> Move Window Next Monitor",
                    "Win+Left / Right -> Snap Half  |  Win+Up -> Expand   |  Win+Down -> Shrink / Minimize",
                    "Win+D -> Show Desktop      |  Win+E -> Files    |  Win+I -> Velox Settings",
                    "Alt+Tab -> Window Switcher |  Ctrl+Win+Left/Right -> Switch Workspace | Alt+F4 -> Close",
                ];
                let box_h = lines.len() as i32 * 20 + 18;
                fill_rounded_rect(&mut self.rgba_buf, w, h, cx, cy, card_w, box_h, 11, [18, 27, 46, 225]);
                for (idx, line) in lines.iter().enumerate() {
                    self.painter.draw_text(
                        &mut self.rgba_buf,
                        w,
                        h,
                        cx + 12,
                        cy + 10 + idx as i32 * 20,
                        card_w - 24,
                        line,
                        11.5,
                        [184, 202, 232, 240],
                    );
                }
                cy += box_h + 16;
            }
            SettingsTab::Audio => {
                let (out_vol, out_muted) = self.snapshot.default_output_vol();
                let (in_vol, in_muted) = self.snapshot.default_input_vol();

                self.draw_section_header(cx, &mut cy, "Master Output & Microphone Levels");
                let out_lbl = if out_muted {
                    format!("{out_vol}% (MUTED)")
                } else {
                    format!("{out_vol}%")
                };
                self.draw_slider_row(
                    cx,
                    cy,
                    "Default Output Volume",
                    &out_lbl,
                    out_vol as f32 / 100.0,
                    SliderTarget::MasterOutVolume,
                );
                cy += 64;
                let in_lbl = if in_muted {
                    format!("{in_vol}% (MUTED)")
                } else {
                    format!("{in_vol}%")
                };
                self.draw_slider_row(
                    cx,
                    cy,
                    "Default Microphone Input Gain",
                    &in_lbl,
                    in_vol as f32 / 100.0,
                    SliderTarget::MasterInVolume,
                );
                cy += 64;

                let m2 = (card_w - 10) / 2;
                let out_name = if self.snapshot.default_sink.is_empty() {
                    "@DEFAULT_SINK@".to_string()
                } else {
                    self.snapshot.default_sink.clone()
                };
                let in_name = if self.snapshot.default_source.is_empty() {
                    "@DEFAULT_SOURCE@".to_string()
                } else {
                    self.snapshot.default_source.clone()
                };
                self.draw_chip_button(
                    cx,
                    cy,
                    m2,
                    32,
                    if out_muted { "Unmute Speakers" } else { "Mute Speakers" },
                    out_muted,
                    None,
                    HitAction::ToggleOutMute(out_name),
                );
                self.draw_chip_button(
                    cx + m2 + 10,
                    cy,
                    m2,
                    32,
                    if in_muted { "Unmute Mic" } else { "Mute Mic" },
                    in_muted,
                    None,
                    HitAction::ToggleInMute(in_name),
                );
                cy += 42;

                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Auto-Route Apps by Role (Comms vs Playback)",
                    "Route Discord/Zoom calls to Comms headset and media players to Playback output",
                    self.config.auto_route_audio_roles,
                    HitAction::ToggleAutoRouteAudio,
                );
                cy += 64;

                self.draw_section_header(cx, &mut cy, "Output Devices, Volume & Role Assignment");
                self.draw_audio_device_cards(cx, &mut cy, card_w, false);
                if self.snapshot.sinks.iter().any(|s| !s.disabled) {
                    self.draw_chip_button(
                        cx,
                        cy,
                        210,
                        30,
                        "Clear Role Overrides",
                        false,
                        None,
                        HitAction::ClearRoles,
                    );
                    cy += 40;
                }

                self.draw_section_header(cx, &mut cy, "Input Devices (Microphones) & Gain");
                self.draw_audio_device_cards(cx, &mut cy, card_w, true);

                // Collect disabled output & input devices so users can re-enable them cleanly.
                let mut disabled_items: Vec<(bool, String, String)> = Vec::new();
                for name in &self.config.disabled_audio_sinks {
                    let desc = self
                        .snapshot
                        .sinks
                        .iter()
                        .find(|s| &s.name == name)
                        .map(|s| s.description.clone())
                        .unwrap_or_else(|| name.clone());
                    disabled_items.push((false, name.clone(), desc));
                }
                for name in &self.config.disabled_audio_sources {
                    let desc = self
                        .snapshot
                        .sources
                        .iter()
                        .find(|s| &s.name == name)
                        .map(|s| s.description.clone())
                        .unwrap_or_else(|| name.clone());
                    disabled_items.push((true, name.clone(), desc));
                }
                for s in &self.snapshot.sinks {
                    if s.disabled && !disabled_items.iter().any(|(inp, n, _)| !*inp && n == &s.name) {
                        disabled_items.push((false, s.name.clone(), s.description.clone()));
                    }
                }
                for s in &self.snapshot.sources {
                    if s.disabled && !disabled_items.iter().any(|(inp, n, _)| *inp && n == &s.name) {
                        disabled_items.push((true, s.name.clone(), s.description.clone()));
                    }
                }
                if !disabled_items.is_empty() {
                    self.draw_section_header(cx, &mut cy, "Disabled Audio Devices");
                    for (is_input, dev_name, dev_desc) in disabled_items {
                        fill_rounded_rect(&mut self.rgba_buf, w, h, cx, cy, card_w, 46, 10, [18, 26, 44, 215]);
                        let kind = if is_input { "Input Mic" } else { "Output" };
                        self.painter.draw_text(
                            &mut self.rgba_buf,
                            w,
                            h,
                            cx + 12,
                            cy + 15,
                            card_w - 130,
                            &format!("{dev_desc} ({kind} • Disabled)"),
                            12.5,
                            [165, 178, 206, 235],
                        );
                        self.draw_chip_button(
                            cx + card_w - 104,
                            cy + 8,
                            92,
                            30,
                            "Enable",
                            false,
                            Some([72, 226, 152]),
                            HitAction::ToggleAudioDisabled {
                                is_input,
                                name: dev_name,
                                disabled: false,
                            },
                        );
                        cy += 54;
                    }
                }

                self.draw_section_header(cx, &mut cy, "Per-App Output & Microphone Capture Mixer");
                if self.snapshot.sink_inputs.is_empty() && self.snapshot.source_outputs.is_empty() {
                    self.painter.draw_text(
                        &mut self.rgba_buf,
                        w,
                        h,
                        cx + 6,
                        cy,
                        card_w,
                        "No active audio playback or capture streams running",
                        12.5,
                        [165, 180, 208, 220],
                    );
                    cy += 28;
                } else {
                    for stream in self.snapshot.sink_inputs.clone() {
                        let role = if stream.is_comms { "Comms" } else { "Media" };
                        let dev_desc = self
                            .snapshot
                            .sinks
                            .iter()
                            .find(|s| s.index == stream.device_index)
                            .map(|s| s.description.clone())
                            .unwrap_or_else(|| "Output".to_string());
                        self.draw_stream_mixer_row(
                            cx,
                            &mut cy,
                            card_w,
                            false,
                            stream.index,
                            &format!("{} ({role})", stream.app_name),
                            stream.volume_pct,
                            stream.muted,
                            &dev_desc,
                        );
                    }
                    for stream in self.snapshot.source_outputs.clone() {
                        let dev_desc = self
                            .snapshot
                            .sources
                            .iter()
                            .find(|s| s.index == stream.device_index)
                            .map(|s| s.description.clone())
                            .unwrap_or_else(|| "Mic".to_string());
                        self.draw_stream_mixer_row(
                            cx,
                            &mut cy,
                            card_w,
                            true,
                            stream.index,
                            &format!("{} (Capture)", stream.app_name),
                            stream.volume_pct,
                            stream.muted,
                            &dev_desc,
                        );
                    }
                }
            }
            SettingsTab::NetworkPower => {
                self.draw_section_header(cx, &mut cy, "Wireless & Bluetooth");
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Wi-Fi Networking",
                    &format!("Status: {}", self.snapshot.network_label),
                    self.snapshot.wifi_enabled,
                    HitAction::ToggleWifi,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Bluetooth Radio",
                    if self.snapshot.bluetooth_on {
                        "Bluetooth controller powered ON"
                    } else {
                        "Bluetooth controller powered OFF"
                    },
                    self.snapshot.bluetooth_on,
                    HitAction::ToggleBluetooth,
                );
                cy += 64;

                self.draw_section_header(cx, &mut cy, "Display Brightness");
                let b_lbl = if self.snapshot.brightness_supported {
                    format!("{}%", self.snapshot.brightness_pct)
                } else {
                    format!("{}% (auto/DDC)", self.snapshot.brightness_pct)
                };
                self.draw_slider_row(
                    cx,
                    cy,
                    "Monitor / Backlight Brightness",
                    &b_lbl,
                    ((self.snapshot.brightness_pct as f32 - 5.0) / 95.0).clamp(0.0, 1.0),
                    SliderTarget::Brightness,
                );
                cy += 68;

                self.draw_section_header(cx, &mut cy, "System Performance Profile");
                let p3 = (card_w - 20) / 3;
                for (i, (id, lbl)) in [
                    ("power-saver", "Power Saver"),
                    ("balanced", "Balanced"),
                    ("performance", "Performance"),
                ]
                .into_iter()
                .enumerate()
                {
                    self.draw_chip_button(
                        cx + i as i32 * (p3 + 10),
                        cy,
                        p3,
                        36,
                        lbl,
                        self.snapshot.power_profile == id,
                        None,
                        HitAction::SetPowerProfile(id.to_string()),
                    );
                }
                cy += 50;
            }
            SettingsTab::Notifications => {
                self.draw_section_header(cx, &mut cy, "General & Interruptions");
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Do Not Disturb (DND)",
                    "Suppress popup banners except critical alerts & Priority Conversations",
                    self.config.notifications_dnd,
                    HitAction::ToggleDnd,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Allow Notification Snoozing",
                    "Show snooze action (15m reminder) on notification toasts and drawer cards",
                    self.config.allow_notification_snooze,
                    HitAction::ToggleNotifSnooze,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Hide Silent Notifications in Status Bar",
                    "Keep taskbar notification pill quiet when only silent or bundled alerts arrive",
                    self.config.hide_silent_notifications_in_status_bar,
                    HitAction::ToggleHideSilentStatusBar,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Notification Cooldown (Up to 1 Min)",
                    "When many arrive quickly, lowers volume & minimizes alerts; view received items in Notification pill",
                    self.config.notification_cooldown_enabled,
                    HitAction::ToggleNotifCooldown,
                );
                cy += 66;

                self.draw_section_header(cx, &mut cy, "Notification Organizer (On-Device AI)");
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Enable Notification Organizer",
                    "Uses AI on your device (may make mistakes). Notifications are never sent to Google.",
                    self.config.notification_organizer_enabled,
                    HitAction::ToggleNotifOrganizer,
                );
                cy += 62;
                if self.config.notification_organizer_enabled {
                    self.painter.draw_text(
                        &mut self.rgba_buf,
                        w,
                        h,
                        cx,
                        cy,
                        card_w,
                        "Choose notification types to silence & bundle together:",
                        12.0,
                        [184, 202, 232, 240],
                    );
                    cy += 22;
                    let cols = 4.min(ORGANIZER_SILENCEABLE_CATEGORIES.len()).max(2) as i32;
                    let bw = (card_w - (cols - 1) * 8) / cols;
                    for (i, cat) in ORGANIZER_SILENCEABLE_CATEGORIES.iter().enumerate() {
                        let active = self
                            .config
                            .organized_silenced_categories
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(cat));
                        let col_i = (i as i32) % cols;
                        let row_i = (i as i32) / cols;
                        self.draw_chip_button(
                            cx + col_i * (bw + 8),
                            cy + row_i * 40,
                            bw,
                            34,
                            cat,
                            active,
                            None,
                            HitAction::ToggleOrganizerSilenceCat((*cat).to_string()),
                        );
                    }
                    cy += 44;
                }

                self.draw_section_header(cx, &mut cy, "Notification Summaries (On-Device AI)");
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Summarize Long Messages & Group Conversations",
                    "Generates quick recaps locally. Summaries use AI on your device, which can make mistakes.",
                    self.config.notification_summaries_enabled,
                    HitAction::ToggleNotifSummaries,
                );
                cy += 66;

                self.draw_section_header(cx, &mut cy, "Conversations & Priority Contacts");
                let mut conv_keys = self.config.priority_conversations.clone();
                for entry in &self.notif_history {
                    if let Some(ref ck) = entry.conversation_key {
                        if !conv_keys.iter().any(|p| p.eq_ignore_ascii_case(ck)) {
                            conv_keys.push(ck.clone());
                        }
                    }
                }
                if conv_keys.is_empty() {
                    self.painter.draw_text(
                        &mut self.rgba_buf,
                        w,
                        h,
                        cx,
                        cy + 4,
                        card_w,
                        "No conversations recorded yet — click ★ in the Notification Drawer to mark Priority.",
                        12.0,
                        [162, 180, 214, 235],
                    );
                    cy += 34;
                } else {
                    for ck in conv_keys.into_iter().take(8) {
                        let is_pri = self
                            .config
                            .priority_conversations
                            .iter()
                            .any(|p| p.eq_ignore_ascii_case(&ck));
                        self.draw_toggle_pill(
                            cx,
                            cy,
                            &format!("★ {ck}"),
                            "Priority conversations bypass cooldown and stay pinned at the top",
                            is_pri,
                            HitAction::TogglePriorityConversation(ck),
                        );
                        cy += 58;
                    }
                }

                self.draw_section_header(cx, &mut cy, "Notification History");
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Record Notification History",
                    &format!("{} recent entries stored locally", self.notif_history.len()),
                    self.config.notification_history_enabled,
                    HitAction::ToggleNotifHistory,
                );
                cy += 62;
                self.draw_chip_button(
                    cx,
                    cy,
                    180,
                    32,
                    "Clear Saved History",
                    false,
                    None,
                    HitAction::ClearNotifHistory,
                );
                cy += 42;
                for h_item in self.notif_history.iter().take(5) {
                    fill_rounded_rect(&mut self.rgba_buf, w, h, cx, cy, card_w, 46, 8, [18, 25, 44, 224]);
                    let hdr = format!("{} • {} [{}]", h_item.app_name, h_item.category.label(), h_item.status);
                    self.painter.draw_text(&mut self.rgba_buf, w, h, cx + 12, cy + 7, card_w - 24, &hdr, 10.5, [154, 178, 214, 235]);
                    self.painter.draw_text(&mut self.rgba_buf, w, h, cx + 12, cy + 24, card_w - 24, &h_item.summary, 12.0, [238, 244, 255, 255]);
                    cy += 52;
                }

                self.draw_section_header(cx, &mut cy, "Per-App Notification Toggles");
                let mut app_names: Vec<String> = self.notif_seen_apps.clone();
                for dapp in &self.desktop_apps {
                    if !app_names.iter().any(|n| n.eq_ignore_ascii_case(&dapp.name)) {
                        app_names.push(dapp.name.clone());
                    }
                }
                for app_name in app_names.into_iter().take(24) {
                    let key = normalize_app_key(&app_name);
                    let disabled = self
                        .config
                        .disabled_notification_apps
                        .iter()
                        .any(|a| normalize_app_key(a) == key);
                    let silent = !disabled
                        && self
                            .config
                            .silent_notification_apps
                            .iter()
                            .any(|a| normalize_app_key(a) == key);
                    fill_rounded_rect(&mut self.rgba_buf, w, h, cx, cy, card_w, 44, 8, [20, 27, 46, 232]);
                    self.painter.draw_text(
                        &mut self.rgba_buf,
                        w,
                        h,
                        cx + 12,
                        cy + 14,
                        card_w - 190,
                        &app_name,
                        12.5,
                        [240, 246, 255, 255],
                    );
                    let sil_x = cx + card_w - 166;
                    self.draw_chip_button(
                        sil_x,
                        cy + 7,
                        74,
                        30,
                        if silent { "Silent" } else { "Normal" },
                        silent,
                        None,
                        HitAction::ToggleAppNotifSilent(app_name.clone()),
                    );
                    let ena_x = cx + card_w - 84;
                    self.draw_chip_button(
                        ena_x,
                        cy + 7,
                        74,
                        30,
                        if disabled { "Blocked" } else { "Allowed" },
                        !disabled,
                        None,
                        HitAction::ToggleAppNotifDisabled(app_name),
                    );
                    cy += 50;
                }
            }
            SettingsTab::AppsStartup => {
                self.draw_section_header(cx, &mut cy, "Default Terminal");
                self.draw_app_picker_row(
                    cx,
                    &mut cy,
                    card_w,
                    &["alacritty", "xfce4-terminal", "konsole", "kitty", "foot", "wezterm", "ghostty"],
                    &self.config.terminal_cmd.clone(),
                    |cmd| HitAction::SetTerminal(cmd),
                );
                self.draw_section_header(cx, &mut cy, "Default Web Browser");
                self.draw_app_picker_row(
                    cx,
                    &mut cy,
                    card_w,
                    &["zen-browser", "firefox", "chromium", "brave", "google-chrome"],
                    &self.config.browser_cmd.clone(),
                    |cmd| HitAction::SetBrowser(cmd),
                );
                self.draw_section_header(cx, &mut cy, "Default File Manager (Win+E)");
                self.draw_app_picker_row(
                    cx,
                    &mut cy,
                    card_w,
                    &["thunar", "nautilus", "dolphin", "nemo", "pcmanfm"],
                    &self.config.files_cmd.clone(),
                    |cmd| HitAction::SetFiles(cmd),
                );
                self.draw_section_header(cx, &mut cy, "Default External App Launcher");
                self.draw_app_picker_row(
                    cx,
                    &mut cy,
                    card_w,
                    &["fuzzel", "rofi", "wofi", "bemenu"],
                    &self.config.launcher_cmd.clone(),
                    |cmd| HitAction::SetLauncher(cmd),
                );

                self.draw_section_header(cx, &mut cy, "Login & Background Agents");
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Auto-Launch Terminal on Login",
                    "Open your preferred terminal immediately when entering Velox WM",
                    self.config.auto_launch_terminal_on_login,
                    HitAction::ToggleAutoTerminalLogin,
                );
                cy += 64;

                let agents = [
                    (
                        "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1",
                        "Polkit Auth Agent",
                    ),
                    ("nm-applet", "NetworkManager Tray Applet"),
                    ("dunst", "Dunst Notification Daemon"),
                    ("mako", "Mako Wayland Notifications"),
                ];
                for (cmd, label) in agents {
                    let active = self.config.autostart.iter().any(|c| c == cmd);
                    self.draw_toggle_pill(
                        cx,
                        cy,
                        label,
                        cmd,
                        active,
                        HitAction::ToggleAutostart(cmd.to_string()),
                    );
                    cy += 60;
                }

                self.draw_section_header(cx, &mut cy, "System Updates, Live Reload & Notifications");
                let repo_note = if self.config.github_update_repo.trim().is_empty() {
                    format!(
                        "Velox v{} • Set github_update_repo in config.toml to watch GitHub releases",
                        env!("CARGO_PKG_VERSION")
                    )
                } else {
                    format!(
                        "Velox v{} • GitHub repo: {}",
                        env!("CARGO_PKG_VERSION"),
                        self.config.github_update_repo
                    )
                };
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Notifications Do Not Disturb (DND)",
                    &repo_note,
                    self.config.notifications_dnd,
                    HitAction::ToggleDnd,
                );
                cy += 62;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Auto-Check GitHub Releases & Installer Binary",
                    "Watches /usr/local/bin/velox-wm (after install.sh) & GitHub releases for live reload notices",
                    self.config.check_github_updates,
                    HitAction::ToggleCheckGithubUpdates,
                );
                cy += 62;

                let u2 = (card_w - 10) / 2;
                self.draw_chip_button(
                    cx,
                    cy,
                    u2,
                    34,
                    "Check Updates Now",
                    false,
                    None,
                    HitAction::CheckUpdatesNow,
                );
                self.draw_chip_button(
                    cx + u2 + 10,
                    cy,
                    u2,
                    34,
                    "Reload WM Seamlessly",
                    true,
                    None,
                    HitAction::SeamlessReloadWm,
                );
                cy += 46;

                self.draw_app_library_editor(cx, &mut cy, card_w);
            }
            SettingsTab::Processes => {
                self.draw_section_header(cx, &mut cy, "Background App Manager (Single-Click Close)");
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Show System & Infrastructure Daemons",
                    "Keep off to show only clean user apps (auto-escalates SIGTERM -> SIGKILL cleanly)",
                    self.config.show_system_processes,
                    HitAction::ToggleShowSystemProcs,
                );
                cy += 62;
                self.draw_chip_button(
                    cx,
                    cy,
                    150,
                    30,
                    "Refresh Now",
                    false,
                    None,
                    HitAction::RefreshSnapshot,
                );
                cy += 38;

                if self.snapshot.bg_apps.is_empty() {
                    self.painter.draw_text(
                        &mut self.rgba_buf,
                        w,
                        h,
                        cx + 6,
                        cy + 8,
                        card_w,
                        "No background apps matched current filter",
                        13.0,
                        [165, 180, 210, 230],
                    );
                    cy += 34;
                } else {
                    for proc in self.snapshot.bg_apps.clone() {
                        fill_rounded_rect(
                            &mut self.rgba_buf,
                            w,
                            h,
                            cx,
                            cy,
                            card_w,
                            46,
                            9,
                            [20, 30, 52, 232],
                        );
                        let label = if proc.pids.len() > 1 {
                            format!(
                                "{}  ({} procs • {} MB)",
                                proc.name,
                                proc.pids.len(),
                                proc.rss_mb
                            )
                        } else {
                            format!(
                                "{}  (PID {} • {} MB)",
                                proc.name, proc.pid, proc.rss_mb
                            )
                        };
                        self.painter.draw_text(
                            &mut self.rgba_buf,
                            w,
                            h,
                            cx + 12,
                            cy + 15,
                            card_w - 178,
                            &label,
                            13.0,
                            [238, 244, 255, 255],
                        );
                        self.draw_chip_button(
                            cx + card_w - 160,
                            cy + 9,
                            72,
                            28,
                            "End",
                            false,
                            None,
                            HitAction::KillProcess {
                                pids: proc.pids.clone(),
                                force: false,
                            },
                        );
                        self.draw_chip_button(
                            cx + card_w - 82,
                            cy + 9,
                            72,
                            28,
                            "Kill!",
                            true,
                            None,
                            HitAction::KillProcess {
                                pids: proc.pids,
                                force: true,
                            },
                        );
                        cy += 52;
                    }
                }
            }
            SettingsTab::MemoryCache => {
                let m = self.snapshot.memory.clone();
                self.draw_section_header(cx, &mut cy, "Live RAM & Page-Cache Telemetry");
                fill_rounded_rect(
                    &mut self.rgba_buf,
                    w,
                    h,
                    cx,
                    cy,
                    card_w,
                    88,
                    12,
                    [20, 30, 52, 232],
                );
                stroke_rounded_rect(
                    &mut self.rgba_buf,
                    w,
                    h,
                    cx,
                    cy,
                    card_w,
                    88,
                    12,
                    1,
                    [accent[0], accent[1], accent[2], 140],
                );
                let l1 = format!(
                    "System Memory: {}% used  •  {} MB free / {} MB total",
                    m.used_pct, m.available_mb, m.total_mb
                );
                let l2 = format!(
                    "Linux Page Cache: {} MB  •  Reclaimable Slab: {} MB  •  Velox WM RSS: {} MB",
                    m.cached_mb, m.reclaimable_mb, m.wm_rss_mb
                );
                let l3 = format!(
                    "Auto-Clean Runs: {}  •  Last Reclaimed: {} MB  •  Hot Launch Binaries: {}",
                    m.cleanups_done, m.last_reclaimed_mb, m.prewarmed_count
                );
                self.painter.draw_text(
                    &mut self.rgba_buf,
                    w,
                    h,
                    cx + 14,
                    cy + 12,
                    card_w - 28,
                    &l1,
                    13.5,
                    [242, 248, 255, 255],
                );
                self.painter.draw_text(
                    &mut self.rgba_buf,
                    w,
                    h,
                    cx + 14,
                    cy + 36,
                    card_w - 28,
                    &l2,
                    11.5,
                    [168, 214, 198, 245],
                );
                self.painter.draw_text(
                    &mut self.rgba_buf,
                    w,
                    h,
                    cx + 14,
                    cy + 58,
                    card_w - 28,
                    &l3,
                    11.5,
                    [168, 192, 232, 240],
                );
                cy += 100;

                let b2 = (card_w - 12) / 2;
                self.draw_chip_button(
                    cx,
                    cy,
                    b2,
                    36,
                    "Trim App & Heap Caches Now",
                    true,
                    None,
                    HitAction::CleanRamCache { deep_kernel: false },
                );
                self.draw_chip_button(
                    cx + b2 + 12,
                    cy,
                    b2,
                    36,
                    "Deep Clean + Kernel Page Cache",
                    false,
                    None,
                    HitAction::CleanRamCache { deep_kernel: true },
                );
                cy += 48;

                self.draw_section_header(cx, &mut cy, "Automatic Hygiene & App Launch Cache");
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Auto-Clean Memory Cache Above Threshold",
                    "Trim glibc heap arenas, glyph/buffer caches & stale temp files automatically",
                    self.config.auto_clean_ram_cache,
                    HitAction::ToggleAutoCleanRam,
                );
                cy += 62;
                let thr_lbl = format!("{}%", self.config.ram_cache_threshold_pct);
                self.draw_slider_row(
                    cx,
                    cy,
                    "RAM Cleanup Trigger Threshold",
                    &thr_lbl,
                    ((self.config.ram_cache_threshold_pct as f32 - 55.0) / 37.0).clamp(0.0, 1.0),
                    SliderTarget::RamThresholdPct,
                );
                cy += 66;
                self.draw_toggle_pill(
                    cx,
                    cy,
                    "Seamless App Launch Pre-Warm Cache",
                    "Cache desktop app index & pre-fault frequent app binaries while RAM headroom is healthy",
                    self.config.app_prewarm_cache,
                    HitAction::ToggleAppPrewarm,
                );
                cy += 64;
            }
        }

        let total_content_bottom = cy + self.scroll_y;
        let visible_h = h - 18;
        self.max_scroll_y = (total_content_bottom - visible_h).max(0);
        if self.scroll_y > self.max_scroll_y {
            self.scroll_y = self.max_scroll_y;
        }

        // Clip out scrolled hit-regions tucked behind the titlebar, then paint the opaque titlebar on top.
        self.hit_regions.retain(|r| r.rect.y >= TITLEBAR_H);
        fill_rounded_rect(&mut self.rgba_buf, w, h, 0, 0, w, TITLEBAR_H, 0, [10, 15, 28, 255]);
        fill_rounded_rect(
            &mut self.rgba_buf,
            w,
            h,
            0,
            0,
            w,
            TITLEBAR_H,
            radius,
            [16, 24, 44, 250],
        );
        fill_rounded_rect(&mut self.rgba_buf, w, h, 0, TITLEBAR_H - 1, w, 1, 0, [74, 98, 144, 95]);
        fill_circle(
            &mut self.rgba_buf,
            w,
            h,
            24,
            TITLEBAR_H / 2,
            9,
            [accent[0], accent[1], accent[2], 245],
        );
        self.painter.draw_text(
            &mut self.rgba_buf,
            w,
            h,
            42,
            15,
            240,
            "Velox System Settings",
            15.5,
            [244, 248, 255, 255],
        );

        let badge_col = if self
            .last_saved_at
            .map(|t| t.elapsed() < Duration::from_secs(2))
            .unwrap_or(false)
        {
            [36, 185, 126, 225]
        } else {
            [42, 58, 90, 210]
        };
        let reset_x = w - 216;
        self.draw_chip_button(
            reset_x,
            10,
            94,
            28,
            "Defaults",
            false,
            None,
            HitAction::ResetDefaults,
        );

        let btn_y = 10;
        let btn_h = 28;
        let min_x = w - 114;
        let max_x = w - 78;
        let close_x = w - 42;
        self.draw_chip_button(min_x, btn_y, 30, btn_h, "_", false, None, HitAction::WindowMinimize);
        self.draw_chip_button(
            max_x,
            btn_y,
            30,
            btn_h,
            if self.maximized { "><" } else { "[]" },
            self.maximized,
            None,
            HitAction::WindowMaximizeToggle,
        );
        let close_rect = HitRect::new(close_x, btn_y, 30, btn_h);
        let close_bg = if self.hovered_rect(close_rect) {
            [225, 68, 92, 235]
        } else {
            [70, 34, 48, 220]
        };
        fill_rounded_rect(&mut self.rgba_buf, w, h, close_x, btn_y, 30, btn_h, 8, close_bg);
        self.painter
            .draw_text(&mut self.rgba_buf, w, h, close_x + 10, btn_y + 7, 20, "X", 12.5, [255, 240, 244, 255]);
        self.hit_regions.push(HitRegion {
            rect: close_rect,
            action: HitAction::WindowClose,
        });

        let status_max_w = (reset_x - 290).max(80);
        if status_max_w > 110 {
            fill_rounded_rect(
                &mut self.rgba_buf,
                w,
                h,
                274,
                11,
                status_max_w,
                26,
                8,
                badge_col,
            );
            self.painter.draw_text(
                &mut self.rgba_buf,
                w,
                h,
                284,
                17,
                status_max_w - 18,
                &self.status_note,
                11.5,
                [242, 248, 255, 255],
            );
        }

        // Re-stroke top outer border after titlebar draw
        stroke_rounded_rect(
            &mut self.rgba_buf,
            w,
            h,
            0,
            0,
            w,
            h,
            radius,
            1,
            [92, 126, 192, 128],
        );

        // Clean bottom-right resize grip indicator
        if !self.maximized {
            for i in 0..3 {
                let d = 6 + i * 4;
                fill_rounded_rect(
                    &mut self.rgba_buf,
                    w,
                    h,
                    w - d - 4,
                    h - 6,
                    2,
                    2,
                    0,
                    [150, 174, 214, 180],
                );
                fill_rounded_rect(
                    &mut self.rgba_buf,
                    w,
                    h,
                    w - 6,
                    h - d - 4,
                    2,
                    2,
                    0,
                    [150, 174, 214, 180],
                );
            }
        }

        // Double-check / 15-second keep-or-revert modal overlay for Display Settings changes.
        if let Some(modal) = self.display_modal.clone() {
            // Keep titlebar window controls clickable while blocking accidental background clicks.
            self.hit_regions.retain(|r| r.rect.y < TITLEBAR_H);
            fill_rounded_rect(&mut self.rgba_buf, w, h, 0, TITLEBAR_H, w, h - TITLEBAR_H, 0, [6, 10, 20, 190]);
            let mw = 460.min(w - 40).max(320);
            let mh = 200;
            let mx = (w - mw) / 2;
            let my = TITLEBAR_H + (h - TITLEBAR_H - mh) / 2;
            fill_rounded_rect(&mut self.rgba_buf, w, h, mx, my, mw, mh, 14, [16, 24, 44, 250]);
            stroke_rounded_rect(
                &mut self.rgba_buf,
                w,
                h,
                mx,
                my,
                mw,
                mh,
                14,
                2,
                [accent[0], accent[1], accent[2], 220],
            );

            let res_chg = if modal.pending_width > 0 && modal.pending_height > 0 {
                format!(
                    "{}x{} -> {}x{} ",
                    modal.prev_width, modal.prev_height, modal.pending_width, modal.pending_height
                )
            } else {
                String::new()
            };
            let pending_hz = if modal.pending_hz == 0 {
                "Auto Max".to_string()
            } else {
                format!("{} Hz", modal.pending_hz)
            };
            let prev_hz = if modal.prev_hz == 0 {
                "Auto Max".to_string()
            } else {
                format!("{} Hz", modal.prev_hz)
            };

            let title = if modal.monitor_name.is_empty() {
                "Apply Display Settings?".to_string()
            } else {
                format!("Apply {} Display Settings?", modal.monitor_name)
            };
            let body = format!(
                "{res_chg}{prev_hz} ({}) -> {pending_hz} ({})?",
                modal.prev_vrr.short_label(),
                modal.pending_vrr.short_label()
            );
            let ok_lbl = "Apply";
            let ok_act = HitAction::ConfirmDisplayModal;
            let cancel_lbl = "Cancel";

            self.painter.draw_text(
                &mut self.rgba_buf,
                w,
                h,
                mx + 22,
                my + 22,
                mw - 44,
                &title,
                16.5,
                [244, 248, 255, 255],
            );
            self.painter.draw_text(
                &mut self.rgba_buf,
                w,
                h,
                mx + 22,
                my + 58,
                mw - 44,
                &body,
                12.5,
                [188, 206, 236, 245],
            );
            let bw = (mw - 56) / 2;
            let by = my + mh - 56;
            self.draw_chip_button(mx + 20, by, bw, 36, ok_lbl, true, None, ok_act);
            self.draw_chip_button(
                mx + 20 + bw + 16,
                by,
                bw,
                36,
                cancel_lbl,
                false,
                None,
                HitAction::CancelRevertDisplayModal,
            );
        } else if let Some((ctx_id, raw_x, raw_y)) = self.app_ctx_menu.clone() {
            if self.active_tab == SettingsTab::AppsStartup {
                self.hit_regions.retain(|r| r.rect.y < TITLEBAR_H);
                let app_opt = self.desktop_apps.iter().find(|a| a.id == ctx_id).cloned();
                let is_term = app_opt.as_ref().is_some_and(|a| a.terminal);
                let is_hid = app_opt.as_ref().is_some_and(|a| a.hidden);
                let game_lbl = match app_opt.as_ref().and_then(|a| a.is_game_override) {
                    None => "Game Detect: Auto",
                    Some(true) => "Game Detect: Force ON",
                    Some(false) => "Game Detect: Force OFF",
                };
                let items: [(&str, HitAction, bool); 6] = [
                    ("Edit Properties Below", HitAction::AppCtxSelectForEdit(ctx_id.clone()), true),
                    ("Launch App", HitAction::AppCtxLaunch(ctx_id.clone()), false),
                    (
                        if is_term { "Run in Terminal: ON" } else { "Run in Terminal: OFF" },
                        HitAction::AppCtxToggleTerminal(ctx_id.clone()),
                        is_term,
                    ),
                    (game_lbl, HitAction::AppCtxCycleGame(ctx_id.clone()), false),
                    (
                        if is_hid { "Unhide in Library" } else { "Hide from Library" },
                        HitAction::AppCtxToggleHidden(ctx_id),
                        is_hid,
                    ),
                    ("Close Menu", HitAction::CloseAppCtxMenu, false),
                ];
                let mw = 236;
                let mh = items.len() as i32 * 32 + 16;
                let mx = raw_x.clamp(12, (w - mw - 12).max(12));
                let my = raw_y.clamp(TITLEBAR_H + 8, (h - mh - 12).max(TITLEBAR_H + 8));
                fill_rounded_rect(&mut self.rgba_buf, w, h, mx, my, mw, mh, 10, [14, 20, 38, 250]);
                stroke_rounded_rect(
                    &mut self.rgba_buf,
                    w,
                    h,
                    mx,
                    my,
                    mw,
                    mh,
                    10,
                    1,
                    [accent[0], accent[1], accent[2], 210],
                );
                for (idx, (lbl, act, active)) in items.into_iter().enumerate() {
                    let iy = my + 8 + idx as i32 * 32;
                    self.draw_chip_button(mx + 8, iy, mw - 16, 27, lbl, active, None, act);
                }
            } else {
                self.app_ctx_menu = None;
            }
        }

        // Mask any scrolled items that poked above the content viewport border before re-drawing titlebar region cleanly
        if content_top > 0 {
            let stride = w * 4;
            if let Ok((buffer, canvas)) = self.pool.create_buffer(
                w,
                h,
                stride,
                wl_shm::Format::Argb8888,
            ) {
                rgba_to_argb8888(&self.rgba_buf, canvas);
                let surf = self.window.wl_surface();
                surf.set_buffer_scale(1);
                surf.damage_buffer(0, 0, w, h);
                surf.frame(qh, surf.clone());
                let _ = buffer.attach_to(surf);
                surf.commit();
            }
        }
        self.needs_redraw = false;
    }

    fn maybe_auto_switch_app_search(&mut self, field: AppTextField) {
        if field != AppTextField::LibSearch || self.app_filter_category == AppCategory::All {
            return;
        }
        let q = self.app_filter_query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return;
        }
        let matches_cat = self.desktop_apps.iter().any(|app| {
            app.category == self.app_filter_category && Self::app_matches_query(app, &q)
        });
        if !matches_cat && self.desktop_apps.iter().any(|app| Self::app_matches_query(app, &q)) {
            self.app_filter_category = AppCategory::All;
            self.status_note = "No matches in category — auto-switched to All Apps".to_string();
        }
    }

    fn app_matches_query(app: &DesktopApp, q: &str) -> bool {
        if q.is_empty() {
            return true;
        }
        app.name.to_ascii_lowercase().contains(q)
            || app.id.to_ascii_lowercase().contains(q)
            || app.comment.to_ascii_lowercase().contains(q)
            || app.effective_category_label().to_ascii_lowercase().contains(q)
            || app.keywords.iter().any(|k| k.to_ascii_lowercase().contains(q))
            || app.exec_args.iter().any(|a| a.to_ascii_lowercase().contains(q))
    }

    fn draw_app_library_editor(&mut self, cx: i32, cy: &mut i32, card_w: i32) {
        let w = self.width as i32;
        let h = self.height as i32;
        let accent = self.config.accent_rgb();

        self.draw_section_header(cx, cy, "App Library, Categories & Detection Editor");
        let b3 = (card_w - 20) / 3;
        self.draw_chip_button(
            cx,
            *cy,
            b3,
            32,
            if self.show_new_app_form { "Close New App" } else { "+ Add Custom App" },
            self.show_new_app_form,
            None,
            HitAction::ToggleNewAppForm,
        );
        self.draw_chip_button(
            cx + b3 + 10,
            *cy,
            b3,
            32,
            if self.show_new_rule_form { "Close Cat Rule" } else { "+ Add Category Rule" },
            self.show_new_rule_form,
            None,
            HitAction::ToggleNewRuleForm,
        );
        self.draw_chip_button(
            cx + (b3 + 10) * 2,
            *cy,
            b3,
            32,
            "Rescan Library",
            false,
            None,
            HitAction::RescanAppLibraryNow,
        );
        *cy += 42;

        // Add Custom App inline form
        if self.show_new_app_form {
            fill_rounded_rect(&mut self.rgba_buf, w, h, cx, *cy, card_w, 336, 10, [18, 25, 44, 240]);
            stroke_rounded_rect(&mut self.rgba_buf, w, h, cx, *cy, card_w, 336, 10, 1, [accent[0], accent[1], accent[2], 180]);
            let fx = cx + 14;
            let fw = card_w - 28;
            let mut fy = *cy + 12;
            let half = (fw - 10) / 2;
            self.draw_text_input(fx, fy, half, "App Name", &self.new_app_name.clone(), "e.g. Steam Big Picture", AppTextField::NewAppName);
            self.draw_text_input(fx + half + 10, fy, half, "Command (Exec)", &self.new_app_exec.clone(), "e.g. steam -tenfoot", AppTextField::NewAppExec);
            fy += 54;
            self.draw_text_input(fx, fy, half, "Description / Comment", &self.new_app_comment.clone(), "Optional description", AppTextField::NewAppComment);
            self.draw_text_input(fx + half + 10, fy, half, "Custom Category Label", &self.new_app_custom_cat.clone(), "Optional custom tab", AppTextField::NewAppCustomCategory);
            fy += 54;
            self.draw_text_input(fx, fy, fw, "Keywords (comma-separated)", &self.new_app_keywords.clone(), "e.g. game, proton, sandbox", AppTextField::NewAppKeywords);
            fy += 54;
            self.painter.draw_text(&mut self.rgba_buf, w, h, fx, fy, fw, "Category:", 11.0, [172, 190, 224, 240]);
            fy += 18;
            let cols = 5i32;
            let cw = (fw - (cols - 1) * 6) / cols;
            for (i, &cat) in AppCategory::ASSIGNABLE.iter().enumerate() {
                let r = (i as i32) / cols;
                let c = (i as i32) % cols;
                self.draw_chip_button(
                    fx + c * (cw + 6),
                    fy + r * 30,
                    cw,
                    26,
                    cat.label(),
                    self.new_app_category == cat,
                    None,
                    HitAction::NewAppSetCategory(cat),
                );
            }
            fy += 66;
            self.draw_chip_button(
                fx,
                fy,
                half,
                32,
                if self.new_app_terminal { "Run in Terminal: YES" } else { "Run in Terminal: NO" },
                self.new_app_terminal,
                None,
                HitAction::NewAppToggleTerminal,
            );
            self.draw_chip_button(
                fx + half + 10,
                fy,
                half,
                32,
                "Create Custom App",
                true,
                None,
                HitAction::CreateCustomApp,
            );
            *cy += 348;
        }

        // Add Category Detection Rule inline form & active rules list
        if self.show_new_rule_form || !self.app_store.category_rules.is_empty() {
            if self.show_new_rule_form {
                fill_rounded_rect(&mut self.rgba_buf, w, h, cx, *cy, card_w, 220, 10, [18, 25, 44, 240]);
                stroke_rounded_rect(&mut self.rgba_buf, w, h, cx, *cy, card_w, 220, 10, 1, [accent[0], accent[1], accent[2], 180]);
                let fx = cx + 14;
                let fw = card_w - 28;
                let mut fy = *cy + 12;
                let half = (fw - 10) / 2;
                self.draw_text_input(fx, fy, half, "Category Name", &self.new_rule_category.clone(), "e.g. Emulators or Simulators", AppTextField::NewRuleCategory);
                self.draw_text_input(fx + half + 10, fy, half, "Detection Keywords (comma-separated)", &self.new_rule_keywords.clone(), "e.g. yuzu, pcsx2, transport", AppTextField::NewRuleKeywords);
                fy += 54;
                self.painter.draw_text(&mut self.rgba_buf, w, h, fx, fy, fw, "Optional Built-in Parent Group:", 11.0, [172, 190, 224, 240]);
                fy += 18;
                let cols = 5i32;
                let cw = (fw - (cols - 1) * 6) / cols;
                self.draw_chip_button(fx, fy, cw, 26, "Custom Only", self.new_rule_builtin.is_none(), None, HitAction::NewRuleSetBuiltinCat(None));
                for (i, &cat) in AppCategory::ASSIGNABLE.iter().take(4).enumerate() {
                    self.draw_chip_button(
                        fx + ((i as i32) + 1) * (cw + 6),
                        fy,
                        cw,
                        26,
                        cat.label(),
                        self.new_rule_builtin == Some(cat),
                        None,
                        HitAction::NewRuleSetBuiltinCat(Some(cat)),
                    );
                }
                fy += 34;
                for (i, &cat) in AppCategory::ASSIGNABLE.iter().skip(4).enumerate() {
                    self.draw_chip_button(
                        fx + (i as i32) * (cw + 6),
                        fy,
                        cw,
                        26,
                        cat.label(),
                        self.new_rule_builtin == Some(cat),
                        None,
                        HitAction::NewRuleSetBuiltinCat(Some(cat)),
                    );
                }
                fy += 36;
                self.draw_chip_button(fx, fy, fw, 32, "Save Category Detection Rule", true, None, HitAction::CreateCategoryRule);
                *cy += 232;
            }
            for (idx, rule) in self.app_store.category_rules.clone().into_iter().enumerate() {
                fill_rounded_rect(&mut self.rgba_buf, w, h, cx, *cy, card_w, 36, 8, [20, 27, 46, 225]);
                let txt = format!("Rule: {}  ←  {}", rule.category_name, rule.keywords.join(", "));
                self.painter.draw_text(&mut self.rgba_buf, w, h, cx + 12, *cy + 11, card_w - 104, &txt, 11.5, [224, 236, 255, 245]);
                self.draw_chip_button(cx + card_w - 84, *cy + 4, 74, 28, "Delete", false, None, HitAction::DeleteCategoryRule(idx));
                *cy += 42;
            }
        }

        // Filter search + category row
        let search_w = if self.app_filter_query.is_empty() { card_w } else { card_w - 78 };
        self.draw_text_input(
            cx,
            *cy,
            search_w,
            "Search Installed & Custom Apps (click to type)",
            &self.app_filter_query.clone(),
            "Filter by name, command, keyword, category...",
            AppTextField::LibSearch,
        );
        if !self.app_filter_query.is_empty() {
            self.draw_chip_button(cx + card_w - 70, *cy + 17, 70, 30, "Clear", false, None, HitAction::ClearAppLibSearch);
        }
        *cy += 56;

        let cat_cols = 5i32;
        let cat_bw = (card_w - (cat_cols - 1) * 6) / cat_cols;
        for (i, &cat) in AppCategory::TABS.iter().enumerate() {
            let r = (i as i32) / cat_cols;
            let c = (i as i32) % cat_cols;
            self.draw_chip_button(
                cx + c * (cat_bw + 6),
                *cy + r * 32,
                cat_bw,
                26,
                cat.label(),
                self.app_filter_category == cat,
                None,
                HitAction::AppLibFilterCategory(cat),
            );
        }
        *cy += ((AppCategory::TABS.len() as i32 + cat_cols - 1) / cat_cols) * 32 + 6;

        let q = self.app_filter_query.trim().to_ascii_lowercase();
        let filtered: Vec<DesktopApp> = self
            .desktop_apps
            .iter()
            .filter(|app| {
                let cat_ok = self.app_filter_category == AppCategory::All
                    || app.category == self.app_filter_category;
                cat_ok && Self::app_matches_query(app, &q)
            })
            .cloned()
            .collect();

        if self.app_filter_category != AppCategory::All && !q.is_empty() {
            let all_cnt = self
                .desktop_apps
                .iter()
                .filter(|app| Self::app_matches_query(app, &q))
                .count();
            if all_cnt > filtered.len() {
                self.draw_chip_button(
                    cx,
                    *cy,
                    card_w,
                    30,
                    &format!("Search in All Categories ({all_cnt})"),
                    true,
                    None,
                    HitAction::AppLibSearchAllCategories,
                );
                *cy += 38;
            }
        }

        if filtered.is_empty() {
            self.painter.draw_text(
                &mut self.rgba_buf,
                w,
                h,
                cx + 4,
                *cy + 4,
                card_w - 8,
                "No apps match current filter.",
                12.0,
                [162, 178, 212, 235],
            );
            *cy += 32;
        } else {
            for app in filtered.iter().take(7) {
                let sel = self.selected_app_id.as_deref() == Some(app.id.as_str());
                let row_rect = HitRect::new(cx, *cy, card_w, 46);
                self.app_row_rects.push((row_rect, app.id.clone()));
                let bg = if sel {
                    [accent[0], accent[1], accent[2], 78]
                } else {
                    [19, 26, 44, 228]
                };
                fill_rounded_rect(&mut self.rgba_buf, w, h, row_rect.x, row_rect.y, row_rect.w, row_rect.h, 8, bg);
                if sel {
                    stroke_rounded_rect(&mut self.rgba_buf, w, h, row_rect.x, row_rect.y, row_rect.w, row_rect.h, 8, 1, [accent[0], accent[1], accent[2], 210]);
                }
                let bc = app.badge_color;
                fill_rounded_rect(&mut self.rgba_buf, w, h, cx + 10, *cy + 11, 24, 24, 6, [bc[0], bc[1], bc[2], 240]);
                if app.is_terminal_app() {
                    self.painter.draw_text(&mut self.rgba_buf, w, h, cx + 14, *cy + 17, 20, ">_", 10.0, [246, 250, 255, 255]);
                }
                let term_tag = if app.is_terminal_app() { " · Term" } else { "" };
                let title = format!(
                    "{}{}{}",
                    app.name,
                    term_tag,
                    if app.hidden { " [Hidden]" } else { "" }
                );
                let sub = format!(
                    "{}  •  {}  (right-click for quick edit)",
                    app.effective_category_label(),
                    app.exec_args.join(" ")
                );
                self.painter.draw_text(&mut self.rgba_buf, w, h, cx + 44, *cy + 7, card_w - 60, &title, 12.5, [242, 247, 255, 255]);
                self.painter.draw_text(&mut self.rgba_buf, w, h, cx + 44, *cy + 25, card_w - 60, &sub, 10.5, [152, 174, 212, 235]);
                self.hit_regions.push(HitRegion {
                    rect: row_rect,
                    action: HitAction::SelectAppForEdit(app.id.clone()),
                });
                *cy += 52;
            }
        }

        // Selected App Full Field Editor
        if let Some(sel_id) = self.selected_app_id.clone() {
            let is_custom = self
                .desktop_apps
                .iter()
                .find(|a| a.id == sel_id)
                .is_some_and(|a| a.is_custom);
            self.draw_section_header(cx, cy, &format!("Edit App Values ({sel_id})"));
            let half = (card_w - 10) / 2;
            self.draw_text_input(cx, *cy, half, "Name", &self.edit_name.clone(), "Display Name", AppTextField::EditName);
            self.draw_text_input(cx + half + 10, *cy, half, "Command (Exec)", &self.edit_exec.clone(), "Executable + flags", AppTextField::EditExec);
            *cy += 56;
            self.draw_text_input(cx, *cy, half, "Comment / Description", &self.edit_comment.clone(), "App subtitle", AppTextField::EditComment);
            self.draw_text_input(cx + half + 10, *cy, half, "Custom Category Label", &self.edit_custom_cat.clone(), "Leave blank for built-in", AppTextField::EditCustomCategory);
            *cy += 56;
            self.draw_text_input(cx, *cy, half, "Keywords (comma-separated)", &self.edit_keywords.clone(), "tag1, tag2", AppTextField::EditKeywords);
            self.draw_text_input(cx + half + 10, *cy, half, "Badge Color (#RRGGBB)", &self.edit_badge_hex.clone(), "#4287F5", AppTextField::EditBadgeHex);
            *cy += 56;

            // Quick color swatches
            self.painter.draw_text(&mut self.rgba_buf, w, h, cx, *cy, card_w, "Badge Color Presets:", 11.0, [170, 188, 222, 240]);
            *cy += 18;
            let sw = (card_w - 7 * 6) / 8;
            for (i, (cname, rgb)) in ACCENT_PRESETS.iter().enumerate() {
                self.draw_chip_button(
                    cx + (i as i32) * (sw + 6),
                    *cy,
                    sw,
                    26,
                    cname,
                    self.edit_badge_color == *rgb,
                    Some(*rgb),
                    HitAction::EditSetBadgeColor(*rgb),
                );
            }
            *cy += 34;

            self.painter.draw_text(&mut self.rgba_buf, w, h, cx, *cy, card_w, "Assigned Category:", 11.0, [170, 188, 222, 240]);
            *cy += 18;
            let e_cols = 5i32;
            let e_cw = (card_w - (e_cols - 1) * 6) / e_cols;
            for (i, &cat) in AppCategory::ASSIGNABLE.iter().enumerate() {
                let r = (i as i32) / e_cols;
                let c = (i as i32) % e_cols;
                self.draw_chip_button(
                    cx + c * (e_cw + 6),
                    *cy + r * 30,
                    e_cw,
                    26,
                    cat.label(),
                    self.edit_category == cat,
                    None,
                    HitAction::EditSetCategory(cat),
                );
            }
            *cy += 66;

            let c3 = (card_w - 16) / 3;
            self.draw_chip_button(
                cx,
                *cy,
                c3,
                30,
                if self.edit_terminal { "Terminal: YES" } else { "Terminal: NO" },
                self.edit_terminal,
                None,
                HitAction::EditToggleTerminal,
            );
            self.draw_chip_button(
                cx + c3 + 8,
                *cy,
                c3,
                30,
                if self.edit_hidden { "Visibility: Hidden" } else { "Visibility: Visible" },
                self.edit_hidden,
                None,
                HitAction::EditToggleHidden,
            );
            let gm_lbl = match self.edit_game_override {
                None => "Game Detect: Auto",
                Some(true) => "Game Detect: Force ON",
                Some(false) => "Game Detect: Force OFF",
            };
            self.draw_chip_button(
                cx + (c3 + 8) * 2,
                *cy,
                c3,
                30,
                gm_lbl,
                self.edit_game_override.is_some(),
                None,
                HitAction::EditCycleGameOverride,
            );
            *cy += 38;

            let half2 = (card_w - 10) / 2;
            self.draw_chip_button(cx, *cy, half2, 34, "Save App Values", true, None, HitAction::SaveAppEdit);
            if is_custom {
                self.draw_chip_button(cx + half2 + 10, *cy, half2, 34, "Delete Custom App", false, None, HitAction::DeleteCustomApp(sel_id));
            } else {
                self.draw_chip_button(cx + half2 + 10, *cy, half2, 34, "Reset to Auto", false, None, HitAction::ResetAppOverride);
            }
            *cy += 44;
        }
    }

    fn draw_section_header(&mut self, x: i32, y: &mut i32, title: &str) {
        let w = self.width as i32;
        let h = self.height as i32;
        self.painter.draw_text(
            &mut self.rgba_buf,
            w,
            h,
            x + 2,
            *y,
            (w - SIDEBAR_W - 60).max(200),
            title,
            13.5,
            [176, 198, 236, 250],
        );
        *y += 24;
    }

    fn draw_audio_device_cards(&mut self, cx: i32, cy: &mut i32, card_w: i32, is_input: bool) {
        let w = self.width as i32;
        let h = self.height as i32;
        let accent = self.config.accent_rgb();
        let devices: Vec<AudioDevice> = if is_input {
            self.snapshot
                .sources
                .iter()
                .filter(|d| !d.disabled)
                .cloned()
                .collect()
        } else {
            self.snapshot
                .sinks
                .iter()
                .filter(|d| !d.disabled)
                .cloned()
                .collect()
        };
        if devices.is_empty() {
            let empty_msg = if is_input {
                "No active PipeWire/PulseAudio input microphones available"
            } else {
                "No active PipeWire/PulseAudio output devices available"
            };
            self.painter.draw_text(
                &mut self.rgba_buf,
                w,
                h,
                cx + 6,
                *cy,
                card_w,
                empty_msg,
                12.5,
                [165, 180, 208, 220],
            );
            *cy += 28;
            return;
        }
        for dev in devices {
            let is_comms = !is_input && self.config.comms_output_device.as_deref() == Some(&dev.name);
            let is_media = !is_input && self.config.media_output_device.as_deref() == Some(&dev.name);
            fill_rounded_rect(
                &mut self.rgba_buf,
                w,
                h,
                cx,
                *cy,
                card_w,
                110,
                10,
                [20, 30, 52, 232],
            );
            stroke_rounded_rect(
                &mut self.rgba_buf,
                w,
                h,
                cx,
                *cy,
                card_w,
                110,
                10,
                1,
                if dev.is_default {
                    [accent[0], accent[1], accent[2], 140]
                } else {
                    [72, 92, 132, 95]
                },
            );
            let title = if dev.description.is_empty() {
                dev.name.as_str()
            } else {
                dev.description.as_str()
            };
            let mut tags = vec![format!("{}%", dev.volume_pct)];
            if dev.muted {
                tags.push("MUTED".to_string());
            }
            if dev.is_default {
                tags.push("Default".to_string());
            }
            if is_comms {
                tags.push("Comms".to_string());
            }
            if is_media {
                tags.push("Media".to_string());
            }
            let status = tags.join(" • ");
            let status_w = self.painter.measure_text(&status, 12.0).min(220);
            self.painter.draw_text(
                &mut self.rgba_buf,
                w,
                h,
                cx + 12,
                *cy + 9,
                (card_w - status_w - 30).max(120),
                title,
                13.0,
                [238, 244, 255, 255],
            );
            self.painter.draw_text(
                &mut self.rgba_buf,
                w,
                h,
                cx + card_w - status_w - 12,
                *cy + 10,
                status_w + 4,
                &status,
                12.0,
                if dev.muted {
                    [242, 128, 132, 255]
                } else {
                    [accent[0], accent[1], accent[2], 255]
                },
            );
            let slider_target = if is_input {
                SliderTarget::SourceVolume { index: dev.index }
            } else {
                SliderTarget::SinkVolume { index: dev.index }
            };
            self.draw_inline_slider_track(
                cx + 12,
                *cy + 36,
                card_w - 24,
                dev.volume_pct as f32 / 100.0,
                slider_target,
            );

            let mut buttons: Vec<(&str, bool, HitAction)> = vec![
                (
                    "Default",
                    dev.is_default,
                    if is_input {
                        HitAction::SetDefaultIn(dev.name.clone())
                    } else {
                        HitAction::SetDefaultOut(dev.name.clone())
                    },
                ),
                (
                    if dev.muted { "Unmute" } else { "Mute" },
                    dev.muted,
                    if is_input {
                        HitAction::ToggleInMute(dev.name.clone())
                    } else {
                        HitAction::ToggleOutMute(dev.name.clone())
                    },
                ),
            ];
            if !is_input {
                buttons.push(("Comms", is_comms, HitAction::SetCommsRole(dev.name.clone())));
                buttons.push(("Playback", is_media, HitAction::SetMediaRole(dev.name.clone())));
            }
            buttons.push((
                "Disable",
                false,
                HitAction::ToggleAudioDisabled {
                    is_input,
                    name: dev.name.clone(),
                    disabled: true,
                },
            ));
            let gap = 8;
            let count = buttons.len() as i32;
            let bw = (card_w - 24 - gap * (count - 1)) / count;
            for (i, (lbl, selected, act)) in buttons.into_iter().enumerate() {
                self.draw_chip_button(
                    cx + 12 + i as i32 * (bw + gap),
                    *cy + 64,
                    bw,
                    32,
                    lbl,
                    selected,
                    None,
                    act,
                );
            }
            *cy += 118;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_stream_mixer_row(
        &mut self,
        cx: i32,
        cy: &mut i32,
        card_w: i32,
        is_input: bool,
        index: u32,
        title: &str,
        volume_pct: u8,
        muted: bool,
        dev_desc: &str,
    ) {
        let pct_lbl = if muted {
            format!("{volume_pct}% (MUTED)")
        } else {
            format!("{volume_pct}%")
        };
        let target = if is_input {
            SliderTarget::CaptureStreamVolume { index }
        } else {
            SliderTarget::StreamVolume { index }
        };
        self.draw_slider_row(cx, *cy, title, &pct_lbl, volume_pct as f32 / 100.0, target);
        *cy += 62;
        let half = (card_w - 10) / 2;
        self.draw_chip_button(
            cx,
            *cy,
            half,
            28,
            if muted { "Unmute Stream" } else { "Mute Stream" },
            muted,
            None,
            HitAction::ToggleStreamMute { is_input, index },
        );
        let route_prefix = if is_input { "Mic" } else { "Out" };
        let short_dev: String = dev_desc.chars().take(18).collect();
        self.draw_chip_button(
            cx + half + 10,
            *cy,
            half,
            28,
            &format!("{route_prefix}: {short_dev}"),
            false,
            None,
            HitAction::CycleStreamDevice { is_input, index },
        );
        *cy += 38;
    }

    fn draw_app_picker_row<F>(
        &mut self,
        x: i32,
        y: &mut i32,
        card_w: i32,
        candidates: &[&str],
        current: &str,
        make_action: F,
    ) where
        F: Fn(String) -> HitAction,
    {
        let cols = ((card_w + 8) / 136).max(2);
        let cw = (card_w - (cols - 1) * 8) / cols;
        for (i, bin) in candidates.iter().enumerate() {
            let row = (i as i32) / cols;
            let col = (i as i32) % cols;
            let installed = executable_exists(bin);
            let label = if installed {
                (*bin).to_string()
            } else {
                format!("{bin} (n/a)")
            };
            self.draw_chip_button(
                x + col * (cw + 8),
                *y + row * 38,
                cw,
                30,
                &label,
                current == *bin,
                Some(if installed {
                    [52, 211, 153]
                } else {
                    [100, 114, 140]
                }),
                make_action((*bin).to_string()),
            );
        }
        let rows = ((candidates.len() as i32 + cols - 1) / cols).max(1);
        *y += rows * 38 + 8;
    }
}

impl CompositorHandler for SettingsApp {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        self.scale = new_factor.max(1);
        self.needs_redraw = true;
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if self.needs_redraw {
            self.render_ui(qh);
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for SettingsApp {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        self.needs_redraw = true;
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        self.needs_redraw = true;
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        self.needs_redraw = true;
    }
}

impl ShmHandler for SettingsApp {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl SeatHandler for SettingsApp {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        if self.seat.is_none() {
            self.seat = Some(seat);
        }
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            if let Ok(ptr) = self.seat_state.get_pointer(qh, &seat) {
                self.pointer = Some(ptr);
                self.seat = Some(seat.clone());
            }
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            let kbd = seat.get_keyboard(qh, ());
            self.keyboard = Some(kbd);
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(ptr) = self.pointer.take() {
                ptr.release();
            }
        }
        if capability == Capability::Keyboard {
            if let Some(kbd) = self.keyboard.take() {
                kbd.release();
            }
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}
}

impl PointerHandler for SettingsApp {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let w = self.width as i32;
        let h = self.height as i32;

        for event in events {
            let px = event.position.0.round() as i32;
            let py = event.position.1.round() as i32;
            self.pointer_x = px;
            self.pointer_y = py;

            match event.kind {
                PointerEventKind::Motion { .. } | PointerEventKind::Enter { .. } => {
                    if let Some((target, rect)) = self.active_drag {
                        self.apply_slider_at(target, rect, px, false);
                    } else {
                        let new_hover = self.hit_regions.iter().rposition(|r| r.rect.contains(px, py));
                        if new_hover != self.hovered_idx {
                            self.hovered_idx = new_hover;
                            self.needs_redraw = true;
                        }
                    }
                }
                PointerEventKind::Leave { .. } => {
                    self.hovered_idx = None;
                    self.needs_redraw = true;
                }
                PointerEventKind::Press {
                    button, ..
                } if button == BTN_RIGHT => {
                    if let Some((_, app_id)) = self
                        .app_row_rects
                        .iter()
                        .rev()
                        .find(|(r, _)| r.contains(px, py))
                        .cloned()
                    {
                        self.select_app_for_edit(&app_id);
                        self.execute_action(HitAction::OpenAppCtxMenu {
                            app_id,
                            x: px,
                            y: py,
                        });
                    } else if self.app_ctx_menu.take().is_some() {
                        self.needs_redraw = true;
                    }
                }
                PointerEventKind::Press {
                    button, serial, ..
                } if button == BTN_LEFT => {
                    if let Some(hit) = self
                        .hit_regions
                        .iter()
                        .rev()
                        .find(|r| r.rect.contains(px, py))
                        .cloned()
                    {
                        match hit.action {
                            HitAction::SliderTrack { target, rect } => {
                                self.active_drag = Some((target, rect));
                                self.apply_slider_at(target, rect, px, false);
                            }
                            other => self.execute_action(other),
                        }
                    } else if self.app_ctx_menu.take().is_some() {
                        self.needs_redraw = true;
                    } else if px >= w - 18 && py >= h - 18 && !self.maximized {
                        if let Some(ref seat) = self.seat {
                            self.window.resize(seat, serial, ResizeEdge::BottomRight);
                        }
                    } else if py < TITLEBAR_H {
                        if let Some(ref seat) = self.seat {
                            self.window.move_(seat, serial);
                        }
                    }
                }
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    if self.active_drag.take().is_some() && self.dirty_config {
                        self.save_config("Slider adjusted");
                    }
                }
                PointerEventKind::Axis { vertical, .. } => {
                    let delta = if vertical.discrete != 0 {
                        vertical.discrete * 36
                    } else {
                        vertical.absolute.round() as i32
                    };
                    let next = (self.scroll_y + delta).clamp(0, self.max_scroll_y);
                    if next != self.scroll_y {
                        self.scroll_y = next;
                        self.needs_redraw = true;
                    }
                }
                _ => {}
            }
        }

        if self.needs_redraw {
            self.render_ui(qh);
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for SettingsApp {
    fn event(
        app: &mut Self,
        _proxy: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                if let WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) = format {
                    if let Ok(Some(keymap)) = unsafe {
                        xkb::Keymap::new_from_fd(
                            &app.xkb_context,
                            fd,
                            size as usize,
                            xkb::KEYMAP_FORMAT_TEXT_V1,
                            xkb::KEYMAP_COMPILE_NO_FLAGS,
                        )
                    } {
                        app.xkb_state = Some(xkb::State::new(&keymap));
                    }
                }
            }
            wl_keyboard::Event::Leave { .. } => {
                app.active_text_field = None;
                app.needs_redraw = true;
                app.render_ui(qh);
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(ref mut xkb_st) = app.xkb_state {
                    xkb_st.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            }
            wl_keyboard::Event::Key { key, state, .. } => {
                if state != WEnum::Value(wl_keyboard::KeyState::Pressed) {
                    return;
                }
                let Some(field) = app.active_text_field else {
                    return;
                };
                let Some(ref xkb_st) = app.xkb_state else {
                    return;
                };
                if xkb_st.mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE)
                    || xkb_st.mod_name_is_active(xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE)
                    || xkb_st.mod_name_is_active(xkb::MOD_NAME_LOGO, xkb::STATE_MODS_EFFECTIVE)
                {
                    return;
                }
                let keycode = xkb::Keycode::new(key + 8);
                let sym = xkb_st.key_get_one_sym(keycode);
                if sym == xkb::Keysym::Escape
                    || sym == xkb::Keysym::Return
                    || sym == xkb::Keysym::KP_Enter
                {
                    app.active_text_field = None;
                    app.needs_redraw = true;
                    app.render_ui(qh);
                    return;
                }
                if sym == xkb::Keysym::BackSpace {
                    app.text_field_mut(field).pop();
                    app.maybe_auto_switch_app_search(field);
                    app.needs_redraw = true;
                    app.render_ui(qh);
                    return;
                }
                let utf8 = xkb_st.key_get_utf8(keycode);
                if !utf8.is_empty() && !utf8.chars().any(char::is_control) {
                    app.text_field_mut(field).push_str(&utf8);
                    app.maybe_auto_switch_app_search(field);
                    app.needs_redraw = true;
                    app.render_ui(qh);
                }
            }
            _ => {}
        }
    }
}

impl WindowHandler for SettingsApp {
    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
        self.loop_signal.stop();
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let new_w = configure.new_size.0.map(|v| v.get()).unwrap_or(self.width).max(760);
        let new_h = configure.new_size.1.map(|v| v.get()).unwrap_or(self.height).max(520);
        let size_changed = new_w != self.width || new_h != self.height;
        self.width = new_w;
        self.height = new_h;
        self.maximized = configure.is_maximized();
        self.configured = true;
        if size_changed {
            self.scroll_y = self.scroll_y.clamp(0, self.max_scroll_y);
        }
        self.render_ui(qh);
    }
}

impl ProvidesRegistryState for SettingsApp {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers!(OutputState, SeatState);
}

delegate_compositor!(SettingsApp);
delegate_output!(SettingsApp);
delegate_shm!(SettingsApp);
delegate_seat!(SettingsApp);
delegate_pointer!(SettingsApp);
delegate_xdg_shell!(SettingsApp);
delegate_xdg_window!(SettingsApp);
delegate_registry!(SettingsApp);

fn main() {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("velox-settings: failed to connect to Wayland display: {err}");
            std::process::exit(1);
        }
    };

    let (globals, event_queue) = registry_queue_init(&conn).expect("registry init");
    let qh = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh).expect("wl_compositor");
    let xdg_shell = XdgShell::bind(&globals, &qh).expect("xdg_wm_base");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm");

    let width = 980u32;
    let height = 660u32;
    let pool = SlotPool::new((width * height * 4) as usize, &shm).expect("shm SlotPool");

    let surface = compositor_state.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::None, &qh);
    window.set_title("Velox System Settings");
    window.set_app_id("velox-settings");
    window.set_min_size(Some((760, 520)));
    window.commit();

    let mut event_loop: EventLoop<SettingsApp> =
        EventLoop::try_new().expect("create calloop EventLoop");
    let loop_handle = event_loop.handle();
    let loop_signal = event_loop.get_signal();

    let config = Config::load_or_create();
    let sys_ctrl = SystemController::start(
        config.comms_output_device.clone(),
        config.media_output_device.clone(),
        config.auto_route_audio_roles,
    );
    sys_ctrl.send(SysCommand::SetShowSystemProcesses {
        enabled: config.show_system_processes,
    });
    sys_ctrl.send(SysCommand::SetDisabledAudioLists {
        sinks: config.disabled_audio_sinks.clone(),
        sources: config.disabled_audio_sources.clone(),
    });
    let snapshot = sys_ctrl.snapshot();
    let app_store = AppLibraryStore::load_or_default();
    let desktop_apps = scan_desktop_apps_including_hidden();
    let initial_app_id = desktop_apps.first().map(|a| a.id.clone());
    let (notif_history, notif_seen_apps) = load_notification_store();

    let mut app = SettingsApp {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        loop_signal,
        pool,
        window,
        pointer: None,
        keyboard: None,
        seat: None,
        xkb_context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
        xkb_state: None,
        width,
        height,
        scale: 1,
        configured: false,
        maximized: false,
        needs_redraw: true,
        pointer_x: 0,
        pointer_y: 0,
        hovered_idx: None,
        active_drag: None,
        scroll_y: 0,
        max_scroll_y: 0,
        active_tab: SettingsTab::Personalize,
        config,
        desktop_apps,
        app_store,
        active_text_field: None,
        app_filter_query: String::new(),
        app_filter_category: AppCategory::All,
        selected_app_id: None,
        edit_name: String::new(),
        edit_comment: String::new(),
        edit_exec: String::new(),
        edit_keywords: String::new(),
        edit_custom_cat: String::new(),
        edit_badge_hex: String::new(),
        edit_badge_color: [66, 135, 245],
        edit_category: AppCategory::Other,
        edit_terminal: false,
        edit_hidden: false,
        edit_game_override: None,
        show_new_app_form: false,
        new_app_name: String::new(),
        new_app_comment: String::new(),
        new_app_exec: String::new(),
        new_app_custom_cat: String::new(),
        new_app_keywords: String::new(),
        new_app_category: AppCategory::Other,
        new_app_terminal: false,
        show_new_rule_form: false,
        new_rule_category: String::new(),
        new_rule_keywords: String::new(),
        new_rule_builtin: None,
        notif_history,
        notif_seen_apps,
        display_telemetry: DisplayTelemetry::load(),
        selected_monitor: None,
        display_modal: None,
        sys_ctrl,
        snapshot,
        painter: UiPainter::new(),
        rgba_buf: Vec::new(),
        hit_regions: Vec::new(),
        app_row_rects: Vec::new(),
        app_ctx_menu: None,
        last_saved_at: None,
        status_note: "Live hot-reload active (~/.config/velox-wm/config.toml)".to_string(),
        dirty_config: false,
    };
    let mut cli_app_target: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--app" {
            cli_app_target = args.next();
        }
    }
    if let Some(target_id) = cli_app_target.or_else(|| {
        take_settings_cmd().and_then(|c| c.strip_prefix("edit-app:").map(str::to_string))
    }) {
        app.active_tab = SettingsTab::AppsStartup;
        app.scroll_y = 520;
        app.select_app_for_edit(&target_id);
        app.status_note = format!("Editing {target_id}");
    } else if let Some(ref first_id) = initial_app_id {
        app.select_app_for_edit(first_id);
    }

    WaylandSource::new(conn, event_queue)
        .insert(loop_handle.clone())
        .expect("insert WaylandSource");

    let qh_timer = qh.clone();
    loop_handle
        .insert_source(
            Timer::from_duration(Duration::from_millis(500)),
            move |_, _, state| {
                let mut changed = false;
                if let Some(cmd) = take_settings_cmd() {
                    if let Some(app_id) = cmd.strip_prefix("edit-app:") {
                        state.app_store = AppLibraryStore::load_or_default();
                        state.desktop_apps = scan_desktop_apps_including_hidden();
                        state.active_tab = SettingsTab::AppsStartup;
                        state.app_ctx_menu = None;
                        state.scroll_y = 520;
                        state.select_app_for_edit(app_id);
                        state.status_note = format!("Editing {app_id}");
                        changed = true;
                    }
                }
                if let Some(next) = state.sys_ctrl.snapshot_if_changed(state.snapshot.version) {
                    state.snapshot = next;
                    changed = true;
                }
                let next_tel = DisplayTelemetry::load();
                if next_tel != state.display_telemetry {
                    state.display_telemetry = next_tel;
                    changed = true;
                }
                let (next_hist, next_seen) = load_notification_store();
                if next_hist.len() != state.notif_history.len()
                    || next_hist.first() != state.notif_history.first()
                    || next_seen != state.notif_seen_apps
                {
                    state.notif_history = next_hist;
                    state.notif_seen_apps = next_seen;
                    changed = true;
                }
                if changed {
                    state.needs_redraw = true;
                    state.render_ui(&qh_timer);
                }
                TimeoutAction::ToDuration(Duration::from_millis(500))
            },
        )
        .expect("insert poll timer");

    event_loop.run(None, &mut app, |_| {}).expect("run settings loop");
}

fn parse_hex_rgb(raw: &str) -> Option<[u8; 3]> {
    let s = raw.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r, g, b])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_ratio_clamps_bounds() {
        let r = HitRect::new(100, 10, 200, 20);
        assert_eq!(slider_ratio(50, r), 0.0);
        assert_eq!(slider_ratio(100, r), 0.0);
        assert_eq!(slider_ratio(200, r), 0.5);
        assert_eq!(slider_ratio(400, r), 1.0);
    }

    #[test]
    fn rgba_converts_to_bgra_argb8888() {
        let src = [10u8, 20, 30, 255, 1, 2, 3, 128];
        let mut dst = [0u8; 8];
        rgba_to_argb8888(&src, &mut dst);
        assert_eq!(dst, [30, 20, 10, 255, 3, 2, 1, 128]);
    }

    #[test]
    fn mode_mhz_formats_cleanly() {
        assert_eq!(format_mode_mhz(0), "Auto");
        assert_eq!(format_mode_mhz(144_000), "144 Hz");
        assert_eq!(format_mode_mhz(143_990), "143.99 Hz");
    }

    #[test]
    fn monitor_config_lookup_creates_sensible_defaults() {
        let mut cfg = Config::default();
        let mon = cfg.monitor_cfg_mut("HDMI-A-1");
        mon.width = 2560;
        mon.height = 1440;
        mon.refresh_hz = 120;
        mon.primary = true;
        assert_eq!(mon.scale, 1.0);
        assert_eq!(cfg.monitor_cfg("HDMI-A-1").map(|m| m.refresh_hz), Some(120));
    }
}
