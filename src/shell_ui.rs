use std::time::{Instant, SystemTime, UNIX_EPOCH};

use drm_fourcc::DrmFourcc;
use smithay::{
    backend::renderer::{
        element::{
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            Kind,
        },
        gles::GlesRenderer,
    },
    utils::{Logical, Point, Rectangle, Transform},
};

use crate::{
    apps::{AppCategory, DesktopApp},
    config::{
        Config, DisplayTelemetry, PanelPosition, ShelfStyle, VrrMode, ACCENT_PRESETS,
        WALLPAPER_PRESETS,
    },
    font::{fill_circle, fill_rounded_rect, stroke_rounded_rect, UiPainter},
    grabs::SnapZone,
    notifications::{
        format_relative_time, normalize_app_key, unix_now_secs as notif_unix_now,
        NotificationItem, NotificationSnapshot, NotificationUrgency,
        SuggestedActionKind, ORGANIZER_SILENCEABLE_CATEGORIES,
    },
    system_ctrl::SystemSnapshot,
    updates::UpdateStatus,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSubmenu {
    None,
    Apps,
    Layout,
    WindowSnap,
    WindowWorkspace,
    AppCategory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextTarget {
    Desktop,
    TaskbarWindow(usize),
    Terminal,
    LibraryApp(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivePopup {
    None,
    StartMenu,
    AudioMixer,
    Brightness,
    NetworkBt,
    QuickSettings,
    ActiveApps,
    CalendarClock,
    TaskView,
    ControlCenter,
    PowerMenu,
    Notifications,
    AltTab,
    DisplayConfirm,
    ReloadConfirm {
        reason: String,
    },
    ContextMenu {
        target: ContextTarget,
        pos: (i32, i32),
    },
}

#[derive(Debug, Clone)]
pub struct DisplayConfirmState {
    pub pending_hz: u32,
    pub prev_hz: u32,
    pub prev_vrr: VrrMode,
    pub verify_deadline: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Appearance,
    Windows,
    Audio,
    DisplayPower,
    StartupApps,
    SystemKeys,
}

impl SettingsTab {
    pub const ALL: &'static [SettingsTab] = &[
        SettingsTab::Appearance,
        SettingsTab::Windows,
        SettingsTab::Audio,
        SettingsTab::DisplayPower,
        SettingsTab::StartupApps,
        SettingsTab::SystemKeys,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Windows => "Windows & Snap",
            Self::Audio => "Audio & Routing",
            Self::DisplayPower => "Display & Net",
            Self::StartupApps => "Startup & Apps",
            Self::SystemKeys => "Shortcuts & Disk",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SliderTarget {
    MasterSink { name: String },
    MasterSource { name: String },
    Stream { is_input: bool, index: u32 },
    Brightness,
    PanelOpacity,
    CornerRadius,
    GapPx,
    MasterRatio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifDrawerTab {
    All,
    Priority,
    Snoozed,
    History,
    Apps,
}

impl NotifDrawerTab {
    pub const ALL: &'static [Self] = &[
        Self::All,
        Self::Priority,
        Self::Snoozed,
        Self::History,
        Self::Apps,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "Inbox",
            Self::Priority => "Priority",
            Self::Snoozed => "Snoozed",
            Self::History => "History",
            Self::Apps => "Apps",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    ToggleStartMenu,
    ToggleAudioMixer,
    ToggleBrightnessPopup,
    ToggleNetworkPopup,
    ToggleQuickSettings,
    ToggleCalendar,
    CalendarStepMonth(i32),
    CalendarResetToday,
    FocusTimerToggle,
    FocusTimerReset(u32),
    ToggleTaskView,
    LaunchSystemSettings,
    ToggleActiveApps,
    ToggleSettings,
    OpenSettingsTab(SettingsTab),
    TogglePowerMenu,
    OpenContextMenu { target: ContextTarget, pos: (i32, i32) },
    ToggleContextSubmenu(ContextSubmenu),
    ClosePopup,
    SwitchWorkspace(usize),
    FocusWindow(usize),
    MinimizeWindow(usize),
    ToggleMaximizeWindow(usize),
    ShowDesktop,
    CloseWindow(usize),
    OpenNewInstanceWindow(usize),
    TerminalCopyPrimary,
    TerminalPasteClipboard,
    TerminalPasteSelection,
    OpenNewTerminalForFocused,
    CloseFocusedTerminal,
    SnapWindow(usize, SnapZone),
    ToggleFloatWindow(usize),
    ToggleAlwaysOnTopWindow(usize),
    ToggleFullscreenWindow(usize),
    MoveWindowNextWs(usize),
    ToggleMaximizeGlobal { ws: usize, idx_in_ws: usize },
    SnapGlobal { ws: usize, idx_in_ws: usize, zone: SnapZone },
    MinimizeGlobal { ws: usize, idx_in_ws: usize },
    CloseGlobal { ws: usize, idx_in_ws: usize },
    MoveNextWsGlobal { ws: usize, idx_in_ws: usize },
    SelectAltTab(usize),
    CycleLayoutMode,
    SelectCategory(AppCategory),
    SelectCustomCategory(String),
    SearchInAllCategories,
    RestorePreviousCategory,
    ClearSearch,
    LaunchApp(usize),
    LaunchQuickTerminal,
    LaunchQuickFiles,
    LaunchQuickBrowser,
    RescanApps,
    SetAccent(String),
    SetWallpaperPreset(String),
    CycleWallpaperPreset,
    ToggleCustomWallpaper,
    TogglePanelPosition,
    ToggleShelfStyle,
    AdjustPanelHeight(i32),
    AdjustBorderWidth(i32),
    ToggleEdgeSnap,
    ToggleWindowsShortcuts,
    ToggleShowSystemProcesses,
    CycleRefreshHz,
    ConfirmDisplayChange,
    KeepDisplayChange,
    CancelRevertDisplayChange,
    CycleVrrMode,
    ToggleVideoBatterySaver,
    CycleIdleRefreshHz,
    ToggleBatterySaverModeset,
    ToggleDimUnfocused,
    ToggleFreezeHidden,
    CleanRamCacheNow { deep_kernel: bool },
    MoveWindowNextMonitor,
    ToggleNotifications,
    ToggleDnd,
    SelectNotifTab(NotifDrawerTab),
    ToggleOrganizerExpanded,
    ToggleOrganizerSilenceCategory(String),
    ClearNotificationHistory,
    DismissNotification(u32),
    SnoozeNotification { id: u32, secs: u64 },
    UnsnoozeNotification(u32),
    TogglePriorityConversation(String),
    OpenInlineReply(u32),
    SendNotificationReply { id: u32, text: String },
    RunSuggestedAction { id: u32, kind: SuggestedActionKind, payload: String },
    ToggleAppNotifDisabled(String),
    ToggleAppNotifSilent(String),
    ClearNotifications,
    InvokeNotificationAction { id: u32, key: String },
    CheckUpdatesNow,
    PromptReloadConfirm(String),
    CancelReloadConfirm,
    SeamlessReloadWm,
    ToggleGamePointerLock,
    ToggleFullscreenTaskbarLock,
    ToggleFocusFollowsMouse,
    ToggleAnimations,
    CycleAnimationSpeed,
    ToggleAutoRouteAudioRoles,
    ToggleAutoLaunchTerminal,
    CycleTerminalApp,
    CycleBrowserApp,
    CycleFilesApp,
    SaveConfigNow,
    ResetConfig,
    ReloadConfig,
    QuitSession,
    LockScreen,
    SystemSuspend,
    SystemReboot,
    SystemPoweroff,
    ToggleSinkMute(String),
    SetDefaultSink(String),
    ToggleSourceMute(String),
    SetDefaultSource(String),
    ToggleStreamMute { is_input: bool, index: u32 },
    CycleStreamDevice { is_input: bool, index: u32 },
    CycleRoleRouting { is_comms: bool },
    AdjustBrightness(i32),
    AdjustDefaultVol(i32),
    SetPowerProfile(String),
    ToggleWifi,
    ToggleBluetooth,
    OpenBluetoothManager,
    OpenNetworkEditor,
    KillBgProcess { pids: Vec<u32>, force: bool },
    RefreshSystem,
    EditLibraryAppInSettings(usize),
    SetLibraryAppCategory(usize, AppCategory),
    ToggleLibraryAppTerminal(usize),
    CycleLibraryAppGameOverride(usize),
    ToggleLibraryAppHidden(usize),
    ToggleQuickTileCustomize,
    OpenQuickTileCustomizer,
    MoveTaskbarQuickTile(String, isize),
    ToggleTaskbarQuickTileVisible(String),
    MoveQuickPanelTile(String, isize),
    ResetTaskbarQuickTiles,
    ToggleMinimalViewWindow(usize),
    StartSliderDrag {
        target: SliderTarget,
        rect: Rectangle<i32, Logical>,
        min: i32,
        max: i32,
    },
}

#[derive(Debug, Clone)]
pub struct HitRegion {
    pub rect: Rectangle<i32, Logical>,
    pub action: UiAction,
    pub right_action: Option<UiAction>,
    pub scroll_action: Option<(UiAction, UiAction)>,
    pub id: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskWindowInfo {
    pub title: String,
    pub focused: bool,
    pub floating: bool,
    pub always_on_top: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub is_terminal: bool,
    pub minimal_view: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalWindowInfo {
    pub workspace: usize,
    pub index_in_ws: usize,
    pub title: String,
    pub focused: bool,
    pub floating: bool,
    pub always_on_top: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub is_terminal: bool,
    pub minimal_view: bool,
}

struct ActiveSliderDrag {
    target: SliderTarget,
    rect: Rectangle<i32, Logical>,
    min: i32,
    max: i32,
}

pub struct ShellUi {
    painter: UiPainter,
    pub active_popup: ActivePopup,
    pub settings_tab: SettingsTab,
    pub search_query: String,
    pub selected_category: AppCategory,
    pub selected_custom_category: Option<String>,
    pub auto_switched_from_category: Option<(AppCategory, Option<String>)>,
    pub app_scroll: usize,
    pub selected_app_row: usize,
    pub alt_tab_index: usize,
    pub save_feedback_until_epoch: u64,
    pub context_submenu: ContextSubmenu,
    pub customize_quick_tiles: bool,
    pub calendar_month_offset: i32,
    pub focus_timer_running: bool,
    pub focus_timer_remaining_secs: u64,
    focus_timer_last_sec: u64,
    pub display_telemetry: DisplayTelemetry,
    pub display_confirm: Option<DisplayConfirmState>,
    hovered_hit_id: Option<usize>,

    popup_anim: f32,
    active_drag: Option<ActiveSliderDrag>,

    anchor_start: i32,
    anchor_taskview: i32,
    anchor_vol: i32,
    anchor_bri: i32,
    anchor_net: i32,
    anchor_control: i32,
    anchor_active: i32,
    anchor_notif: i32,
    anchor_clock: i32,
    anchor_power: i32,
    pub notif_scroll: usize,
    pub notif_tab: NotifDrawerTab,
    pub notif_organizer_expanded: bool,
    pub notif_reply_target: Option<u32>,
    pub notif_reply_draft: String,

    panel_pixels: Vec<u8>,
    panel_buffer: Option<MemoryRenderBuffer>,
    panel_buf_size: (i32, i32),
    panel_rect: Rectangle<i32, Logical>,
    panel_hits: Vec<HitRegion>,
    panel_signature: String,
    pub shelf_visible: bool,

    popup_pixels: Vec<u8>,
    popup_buffer: Option<MemoryRenderBuffer>,
    popup_buf_size: (i32, i32),
    popup_rect: Rectangle<i32, Logical>,
    popup_hits: Vec<HitRegion>,
    popup_signature: String,

    toast_pixels: Vec<u8>,
    toast_buffer: Option<MemoryRenderBuffer>,
    toast_buf_size: (i32, i32),
    toast_rect: Rectangle<i32, Logical>,
    toast_hits: Vec<HitRegion>,
    toast_signature: String,

    saver_overlay_pixels: Vec<u8>,
    saver_overlay_buffer: Option<MemoryRenderBuffer>,
    saver_overlay_buf_size: (i32, i32),
    saver_overlay_rect: Rectangle<i32, Logical>,
    saver_overlay_signature: String,

    lock_pixels: Vec<u8>,
    lock_buffer: Option<MemoryRenderBuffer>,
    lock_buf_size: (i32, i32),
    lock_signature: String,

    cached_clock_sec: u64,
    cached_clock: (String, String, String),
    cached_uptime: String,
}

impl ShellUi {
    pub fn new() -> Self {
        Self {
            painter: UiPainter::new(),
            active_popup: ActivePopup::None,
            settings_tab: SettingsTab::Appearance,
            search_query: String::new(),
            selected_category: AppCategory::All,
            selected_custom_category: None,
            auto_switched_from_category: None,
            app_scroll: 0,
            selected_app_row: 0,
            alt_tab_index: 0,
            save_feedback_until_epoch: 0,
            context_submenu: ContextSubmenu::None,
            customize_quick_tiles: false,
            calendar_month_offset: 0,
            focus_timer_running: false,
            focus_timer_remaining_secs: 25 * 60,
            focus_timer_last_sec: 0,
            display_telemetry: DisplayTelemetry::default(),
            display_confirm: None,
            hovered_hit_id: None,
            popup_anim: 0.0,
            active_drag: None,
            anchor_start: 54,
            anchor_taskview: 132,
            anchor_vol: 980,
            anchor_bri: 1050,
            anchor_net: 1120,
            anchor_control: 1180,
            anchor_active: 1240,
            anchor_notif: 1295,
            anchor_clock: 1340,
            anchor_power: 1410,
            notif_scroll: 0,
            notif_tab: NotifDrawerTab::All,
            notif_organizer_expanded: false,
            notif_reply_target: None,
            notif_reply_draft: String::new(),
            panel_pixels: Vec::new(),
            panel_buffer: None,
            panel_buf_size: (0, 0),
            panel_rect: Rectangle::default(),
            panel_hits: Vec::new(),
            panel_signature: String::new(),
            shelf_visible: true,
            popup_pixels: Vec::new(),
            popup_buffer: None,
            popup_buf_size: (0, 0),
            popup_rect: Rectangle::default(),
            popup_hits: Vec::new(),
            popup_signature: String::new(),
            toast_pixels: Vec::new(),
            toast_buffer: None,
            toast_buf_size: (0, 0),
            toast_rect: Rectangle::default(),
            toast_hits: Vec::new(),
            toast_signature: String::new(),
            saver_overlay_pixels: Vec::new(),
            saver_overlay_buffer: None,
            saver_overlay_buf_size: (0, 0),
            saver_overlay_rect: Rectangle::default(),
            saver_overlay_signature: String::new(),
            lock_pixels: Vec::new(),
            lock_buffer: None,
            lock_buf_size: (0, 0),
            lock_signature: String::new(),
            cached_clock_sec: 0,
            cached_clock: ("00:00".to_string(), "Day".to_string(), "00:00:00".to_string()),
            cached_uptime: "0m".to_string(),
        }
    }

    pub fn notify_saved(&mut self) {
        self.save_feedback_until_epoch = unix_secs_now() + 3;
    }

    pub fn is_popup_open(&self) -> bool {
        self.active_popup != ActivePopup::None
    }

    pub fn is_alt_tab_open(&self) -> bool {
        self.active_popup == ActivePopup::AltTab
    }

    pub fn is_animating(&self) -> bool {
        self.active_popup != ActivePopup::None && self.popup_anim < 0.999
    }

    pub fn is_dragging_slider(&self) -> bool {
        self.active_drag.is_some()
    }

    pub fn close_popup(&mut self) {
        self.active_popup = ActivePopup::None;
        self.context_submenu = ContextSubmenu::None;
        self.customize_quick_tiles = false;
        self.active_drag = None;
        self.hovered_hit_id = None;
        self.popup_anim = 0.0;
    }

    pub fn open_popup(&mut self, popup: ActivePopup) {
        if self.active_popup == popup && !matches!(popup, ActivePopup::ContextMenu { .. }) {
            self.close_popup();
        } else {
            self.active_popup = popup.clone();
            self.context_submenu = ContextSubmenu::None;
            if !matches!(popup, ActivePopup::QuickSettings) {
                self.customize_quick_tiles = false;
            }
            self.active_drag = None;
            self.hovered_hit_id = None;
            self.popup_anim = if matches!(popup, ActivePopup::AltTab | ActivePopup::TaskView) {
                0.70
            } else {
                0.10
            };
            if matches!(popup, ActivePopup::StartMenu) {
                self.search_query.clear();
                self.app_scroll = 0;
                self.selected_app_row = 0;
            }
            if matches!(popup, ActivePopup::CalendarClock) {
                self.calendar_month_offset = 0;
            }
        }
    }

    pub fn open_context_menu(&mut self, target: ContextTarget, pos: (i32, i32)) {
        self.open_popup(ActivePopup::ContextMenu { target, pos });
    }

    pub fn toggle_context_submenu(&mut self, sub: ContextSubmenu) {
        self.context_submenu = if self.context_submenu == sub {
            ContextSubmenu::None
        } else {
            sub
        };
    }

    pub fn toggle_focus_timer(&mut self) {
        self.focus_timer_running = !self.focus_timer_running;
        self.focus_timer_last_sec = unix_secs_now();
    }

    pub fn reset_focus_timer(&mut self, mins: u32) {
        self.focus_timer_running = false;
        self.focus_timer_remaining_secs = (mins as u64).max(1) * 60;
        self.focus_timer_last_sec = unix_secs_now();
    }

    pub fn prompt_display_confirm(&mut self, pending_hz: u32, prev_hz: u32, prev_vrr: VrrMode) {
        self.display_confirm = Some(DisplayConfirmState {
            pending_hz,
            prev_hz,
            prev_vrr,
            verify_deadline: None,
        });
        self.active_popup = ActivePopup::DisplayConfirm;
        self.popup_anim = 0.85;
        self.hovered_hit_id = None;
    }

    pub fn confirm_and_apply_display_immediately(&mut self) -> Option<u32> {
        let state = self.display_confirm.take()?;
        self.close_popup();
        Some(state.pending_hz)
    }

    pub fn trim_caches(&mut self) {
        self.painter.trim_glyph_cache(1536);
        if self.active_popup == ActivePopup::None {
            self.popup_pixels.clear();
            self.popup_pixels.shrink_to(0);
            self.popup_buffer = None;
            self.popup_signature.clear();
        }
        if self.toast_buffer.is_none() {
            self.toast_pixels.clear();
            self.toast_pixels.shrink_to(0);
        }
        if self.saver_overlay_buffer.is_none() {
            self.saver_overlay_pixels.clear();
            self.saver_overlay_pixels.shrink_to(0);
        }
    }

    pub fn keep_display_confirm(&mut self) {
        self.display_confirm = None;
        self.close_popup();
    }

    pub fn revert_display_confirm(&mut self) -> Option<(u32, VrrMode)> {
        let state = self.display_confirm.take()?;
        let had_applied = state.verify_deadline.is_some();
        self.close_popup();
        if had_applied {
            Some((state.prev_hz, state.prev_vrr))
        } else {
            None
        }
    }

    pub fn tick_display_confirm_expired(&mut self) -> Option<(u32, VrrMode)> {
        let expired = self
            .display_confirm
            .as_ref()
            .and_then(|d| d.verify_deadline)
            .map(|dl| Instant::now() >= dl)
            .unwrap_or(false);
        if expired {
            return self.revert_display_confirm();
        }
        None
    }

    pub fn step_alt_tab(&mut self, total_windows: usize, forward: bool) {
        if total_windows == 0 {
            return;
        }
        if self.active_popup != ActivePopup::AltTab {
            self.active_popup = ActivePopup::AltTab;
            self.popup_anim = 0.7;
            self.alt_tab_index = if total_windows > 1 { 1 } else { 0 };
        } else if forward {
            self.alt_tab_index = (self.alt_tab_index + 1) % total_windows;
        } else {
            self.alt_tab_index = (self.alt_tab_index + total_windows - 1) % total_windows;
        }
    }

    pub fn toggle_start_menu(&mut self) {
        self.open_popup(ActivePopup::StartMenu);
    }

    pub fn toggle_audio_mixer(&mut self) {
        self.open_popup(ActivePopup::AudioMixer);
    }

    pub fn toggle_brightness_popup(&mut self) {
        self.open_popup(ActivePopup::Brightness);
    }

    pub fn toggle_network_popup(&mut self) {
        self.open_popup(ActivePopup::NetworkBt);
    }

    pub fn toggle_quick_settings(&mut self) {
        self.open_popup(ActivePopup::QuickSettings);
    }

    pub fn toggle_calendar(&mut self) {
        self.open_popup(ActivePopup::CalendarClock);
    }

    pub fn toggle_task_view(&mut self) {
        self.open_popup(ActivePopup::TaskView);
    }

    pub fn toggle_active_apps(&mut self) {
        self.open_popup(ActivePopup::ActiveApps);
    }

    pub fn toggle_settings(&mut self) {
        self.open_popup(ActivePopup::ControlCenter);
    }

    pub fn toggle_power_menu(&mut self) {
        self.open_popup(ActivePopup::PowerMenu);
    }

    pub fn toggle_notifications(&mut self) {
        self.notif_scroll = 0;
        self.open_popup(ActivePopup::Notifications);
    }

    pub fn prompt_reload_confirm(&mut self, reason: impl Into<String>) {
        self.active_popup = ActivePopup::ReloadConfirm {
            reason: reason.into(),
        };
        self.popup_anim = 0.85;
        self.hovered_hit_id = None;
        self.popup_signature.clear();
    }

    fn toast_eligible(
        n: &NotificationItem,
        notifs: &NotificationSnapshot,
        config: &Config,
        now: u64,
    ) -> bool {
        let not_expired = n.expire_at_epoch == 0 || now <= n.expire_at_epoch;
        if !not_expired || n.read {
            return n.is_update_notice && not_expired;
        }
        if config.notifications_dnd {
            return n.is_update_notice
                || n.urgency == NotificationUrgency::Critical
                || n.is_priority;
        }
        if notifs.cooldown_active && config.notification_cooldown_enabled {
            return n.is_priority
                || n.is_update_notice
                || n.urgency == NotificationUrgency::Critical;
        }
        !n.is_silent || n.is_priority || n.is_update_notice
    }

    pub fn should_expire_toast(
        &self,
        notifs: &NotificationSnapshot,
        config: &Config,
    ) -> bool {
        if self.toast_buffer.is_none() {
            return false;
        }
        let now = notif_unix_now();
        !notifs.items.iter().any(|n| Self::toast_eligible(n, notifs, config, now))
    }

    pub fn captures_pointer(&self, pos: Point<f64, Logical>) -> bool {
        if self.active_drag.is_some() {
            return true;
        }
        let p = pos.to_i32_round();
        if self.shelf_visible && self.panel_rect.contains(p) {
            return true;
        }
        if self.active_popup != ActivePopup::None && self.popup_rect.contains(p) {
            return true;
        }
        if self.toast_buffer.is_some() && self.toast_rect.contains(p) {
            return true;
        }
        false
    }

    pub fn begin_slider_drag(
        &mut self,
        target: SliderTarget,
        rect: Rectangle<i32, Logical>,
        min: i32,
        max: i32,
    ) {
        self.active_drag = Some(ActiveSliderDrag {
            target,
            rect,
            min,
            max,
        });
    }

    pub fn end_slider_drag(&mut self) {
        self.active_drag = None;
    }

    pub fn update_slider_drag(&self, pos: Point<f64, Logical>) -> Option<(SliderTarget, i32)> {
        let drag = self.active_drag.as_ref()?;
        let w = drag.rect.size.w.max(1) as f64;
        let t = ((pos.x - drag.rect.loc.x as f64) / w).clamp(0.0, 1.0);
        let val = (drag.min as f64 + t * (drag.max - drag.min) as f64).round() as i32;
        Some((drag.target.clone(), val.clamp(drag.min, drag.max)))
    }

    pub fn update_hover(&mut self, pos: Point<f64, Logical>) -> bool {
        let p = pos.to_i32_round();
        let panel_iter = self.shelf_visible.then_some(self.panel_hits.iter()).into_iter().flatten();
        let new_hover = self
            .popup_hits
            .iter()
            .chain(self.toast_hits.iter())
            .chain(panel_iter)
            .find(|h| h.rect.contains(p))
            .map(|h| h.id);
        if new_hover != self.hovered_hit_id {
            self.hovered_hit_id = new_hover;
            true
        } else {
            false
        }
    }

    pub fn click_at(&self, pos: Point<f64, Logical>, right_click: bool) -> Option<UiAction> {
        let p = pos.to_i32_round();
        if self.toast_buffer.is_some() && self.toast_rect.contains(p) {
            if let Some(hit) = self.toast_hits.iter().rev().find(|h| h.rect.contains(p)) {
                return Some(hit.action.clone());
            }
        }
        if self.active_popup != ActivePopup::None {
            if let Some(hit) = self.popup_hits.iter().rev().find(|h| h.rect.contains(p)) {
                return if right_click {
                    hit.right_action.clone().map(|act| match act {
                        UiAction::OpenContextMenu { target, pos } if pos == (0, 0) => {
                            UiAction::OpenContextMenu {
                                target,
                                pos: (p.x, p.y),
                            }
                        }
                        other => other,
                    })
                } else {
                    Some(hit.action.clone())
                };
            }
        }
        if self.shelf_visible {
            if let Some(hit) = self.panel_hits.iter().rev().find(|h| h.rect.contains(p)) {
                return if right_click {
                    hit.right_action.clone().map(|act| match act {
                        UiAction::OpenContextMenu { target, pos } if pos == (0, 0) => {
                            UiAction::OpenContextMenu {
                                target,
                                pos: (p.x, p.y),
                            }
                        }
                        other => other,
                    })
                } else {
                    Some(hit.action.clone())
                };
            }
        }
        if self.active_popup != ActivePopup::None && !self.popup_rect.contains(p) {
            return Some(UiAction::ClosePopup);
        }
        None
    }

    pub fn scroll_at(
        &mut self,
        pos: Point<f64, Logical>,
        delta_y: f64,
        total_filtered: usize,
    ) -> Option<UiAction> {
        let p = pos.to_i32_round();
        if delta_y == 0.0 {
            return None;
        }
        if self.shelf_visible {
            for hit in self.panel_hits.iter().rev() {
                if hit.rect.contains(p) {
                    if let Some((ref up_act, ref down_act)) = hit.scroll_action {
                        return Some(if delta_y < 0.0 {
                            up_act.clone()
                        } else {
                            down_act.clone()
                        });
                    }
                }
            }
        }
        if self.active_popup == ActivePopup::StartMenu && self.popup_rect.contains(p) {
            let visible_rows = 8usize;
            let max_scroll = total_filtered.saturating_sub(visible_rows);
            if delta_y > 0.0 {
                self.app_scroll = (self.app_scroll + 1).min(max_scroll);
            } else if delta_y < 0.0 {
                self.app_scroll = self.app_scroll.saturating_sub(1);
            }
            return Some(UiAction::RefreshSystem);
        }
        if self.active_popup == ActivePopup::Notifications && self.popup_rect.contains(p) {
            if delta_y > 0.0 {
                self.notif_scroll = self.notif_scroll.saturating_add(1).min(32);
            } else if delta_y < 0.0 {
                self.notif_scroll = self.notif_scroll.saturating_sub(1);
            }
            return Some(UiAction::RefreshSystem);
        }
        None
    }

    fn app_matches_query(app: &DesktopApp, q: &str) -> bool {
        if q.is_empty() {
            return true;
        }
        app.name.to_ascii_lowercase().contains(q)
            || app.comment.to_ascii_lowercase().contains(q)
            || app.effective_category_label().to_ascii_lowercase().contains(q)
            || app.keywords.iter().any(|k| k.to_ascii_lowercase().contains(q))
            || app.exec_args.iter().any(|a| a.to_ascii_lowercase().contains(q))
    }

    pub fn filtered_app_indices(&self, apps: &[DesktopApp]) -> Vec<usize> {
        let q = self.search_query.trim().to_ascii_lowercase();
        apps.iter()
            .enumerate()
            .filter(|(_, app)| {
                if app.hidden {
                    return false;
                }
                let cat_ok = if let Some(ref custom) = self.selected_custom_category {
                    app.custom_category
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(custom))
                } else {
                    self.selected_category == AppCategory::All
                        || app.category == self.selected_category
                };
                if !cat_ok {
                    return false;
                }
                Self::app_matches_query(app, &q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn filtered_app_indices_all(&self, apps: &[DesktopApp]) -> Vec<usize> {
        let q = self.search_query.trim().to_ascii_lowercase();
        apps.iter()
            .enumerate()
            .filter(|(_, app)| !app.hidden && Self::app_matches_query(app, &q))
            .map(|(i, _)| i)
            .collect()
    }

    /// Automatically widens a filtered category search to `All` when zero results remain inside the category
    /// and matches exist globally.
    pub fn apply_search_smart_switch(&mut self, apps: &[DesktopApp]) -> bool {
        if self.search_query.trim().is_empty() {
            return false;
        }
        if self.selected_category == AppCategory::All && self.selected_custom_category.is_none() {
            return false;
        }
        if !self.filtered_app_indices(apps).is_empty() {
            return false;
        }
        if self.filtered_app_indices_all(apps).is_empty() {
            return false;
        }
        let prev_custom = self.selected_custom_category.take();
        self.auto_switched_from_category = Some((self.selected_category, prev_custom));
        self.selected_category = AppCategory::All;
        self.app_scroll = 0;
        self.selected_app_row = 0;
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        current_output_geo: Rectangle<i32, Logical>,
        panel_output_geo: Rectangle<i32, Logical>,
        popup_output_geo: Rectangle<i32, Logical>,
        is_primary_ui: bool,
        scale: f64,
        dt_secs: f32,
        config: &Config,
        active_ws: usize,
        ws_counts: &[usize; 9],
        tasks: &[TaskWindowInfo],
        global_windows: &[GlobalWindowInfo],
        apps: &[DesktopApp],
        sys: &SystemSnapshot,
        notifs: &NotificationSnapshot,
        updates: &UpdateStatus,
        show_shelf: bool,
        allow_toasts: bool,
    ) -> Vec<MemoryRenderBufferRenderElement<GlesRenderer>> {
        let mut out = Vec::new();
        if current_output_geo.size.w <= 0 || current_output_geo.size.h <= 0 {
            return out;
        }

        if self.active_popup != ActivePopup::None && self.popup_anim < 1.0 {
            if config.animations_enabled {
                let speed = (dt_secs * 16.0).clamp(0.10, 0.65);
                self.popup_anim = (self.popup_anim + (1.0 - self.popup_anim) * speed).min(1.0);
                if 1.0 - self.popup_anim < 0.01 {
                    self.popup_anim = 1.0;
                }
            } else {
                self.popup_anim = 1.0;
            }
        }

        let now_sec = unix_secs_now();
        if now_sec != self.cached_clock_sec {
            self.cached_clock_sec = now_sec;
            self.cached_clock = current_local_clock(now_sec);
            self.cached_uptime = read_system_uptime();
            if self.focus_timer_running {
                if self.focus_timer_last_sec > 0 && now_sec > self.focus_timer_last_sec {
                    let elapsed = now_sec - self.focus_timer_last_sec;
                    self.focus_timer_remaining_secs =
                        self.focus_timer_remaining_secs.saturating_sub(elapsed);
                    if self.focus_timer_remaining_secs == 0 {
                        self.focus_timer_running = false;
                    }
                }
                self.focus_timer_last_sec = now_sec;
            }
        }
        let clock_str = self.cached_clock.0.clone();
        let date_str = self.cached_clock.1.clone();
        let clock_full_str = self.cached_clock.2.clone();

        if is_primary_ui {
            self.shelf_visible = show_shelf;
            if show_shelf {
                self.ensure_panel(
                    panel_output_geo,
                    config,
                    active_ws,
                    ws_counts,
                    tasks,
                    global_windows.len(),
                    sys,
                    notifs,
                    updates,
                    &clock_str,
                    &date_str,
                );
            } else {
                self.panel_rect = Rectangle::default();
                self.panel_hits.clear();
            }

            self.ensure_toast(panel_output_geo, config, notifs, updates, allow_toasts);
            self.ensure_saver_overlay(panel_output_geo, config, allow_toasts);
        }

        self.ensure_popup(
            panel_output_geo,
            popup_output_geo,
            config,
            ws_counts,
            tasks,
            global_windows,
            apps,
            sys,
            notifs,
            updates,
            &clock_full_str,
        );

        let mut push_rel_elem = |out: &mut Vec<MemoryRenderBufferRenderElement<GlesRenderer>>,
                             buf: &MemoryRenderBuffer,
                             glob_x: i32,
                             glob_y: i32,
                             alpha: Option<f32>| {
            let rel_x = glob_x - current_output_geo.loc.x;
            let rel_y = glob_y - current_output_geo.loc.y;
            let pos = Point::<f64, Logical>::from((rel_x as f64, rel_y as f64))
                .to_physical_precise_round::<f64, i32>(scale);
            if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                (pos.x as f64, pos.y as f64),
                buf,
                alpha,
                None,
                None,
                Kind::Unspecified,
            ) {
                out.push(elem);
            }
        };

        if self.active_popup != ActivePopup::None && current_output_geo.overlaps(self.popup_rect) {
            if let Some(ref buf) = self.popup_buffer {
                let slide_offset = match self.active_popup {
                    ActivePopup::AltTab | ActivePopup::TaskView | ActivePopup::ContextMenu { .. } => 0,
                    _ => ((1.0 - self.popup_anim) * 14.0).round() as i32,
                };
                let y_anim = match config.panel_position {
                    PanelPosition::Top => self.popup_rect.loc.y - slide_offset,
                    PanelPosition::Bottom => self.popup_rect.loc.y + slide_offset,
                };
                push_rel_elem(
                    &mut out,
                    buf,
                    self.popup_rect.loc.x,
                    y_anim,
                    Some(self.popup_anim),
                );
            }
        }

        if is_primary_ui && allow_toasts {
            if let Some(ref buf) = self.toast_buffer {
                push_rel_elem(&mut out, buf, self.toast_rect.loc.x, self.toast_rect.loc.y, None);
            }
            if config.show_refresh_overlay {
                if let Some(ref buf) = self.saver_overlay_buffer {
                    push_rel_elem(
                        &mut out,
                        buf,
                        self.saver_overlay_rect.loc.x,
                        self.saver_overlay_rect.loc.y,
                        None,
                    );
                }
            }
        }

        if is_primary_ui && show_shelf {
            if let Some(ref buf) = self.panel_buffer {
                push_rel_elem(&mut out, buf, self.panel_rect.loc.x, self.panel_rect.loc.y, None);
            }
        }

        out
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_panel(
        &mut self,
        output_geo: Rectangle<i32, Logical>,
        config: &Config,
        active_ws: usize,
        ws_counts: &[usize; 9],
        tasks: &[TaskWindowInfo],
        total_windows: usize,
        sys: &SystemSnapshot,
        notifs: &NotificationSnapshot,
        updates: &UpdateStatus,
        clock_str: &str,
        date_str: &str,
    ) {
        let ph = config.panel_height.clamp(34, 56);
        let floating = config.shelf_style == ShelfStyle::Floating;
        let h_margin = if floating { 14 } else { 0 };
        let v_margin = if floating { 8 } else { 0 };
        let pw = (output_geo.size.w - h_margin * 2).max(400);
        let py = output_geo.loc.y
            + match config.panel_position {
                PanelPosition::Top => v_margin,
                PanelPosition::Bottom => (output_geo.size.h - ph - v_margin).max(0),
            };
        let px = output_geo.loc.x + h_margin;
        self.panel_rect = Rectangle::new((px, py).into(), (pw, ph).into());

        let sig = format!(
            "{}x{}@{},{}:{:?}:{}:{}:{}:{}:{:?}:{:?}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{:?}:{}:{}:{}",
            pw,
            ph,
            px,
            py,
            config.shelf_style,
            config.panel_opacity,
            config.corner_radius,
            config.accent_theme,
            active_ws,
            ws_counts,
            tasks,
            total_windows,
            format!("{:?}", self.active_popup),
            clock_str,
            date_str,
            config.layout_mode.short_label(),
            sys.version,
            self.hovered_hit_id.unwrap_or(usize::MAX),
            notifs.version,
            config.notifications_dnd,
            config.hide_silent_notifications_in_status_bar,
            updates.local_update_ready,
            updates.github_latest_tag,
            config.taskbar_quick_tiles.join(","),
            config.hidden_quick_tiles.join(","),
            config.minimal_view_enabled,
        );
        if sig == self.panel_signature && self.panel_buffer.is_some() {
            return;
        }
        self.panel_signature = sig;
        self.panel_hits.clear();

        let needed = (pw * ph * 4) as usize;
        if self.panel_pixels.len() != needed {
            self.panel_pixels.resize(needed, 0);
        } else {
            self.panel_pixels.fill(0);
        }
        let buf = &mut self.panel_pixels;
        let accent = config.accent_rgb();
        let bg_a = config.panel_opacity.clamp(120, 250);
        let radius = if floating {
            config.corner_radius.clamp(6, 18)
        } else {
            0
        };

        fill_rounded_rect(buf, pw, ph, 0, 0, pw, ph, radius, [11, 15, 28, bg_a]);
        if floating {
            stroke_rounded_rect(buf, pw, ph, 0, 0, pw, ph, radius, 1, [82, 108, 160, 125]);
        } else {
            let border_y = match config.panel_position {
                PanelPosition::Top => ph - 1,
                PanelPosition::Bottom => 0,
            };
            fill_rounded_rect(buf, pw, ph, 0, border_y, pw, 1, 0, [74, 96, 142, 115]);
        }

        let mut next_id = 1usize;
        let btn_h = ph - 10;
        let btn_y = 5;
        let mut cursor_x = 10;

        // 1. ChromeOS-style Launcher Circle/Pill Button
        let start_w = 88;
        let start_active = self.active_popup == ActivePopup::StartMenu;
        let start_hover = self.hovered_hit_id == Some(next_id);
        let start_bg = if start_active {
            [accent[0], accent[1], accent[2], 240]
        } else if start_hover {
            [accent[0], accent[1], accent[2], 160]
        } else {
            [28, 36, 58, 225]
        };
        fill_rounded_rect(buf, pw, ph, cursor_x, btn_y, start_w, btn_h, 14, start_bg);
        let cy = btn_y + btn_h / 2;
        fill_circle(buf, pw, ph, cursor_x + 17, cy, 8, [235, 242, 255, 55]);
        fill_circle(buf, pw, ph, cursor_x + 17, cy, 4, [248, 250, 255, 250]);
        self.painter.draw_text(
            buf,
            pw,
            ph,
            cursor_x + 30,
            btn_y + (btn_h - 14) / 2,
            52,
            "Apps",
            13.5,
            [245, 248, 255, 255],
        );
        self.anchor_start = px + cursor_x + start_w / 2;
        self.panel_hits.push(HitRegion {
            rect: Rectangle::new((px + cursor_x, py + btn_y).into(), (start_w, btn_h).into()),
            action: UiAction::ToggleStartMenu,
            right_action: Some(UiAction::OpenContextMenu {
                target: ContextTarget::Desktop,
                pos: (px + cursor_x, py + btn_y),
            }),
            scroll_action: None,
            id: next_id,
        });
        next_id += 1;
        cursor_x += start_w + 6;

        // 1b. Multitasking Overview ("View") Button
        let tv_w = 54;
        let tv_active = self.active_popup == ActivePopup::TaskView;
        let tv_hover = self.hovered_hit_id == Some(next_id);
        let tv_bg = if tv_active {
            [accent[0], accent[1], accent[2], 235]
        } else if tv_hover {
            [48, 60, 92, 225]
        } else {
            [24, 31, 50, 220]
        };
        fill_rounded_rect(buf, pw, ph, cursor_x, btn_y, tv_w, btn_h, 11, tv_bg);
        let tv_tx = cursor_x + (tv_w - self.painter.measure_text("View", 11.5)) / 2;
        self.painter.draw_text(
            buf,
            pw,
            ph,
            tv_tx,
            btn_y + (btn_h - 12) / 2,
            tv_w - 6,
            "View",
            11.5,
            [236, 242, 255, 255],
        );
        self.anchor_taskview = px + cursor_x + tv_w / 2;
        self.panel_hits.push(HitRegion {
            rect: Rectangle::new((px + cursor_x, py + btn_y).into(), (tv_w, btn_h).into()),
            action: UiAction::ToggleTaskView,
            right_action: Some(UiAction::ShowDesktop),
            scroll_action: None,
            id: next_id,
        });
        next_id += 1;
        cursor_x += tv_w + 8;

        // 2. Compact Workspace Pills (1..9)
        let ws_w = 22;
        for ws in 0..9 {
            let is_active = ws == active_ws;
            let has_windows = ws_counts[ws] > 0;
            let hovered = self.hovered_hit_id == Some(next_id);
            let bg = if is_active {
                [accent[0], accent[1], accent[2], 230]
            } else if hovered {
                [48, 60, 92, 220]
            } else if has_windows {
                [26, 34, 54, 215]
            } else {
                [18, 23, 38, 145]
            };
            fill_rounded_rect(buf, pw, ph, cursor_x, btn_y + 2, ws_w, btn_h - 4, 8, bg);
            if has_windows && !is_active {
                fill_circle(
                    buf,
                    pw,
                    ph,
                    cursor_x + ws_w / 2,
                    btn_y + btn_h - 4,
                    2,
                    [accent[0], accent[1], accent[2], 245],
                );
            }
            let txt_col = if is_active || has_windows {
                [242, 246, 255, 255]
            } else {
                [128, 142, 174, 210]
            };
            self.painter.draw_text(
                buf,
                pw,
                ph,
                cursor_x + 7,
                btn_y + (btn_h - 12) / 2,
                14,
                &(ws + 1).to_string(),
                12.0,
                txt_col,
            );
            self.panel_hits.push(HitRegion {
                rect: Rectangle::new((px + cursor_x, py + btn_y).into(), (ws_w, btn_h).into()),
                action: UiAction::SwitchWorkspace(ws),
                right_action: None,
                scroll_action: None,
                id: next_id,
            });
            next_id += 1;
            cursor_x += ws_w + 3;
        }
        cursor_x += 10;

        // 3. Right ChromeOS Status Tray (right-to-left)
        let mut right_x = pw - 10;

        // Power Pill
        let pwr_w = 50;
        right_x -= pwr_w;
        let pwr_active = self.active_popup == ActivePopup::PowerMenu;
        let pwr_hover = self.hovered_hit_id == Some(next_id);
        let pwr_bg = if pwr_active {
            [215, 68, 86, 235]
        } else if pwr_hover {
            [180, 58, 76, 200]
        } else {
            [32, 38, 56, 215]
        };
        fill_rounded_rect(buf, pw, ph, right_x, btn_y, pwr_w, btn_h, 10, pwr_bg);
        let ptx = right_x + (pwr_w - self.painter.measure_text("Power", 11.5)) / 2;
        self.painter.draw_text(
            buf,
            pw,
            ph,
            ptx,
            btn_y + (btn_h - 12) / 2,
            46,
            "Power",
            11.5,
            [245, 240, 245, 255],
        );
        self.anchor_power = px + right_x + pwr_w / 2;
        self.panel_hits.push(HitRegion {
            rect: Rectangle::new((px + right_x, py + btn_y).into(), (pwr_w, btn_h).into()),
            action: UiAction::TogglePowerMenu,
            right_action: None,
            scroll_action: None,
            id: next_id,
        });
        next_id += 1;
        right_x -= 6;

        // Clock & Date Pill (Clickable -> opens Calendar & Clock popover anchored right at pill!)
        let clock_label = format!("{clock_str}  {date_str}");
        let clock_w = (self.painter.measure_text(&clock_label, 12.5) + 20).max(118);
        right_x -= clock_w;
        let clock_active = self.active_popup == ActivePopup::CalendarClock;
        let clock_hover = self.hovered_hit_id == Some(next_id);
        let clock_bg = if clock_active {
            [accent[0], accent[1], accent[2], 235]
        } else if clock_hover {
            [48, 60, 92, 225]
        } else {
            [24, 31, 50, 220]
        };
        fill_rounded_rect(buf, pw, ph, right_x, btn_y, clock_w, btn_h, 10, clock_bg);
        self.painter.draw_text(
            buf,
            pw,
            ph,
            right_x + 10,
            btn_y + (btn_h - 13) / 2,
            clock_w - 14,
            &clock_label,
            12.5,
            [236, 242, 255, 255],
        );
        self.anchor_clock = px + right_x + clock_w / 2;
        self.panel_hits.push(HitRegion {
            rect: Rectangle::new((px + right_x, py + btn_y).into(), (clock_w, btn_h).into()),
            action: UiAction::ToggleCalendar,
            right_action: Some(UiAction::ShowDesktop),
            scroll_action: None,
            id: next_id,
        });
        next_id += 1;
        right_x -= 6;

        // Default anchors when any customizable pill is hidden by user configuration
        let fallback_anchor = px + right_x;
        self.anchor_notif = fallback_anchor;
        self.anchor_active = fallback_anchor;
        self.anchor_control = fallback_anchor;
        self.anchor_net = fallback_anchor;
        self.anchor_bri = fallback_anchor;
        self.anchor_vol = fallback_anchor;

        // Customizable right-side Quick Access pills rendered right-to-left so config array order
        // matches left-to-right visual order on the panel.
        let ordered_tiles = config.effective_taskbar_tiles();
        for tile_id in ordered_tiles.iter().rev() {
            match tile_id.as_str() {
                "notif" => {
                    let has_update = updates.local_update_ready || updates.github_latest_tag.is_some();
                    let notif_label = if has_update && notifs.status_bar_unread_count > 0 {
                        format!("Upd {}", notifs.status_bar_unread_count)
                    } else if has_update {
                        "Upd!".to_string()
                    } else if config.notifications_dnd {
                        "DND".to_string()
                    } else if notifs.cooldown_active && notifs.cooldown_received_count > 0 {
                        format!("Cool +{}", notifs.cooldown_received_count)
                    } else if notifs.cooldown_active {
                        format!("Cool {}s", notifs.cooldown_remaining_secs)
                    } else if notifs.status_bar_unread_count > 0 {
                        format!("🔔 {}", notifs.status_bar_unread_count)
                    } else if !notifs.items.is_empty() && !config.hide_silent_notifications_in_status_bar {
                        format!("Notif {}", notifs.items.len())
                    } else {
                        "Notif".to_string()
                    };
                    let notif_w = (self.painter.measure_text(&notif_label, 11.5) + 22).clamp(58, 108);
                    if right_x - notif_w <= cursor_x + 48 {
                        continue;
                    }
                    right_x -= notif_w;
                    let notif_active = self.active_popup == ActivePopup::Notifications;
                    let notif_hover = self.hovered_hit_id == Some(next_id);
                    let notif_bg = if notif_active {
                        [accent[0], accent[1], accent[2], 235]
                    } else if has_update {
                        [38, 96, 82, 230]
                    } else if notifs.cooldown_active {
                        [74, 58, 104, 230]
                    } else if notif_hover {
                        [48, 60, 92, 225]
                    } else if notifs.status_bar_unread_count > 0 && !config.notifications_dnd {
                        [34, 48, 82, 230]
                    } else {
                        [24, 31, 50, 220]
                    };
                    fill_rounded_rect(buf, pw, ph, right_x, btn_y, notif_w, btn_h, 10, notif_bg);
                    let dot_col = if has_update {
                        [72, 226, 152, 255]
                    } else if config.notifications_dnd {
                        [138, 148, 180, 220]
                    } else if notifs.cooldown_active {
                        [245, 184, 72, 255]
                    } else if notifs.status_bar_unread_count > 0 {
                        [accent[0], accent[1], accent[2], 255]
                    } else {
                        [96, 118, 158, 220]
                    };
                    fill_circle(buf, pw, ph, right_x + 9, btn_y + btn_h / 2, 3, dot_col);
                    self.painter.draw_text(
                        buf,
                        pw,
                        ph,
                        right_x + 17,
                        btn_y + (btn_h - 12) / 2,
                        notif_w - 20,
                        &notif_label,
                        11.5,
                        [232, 240, 255, 255],
                    );
                    self.anchor_notif = px + right_x + notif_w / 2;
                    self.panel_hits.push(HitRegion {
                        rect: Rectangle::new((px + right_x, py + btn_y).into(), (notif_w, btn_h).into()),
                        action: UiAction::ToggleNotifications,
                        right_action: Some(UiAction::ToggleDnd),
                        scroll_action: None,
                        id: next_id,
                    });
                    next_id += 1;
                    right_x -= 6;
                }
                "bg_processes" => {
                    let bg_count = sys.bg_apps.len();
                    let active_label = if total_windows > 0 {
                        format!("{} Win | {} Bg", total_windows, bg_count)
                    } else {
                        format!("{} Active", bg_count)
                    };
                    let act_w = (self.painter.measure_text(&active_label, 11.5) + 24).clamp(78, 128);
                    if right_x - act_w <= cursor_x + 48 {
                        continue;
                    }
                    right_x -= act_w;
                    let act_active = self.active_popup == ActivePopup::ActiveApps;
                    let act_hover = self.hovered_hit_id == Some(next_id);
                    let act_bg = if act_active {
                        [accent[0], accent[1], accent[2], 235]
                    } else if act_hover {
                        [48, 60, 92, 225]
                    } else {
                        [24, 31, 50, 220]
                    };
                    fill_rounded_rect(buf, pw, ph, right_x, btn_y, act_w, btn_h, 10, act_bg);
                    fill_circle(
                        buf,
                        pw,
                        ph,
                        right_x + 10,
                        btn_y + btn_h / 2,
                        3,
                        [56, 204, 142, 255],
                    );
                    self.painter.draw_text(
                        buf,
                        pw,
                        ph,
                        right_x + 18,
                        btn_y + (btn_h - 12) / 2,
                        act_w - 22,
                        &active_label,
                        11.5,
                        [228, 236, 254, 255],
                    );
                    self.anchor_active = px + right_x + act_w / 2;
                    self.panel_hits.push(HitRegion {
                        rect: Rectangle::new((px + right_x, py + btn_y).into(), (act_w, btn_h).into()),
                        action: UiAction::ToggleActiveApps,
                        right_action: Some(UiAction::ToggleActiveApps),
                        scroll_action: None,
                        id: next_id,
                    });
                    next_id += 1;
                    right_x -= 6;
                }
                "control" => {
                    let set_w = 68;
                    if right_x - set_w <= cursor_x + 48 {
                        continue;
                    }
                    right_x -= set_w;
                    let set_active = matches!(
                        self.active_popup,
                        ActivePopup::QuickSettings | ActivePopup::ControlCenter
                    );
                    let set_hover = self.hovered_hit_id == Some(next_id);
                    let set_bg = if set_active {
                        [accent[0], accent[1], accent[2], 235]
                    } else if set_hover {
                        [48, 60, 92, 225]
                    } else {
                        [28, 36, 56, 220]
                    };
                    fill_rounded_rect(buf, pw, ph, right_x, btn_y, set_w, btn_h, 10, set_bg);
                    let stx = right_x + (set_w - self.painter.measure_text("Control", 12.0)) / 2;
                    self.painter.draw_text(
                        buf,
                        pw,
                        ph,
                        stx,
                        btn_y + (btn_h - 12) / 2,
                        60,
                        "Control",
                        12.0,
                        [240, 244, 255, 255],
                    );
                    self.anchor_control = px + right_x + set_w / 2;
                    self.panel_hits.push(HitRegion {
                        rect: Rectangle::new((px + right_x, py + btn_y).into(), (set_w, btn_h).into()),
                        action: UiAction::ToggleQuickSettings,
                        right_action: Some(UiAction::LaunchSystemSettings),
                        scroll_action: None,
                        id: next_id,
                    });
                    next_id += 1;
                    right_x -= 6;
                }
                "wifi" => {
                    let net_short: String = sys.network_label.chars().take(8).collect();
                    let net_label = if sys.bluetooth_on {
                        format!("{net_short}|BT")
                    } else {
                        net_short
                    };
                    let net_w = (self.painter.measure_text(&net_label, 11.5) + 20).clamp(58, 114);
                    if right_x - net_w <= cursor_x + 48 {
                        continue;
                    }
                    right_x -= net_w;
                    let net_active = self.active_popup == ActivePopup::NetworkBt;
                    let net_hover = self.hovered_hit_id == Some(next_id);
                    fill_rounded_rect(
                        buf,
                        pw,
                        ph,
                        right_x,
                        btn_y,
                        net_w,
                        btn_h,
                        10,
                        if net_active {
                            [accent[0], accent[1], accent[2], 235]
                        } else if net_hover {
                            [48, 60, 92, 225]
                        } else {
                            [24, 31, 50, 215]
                        },
                    );
                    fill_circle(
                        buf,
                        pw,
                        ph,
                        right_x + 9,
                        btn_y + btn_h / 2,
                        3,
                        if sys.wifi_enabled {
                            [56, 204, 142, 255]
                        } else {
                            [225, 112, 85, 255]
                        },
                    );
                    self.painter.draw_text(
                        buf,
                        pw,
                        ph,
                        right_x + 16,
                        btn_y + (btn_h - 12) / 2,
                        net_w - 20,
                        &net_label,
                        11.5,
                        [222, 232, 250, 255],
                    );
                    self.anchor_net = px + right_x + net_w / 2;
                    self.panel_hits.push(HitRegion {
                        rect: Rectangle::new((px + right_x, py + btn_y).into(), (net_w, btn_h).into()),
                        action: UiAction::ToggleNetworkPopup,
                        right_action: Some(UiAction::ToggleWifi),
                        scroll_action: None,
                        id: next_id,
                    });
                    next_id += 1;
                    right_x -= 6;
                }
                "bright" => {
                    let bri_label = format!("Bri {}%", sys.brightness_pct);
                    let bri_w = (self.painter.measure_text(&bri_label, 11.5) + 16).max(60);
                    if right_x - bri_w <= cursor_x + 48 {
                        continue;
                    }
                    right_x -= bri_w;
                    let bri_active = self.active_popup == ActivePopup::Brightness;
                    let bri_hover = self.hovered_hit_id == Some(next_id);
                    fill_rounded_rect(
                        buf,
                        pw,
                        ph,
                        right_x,
                        btn_y,
                        bri_w,
                        btn_h,
                        10,
                        if bri_active {
                            [accent[0], accent[1], accent[2], 235]
                        } else if bri_hover {
                            [48, 60, 92, 225]
                        } else {
                            [24, 31, 50, 215]
                        },
                    );
                    let btx = right_x + (bri_w - self.painter.measure_text(&bri_label, 11.5)) / 2;
                    self.painter.draw_text(
                        buf,
                        pw,
                        ph,
                        btx,
                        btn_y + (btn_h - 12) / 2,
                        bri_w - 8,
                        &bri_label,
                        11.5,
                        [242, 220, 152, 255],
                    );
                    self.anchor_bri = px + right_x + bri_w / 2;
                    self.panel_hits.push(HitRegion {
                        rect: Rectangle::new((px + right_x, py + btn_y).into(), (bri_w, btn_h).into()),
                        action: UiAction::ToggleBrightnessPopup,
                        right_action: None,
                        scroll_action: Some((
                            UiAction::AdjustBrightness(5),
                            UiAction::AdjustBrightness(-5),
                        )),
                        id: next_id,
                    });
                    next_id += 1;
                    right_x -= 6;
                }
                "vol" => {
                    let (vol_pct, vol_muted) = sys.default_output_vol();
                    let vol_label = if vol_muted {
                        format!("Mute {}%", vol_pct)
                    } else {
                        format!("Vol {}%", vol_pct)
                    };
                    let vol_w = (self.painter.measure_text(&vol_label, 11.5) + 22).max(70);
                    if right_x - vol_w <= cursor_x + 48 {
                        continue;
                    }
                    right_x -= vol_w;
                    let vol_active = self.active_popup == ActivePopup::AudioMixer;
                    let vol_hover = self.hovered_hit_id == Some(next_id);
                    let vol_bg = if vol_active {
                        [accent[0], accent[1], accent[2], 235]
                    } else if vol_hover {
                        [48, 60, 92, 225]
                    } else if vol_muted {
                        [92, 42, 56, 210]
                    } else {
                        [26, 35, 58, 220]
                    };
                    fill_rounded_rect(buf, pw, ph, right_x, btn_y, vol_w, btn_h, 10, vol_bg);
                    let bar_w = ((vol_w - 14) * vol_pct.min(100) as i32) / 100;
                    fill_rounded_rect(
                        buf,
                        pw,
                        ph,
                        right_x + 7,
                        btn_y + btn_h - 4,
                        vol_w - 14,
                        2,
                        1,
                        [40, 50, 74, 180],
                    );
                    if !vol_muted && bar_w > 0 {
                        fill_rounded_rect(
                            buf,
                            pw,
                            ph,
                            right_x + 7,
                            btn_y + btn_h - 4,
                            bar_w,
                            2,
                            1,
                            [accent[0], accent[1], accent[2], 255],
                        );
                    }
                    let vtx = right_x + (vol_w - self.painter.measure_text(&vol_label, 11.5)) / 2;
                    self.painter.draw_text(
                        buf,
                        pw,
                        ph,
                        vtx,
                        btn_y + (btn_h - 14) / 2,
                        vol_w - 8,
                        &vol_label,
                        11.5,
                        [236, 242, 255, 255],
                    );
                    self.anchor_vol = px + right_x + vol_w / 2;
                    self.panel_hits.push(HitRegion {
                        rect: Rectangle::new((px + right_x, py + btn_y).into(), (vol_w, btn_h).into()),
                        action: UiAction::ToggleAudioMixer,
                        right_action: Some(UiAction::ToggleSinkMute("@DEFAULT_SINK@".to_string())),
                        scroll_action: Some((
                            UiAction::AdjustDefaultVol(5),
                            UiAction::AdjustDefaultVol(-5),
                        )),
                        id: next_id,
                    });
                    next_id += 1;
                    right_x -= 6;
                }
                "float" => {
                    let mode_label = config.layout_mode.short_label();
                    let mode_w = 46;
                    if right_x - mode_w <= cursor_x + 48 {
                        continue;
                    }
                    right_x -= mode_w;
                    let mode_hover = self.hovered_hit_id == Some(next_id);
                    fill_rounded_rect(
                        buf,
                        pw,
                        ph,
                        right_x,
                        btn_y,
                        mode_w,
                        btn_h,
                        10,
                        if mode_hover {
                            [accent[0], accent[1], accent[2], 185]
                        } else {
                            [28, 36, 56, 215]
                        },
                    );
                    let mtx = right_x + (mode_w - self.painter.measure_text(mode_label, 11.5)) / 2;
                    self.painter.draw_text(
                        buf,
                        pw,
                        ph,
                        mtx,
                        btn_y + (btn_h - 12) / 2,
                        mode_w,
                        mode_label,
                        11.5,
                        [232, 240, 255, 255],
                    );
                    self.panel_hits.push(HitRegion {
                        rect: Rectangle::new((px + right_x, py + btn_y).into(), (mode_w, btn_h).into()),
                        action: UiAction::CycleLayoutMode,
                        right_action: Some(UiAction::OpenQuickTileCustomizer),
                        scroll_action: None,
                        id: next_id,
                    });
                    next_id += 1;
                    right_x -= 6;
                }
                _ => {}
            }
        }
        right_x -= 4;

        // 4. ChromeOS-Style Centered App Shelf (Taskbar)
        let avail_w = (right_x - cursor_x).max(0);
        if !tasks.is_empty() && avail_w > 70 {
            let max_task_w = 190;
            let each_w = ((avail_w - (tasks.len() as i32 - 1) * 8) / tasks.len() as i32)
                .clamp(84, max_task_w);
            let total_shelf_w =
                tasks.len() as i32 * each_w + (tasks.len() as i32 - 1) * 8;
            let mut sx = cursor_x + ((avail_w - total_shelf_w).max(0) / 2);

            for (idx, task) in tasks.iter().enumerate() {
                if sx + each_w > right_x {
                    break;
                }
                let hovered = self.hovered_hit_id == Some(next_id);
                let bg = if task.focused {
                    [42, 54, 86, 245]
                } else if hovered {
                    [34, 44, 72, 230]
                } else if task.minimized {
                    [16, 20, 34, 165]
                } else {
                    [22, 28, 46, 200]
                };
                fill_rounded_rect(buf, pw, ph, sx, btn_y, each_w, btn_h, 11, bg);

                // ChromeOS-style glowing dot/pill indicator under active window
                if task.focused {
                    fill_rounded_rect(
                        buf,
                        pw,
                        ph,
                        sx + each_w / 2 - 14,
                        btn_y + btn_h - 3,
                        28,
                        3,
                        1,
                        [accent[0], accent[1], accent[2], 255],
                    );
                } else if task.minimized {
                    fill_circle(
                        buf,
                        pw,
                        ph,
                        sx + each_w / 2,
                        btn_y + btn_h - 3,
                        2,
                        [95, 110, 140, 160],
                    );
                } else {
                    fill_circle(
                        buf,
                        pw,
                        ph,
                        sx + each_w / 2,
                        btn_y + btn_h - 3,
                        2,
                        [150, 168, 204, 210],
                    );
                }

                // App badge dot: dedicated `>_` glyph for terminals, initial letter otherwise.
                let badge_txt = if task.is_terminal {
                    ">_".to_string()
                } else {
                    task.title
                        .chars()
                        .next()
                        .unwrap_or('W')
                        .to_uppercase()
                        .to_string()
                };
                let badge_font = if task.is_terminal { 10.5 } else { 11.5 };
                let badge_col = if task.focused {
                    [accent[0], accent[1], accent[2], 240]
                } else if task.is_terminal {
                    [32, 92, 108, 230]
                } else {
                    [52, 66, 98, 220]
                };
                let badge_r = (btn_h - 10).clamp(16, 22);
                fill_rounded_rect(
                    buf,
                    pw,
                    ph,
                    sx + 6,
                    btn_y + (btn_h - badge_r) / 2,
                    badge_r,
                    badge_r,
                    6,
                    badge_col,
                );
                let itx = sx + 6 + ((badge_r - self.painter.measure_text(&badge_txt, badge_font)) / 2).max(1);
                self.painter.draw_text(
                    buf,
                    pw,
                    ph,
                    itx,
                    btn_y + (btn_h - 12) / 2,
                    badge_r,
                    &badge_txt,
                    badge_font,
                    [248, 250, 255, 255],
                );
                if task.always_on_top {
                    fill_circle(
                        buf,
                        pw,
                        ph,
                        sx + 6 + badge_r - 2,
                        btn_y + (btn_h - badge_r) / 2 + 2,
                        3,
                        [245, 198, 72, 255],
                    );
                }

                let suffix = if task.minimized {
                    " ·Min"
                } else if task.minimal_view {
                    " ·Card"
                } else if task.always_on_top {
                    " ·Top"
                } else if task.maximized {
                    " ·Max"
                } else if task.floating {
                    " ·F"
                } else {
                    ""
                };
                let title = if suffix.is_empty() {
                    task.title.clone()
                } else {
                    format!("{}{}", task.title, suffix)
                };
                self.painter.draw_text(
                    buf,
                    pw,
                    ph,
                    sx + badge_r + 12,
                    btn_y + (btn_h - 12) / 2,
                    each_w - badge_r - 32,
                    &title,
                    12.5,
                    if task.focused {
                        [248, 250, 255, 255]
                    } else if task.minimized {
                        [142, 156, 186, 200]
                    } else {
                        [192, 206, 234, 240]
                    },
                );

                self.panel_hits.push(HitRegion {
                    rect: Rectangle::new((px + sx, py + btn_y).into(), (each_w - 20, btn_h).into()),
                    action: UiAction::FocusWindow(idx),
                    right_action: Some(UiAction::OpenContextMenu {
                        target: ContextTarget::TaskbarWindow(idx),
                        pos: (px + sx, py),
                    }),
                    scroll_action: None,
                    id: next_id,
                });
                next_id += 1;

                let close_hover = self.hovered_hit_id == Some(next_id);
                let cx_btn = sx + each_w - 20;
                if close_hover || task.focused {
                    if close_hover {
                        fill_rounded_rect(
                            buf,
                            pw,
                            ph,
                            cx_btn,
                            btn_y + 5,
                            16,
                            btn_h - 10,
                            5,
                            [200, 62, 82, 235],
                        );
                    }
                    self.painter.draw_text(
                        buf,
                        pw,
                        ph,
                        cx_btn + 4,
                        btn_y + (btn_h - 11) / 2,
                        12,
                        "x",
                        11.0,
                        if close_hover {
                            [255, 255, 255, 255]
                        } else {
                            [150, 165, 195, 210]
                        },
                    );
                }
                self.panel_hits.push(HitRegion {
                    rect: Rectangle::new((px + cx_btn, py + btn_y).into(), (20, btn_h).into()),
                    action: UiAction::CloseWindow(idx),
                    right_action: None,
                    scroll_action: None,
                    id: next_id,
                });
                next_id += 1;

                sx += each_w + 8;
            }
        }

        let panel_bytes = std::mem::take(&mut self.panel_pixels);
        update_memory_buffer(&mut self.panel_buffer, &mut self.panel_buf_size, (pw, ph), &panel_bytes);
        self.panel_pixels = panel_bytes;
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_popup(
        &mut self,
        panel_geo: Rectangle<i32, Logical>,
        popup_geo: Rectangle<i32, Logical>,
        config: &Config,
        ws_counts: &[usize; 9],
        tasks: &[TaskWindowInfo],
        global_windows: &[GlobalWindowInfo],
        apps: &[DesktopApp],
        sys: &SystemSnapshot,
        notifs: &NotificationSnapshot,
        updates: &UpdateStatus,
        clock_full_str: &str,
    ) {
        if self.active_popup == ActivePopup::None {
            self.popup_hits.clear();
            self.popup_signature.clear();
            return;
        }

        let ph = config.panel_height.clamp(34, 56);
        let edge_pad = if config.shelf_style == ShelfStyle::Floating {
            16
        } else {
            10
        };
        let shelf_y = |h: i32| -> i32 {
            panel_geo.loc.y
                + match config.panel_position {
                    PanelPosition::Top => ph + edge_pad,
                    PanelPosition::Bottom => (panel_geo.size.h - ph - h - edge_pad).max(10),
                }
        };
        let anchored_x = |center: i32, w: i32| -> i32 {
            let min_x = panel_geo.loc.x + 12;
            let max_x = (panel_geo.loc.x + panel_geo.size.w - w - 12).max(min_x);
            (center - w / 2).clamp(min_x, max_x)
        };
        let center_in_popup = |w: i32, h: i32| -> (i32, i32) {
            (
                popup_geo.loc.x + ((popup_geo.size.w - w) / 2).max(12),
                popup_geo.loc.y + ((popup_geo.size.h - h) / 2).max(12),
            )
        };

        let (pw, p_height, px, py) = match &self.active_popup {
            ActivePopup::None => return,
            ActivePopup::StartMenu => {
                let w = 680.min(panel_geo.size.w - 24).max(360);
                let h = 592.min(panel_geo.size.h - ph - 32).max(320);
                (w, h, anchored_x(self.anchor_start, w), shelf_y(h))
            }
            ActivePopup::AudioMixer => {
                let w = 490.min(panel_geo.size.w - 24).max(340);
                let h = 580.min(panel_geo.size.h - ph - 32).max(340);
                (w, h, anchored_x(self.anchor_vol, w), shelf_y(h))
            }
            ActivePopup::Brightness => {
                let w = 320.min(panel_geo.size.w - 24).max(260);
                let h = 216;
                (w, h, anchored_x(self.anchor_bri, w), shelf_y(h))
            }
            ActivePopup::NetworkBt => {
                let w = 348.min(panel_geo.size.w - 24).max(280);
                let h = 248;
                (w, h, anchored_x(self.anchor_net, w), shelf_y(h))
            }
            ActivePopup::QuickSettings => {
                let w = 420.min(panel_geo.size.w - 24).max(320);
                let base_h = if self.customize_quick_tiles { 676 } else { 516 };
                let h = base_h.min(panel_geo.size.h - ph - 32).max(380);
                (w, h, anchored_x(self.anchor_control, w), shelf_y(h))
            }
            ActivePopup::ActiveApps => {
                let w = 472.min(panel_geo.size.w - 24).max(340);
                let h = 532.min(panel_geo.size.h - ph - 32).max(320);
                (w, h, anchored_x(self.anchor_active, w), shelf_y(h))
            }
            ActivePopup::CalendarClock => {
                let w = 352.min(panel_geo.size.w - 24).max(300);
                let h = 446.min(panel_geo.size.h - ph - 32).max(360);
                (w, h, anchored_x(self.anchor_clock, w), shelf_y(h))
            }
            ActivePopup::TaskView => {
                let w = 900.min(popup_geo.size.w - 40).max(460);
                let h = 560.min(popup_geo.size.h - ph - 48).max(340);
                let (cx, cy) = center_in_popup(w, h);
                (w, h, cx, cy)
            }
            ActivePopup::ControlCenter => {
                let w = 740.min(panel_geo.size.w - 24).max(480);
                let h = 606.min(panel_geo.size.h - ph - 32).max(380);
                (w, h, anchored_x(self.anchor_control, w), shelf_y(h))
            }
            ActivePopup::PowerMenu => {
                let w = 236;
                let h = 268;
                (w, h, anchored_x(self.anchor_power, w), shelf_y(h))
            }
            ActivePopup::Notifications => {
                let w = 456.min(panel_geo.size.w - 24).max(340);
                let h = 610.min(panel_geo.size.h - ph - 32).max(360);
                (w, h, anchored_x(self.anchor_notif, w), shelf_y(h))
            }
            ActivePopup::AltTab => {
                let w = 540.min(popup_geo.size.w - 40).max(320);
                let rows = global_windows.len().clamp(1, 7) as i32;
                let h = (76 + rows * 52).min(popup_geo.size.h - 80);
                let (cx, cy) = center_in_popup(w, h);
                (w, h, cx, cy)
            }
            ActivePopup::DisplayConfirm => {
                let w = 460.min(popup_geo.size.w - 32).max(320);
                let h = 196;
                let (cx, cy) = center_in_popup(w, h);
                (w, h, cx, cy)
            }
            ActivePopup::ReloadConfirm { .. } => {
                let w = 568.min(popup_geo.size.w - 32).max(340);
                let h = 304;
                let (cx, cy) = center_in_popup(w, h);
                (w, h, cx, cy)
            }
            ActivePopup::ContextMenu { target, pos } => {
                let w = 256;
                let h = match target {
                    ContextTarget::Desktop => match self.context_submenu {
                        ContextSubmenu::Apps => 420,
                        ContextSubmenu::Layout => 452,
                        _ => 326,
                    },
                    ContextTarget::TaskbarWindow(_) => match self.context_submenu {
                        ContextSubmenu::WindowSnap => 492,
                        ContextSubmenu::WindowWorkspace => 468,
                        _ => 370,
                    },
                    ContextTarget::Terminal => 178,
                    ContextTarget::LibraryApp(_) => match self.context_submenu {
                        ContextSubmenu::AppCategory => 472,
                        _ => 264,
                    },
                };
                let target_geo = if matches!(target, ContextTarget::TaskbarWindow(_)) {
                    panel_geo
                } else {
                    popup_geo
                };
                let min_x = target_geo.loc.x + 10;
                let max_x = (target_geo.loc.x + target_geo.size.w - w - 10).max(min_x);
                let x = pos.0.clamp(min_x, max_x);
                let y = match target {
                    ContextTarget::Desktop | ContextTarget::Terminal | ContextTarget::LibraryApp(_) => {
                        let min_y = target_geo.loc.y + 10;
                        let bottom_pad = if target_geo == panel_geo
                            && config.panel_position == PanelPosition::Bottom
                        {
                            ph + 16
                        } else {
                            16
                        };
                        let max_y = (target_geo.loc.y + target_geo.size.h - h - bottom_pad).max(min_y);
                        pos.1.clamp(min_y, max_y)
                    }
                    ContextTarget::TaskbarWindow(_) => shelf_y(h),
                };
                (w, h, x, y)
            }
        };

        self.popup_rect = Rectangle::new((px, py).into(), (pw, p_height).into());

        let saved_banner = unix_secs_now() <= self.save_feedback_until_epoch;
        let clock_sig = if self.active_popup == ActivePopup::CalendarClock {
            clock_full_str
        } else {
            ""
        };
        let display_sig = self
            .display_telemetry
            .outputs
            .first()
            .map(|o| {
                format!(
                    "{}-{}-{}-{}",
                    o.current_hz, o.vrr_enabled, o.battery_saver_state, o.active_pacing_hz
                )
            })
            .unwrap_or_default();
        let confirm_sig = self
            .display_confirm
            .as_ref()
            .map(|c| {
                let rem = c
                    .verify_deadline
                    .map(|dl| dl.saturating_duration_since(Instant::now()).as_secs())
                    .unwrap_or(99);
                format!("{}:{}:{}", c.pending_hz, c.verify_deadline.is_some(), rem)
            })
            .unwrap_or_default();
        let cat_sig = format!(
            "{}:{}",
            self.selected_custom_category.as_deref().unwrap_or(""),
            self.auto_switched_from_category
                .as_ref()
                .map(|(c, cu)| format!("{c:?}:{cu:?}"))
                .unwrap_or_default()
        );
        let tiles_sig = format!(
            "{}:{}:{}:{}",
            self.customize_quick_tiles,
            config.taskbar_quick_tiles.join(","),
            config.hidden_quick_tiles.join(","),
            config.quick_panel_tiles.join(","),
        );
        let sig = format!(
            "{:?}:{:?}:{:?}:{}x{}@{},{}:{}:{:?}:{}:{}:{}:{}:{}:{:?}:{:?}:{}:{}:{}:{}:{}:{}:{}:{}:{:?}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            self.active_popup,
            self.settings_tab,
            self.context_submenu,
            pw,
            p_height,
            px,
            py,
            self.search_query,
            self.selected_category,
            cat_sig,
            self.app_scroll,
            self.selected_app_row,
            self.alt_tab_index,
            apps.len(),
            tasks,
            global_windows,
            sys.version,
            self.hovered_hit_id.unwrap_or(usize::MAX),
            config.accent_theme,
            config.wallpaper_preset,
            config.use_custom_wallpaper,
            config.panel_position.label(),
            config.shelf_style.label(),
            config.panel_opacity,
            config.layout_mode,
            saved_banner,
            self.calendar_month_offset,
            self.focus_timer_running,
            self.focus_timer_remaining_secs,
            clock_sig,
            display_sig,
            confirm_sig,
            tiles_sig,
            format!(
                "{}-{}-{}-{}-{}-{}-{}-{}-{}-{:?}-{}-{}-{}-{}-{}-{}-{}-{}:{}:{}:{}:{}:{}:{}:{}:{}",
                config.gap_px,
                (config.master_ratio * 100.0).round() as i32,
                config.border_width,
                config.corner_radius,
                config.edge_snap,
                config.windows_shortcuts,
                config.show_system_processes,
                config.output_refresh_hz,
                config.auto_route_audio_roles,
                config.vrr_mode,
                config.video_battery_saver,
                config.idle_refresh_hz,
                config.dim_unfocused_windows,
                config.freeze_hidden_windows,
                config.confirm_before_display_apply,
                config.auto_clean_ram_cache,
                sys.memory.used_pct,
                sys.memory.cleanups_done,
                notifs.version,
                self.notif_scroll,
                self.notif_tab as u8,
                self.notif_organizer_expanded,
                format_args!(
                    "{:?}-{}-{}",
                    self.notif_reply_target, self.notif_reply_draft, config.notifications_dnd
                ),
                format_args!(
                    "{}-{}-{}-{}-{}-{}-{}:{}:{}:{}:{}",
                    config.allow_notification_snooze,
                    config.hide_silent_notifications_in_status_bar,
                    config.notification_cooldown_enabled,
                    config.notification_organizer_enabled,
                    config.notification_summaries_enabled,
                    config.notification_history_enabled,
                    config.organized_silenced_categories.join(","),
                    config.priority_conversations.join(","),
                    config.disabled_notification_apps.join(","),
                    config.silent_notification_apps.join(","),
                    updates.local_update_ready,
                ),
                updates.checking,
                updates.github_latest_tag.as_deref().unwrap_or("")
            )
        );
        if sig == self.popup_signature && self.popup_buffer.is_some() {
            return;
        }
        self.popup_signature = sig;
        self.popup_hits.clear();

        let needed = (pw * p_height * 4) as usize;
        if self.popup_pixels.len() != needed {
            self.popup_pixels.resize(needed, 0);
        } else {
            self.popup_pixels.fill(0);
        }

        let mut buf = std::mem::take(&mut self.popup_pixels);
        fill_rounded_rect(&mut buf, pw, p_height, 0, 0, pw, p_height, 14, [12, 16, 30, 245]);
        stroke_rounded_rect(&mut buf, pw, p_height, 0, 0, pw, p_height, 14, 1, [82, 108, 160, 148]);

        match self.active_popup.clone() {
            ActivePopup::None => {}
            ActivePopup::StartMenu => self.draw_start_menu(&mut buf, pw, p_height, px, py, config, apps),
            ActivePopup::AudioMixer => self.draw_audio_mixer(&mut buf, pw, p_height, px, py, config, sys),
            ActivePopup::Brightness => self.draw_brightness_popup(&mut buf, pw, p_height, px, py, config, sys),
            ActivePopup::NetworkBt => self.draw_network_bt_popup(&mut buf, pw, p_height, px, py, config, sys),
            ActivePopup::QuickSettings => self.draw_quick_settings(&mut buf, pw, p_height, px, py, config, sys),
            ActivePopup::ActiveApps => {
                self.draw_active_apps(&mut buf, pw, p_height, px, py, config, tasks, sys)
            }
            ActivePopup::CalendarClock => {
                self.draw_calendar_clock(&mut buf, pw, p_height, px, py, config, clock_full_str)
            }
            ActivePopup::TaskView => {
                self.draw_task_view(&mut buf, pw, p_height, px, py, config, ws_counts, global_windows)
            }
            ActivePopup::ControlCenter => {
                self.draw_control_center(&mut buf, pw, p_height, px, py, config, sys, saved_banner)
            }
            ActivePopup::PowerMenu => self.draw_power_menu(&mut buf, pw, p_height, px, py, config),
            ActivePopup::Notifications => {
                self.draw_notifications_drawer(&mut buf, pw, p_height, px, py, config, apps, notifs, updates)
            }
            ActivePopup::AltTab => {
                self.draw_alt_tab(&mut buf, pw, p_height, px, py, config, global_windows)
            }
            ActivePopup::DisplayConfirm => {
                self.draw_display_confirm(&mut buf, pw, p_height, px, py, config)
            }
            ActivePopup::ReloadConfirm { ref reason } => {
                self.draw_reload_confirm(&mut buf, pw, p_height, px, py, config, reason)
            }
            ActivePopup::ContextMenu { target, .. } => {
                self.draw_context_menu(&mut buf, pw, p_height, px, py, config, apps, tasks, &target)
            }
        }

        update_memory_buffer(&mut self.popup_buffer, &mut self.popup_buf_size, (pw, p_height), &buf);
        self.popup_pixels = buf;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_active_apps(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        tasks: &[TaskWindowInfo],
        sys: &SystemSnapshot,
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 6000usize;
        let mut y = 14;

        self.painter.draw_text(
            buf,
            w,
            h,
            18,
            y,
            260,
            "Active Windows & Background Apps",
            15.0,
            [244, 247, 255, 255],
        );
        self.draw_close_btn(buf, w, h, ox, oy, y - 2, &mut hit_id);
        y += 32;

        // Open windows on current workspace
        self.painter.draw_text(
            buf,
            w,
            h,
            18,
            y,
            240,
            &format!("Open Windows ({})", tasks.len()),
            12.0,
            [176, 192, 224, 255],
        );
        y += 20;
        for (idx, t) in tasks.iter().take(4).enumerate() {
            fill_rounded_rect(buf, w, h, 14, y, w - 28, 30, 7, [22, 29, 48, 220]);
            self.painter
                .draw_text(buf, w, h, 24, y + 8, w - 224, &t.title, 12.0, [240, 245, 255, 255]);

            let ex_lbl = if t.maximized { "Shrink" } else { "Expand" };
            let ex_x = w - 208;
            let ex_hover = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                ex_x,
                y + 4,
                50,
                22,
                5,
                if ex_hover { [accent[0], accent[1], accent[2], 230] } else { [32, 42, 68, 220] },
            );
            self.painter
                .draw_text(buf, w, h, ex_x + 6, y + 9, 42, ex_lbl, 10.0, [242, 246, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + ex_x, oy + y + 4).into(), (50, 22).into()),
                action: UiAction::ToggleMaximizeWindow(idx),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;

            let mx = w - 154;
            let m_lbl = if t.minimized { "Show" } else { "Min" };
            let m_hover = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                mx,
                y + 4,
                38,
                22,
                5,
                if m_hover { [accent[0], accent[1], accent[2], 230] } else { [32, 42, 68, 220] },
            );
            self.painter
                .draw_text(buf, w, h, mx + 6, y + 9, 30, m_lbl, 10.0, [242, 246, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + mx, oy + y + 4).into(), (38, 22).into()),
                action: UiAction::MinimizeWindow(idx),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;

            let fx = w - 112;
            let f_hover = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                fx,
                y + 4,
                44,
                22,
                5,
                if f_hover { [accent[0], accent[1], accent[2], 230] } else { [32, 42, 68, 220] },
            );
            self.painter
                .draw_text(buf, w, h, fx + 6, y + 9, 36, "Focus", 10.0, [242, 246, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + fx, oy + y + 4).into(), (44, 22).into()),
                action: UiAction::FocusWindow(idx),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;

            let cx = w - 64;
            let c_hover = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                cx,
                y + 4,
                44,
                22,
                5,
                if c_hover { [200, 62, 82, 235] } else { [78, 40, 56, 215] },
            );
            self.painter
                .draw_text(buf, w, h, cx + 6, y + 9, 34, "Close", 10.0, [246, 242, 246, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + cx, oy + y + 4).into(), (44, 22).into()),
                action: UiAction::CloseWindow(idx),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            y += 34;
        }

        y += 6;
        self.painter.draw_text(
            buf,
            w,
            h,
            18,
            y,
            245,
            "User Background Apps (1-Click Closes)",
            11.5,
            [176, 192, 224, 255],
        );
        let sys_hover = self.hovered_hit_id == Some(hit_id);
        let sys_lbl = if config.show_system_processes {
            "Sys: ON"
        } else {
            "Sys: OFF"
        };
        fill_rounded_rect(
            buf,
            w,
            h,
            w - 94,
            y - 2,
            78,
            20,
            6,
            if config.show_system_processes {
                [accent[0], accent[1], accent[2], 225]
            } else if sys_hover {
                [48, 60, 92, 225]
            } else {
                [28, 36, 56, 220]
            },
        );
        self.painter.draw_text(
            buf,
            w,
            h,
            w - 82,
            y + 2,
            60,
            sys_lbl,
            10.5,
            [236, 242, 255, 255],
        );
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + w - 94, oy + y - 2).into(), (78, 20).into()),
            action: UiAction::ToggleShowSystemProcesses,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
        hit_id += 1;
        y += 22;

        let max_rows = ((h - y - 42) / 34).max(1) as usize;
        if sys.bg_apps.is_empty() {
            self.painter.draw_text(
                buf,
                w,
                h,
                22,
                y + 6,
                w - 44,
                "No standalone user background apps detected.",
                11.5,
                [146, 162, 194, 220],
            );
        }
        for proc_info in sys.bg_apps.iter().take(max_rows) {
            fill_rounded_rect(buf, w, h, 14, y, w - 28, 30, 7, [20, 26, 44, 210]);
            let label = if proc_info.pids.len() > 1 {
                format!("{}  ({}p · {} MB)", proc_info.name, proc_info.pids.len(), proc_info.rss_mb)
            } else {
                format!("{}  ({} MB)", proc_info.name, proc_info.rss_mb)
            };
            self.painter.draw_text(
                buf,
                w,
                h,
                24,
                y + 8,
                w - 130,
                &label,
                12.0,
                [230, 238, 254, 255],
            );

            let kx = w - 92;
            let k_hover = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                kx,
                y + 4,
                70,
                22,
                5,
                if k_hover { [210, 64, 84, 240] } else { [96, 44, 58, 220] },
            );
            self.painter.draw_text(
                buf,
                w,
                h,
                kx + 12,
                y + 9,
                54,
                "Turn Off",
                10.5,
                [248, 244, 248, 255],
            );
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + kx, oy + y + 4).into(), (70, 22).into()),
                action: UiAction::KillBgProcess {
                    pids: proc_info.pids.clone(),
                    force: false,
                },
                right_action: Some(UiAction::KillBgProcess {
                    pids: proc_info.pids.clone(),
                    force: true,
                }),
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            y += 34;
        }

        let r_hover = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            w - 104,
            h - 36,
            88,
            24,
            6,
            if r_hover { [accent[0], accent[1], accent[2], 225] } else { [30, 38, 60, 220] },
        );
        self.painter
            .draw_text(buf, w, h, w - 84, h - 30, 64, "Refresh", 11.5, [242, 246, 255, 255]);
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + w - 104, oy + h - 36).into(), (88, 24).into()),
            action: UiAction::RefreshSystem,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_alt_tab(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        global_windows: &[GlobalWindowInfo],
    ) {
        let accent = config.accent_rgb();
        let mut y = 16;
        self.painter.draw_text(
            buf,
            w,
            h,
            20,
            y,
            w - 40,
            "Switch Window (Alt+Tab / Click)",
            14.5,
            [244, 247, 255, 255],
        );
        y += 34;

        if global_windows.is_empty() {
            self.painter.draw_text(
                buf,
                w,
                h,
                24,
                y + 10,
                w - 48,
                "No open windows.",
                13.0,
                [150, 165, 196, 230],
            );
            return;
        }

        let sel = self.alt_tab_index.min(global_windows.len() - 1);
        for (idx, gw) in global_windows.iter().take(7).enumerate() {
            let selected = idx == sel;
            let hit_id = 7000 + idx;
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if selected {
                [accent[0], accent[1], accent[2], 145]
            } else if hovered {
                [38, 48, 76, 225]
            } else {
                [20, 26, 44, 200]
            };
            fill_rounded_rect(buf, w, h, 14, y, w - 28, 44, 9, bg);
            if selected {
                stroke_rounded_rect(
                    buf,
                    w,
                    h,
                    14,
                    y,
                    w - 28,
                    44,
                    9,
                    2,
                    [accent[0], accent[1], accent[2], 255],
                );
            }

            let ws_tag = format!("WS {}", gw.workspace + 1);
            fill_rounded_rect(buf, w, h, 24, y + 11, 44, 22, 6, [28, 36, 58, 235]);
            self.painter
                .draw_text(buf, w, h, 31, y + 16, 36, &ws_tag, 11.0, [214, 226, 252, 255]);

            self.painter.draw_text(
                buf,
                w,
                h,
                78,
                y + 14,
                w - 190,
                &gw.title,
                13.5,
                [246, 249, 255, 255],
            );

            let status = if gw.focused {
                "Active"
            } else if gw.floating {
                "Floating"
            } else {
                "Tiled"
            };
            self.painter.draw_text(
                buf,
                w,
                h,
                w - 86,
                y + 15,
                64,
                status,
                11.5,
                if gw.focused {
                    [110, 235, 175, 255]
                } else {
                    [170, 186, 218, 230]
                },
            );

            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + 14, oy + y).into(), (w - 28, 44).into()),
                action: UiAction::SelectAltTab(idx),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            y += 50;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_context_menu(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        apps: &[DesktopApp],
        tasks: &[TaskWindowInfo],
        target: &ContextTarget,
    ) {
        let accent = config.accent_rgb();
        let mut items: Vec<(String, UiAction, bool)> = Vec::new();
        match target {
            ContextTarget::Desktop => {
                items.push(("Open Terminal".into(), UiAction::LaunchQuickTerminal, false));
                items.push(("Open File Manager".into(), UiAction::LaunchQuickFiles, false));
                items.push(("Open Web Browser".into(), UiAction::LaunchQuickBrowser, false));
                let apps_exp = self.context_submenu == ContextSubmenu::Apps;
                items.push((
                    if apps_exp { "v Launch & Views" } else { "> Launch & Views" }.into(),
                    UiAction::ToggleContextSubmenu(ContextSubmenu::Apps),
                    apps_exp,
                ));
                if apps_exp {
                    items.push(("   · Applications Menu".into(), UiAction::ToggleStartMenu, false));
                    items.push(("   · Task View (Win+Tab)".into(), UiAction::ToggleTaskView, false));
                    items.push(("   · Show Desktop (Win+D)".into(), UiAction::ShowDesktop, false));
                }
                let lay_exp = self.context_submenu == ContextSubmenu::Layout;
                items.push((
                    if lay_exp { "v Layout & Visuals" } else { "> Layout & Visuals" }.into(),
                    UiAction::ToggleContextSubmenu(ContextSubmenu::Layout),
                    lay_exp,
                ));
                if lay_exp {
                    items.push(("   · Cycle Layout Mode".into(), UiAction::CycleLayoutMode, false));
                    items.push(("   · Next Wallpaper".into(), UiAction::CycleWallpaperPreset, false));
                    items.push(("   · Toggle Edge Snap".into(), UiAction::ToggleEdgeSnap, false));
                    items.push(("   · Audio Mixer".into(), UiAction::ToggleAudioMixer, false));
                }
                items.push(("Customize Taskbar Tiles".into(), UiAction::OpenQuickTileCustomizer, false));
                items.push(("System Settings App".into(), UiAction::LaunchSystemSettings, false));
                items.push(("Active & Background Apps".into(), UiAction::ToggleActiveApps, false));
                items.push(("Exit to LightDM".into(), UiAction::QuitSession, false));
            }
            ContextTarget::TaskbarWindow(idx) => {
                let info = tasks.get(*idx);
                let expand_lbl = if info.is_some_and(|t| t.maximized) {
                    "Shrink / Restore Window"
                } else {
                    "Expand / Maximize Window"
                };
                let min_lbl = if info.is_some_and(|t| t.minimized) {
                    "Restore Window"
                } else {
                    "Minimize Window"
                };
                let min_card_active = info.is_some_and(|t| t.minimal_view);
                items.push(("Focus / Raise Window".into(), UiAction::FocusWindow(*idx), false));
                let is_pinned = info.is_some_and(|t| t.always_on_top);
                items.push((
                    if is_pinned {
                        "Unpin from Top"
                    } else {
                        "Pin on Top (Keep Above)"
                    }
                    .into(),
                    UiAction::ToggleAlwaysOnTopWindow(*idx),
                    is_pinned,
                ));
                items.push(("Open Another Window".into(), UiAction::OpenNewInstanceWindow(*idx), false));
                items.push((
                    if min_card_active {
                        "Expand from Minimal Card"
                    } else {
                        "Minimal Card View"
                    }
                    .into(),
                    UiAction::ToggleMinimalViewWindow(*idx),
                    min_card_active,
                ));
                items.push((expand_lbl.into(), UiAction::ToggleMaximizeWindow(*idx), false));
                items.push((min_lbl.into(), UiAction::MinimizeWindow(*idx), false));
                let snap_exp = self.context_submenu == ContextSubmenu::WindowSnap;
                items.push((
                    if snap_exp { "v Snap & Mode" } else { "> Snap & Mode" }.into(),
                    UiAction::ToggleContextSubmenu(ContextSubmenu::WindowSnap),
                    snap_exp,
                ));
                if snap_exp {
                    items.push(("   · Snap Left Half".into(), UiAction::SnapWindow(*idx, SnapZone::LeftHalf), false));
                    items.push(("   · Snap Right Half".into(), UiAction::SnapWindow(*idx, SnapZone::RightHalf), false));
                    items.push(("   · Toggle Floating".into(), UiAction::ToggleFloatWindow(*idx), false));
                    items.push(("   · Toggle Fullscreen".into(), UiAction::ToggleFullscreenWindow(*idx), false));
                }
                let ws_exp = self.context_submenu == ContextSubmenu::WindowWorkspace;
                items.push((
                    if ws_exp { "v Move Workspace" } else { "> Move Workspace" }.into(),
                    UiAction::ToggleContextSubmenu(ContextSubmenu::WindowWorkspace),
                    ws_exp,
                ));
                if ws_exp {
                    items.push(("   · Send to Next WS".into(), UiAction::MoveWindowNextWs(*idx), false));
                    items.push(("   · Move to Next Monitor".into(), UiAction::MoveWindowNextMonitor, false));
                    items.push(("   · Task View Cards".into(), UiAction::ToggleTaskView, false));
                }
                items.push(("Send to Next Workspace".into(), UiAction::MoveWindowNextWs(*idx), false));
                items.push(("Close Window".into(), UiAction::CloseWindow(*idx), false));
            }
            ContextTarget::Terminal => {
                items.push(("Copy Selected Text".into(), UiAction::TerminalCopyPrimary, false));
                items.push(("Paste Clipboard".into(), UiAction::TerminalPasteClipboard, false));
                items.push(("Paste Selection (Middle)".into(), UiAction::TerminalPasteSelection, false));
                items.push(("Open Another Window".into(), UiAction::OpenNewTerminalForFocused, false));
                items.push(("Close Terminal".into(), UiAction::CloseFocusedTerminal, false));
            }
            ContextTarget::LibraryApp(app_idx) => {
                let app = apps.get(*app_idx);
                items.push((
                    app.map(|a| format!("Launch {}", a.name))
                        .unwrap_or_else(|| "Launch App".to_string()),
                    UiAction::LaunchApp(*app_idx),
                    false,
                ));
                items.push((
                    "Edit Values in Settings...".into(),
                    UiAction::EditLibraryAppInSettings(*app_idx),
                    true,
                ));
                let cat_exp = self.context_submenu == ContextSubmenu::AppCategory;
                let cur_cat = app
                    .map(|a| a.effective_category_label().to_string())
                    .unwrap_or_else(|| "Other".to_string());
                items.push((
                    if cat_exp {
                        format!("v Category ({cur_cat})")
                    } else {
                        format!("> Category ({cur_cat})")
                    },
                    UiAction::ToggleContextSubmenu(ContextSubmenu::AppCategory),
                    cat_exp,
                ));
                if cat_exp {
                    for &cat in AppCategory::ASSIGNABLE.iter() {
                        let is_cur = app.is_some_and(|a| a.category == cat);
                        items.push((
                            format!("   · {}", cat.label()),
                            UiAction::SetLibraryAppCategory(*app_idx, cat),
                            is_cur,
                        ));
                    }
                }
                let is_term = app.is_some_and(|a| a.is_terminal_app());
                items.push((
                    if is_term {
                        "Run in Terminal: ON"
                    } else {
                        "Run in Terminal: OFF"
                    }
                    .into(),
                    UiAction::ToggleLibraryAppTerminal(*app_idx),
                    is_term,
                ));
                let game_lbl = match app.and_then(|a| a.is_game_override) {
                    None => "Game Detect: Auto",
                    Some(true) => "Game Detect: Force ON",
                    Some(false) => "Game Detect: Force OFF",
                };
                items.push((
                    game_lbl.into(),
                    UiAction::CycleLibraryAppGameOverride(*app_idx),
                    app.and_then(|a| a.is_game_override).is_some(),
                ));
                let is_hidden = app.is_some_and(|a| a.hidden);
                items.push((
                    if is_hidden {
                        "Unhide in App Library"
                    } else {
                        "Hide from App Library"
                    }
                    .into(),
                    UiAction::ToggleLibraryAppHidden(*app_idx),
                    is_hidden,
                ));
            }
        }

        let mut y = 10;
        for (i, (label, act, highlighted)) in items.into_iter().enumerate() {
            if y + 28 > h - 6 {
                break;
            }
            let hit_id = 8000 + i;
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if hovered {
                [accent[0], accent[1], accent[2], 225]
            } else if highlighted {
                [36, 48, 80, 225]
            } else {
                [22, 28, 46, 200]
            };
            fill_rounded_rect(buf, w, h, 10, y, w - 20, 27, 7, bg);
            self.painter.draw_text(
                buf,
                w,
                h,
                20,
                y + 7,
                w - 36,
                &label,
                12.0,
                [242, 246, 255, 255],
            );
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + 10, oy + y).into(), (w - 20, 27).into()),
                action: act,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            y += 32;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_brightness_popup(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        sys: &SystemSnapshot,
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 9100usize;
        let mut y = 14;
        self.painter.draw_text(
            buf,
            w,
            h,
            18,
            y,
            220,
            "Display Brightness",
            15.0,
            [244, 247, 255, 255],
        );
        self.draw_close_btn(buf, w, h, ox, oy, y - 2, &mut hit_id);
        y += 34;

        fill_rounded_rect(buf, w, h, 14, y, w - 28, 66, 10, [20, 26, 44, 220]);
        let bri_lbl = format!("Brightness {}%", sys.brightness_pct);
        self.painter
            .draw_text(buf, w, h, 24, y + 10, 180, &bri_lbl, 13.0, [242, 220, 152, 255]);
        self.draw_slider(
            buf,
            w,
            h,
            ox,
            oy,
            24,
            y + 34,
            w - 48,
            sys.brightness_pct as i32,
            5,
            100,
            [245, 188, 66],
            SliderTarget::Brightness,
            &mut hit_id,
        );
        y += 76;

        let mut bx = 14;
        let preset_w = (w - 28 - 18) / 4;
        for pct in [25, 50, 75, 100] {
            let active = (sys.brightness_pct as i32 - pct).abs() <= 3;
            let hovered = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                bx,
                y,
                preset_w,
                28,
                7,
                if active {
                    [accent[0], accent[1], accent[2], 225]
                } else if hovered {
                    [44, 56, 86, 225]
                } else {
                    [26, 34, 54, 215]
                },
            );
            let txt = format!("{pct}%");
            let tx = bx + (preset_w - self.painter.measure_text(&txt, 11.5)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 7, preset_w, &txt, 11.5, [242, 246, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + bx, oy + y).into(), (preset_w, 28).into()),
                action: UiAction::AdjustBrightness(pct - sys.brightness_pct as i32),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            bx += preset_w + 6;
        }
        y += 38;

        let hovered = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            14,
            y,
            w - 28,
            32,
            8,
            if hovered { [accent[0], accent[1], accent[2], 235] } else { [28, 38, 64, 220] },
        );
        self.painter.draw_text(
            buf,
            w,
            h,
            26,
            y + 9,
            w - 52,
            "Open Full System Settings App",
            12.0,
            [244, 247, 255, 255],
        );
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + 14, oy + y).into(), (w - 28, 32).into()),
            action: UiAction::LaunchSystemSettings,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_network_bt_popup(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        sys: &SystemSnapshot,
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 9200usize;
        let mut y = 14;
        self.painter.draw_text(
            buf,
            w,
            h,
            18,
            y,
            240,
            "Network & Bluetooth",
            15.0,
            [244, 247, 255, 255],
        );
        self.draw_close_btn(buf, w, h, ox, oy, y - 2, &mut hit_id);
        y += 36;

        let rows = [
            (
                format!("Wi-Fi: {}", sys.network_label),
                if sys.wifi_enabled { "ON" } else { "OFF" },
                sys.wifi_enabled,
                UiAction::ToggleWifi,
            ),
            (
                "Bluetooth Radio".to_string(),
                if sys.bluetooth_on { "ON" } else { "OFF" },
                sys.bluetooth_on,
                UiAction::ToggleBluetooth,
            ),
        ];
        for (lbl, state_txt, on, act) in rows {
            fill_rounded_rect(buf, w, h, 14, y, w - 28, 42, 9, [20, 26, 44, 220]);
            self.painter
                .draw_text(buf, w, h, 24, y + 13, w - 124, &lbl, 12.5, [236, 242, 255, 255]);
            let bx = w - 82;
            let hovered = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                bx,
                y + 8,
                56,
                26,
                7,
                if on {
                    [accent[0], accent[1], accent[2], if hovered { 250 } else { 220 }]
                } else if hovered {
                    [50, 62, 92, 225]
                } else {
                    [34, 42, 64, 215]
                },
            );
            let tx = bx + (56 - self.painter.measure_text(state_txt, 11.5)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 15, 48, state_txt, 11.5, [246, 248, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + bx, oy + y + 8).into(), (56, 26).into()),
                action: act,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            y += 48;
        }

        let half_w = (w - 34) / 2;
        for (i, (label, act)) in [
            ("Wi-Fi Manager", UiAction::OpenNetworkEditor),
            ("Bluetooth App", UiAction::OpenBluetoothManager),
        ]
        .into_iter()
        .enumerate()
        {
            let bx = 14 + i as i32 * (half_w + 6);
            let hovered = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                bx,
                y,
                half_w,
                32,
                8,
                if hovered { [44, 56, 86, 225] } else { [24, 32, 52, 215] },
            );
            let tx = bx + (half_w - self.painter.measure_text(label, 11.5)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 9, half_w - 8, label, 11.5, [232, 240, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + bx, oy + y).into(), (half_w, 32).into()),
                action: act,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
        y += 42;

        let hovered = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            14,
            y,
            w - 28,
            34,
            8,
            if hovered { [accent[0], accent[1], accent[2], 235] } else { [28, 38, 64, 220] },
        );
        self.painter.draw_text(
            buf,
            w,
            h,
            26,
            y + 10,
            w - 52,
            "Open Full System Settings App",
            12.0,
            [244, 247, 255, 255],
        );
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + 14, oy + y).into(), (w - 28, 34).into()),
            action: UiAction::LaunchSystemSettings,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_quick_settings(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        sys: &SystemSnapshot,
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 9300usize;
        let mut y = 14;
        self.painter.draw_text(
            buf,
            w,
            h,
            18,
            y,
            200,
            "Quick Settings",
            15.5,
            [244, 247, 255, 255],
        );
        let cust_hovered = self.hovered_hit_id == Some(hit_id);
        let cust_x = w - 152;
        fill_rounded_rect(
            buf,
            w,
            h,
            cust_x,
            y - 2,
            108,
            24,
            7,
            if self.customize_quick_tiles {
                [accent[0], accent[1], accent[2], 235]
            } else if cust_hovered {
                [48, 60, 92, 225]
            } else {
                [26, 35, 58, 220]
            },
        );
        let cust_lbl = if self.customize_quick_tiles {
            "Done Tiles"
        } else {
            "Customize Tiles"
        };
        let clx = cust_x + (108 - self.painter.measure_text(cust_lbl, 11.0)) / 2;
        self.painter.draw_text(
            buf,
            w,
            h,
            clx,
            y + 4,
            102,
            cust_lbl,
            11.0,
            [242, 246, 255, 255],
        );
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + cust_x, oy + y - 2).into(), (108, 24).into()),
            action: UiAction::ToggleQuickTileCustomize,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
        hit_id += 1;
        self.draw_close_btn(buf, w, h, ox, oy, y - 2, &mut hit_id);
        y += 36;

        let panel_keys = config.effective_quick_panel_tiles();
        let tiles: Vec<(String, String, bool, UiAction)> = panel_keys
            .iter()
            .take(4)
            .filter_map(|k| match k.as_str() {
                "wifi" => Some((
                    k.clone(),
                    format!("Wi-Fi: {}", sys.network_label.chars().take(10).collect::<String>()),
                    sys.wifi_enabled,
                    UiAction::ToggleWifi,
                )),
                "bluetooth" => Some((
                    k.clone(),
                    if sys.bluetooth_on { "Bluetooth: ON".into() } else { "Bluetooth: OFF".into() },
                    sys.bluetooth_on,
                    UiAction::ToggleBluetooth,
                )),
                "layout" => Some((
                    k.clone(),
                    format!("Layout: {}", config.layout_mode.short_label()),
                    true,
                    UiAction::CycleLayoutMode,
                )),
                "edge_snap" => Some((
                    k.clone(),
                    if config.edge_snap { "Edge Snap: ON".into() } else { "Edge Snap: OFF".into() },
                    config.edge_snap,
                    UiAction::ToggleEdgeSnap,
                )),
                "dim_inactive" => Some((
                    k.clone(),
                    if config.dim_unfocused_windows { "Dim Inactive: ON".into() } else { "Dim Inactive: OFF".into() },
                    config.dim_unfocused_windows,
                    UiAction::ToggleDimUnfocused,
                )),
                "video_saver" => Some((
                    k.clone(),
                    if config.video_battery_saver { "Vid Saver: ON".into() } else { "Vid Saver: OFF".into() },
                    config.video_battery_saver,
                    UiAction::ToggleVideoBatterySaver,
                )),
                _ => None,
            })
            .collect();
        let tile_w = (w - 34) / 2;
        for (i, (key, label, on, act)) in tiles.into_iter().enumerate() {
            let col = (i % 2) as i32;
            let row = (i / 2) as i32;
            let tx = 14 + col * (tile_w + 6);
            let ty = y + row * 46;
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if on {
                [accent[0], accent[1], accent[2], if hovered { 235 } else { 190 }]
            } else if hovered {
                [44, 56, 86, 225]
            } else {
                [24, 31, 50, 220]
            };
            fill_rounded_rect(buf, w, h, tx, ty, tile_w, 38, 9, bg);
            self.painter.draw_text(
                buf,
                w,
                h,
                tx + 12,
                ty + 12,
                tile_w - 20,
                &label,
                12.0,
                [245, 248, 255, 255],
            );
            let move_delta = if i == 0 { 1 } else { -1 };
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + tx, oy + ty).into(), (tile_w, 38).into()),
                action: act,
                right_action: Some(UiAction::MoveQuickPanelTile(key, move_delta)),
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
        y += 98;

        // Power Profile Pills
        let prof_w = (w - 40) / 3;
        for (i, (prof, lbl)) in [
            ("power-saver", "Power Saver"),
            ("balanced", "Balanced"),
            ("performance", "Performance"),
        ]
        .into_iter()
        .enumerate()
        {
            let active = sys.power_profile == prof;
            let px_i = 14 + i as i32 * (prof_w + 6);
            let hovered = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                px_i,
                y,
                prof_w,
                28,
                7,
                if active {
                    [accent[0], accent[1], accent[2], 230]
                } else if hovered {
                    [44, 56, 86, 225]
                } else {
                    [22, 29, 48, 215]
                },
            );
            let tx = px_i + (prof_w - self.painter.measure_text(lbl, 11.0)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 8, prof_w - 6, lbl, 11.0, [242, 246, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + px_i, oy + y).into(), (prof_w, 28).into()),
                action: UiAction::SetPowerProfile(prof.to_string()),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
        y += 40;

        let (vol, muted) = sys.default_output_vol();
        fill_rounded_rect(buf, w, h, 14, y, w - 28, 92, 10, [20, 26, 44, 215]);
        self.painter.draw_text(
            buf,
            w,
            h,
            24,
            y + 8,
            w - 48,
            &format!("Volume {}%{}", vol, if muted { " (Muted)" } else { "" }),
            11.5,
            [225, 235, 252, 255],
        );
        self.draw_slider(
            buf,
            w,
            h,
            ox,
            oy,
            24,
            y + 24,
            w - 48,
            vol as i32,
            0,
            150,
            accent,
            SliderTarget::MasterSink {
                name: "@DEFAULT_SINK@".to_string(),
            },
            &mut hit_id,
        );
        self.painter.draw_text(
            buf,
            w,
            h,
            24,
            y + 50,
            w - 48,
            &format!("Brightness {}%", sys.brightness_pct),
            11.5,
            [242, 222, 154, 255],
        );
        self.draw_slider(
            buf,
            w,
            h,
            ox,
            oy,
            24,
            y + 66,
            w - 48,
            sys.brightness_pct as i32,
            5,
            100,
            [245, 188, 66],
            SliderTarget::Brightness,
            &mut hit_id,
        );
        y += 104;

        // VRR, Video Battery Saver & Monitor Refresh Rate Quick Row
        let cur_hz = self
            .display_telemetry
            .outputs
            .first()
            .map(|o| o.current_hz)
            .unwrap_or(config.output_refresh_hz);
        let disp_badge = self
            .display_telemetry
            .outputs
            .first()
            .map(|o| o.battery_saver_state.clone())
            .unwrap_or_else(|| format!("{cur_hz}Hz"));
        self.painter.draw_text(
            buf,
            w,
            h,
            16,
            y,
            w - 32,
            &format!("Display & Battery: {disp_badge}"),
            11.0,
            [170, 212, 198, 240],
        );
        y += 18;
        let q3 = (w - 40) / 3;
        let disp_tiles = [
            (
                config.vrr_mode.short_label().to_string(),
                config.vrr_mode != VrrMode::Off,
                UiAction::CycleVrrMode,
            ),
            (
                if config.video_battery_saver {
                    "Vid Saver ON".to_string()
                } else {
                    "Vid Saver OFF".to_string()
                },
                config.video_battery_saver,
                UiAction::ToggleVideoBatterySaver,
            ),
            (
                format!("Hz: {}", if config.output_refresh_hz == 0 { cur_hz } else { config.output_refresh_hz }),
                true,
                UiAction::CycleRefreshHz,
            ),
        ];
        for (i, (lbl, on, act)) in disp_tiles.into_iter().enumerate() {
            let bx = 14 + i as i32 * (q3 + 6);
            let hovered = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                bx,
                y,
                q3,
                28,
                7,
                if on {
                    [accent[0], accent[1], accent[2], if hovered { 235 } else { 195 }]
                } else if hovered {
                    [44, 56, 86, 225]
                } else {
                    [24, 31, 50, 220]
                },
            );
            let tx = bx + (q3 - self.painter.measure_text(&lbl, 11.0)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 8, q3 - 6, &lbl, 11.0, [244, 248, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + bx, oy + y).into(), (q3, 28).into()),
                action: act,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
        y += 34;

        let u2 = (w - 34) / 2;
        let util_tiles = [
            (
                if config.dim_unfocused_windows {
                    "Dim Inactive: ON".to_string()
                } else {
                    "Dim Inactive: OFF".to_string()
                },
                config.dim_unfocused_windows,
                UiAction::ToggleDimUnfocused,
            ),
            (
                format!(
                    "Trim RAM ({}% • {}MB free)",
                    sys.memory.used_pct, sys.memory.available_mb
                ),
                false,
                UiAction::CleanRamCacheNow { deep_kernel: false },
            ),
        ];
        for (i, (lbl, on, act)) in util_tiles.into_iter().enumerate() {
            let ux = 14 + i as i32 * (u2 + 6);
            let hovered = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                ux,
                y,
                u2,
                28,
                7,
                if on {
                    [accent[0], accent[1], accent[2], if hovered { 235 } else { 195 }]
                } else if hovered {
                    [44, 56, 86, 225]
                } else {
                    [24, 31, 50, 220]
                },
            );
            let tx = ux + (u2 - self.painter.measure_text(&lbl, 11.0)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 8, u2 - 8, &lbl, 11.0, [240, 246, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + ux, oy + y).into(), (u2, 28).into()),
                action: act,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
        y += 36;

        let hovered_app = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            14,
            y,
            w - 134,
            34,
            8,
            if hovered_app {
                [accent[0], accent[1], accent[2], 240]
            } else {
                [accent[0], accent[1], accent[2], 195]
            },
        );
        self.painter.draw_text(
            buf,
            w,
            h,
            24,
            y + 10,
            w - 150,
            "Open Full System Settings App",
            12.0,
            [248, 250, 255, 255],
        );
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + 14, oy + y).into(), (w - 134, 34).into()),
            action: UiAction::LaunchSystemSettings,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
        hit_id += 1;

        let hovered_cc = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            w - 114,
            y,
            100,
            34,
            8,
            if hovered_cc { [44, 56, 86, 225] } else { [26, 34, 54, 220] },
        );
        let qtx = w - 114 + (100 - self.painter.measure_text("Quick Panel", 11.5)) / 2;
        self.painter.draw_text(
            buf,
            w,
            h,
            qtx,
            y + 10,
            92,
            "Quick Panel",
            11.5,
            [230, 238, 255, 255],
        );
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + w - 114, oy + y).into(), (100, 34).into()),
            action: UiAction::ToggleSettings,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
        hit_id += 1;
        y += 42;

        if self.customize_quick_tiles {
            fill_rounded_rect(buf, w, h, 14, y, w - 28, h - y - 12, 10, [16, 22, 38, 225]);
            stroke_rounded_rect(
                buf,
                w,
                h,
                14,
                y,
                w - 28,
                h - y - 12,
                10,
                1,
                [accent[0], accent[1], accent[2], 140],
            );
            self.painter.draw_text(
                buf,
                w,
                h,
                24,
                y + 8,
                w - 130,
                "Taskbar Quick Access Tile Order",
                11.5,
                [214, 228, 252, 255],
            );
            let rst_x = w - 88;
            fill_rounded_rect(buf, w, h, rst_x, y + 5, 64, 20, 6, [36, 48, 76, 220]);
            self.painter
                .draw_text(buf, w, h, rst_x + 15, y + 9, 44, "Reset", 10.5, [228, 238, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + rst_x, oy + y + 5).into(), (64, 20).into()),
                action: UiAction::ResetTaskbarQuickTiles,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            let mut ry = y + 30;
            for (tile, visible) in config.all_taskbar_tiles_ordered() {
                if ry + 26 > h - 14 {
                    break;
                }
                let hidden = !visible;
                let label = crate::config::quick_tile_label(&tile);
                fill_rounded_rect(
                    buf,
                    w,
                    h,
                    22,
                    ry,
                    w - 44,
                    24,
                    6,
                    if hidden { [20, 26, 42, 160] } else { [26, 36, 58, 215] },
                );
                self.painter.draw_text(
                    buf,
                    w,
                    h,
                    30,
                    ry + 6,
                    w - 170,
                    label,
                    11.0,
                    if hidden { [136, 148, 174, 210] } else { [238, 244, 255, 255] },
                );

                // [<] Move Left button
                let lx = w - 134;
                fill_rounded_rect(buf, w, h, lx, ry + 2, 24, 20, 5, [34, 46, 72, 220]);
                self.painter
                    .draw_text(buf, w, h, lx + 8, ry + 6, 14, "<", 11.0, [224, 236, 255, 255]);
                self.popup_hits.push(HitRegion {
                    rect: Rectangle::new((ox + lx, oy + ry + 2).into(), (24, 20).into()),
                    action: UiAction::MoveTaskbarQuickTile(tile.clone(), -1),
                    right_action: None,
                    scroll_action: None,
                    id: hit_id,
                });
                hit_id += 1;

                // [>] Move Right button
                let rx = w - 106;
                fill_rounded_rect(buf, w, h, rx, ry + 2, 24, 20, 5, [34, 46, 72, 220]);
                self.painter
                    .draw_text(buf, w, h, rx + 8, ry + 6, 14, ">", 11.0, [224, 236, 255, 255]);
                self.popup_hits.push(HitRegion {
                    rect: Rectangle::new((ox + rx, oy + ry + 2).into(), (24, 20).into()),
                    action: UiAction::MoveTaskbarQuickTile(tile.clone(), 1),
                    right_action: None,
                    scroll_action: None,
                    id: hit_id,
                });
                hit_id += 1;

                // [On/Off] Visibility toggle
                let vx = w - 78;
                fill_rounded_rect(
                    buf,
                    w,
                    h,
                    vx,
                    ry + 2,
                    50,
                    20,
                    5,
                    if hidden { [64, 40, 52, 220] } else { [accent[0], accent[1], accent[2], 215] },
                );
                let vlbl = if hidden { "Hidden" } else { "Shown" };
                self.painter
                    .draw_text(buf, w, h, vx + 8, ry + 6, 40, vlbl, 10.5, [244, 248, 255, 255]);
                self.popup_hits.push(HitRegion {
                    rect: Rectangle::new((ox + vx, oy + ry + 2).into(), (50, 20).into()),
                    action: UiAction::ToggleTaskbarQuickTileVisible(tile),
                    right_action: None,
                    scroll_action: None,
                    id: hit_id,
                });
                hit_id += 1;
                ry += 28;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_calendar_clock(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        clock_full_str: &str,
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 9400usize;
        let mut y = 14;

        self.painter.draw_text(
            buf,
            w,
            h,
            18,
            y,
            220,
            "Clock & Calendar",
            15.0,
            [244, 247, 255, 255],
        );
        self.draw_close_btn(buf, w, h, ox, oy, y - 2, &mut hit_id);
        y += 32;

        let (today_y, today_m, today_d) = current_local_ymd(self.cached_clock_sec);
        let full_date = format_full_date(today_y, today_m, today_d);

        // Digital Clock & Uptime Hero Card
        fill_rounded_rect(buf, w, h, 14, y, w - 28, 86, 11, [20, 27, 46, 230]);
        stroke_rounded_rect(buf, w, h, 14, y, w - 28, 86, 11, 1, [accent[0], accent[1], accent[2], 125]);
        self.painter.draw_text(
            buf,
            w,
            h,
            26,
            y + 12,
            220,
            clock_full_str,
            28.0,
            [246, 249, 255, 255],
        );
        self.painter.draw_text(
            buf,
            w,
            h,
            26,
            y + 46,
            w - 52,
            &full_date,
            12.5,
            [accent[0], accent[1], accent[2], 255],
        );
        self.painter.draw_text(
            buf,
            w,
            h,
            26,
            y + 64,
            w - 52,
            &format!("Uptime: {}", self.cached_uptime),
            11.0,
            [168, 182, 214, 230],
        );
        y += 96;

        // Calendar Month Header
        let (view_y, view_m) = add_months(today_y, today_m, self.calendar_month_offset);
        let m_title = format!("{} {}", month_name(view_m), view_y);
        self.painter.draw_text(
            buf,
            w,
            h,
            18,
            y + 5,
            170,
            &m_title,
            14.0,
            [240, 245, 255, 255],
        );

        let nav_btns = [
            ("<", UiAction::CalendarStepMonth(-1), w - 122, 26),
            ("Today", UiAction::CalendarResetToday, w - 90, 50),
            (">", UiAction::CalendarStepMonth(1), w - 34, 26),
        ];
        for (lbl, act, bx, bw) in nav_btns {
            let hovered = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                bx,
                y,
                bw,
                24,
                6,
                if hovered { [accent[0], accent[1], accent[2], 225] } else { [28, 36, 58, 220] },
            );
            let tx = bx + (bw - self.painter.measure_text(lbl, 11.0)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 6, bw, lbl, 11.0, [242, 246, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + bx, oy + y).into(), (bw, 24).into()),
                action: act,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
        y += 32;

        // Calendar Day Headers & 6x7 Grid
        let cell_w = (w - 28) / 7;
        for (i, wd) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].into_iter().enumerate() {
            let cx = 14 + i as i32 * cell_w + (cell_w - self.painter.measure_text(wd, 11.0)) / 2;
            let col = if i >= 5 {
                [172, 184, 214, 215]
            } else {
                [136, 152, 186, 220]
            };
            self.painter.draw_text(buf, w, h, cx, y, cell_w, wd, 11.0, col);
        }
        y += 20;

        let first_wd = weekday_mon0(view_y, view_m, 1) as i32;
        let dim = days_in_month(view_y, view_m) as i32;
        for row in 0..6 {
            for col in 0..7 {
                let slot = row * 7 + col;
                let day = slot - first_wd + 1;
                if day < 1 || day > dim {
                    continue;
                }
                let cx = 14 + col * cell_w;
                let cy = y + row * 28;
                let is_today = view_y == today_y && view_m == today_m && day as u32 == today_d;
                if is_today {
                    fill_rounded_rect(
                        buf,
                        w,
                        h,
                        cx + 3,
                        cy + 1,
                        cell_w - 6,
                        24,
                        7,
                        [accent[0], accent[1], accent[2], 240],
                    );
                } else if col >= 5 {
                    fill_rounded_rect(buf, w, h, cx + 3, cy + 1, cell_w - 6, 24, 7, [20, 26, 44, 150]);
                }
                let day_s = day.to_string();
                let dtx = cx + (cell_w - self.painter.measure_text(&day_s, 12.0)) / 2;
                let text_col = if is_today {
                    [252, 254, 255, 255]
                } else if col >= 5 {
                    [182, 198, 232, 235]
                } else {
                    [232, 238, 252, 250]
                };
                self.painter
                    .draw_text(buf, w, h, dtx, cy + 6, cell_w, &day_s, 12.0, text_col);
            }
        }
        y += 6 * 28 + 10;

        // Built-in Focus / Pomodoro Timer Bar
        fill_rounded_rect(buf, w, h, 14, y, w - 28, 44, 10, [20, 27, 46, 220]);
        let mins = self.focus_timer_remaining_secs / 60;
        let secs = self.focus_timer_remaining_secs % 60;
        let timer_lbl = format!("Focus {:02}:{:02}", mins, secs);
        self.painter.draw_text(
            buf,
            w,
            h,
            24,
            y + 14,
            116,
            &timer_lbl,
            13.0,
            if self.focus_timer_running {
                [88, 226, 158, 255]
            } else {
                [230, 238, 254, 255]
            },
        );
        let tbtns = [
            (
                if self.focus_timer_running { "Pause" } else { "Start" },
                UiAction::FocusTimerToggle,
                w - 178,
                56,
            ),
            ("25m", UiAction::FocusTimerReset(25), w - 116, 46),
            ("5m", UiAction::FocusTimerReset(5), w - 64, 42),
        ];
        for (lbl, act, bx, bw) in tbtns {
            let hovered = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                bx,
                y + 9,
                bw,
                26,
                7,
                if hovered { [accent[0], accent[1], accent[2], 230] } else { [32, 42, 68, 220] },
            );
            let tx = bx + (bw - self.painter.measure_text(lbl, 11.0)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 16, bw, lbl, 11.0, [244, 247, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + bx, oy + y + 9).into(), (bw, 26).into()),
                action: act,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_task_view(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        ws_counts: &[usize; 9],
        global_windows: &[GlobalWindowInfo],
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 9600usize;
        let mut y = 16;

        self.painter.draw_text(
            buf,
            w,
            h,
            20,
            y,
            340,
            "Multitasking & Virtual Desktops (Win+Tab)",
            16.0,
            [244, 247, 255, 255],
        );

        let top_btns = [
            ("Show Desktop", UiAction::ShowDesktop, w - 280, 108),
            (
                config.layout_mode.label(),
                UiAction::CycleLayoutMode,
                w - 166,
                114,
            ),
        ];
        for (lbl, act, bx, bw) in top_btns {
            let hovered = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                bx,
                y - 2,
                bw,
                26,
                7,
                if hovered { [accent[0], accent[1], accent[2], 230] } else { [30, 40, 66, 220] },
            );
            let tx = bx + (bw - self.painter.measure_text(lbl, 11.5)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 4, bw - 8, lbl, 11.5, [242, 246, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + bx, oy + y - 2).into(), (bw, 26).into()),
                action: act,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
        self.draw_close_btn(buf, w, h, ox, oy, y - 2, &mut hit_id);
        y += 38;

        // Virtual Desktop Bar (1..9)
        let desk_w = ((w - 40) - 8 * 6) / 9;
        for ws in 0..9 {
            let dx = 20 + ws as i32 * (desk_w + 6);
            let count = ws_counts[ws];
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if count > 0 {
                [28, 38, 64, 230]
            } else if hovered {
                [42, 54, 86, 220]
            } else {
                [18, 24, 40, 200]
            };
            fill_rounded_rect(buf, w, h, dx, y, desk_w, 34, 8, bg);
            if count > 0 {
                stroke_rounded_rect(
                    buf,
                    w,
                    h,
                    dx,
                    y,
                    desk_w,
                    34,
                    8,
                    1,
                    [accent[0], accent[1], accent[2], 180],
                );
            }
            let lbl = if count > 0 {
                format!("Desktop {} ({})", ws + 1, count)
            } else {
                format!("Desktop {}", ws + 1)
            };
            let tx = dx + (desk_w - self.painter.measure_text(&lbl, 11.0)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 10, desk_w - 6, &lbl, 11.0, [236, 242, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + dx, oy + y).into(), (desk_w, 34).into()),
                action: UiAction::SwitchWorkspace(ws),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
        y += 48;

        if global_windows.is_empty() {
            self.painter.draw_text(
                buf,
                w,
                h,
                24,
                y + 18,
                w - 48,
                "No open windows. Launch apps from the Apps menu or press Super+Return for Terminal.",
                13.5,
                [162, 178, 210, 230],
            );
            return;
        }

        // Responsive 2-column window cards with instant focus, expand/shrink, snap, minimize, move & close
        let card_w = (w - 52) / 2;
        let card_h = 96;
        for (idx, gw) in global_windows.iter().take(8).enumerate() {
            let col = (idx % 2) as i32;
            let row = (idx / 2) as i32;
            let cx = 20 + col * (card_w + 12);
            let cy = y + row * (card_h + 10);
            if cy + card_h > h - 14 {
                break;
            }
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if gw.focused {
                [30, 42, 72, 240]
            } else if hovered {
                [26, 36, 60, 235]
            } else {
                [18, 24, 42, 225]
            };
            fill_rounded_rect(buf, w, h, cx, cy, card_w, card_h, 11, bg);
            if gw.focused {
                stroke_rounded_rect(
                    buf,
                    w,
                    h,
                    cx,
                    cy,
                    card_w,
                    card_h,
                    11,
                    2,
                    [accent[0], accent[1], accent[2], 240],
                );
            }

            let ws_tag = format!("WS {}", gw.workspace + 1);
            fill_rounded_rect(buf, w, h, cx + 12, cy + 12, 46, 20, 6, [accent[0], accent[1], accent[2], 210]);
            self.painter
                .draw_text(buf, w, h, cx + 18, cy + 16, 40, &ws_tag, 10.5, [248, 250, 255, 255]);

            let state_str = if gw.minimized {
                "Minimized"
            } else if gw.maximized {
                "Expanded"
            } else if gw.focused {
                "Active"
            } else if gw.floating {
                "Floating"
            } else {
                "Tiled"
            };
            self.painter.draw_text(
                buf,
                w,
                h,
                cx + 66,
                cy + 15,
                card_w - 110,
                state_str,
                11.0,
                [172, 188, 222, 235],
            );

            self.painter.draw_text(
                buf,
                w,
                h,
                cx + 12,
                cy + 40,
                card_w - 24,
                &gw.title,
                13.5,
                [246, 249, 255, 255],
            );

            // Card click background focuses/unminimizes window
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + cx, oy + cy).into(), (card_w, 60).into()),
                action: UiAction::SelectAltTab(idx),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;

            // Card Action Strip
            let act_y = cy + 64;
            let actions = [
                (
                    if gw.maximized { "Shrink" } else { "Expand" },
                    UiAction::ToggleMaximizeGlobal {
                        ws: gw.workspace,
                        idx_in_ws: gw.index_in_ws,
                    },
                    60,
                    [34, 46, 74, 225],
                ),
                (
                    "< Half",
                    UiAction::SnapGlobal {
                        ws: gw.workspace,
                        idx_in_ws: gw.index_in_ws,
                        zone: SnapZone::LeftHalf,
                    },
                    52,
                    [30, 40, 66, 225],
                ),
                (
                    "Half >",
                    UiAction::SnapGlobal {
                        ws: gw.workspace,
                        idx_in_ws: gw.index_in_ws,
                        zone: SnapZone::RightHalf,
                    },
                    52,
                    [30, 40, 66, 225],
                ),
                (
                    if gw.minimized { "Show" } else { "Min" },
                    UiAction::MinimizeGlobal {
                        ws: gw.workspace,
                        idx_in_ws: gw.index_in_ws,
                    },
                    44,
                    [30, 40, 66, 225],
                ),
                (
                    "->WS",
                    UiAction::MoveNextWsGlobal {
                        ws: gw.workspace,
                        idx_in_ws: gw.index_in_ws,
                    },
                    48,
                    [30, 40, 66, 225],
                ),
                (
                    "Close",
                    UiAction::CloseGlobal {
                        ws: gw.workspace,
                        idx_in_ws: gw.index_in_ws,
                    },
                    50,
                    [92, 42, 58, 225],
                ),
            ];
            let mut ax = cx + 12;
            for (lbl, act, bw, base_col) in actions {
                if ax + bw > cx + card_w - 8 {
                    break;
                }
                let bhov = self.hovered_hit_id == Some(hit_id);
                fill_rounded_rect(
                    buf,
                    w,
                    h,
                    ax,
                    act_y,
                    bw,
                    22,
                    6,
                    if bhov { [accent[0], accent[1], accent[2], 235] } else { base_col },
                );
                let tx = ax + (bw - self.painter.measure_text(lbl, 10.5)) / 2;
                self.painter
                    .draw_text(buf, w, h, tx, act_y + 5, bw - 4, lbl, 10.5, [242, 246, 255, 255]);
                self.popup_hits.push(HitRegion {
                    rect: Rectangle::new((ox + ax, oy + act_y).into(), (bw, 22).into()),
                    action: act,
                    right_action: None,
                    scroll_action: None,
                    id: hit_id,
                });
                hit_id += 1;
                ax += bw + 6;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_slider(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        x: i32,
        y: i32,
        sw: i32,
        val: i32,
        min: i32,
        max: i32,
        accent: [u8; 3],
        target: SliderTarget,
        hit_id: &mut usize,
    ) {
        let track_h = 8;
        let track_y = y + 9;
        let span = (max - min).max(1) as f32;
        let t = ((val - min) as f32 / span).clamp(0.0, 1.0);
        let fill_w = (sw as f32 * t).round() as i32;

        fill_rounded_rect(buf, w, h, x, track_y, sw, track_h, 4, [30, 38, 60, 235]);
        if fill_w > 0 {
            fill_rounded_rect(
                buf,
                w,
                h,
                x,
                track_y,
                fill_w.max(6).min(sw),
                track_h,
                4,
                [accent[0], accent[1], accent[2], 245],
            );
        }
        let knob_x = x + fill_w.clamp(4, sw - 4);
        let hovered = self.hovered_hit_id == Some(*hit_id);
        fill_circle(
            buf,
            w,
            h,
            knob_x,
            track_y + track_h / 2,
            if hovered { 8 } else { 7 },
            [244, 248, 255, 255],
        );

        let hit_rect = Rectangle::new((ox + x, oy + y).into(), (sw, 26).into());
        self.popup_hits.push(HitRegion {
            rect: hit_rect,
            action: UiAction::StartSliderDrag {
                target,
                rect: hit_rect,
                min,
                max,
            },
            right_action: None,
            scroll_action: None,
            id: *hit_id,
        });
        *hit_id += 1;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_audio_mixer(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        sys: &SystemSnapshot,
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 4000usize;
        let mut y = 14;

        self.painter.draw_text(
            buf,
            w,
            h,
            18,
            y,
            260,
            "Audio Mixer & Device Router",
            15.5,
            [244, 247, 255, 255],
        );
        self.draw_close_btn(buf, w, h, ox, oy, y - 2, &mut hit_id);
        y += 34;

        // Master Output Card
        fill_rounded_rect(buf, w, h, 14, y, w - 28, 96, 10, [20, 26, 44, 215]);
        let (out_vol, out_muted) = sys.default_output_vol();
        let out_title = format!(
            "Output Volume  {}%{}",
            out_vol,
            if out_muted { " (Muted)" } else { "" }
        );
        self.painter
            .draw_text(buf, w, h, 26, y + 10, 240, &out_title, 13.0, [238, 244, 255, 255]);

        let mute_hovered = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            w - 92,
            y + 6,
            64,
            22,
            6,
            if out_muted {
                [195, 62, 82, 235]
            } else if mute_hovered {
                [48, 60, 92, 230]
            } else {
                [32, 42, 68, 220]
            },
        );
        self.painter.draw_text(
            buf,
            w,
            h,
            w - 76,
            y + 11,
            48,
            if out_muted { "Unmute" } else { "Mute" },
            11.0,
            [242, 246, 255, 255],
        );
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + w - 92, oy + y + 6).into(), (64, 22).into()),
            action: UiAction::ToggleSinkMute("@DEFAULT_SINK@".to_string()),
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
        hit_id += 1;

        self.draw_slider(
            buf,
            w,
            h,
            ox,
            oy,
            26,
            y + 32,
            w - 52,
            out_vol as i32,
            0,
            150,
            accent,
            SliderTarget::MasterSink {
                name: "@DEFAULT_SINK@".to_string(),
            },
            &mut hit_id,
        );

        let mut dx = 26;
        for sink in sys.sinks.iter().filter(|s| !s.disabled).take(4) {
            let short: String = sink.description.chars().take(16).collect();
            let bw = (self.painter.measure_text(&short, 11.0) + 16).min(132);
            if dx + bw > w - 22 {
                break;
            }
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if sink.is_default {
                [accent[0], accent[1], accent[2], 225]
            } else if hovered {
                [48, 60, 92, 225]
            } else {
                [28, 36, 56, 210]
            };
            fill_rounded_rect(buf, w, h, dx, y + 64, bw, 22, 6, bg);
            self.painter.draw_text(
                buf,
                w,
                h,
                dx + 8,
                y + 69,
                bw - 12,
                &short,
                11.0,
                [238, 244, 255, 255],
            );
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + dx, oy + y + 64).into(), (bw, 22).into()),
                action: UiAction::SetDefaultSink(sink.name.clone()),
                right_action: Some(UiAction::ToggleSinkMute(sink.name.clone())),
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            dx += bw + 6;
        }
        y += 106;

        // Master Input Card
        fill_rounded_rect(buf, w, h, 14, y, w - 28, 96, 10, [20, 26, 44, 215]);
        let (in_vol, in_muted) = sys.default_input_vol();
        let in_title = format!(
            "Microphone Input  {}%{}",
            in_vol,
            if in_muted { " (Muted)" } else { "" }
        );
        self.painter
            .draw_text(buf, w, h, 26, y + 10, 240, &in_title, 13.0, [238, 244, 255, 255]);

        let im_hover = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            w - 92,
            y + 6,
            64,
            22,
            6,
            if in_muted {
                [195, 62, 82, 235]
            } else if im_hover {
                [48, 60, 92, 230]
            } else {
                [32, 42, 68, 220]
            },
        );
        self.painter.draw_text(
            buf,
            w,
            h,
            w - 76,
            y + 11,
            48,
            if in_muted { "Unmute" } else { "Mute" },
            11.0,
            [242, 246, 255, 255],
        );
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + w - 92, oy + y + 6).into(), (64, 22).into()),
            action: UiAction::ToggleSourceMute("@DEFAULT_SOURCE@".to_string()),
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
        hit_id += 1;

        self.draw_slider(
            buf,
            w,
            h,
            ox,
            oy,
            26,
            y + 32,
            w - 52,
            in_vol as i32,
            0,
            150,
            [147, 94, 247],
            SliderTarget::MasterSource {
                name: "@DEFAULT_SOURCE@".to_string(),
            },
            &mut hit_id,
        );

        let mut sx = 26;
        for src in sys.sources.iter().filter(|s| !s.disabled).take(4) {
            let short: String = src.description.chars().take(16).collect();
            let bw = (self.painter.measure_text(&short, 11.0) + 16).min(132);
            if sx + bw > w - 22 {
                break;
            }
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if src.is_default {
                [147, 94, 247, 225]
            } else if hovered {
                [48, 60, 92, 225]
            } else {
                [28, 36, 56, 210]
            };
            fill_rounded_rect(buf, w, h, sx, y + 64, bw, 22, 6, bg);
            self.painter.draw_text(
                buf,
                w,
                h,
                sx + 8,
                y + 69,
                bw - 12,
                &short,
                11.0,
                [238, 244, 255, 255],
            );
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + sx, oy + y + 64).into(), (bw, 22).into()),
                action: UiAction::SetDefaultSource(src.name.clone()),
                right_action: Some(UiAction::ToggleSourceMute(src.name.clone())),
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            sx += bw + 6;
        }
        y += 106;

        // Role Routing Row
        fill_rounded_rect(buf, w, h, 14, y, w - 28, 42, 9, [20, 26, 44, 200]);
        let comms_label = sys
            .comms_sink
            .as_deref()
            .and_then(|n| sys.sinks.iter().find(|s| !s.disabled && s.name == n))
            .map(|s| s.description.chars().take(12).collect::<String>())
            .unwrap_or_else(|| "Default".to_string());
        let media_label = sys
            .media_sink
            .as_deref()
            .and_then(|n| sys.sinks.iter().find(|s| !s.disabled && s.name == n))
            .map(|s| s.description.chars().take(12).collect::<String>())
            .unwrap_or_else(|| "Default".to_string());

        let half_w = (w - 48) / 2;
        for (idx, (prefix, val, is_comms)) in [
            ("Comms:", comms_label, true),
            ("Media:", media_label, false),
        ]
        .into_iter()
        .enumerate()
        {
            let bx = 22 + (idx as i32) * (half_w + 8);
            let hovered = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                bx,
                y + 8,
                half_w,
                26,
                7,
                if hovered {
                    [accent[0], accent[1], accent[2], 195]
                } else {
                    [30, 38, 60, 225]
                },
            );
            let txt = format!("{prefix} {val}");
            self.painter.draw_text(
                buf,
                w,
                h,
                bx + 10,
                y + 15,
                half_w - 16,
                &txt,
                11.5,
                [238, 244, 255, 255],
            );
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + bx, oy + y + 8).into(), (half_w, 26).into()),
                action: UiAction::CycleRoleRouting { is_comms },
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
        y += 50;

        self.painter.draw_text(
            buf,
            w,
            h,
            18,
            y,
            300,
            "Active App Streams (Click device to route)",
            12.5,
            [185, 200, 232, 255],
        );
        y += 22;

        let bottom_reserve = 56;
        let max_streams = ((h - y - bottom_reserve) / 56).max(1) as usize;
        if sys.sink_inputs.is_empty() && sys.source_outputs.is_empty() {
            fill_rounded_rect(buf, w, h, 14, y, w - 28, 44, 9, [18, 24, 42, 180]);
            self.painter.draw_text(
                buf,
                w,
                h,
                26,
                y + 15,
                w - 52,
                "No active audio streams (play audio or join Discord call).",
                11.5,
                [146, 160, 192, 225],
            );
        } else {
            let mut rendered = 0usize;
            for st in &sys.sink_inputs {
                if rendered >= max_streams {
                    break;
                }
                let dev_desc = sys
                    .sinks
                    .iter()
                    .find(|s| s.index == st.device_index)
                    .map(|s| s.description.chars().take(14).collect::<String>())
                    .unwrap_or_else(|| "Output".to_string());
                self.draw_stream_row(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    &mut y,
                    &mut hit_id,
                    st,
                    false,
                    &dev_desc,
                    accent,
                );
                rendered += 1;
            }
            for st in &sys.source_outputs {
                if rendered >= max_streams {
                    break;
                }
                let dev_desc = sys
                    .sources
                    .iter()
                    .find(|s| s.index == st.device_index)
                    .map(|s| s.description.chars().take(14).collect::<String>())
                    .unwrap_or_else(|| "Mic".to_string());
                self.draw_stream_row(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    &mut y,
                    &mut hit_id,
                    st,
                    true,
                    &dev_desc,
                    [147, 94, 247],
                );
                rendered += 1;
            }
        }

        let by = h - 48;
        fill_rounded_rect(buf, w, h, 14, by, w - 28, 36, 9, [20, 26, 44, 215]);
        let bri_lbl = format!("Brightness {}%", sys.brightness_pct);
        self.painter
            .draw_text(buf, w, h, 24, by + 11, 110, &bri_lbl, 12.0, [242, 222, 154, 255]);
        self.draw_slider(
            buf,
            w,
            h,
            ox,
            oy,
            138,
            by + 5,
            w - 166,
            sys.brightness_pct as i32,
            5,
            100,
            [245, 188, 66],
            SliderTarget::Brightness,
            &mut hit_id,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_stream_row(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        y: &mut i32,
        hit_id: &mut usize,
        st: &crate::system_ctrl::AudioStream,
        is_input: bool,
        dev_desc: &str,
        accent: [u8; 3],
    ) {
        fill_rounded_rect(buf, w, h, 14, *y, w - 28, 50, 8, [18, 24, 42, 200]);

        let role_tag = if is_input {
            "Mic"
        } else if st.is_comms {
            "Comms"
        } else {
            "Play"
        };
        let tag_col = if is_input {
            [147, 94, 247, 220]
        } else if st.is_comms {
            [36, 186, 132, 220]
        } else {
            [accent[0], accent[1], accent[2], 210]
        };
        fill_rounded_rect(buf, w, h, 22, *y + 6, 46, 18, 5, tag_col);
        self.painter
            .draw_text(buf, w, h, 28, *y + 9, 40, role_tag, 10.5, [248, 250, 255, 255]);

        let title = format!("{}  {}%", st.app_name, st.volume_pct);
        self.painter
            .draw_text(buf, w, h, 76, *y + 7, w - 250, &title, 12.0, [240, 245, 255, 255]);

        let route_w = 112;
        let rx = w - route_w - 64;
        let r_hover = self.hovered_hit_id == Some(*hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            rx,
            *y + 5,
            route_w,
            20,
            5,
            if r_hover {
                [accent[0], accent[1], accent[2], 220]
            } else {
                [30, 38, 60, 220]
            },
        );
        self.painter.draw_text(
            buf,
            w,
            h,
            rx + 6,
            *y + 9,
            route_w - 10,
            dev_desc,
            10.5,
            [232, 240, 255, 255],
        );
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + rx, oy + *y + 5).into(), (route_w, 20).into()),
            action: UiAction::CycleStreamDevice {
                is_input,
                index: st.index,
            },
            right_action: None,
            scroll_action: None,
            id: *hit_id,
        });
        *hit_id += 1;

        let mx = w - 58;
        let m_hover = self.hovered_hit_id == Some(*hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            mx,
            *y + 5,
            36,
            20,
            5,
            if st.muted {
                [195, 62, 82, 235]
            } else if m_hover {
                [48, 60, 92, 225]
            } else {
                [30, 38, 60, 220]
            },
        );
        self.painter
            .draw_text(buf, w, h, mx + 6, *y + 9, 28, "Mute", 10.5, [240, 245, 255, 255]);
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + mx, oy + *y + 5).into(), (36, 20).into()),
            action: UiAction::ToggleStreamMute {
                is_input,
                index: st.index,
            },
            right_action: None,
            scroll_action: None,
            id: *hit_id,
        });
        *hit_id += 1;

        self.draw_slider(
            buf,
            w,
            h,
            ox,
            oy,
            22,
            *y + 24,
            w - 44,
            st.volume_pct as i32,
            0,
            150,
            accent,
            SliderTarget::Stream {
                is_input,
                index: st.index,
            },
            hit_id,
        );

        *y += 56;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_control_center(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        sys: &SystemSnapshot,
        saved_banner: bool,
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 5000usize;

        let side_w = 164;
        fill_rounded_rect(buf, w, h, 8, 8, side_w - 8, h - 16, 10, [16, 22, 38, 225]);
        self.painter.draw_text(
            buf,
            w,
            h,
            22,
            20,
            side_w - 28,
            "Velox Settings",
            15.5,
            [244, 247, 255, 255],
        );

        let mut ty = 58;
        for &tab in SettingsTab::ALL {
            let active = self.settings_tab == tab;
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if active {
                [accent[0], accent[1], accent[2], 228]
            } else if hovered {
                [38, 48, 76, 225]
            } else {
                [24, 30, 50, 190]
            };
            fill_rounded_rect(buf, w, h, 16, ty, side_w - 24, 34, 8, bg);
            self.painter.draw_text(
                buf,
                w,
                h,
                26,
                ty + 10,
                side_w - 40,
                tab.label(),
                12.5,
                if active { [250, 252, 255, 255] } else { [196, 208, 236, 245] },
            );
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + 16, oy + ty).into(), (side_w - 24, 34).into()),
                action: UiAction::OpenSettingsTab(tab),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            ty += 42;
        }

        let cx = side_w + 16;
        let cw = w - cx - 16;
        let mut y = 20;

        self.painter.draw_text(
            buf,
            w,
            h,
            cx,
            y,
            cw - 50,
            self.settings_tab.label(),
            16.5,
            [244, 247, 255, 255],
        );
        self.draw_close_btn(buf, w, h, ox, oy, y - 2, &mut hit_id);
        y += 38;

        match self.settings_tab {
            SettingsTab::Appearance => {
                self.painter
                    .draw_text(buf, w, h, cx, y, cw, "Accent Theme", 12.5, [205, 218, 244, 255]);
                y += 20;
                let mut sx = cx;
                for &(name, rgb) in ACCENT_PRESETS {
                    let active = config.accent_theme.eq_ignore_ascii_case(name);
                    let hovered = self.hovered_hit_id == Some(hit_id);
                    fill_rounded_rect(buf, w, h, sx, y, 48, 26, 7, [rgb[0], rgb[1], rgb[2], 238]);
                    if active || hovered {
                        stroke_rounded_rect(
                            buf,
                            w,
                            h,
                            sx - 2,
                            y - 2,
                            52,
                            30,
                            8,
                            2,
                            [248, 250, 255, if active { 255 } else { 170 }],
                        );
                    }
                    self.popup_hits.push(HitRegion {
                        rect: Rectangle::new((ox + sx, oy + y).into(), (48, 26).into()),
                        action: UiAction::SetAccent(name.to_string()),
                        right_action: None,
                        scroll_action: None,
                        id: hit_id,
                    });
                    hit_id += 1;
                    sx += 56;
                }
                y += 40;

                self.painter
                    .draw_text(buf, w, h, cx, y, cw, "Wallpaper Theme", 12.5, [205, 218, 244, 255]);
                y += 20;
                let mut wx = cx;
                for &preset in WALLPAPER_PRESETS {
                    let active = (!config.use_custom_wallpaper
                        || config.resolve_wallpaper_path().is_none())
                        && config.wallpaper_preset.eq_ignore_ascii_case(preset);
                    let hovered = self.hovered_hit_id == Some(hit_id);
                    let bw = 72;
                    let bg = if active {
                        [accent[0], accent[1], accent[2], 232]
                    } else if hovered {
                        [48, 60, 92, 225]
                    } else {
                        [26, 34, 54, 215]
                    };
                    fill_rounded_rect(buf, w, h, wx, y, bw, 26, 7, bg);
                    let tx = wx + (bw - self.painter.measure_text(preset, 11.5)) / 2;
                    self.painter
                        .draw_text(buf, w, h, tx, y + 6, bw, preset, 11.5, [242, 246, 255, 255]);
                    self.popup_hits.push(HitRegion {
                        rect: Rectangle::new((ox + wx, oy + y).into(), (bw, 26).into()),
                        action: UiAction::SetWallpaperPreset(preset.to_string()),
                        right_action: None,
                        scroll_action: None,
                        id: hit_id,
                    });
                    hit_id += 1;
                    wx += bw + 6;
                }
                y += 38;

                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Custom Wallpaper (~/.config/velox-wm/wallpaper.*)",
                    config.use_custom_wallpaper,
                    accent,
                    UiAction::ToggleCustomWallpaper,
                );
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Shelf Design Style",
                    config.shelf_style.label(),
                    accent,
                    UiAction::ToggleShelfStyle,
                );
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Shelf Position",
                    config.panel_position.label(),
                    accent,
                    UiAction::TogglePanelPosition,
                );
                self.draw_cc_stepper(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Shelf Height",
                    &format!("{} px", config.panel_height),
                    accent,
                    UiAction::AdjustPanelHeight(-2),
                    UiAction::AdjustPanelHeight(2),
                );
                self.draw_cc_slider(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    &format!("Shelf Opacity ({}%)", (config.panel_opacity as i32 * 100) / 255),
                    config.panel_opacity as i32,
                    120,
                    250,
                    accent,
                    SliderTarget::PanelOpacity,
                );
                self.draw_cc_slider(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    &format!("Shelf Corner Radius ({} px)", config.corner_radius),
                    config.corner_radius,
                    0,
                    18,
                    accent,
                    SliderTarget::CornerRadius,
                );
            }
            SettingsTab::Windows => {
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Tiling Layout Mode",
                    config.layout_mode.label(),
                    accent,
                    UiAction::CycleLayoutMode,
                );
                self.draw_cc_slider(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    &format!("Window Gaps ({} px)", config.gap_px),
                    config.gap_px,
                    0,
                    32,
                    accent,
                    SliderTarget::GapPx,
                );
                self.draw_cc_slider(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    &format!("Master Split Ratio ({}%)", (config.master_ratio * 100.0).round() as i32),
                    (config.master_ratio * 100.0).round() as i32,
                    40,
                    75,
                    accent,
                    SliderTarget::MasterRatio,
                );
                self.draw_cc_stepper(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Active Window Border",
                    &format!("{} px", config.border_width),
                    accent,
                    UiAction::AdjustBorderWidth(-1),
                    UiAction::AdjustBorderWidth(1),
                );
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Drag Edge Snapping (Halves / Quarters / Max)",
                    config.edge_snap,
                    accent,
                    UiAction::ToggleEdgeSnap,
                );
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Focus Follows Mouse (Hover Without Raising)",
                    config.focus_follows_mouse,
                    accent,
                    UiAction::ToggleFocusFollowsMouse,
                );
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Smooth Animations",
                    config.animations_enabled,
                    accent,
                    UiAction::ToggleAnimations,
                );
                let speed_label = if config.animation_speed >= 0.34 {
                    "Fast"
                } else if config.animation_speed >= 0.20 {
                    "Smooth"
                } else {
                    "Relaxed"
                };
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Animation Feel",
                    speed_label,
                    accent,
                    UiAction::CycleAnimationSpeed,
                );
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Lock Focused Game Cursor (Prevent Monitor Escape)",
                    config.game_pointer_lock,
                    accent,
                    UiAction::ToggleGamePointerLock,
                );
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Lock Taskbar During Fullscreen Games",
                    config.lock_fullscreen_taskbar,
                    accent,
                    UiAction::ToggleFullscreenTaskbarLock,
                );
            }
            SettingsTab::Audio => {
                let (ov, om) = sys.default_output_vol();
                self.draw_cc_slider(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    &format!("Master Output ({}%{})", ov, if om { " Muted" } else { "" }),
                    ov as i32,
                    0,
                    150,
                    accent,
                    SliderTarget::MasterSink {
                        name: "@DEFAULT_SINK@".to_string(),
                    },
                );
                for sink in sys.sinks.iter().filter(|s| !s.disabled).take(3) {
                    let lbl = format!(
                        "Out: {} ({}%)",
                        sink.description.chars().take(30).collect::<String>(),
                        sink.volume_pct
                    );
                    self.draw_cc_toggle(
                        buf,
                        w,
                        h,
                        ox,
                        oy,
                        cx,
                        cw,
                        &mut y,
                        &mut hit_id,
                        &lbl,
                        sink.is_default,
                        accent,
                        UiAction::SetDefaultSink(sink.name.clone()),
                    );
                }

                let (iv, im) = sys.default_input_vol();
                self.draw_cc_slider(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    &format!("Mic Input ({}%{})", iv, if im { " Muted" } else { "" }),
                    iv as i32,
                    0,
                    150,
                    [147, 94, 247],
                    SliderTarget::MasterSource {
                        name: "@DEFAULT_SOURCE@".to_string(),
                    },
                );
                for src in sys.sources.iter().filter(|s| !s.disabled).take(2) {
                    let lbl = format!(
                        "Mic: {} ({}%)",
                        src.description.chars().take(30).collect::<String>(),
                        src.volume_pct
                    );
                    self.draw_cc_toggle(
                        buf,
                        w,
                        h,
                        ox,
                        oy,
                        cx,
                        cw,
                        &mut y,
                        &mut hit_id,
                        &lbl,
                        src.is_default,
                        [147, 94, 247],
                        UiAction::SetDefaultSource(src.name.clone()),
                    );
                }

                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Auto-Route Comms & Media Apps on Launch",
                    config.auto_route_audio_roles,
                    accent,
                    UiAction::ToggleAutoRouteAudioRoles,
                );
                let comms_name = sys
                    .comms_sink
                    .as_deref()
                    .and_then(|n| sys.sinks.iter().find(|s| !s.disabled && s.name == n))
                    .map(|s| s.description.chars().take(14).collect::<String>())
                    .unwrap_or_else(|| "Default".to_string());
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Communications Device (Discord/Voice)",
                    &comms_name,
                    accent,
                    UiAction::CycleRoleRouting { is_comms: true },
                );
                let media_name = sys
                    .media_sink
                    .as_deref()
                    .and_then(|n| sys.sinks.iter().find(|s| !s.disabled && s.name == n))
                    .map(|s| s.description.chars().take(14).collect::<String>())
                    .unwrap_or_else(|| "Default".to_string());
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Playback / Media Device (Browser/Music)",
                    &media_name,
                    accent,
                    UiAction::CycleRoleRouting { is_comms: false },
                );
            }
            SettingsTab::DisplayPower => {
                self.draw_cc_slider(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    &format!("Display Brightness ({}%)", sys.brightness_pct),
                    sys.brightness_pct as i32,
                    5,
                    100,
                    [245, 188, 66],
                    SliderTarget::Brightness,
                );
                self.painter
                    .draw_text(buf, w, h, cx, y, cw, "Power Profile", 12.5, [205, 218, 244, 255]);
                y += 22;
                let mut px_btn = cx;
                for prof in ["performance", "balanced", "power-saver"] {
                    let active = sys.power_profile == prof;
                    let hovered = self.hovered_hit_id == Some(hit_id);
                    let bw = 112;
                    let bg = if active {
                        [accent[0], accent[1], accent[2], 232]
                    } else if hovered {
                        [48, 60, 92, 225]
                    } else {
                        [26, 34, 54, 215]
                    };
                    fill_rounded_rect(buf, w, h, px_btn, y, bw, 28, 7, bg);
                    let tx = px_btn + (bw - self.painter.measure_text(prof, 12.0)) / 2;
                    self.painter
                        .draw_text(buf, w, h, tx, y + 7, bw, prof, 12.0, [242, 246, 255, 255]);
                    self.popup_hits.push(HitRegion {
                        rect: Rectangle::new((ox + px_btn, oy + y).into(), (bw, 28).into()),
                        action: UiAction::SetPowerProfile(prof.to_string()),
                        right_action: None,
                        scroll_action: None,
                        id: hit_id,
                    });
                    hit_id += 1;
                    px_btn += bw + 10;
                }
                y += 42;

                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    &format!("Wi-Fi Radio ({})", sys.network_label),
                    sys.wifi_enabled,
                    [56, 204, 142],
                    UiAction::ToggleWifi,
                );
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Bluetooth Radio",
                    sys.bluetooth_on,
                    accent,
                    UiAction::ToggleBluetooth,
                );
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Bluetooth Manager",
                    "Blueman",
                    accent,
                    UiAction::OpenBluetoothManager,
                );
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Network Connections",
                    "Editor",
                    accent,
                    UiAction::OpenNetworkEditor,
                );
            }
            SettingsTab::StartupApps => {
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Default Terminal App",
                    &config.terminal_cmd,
                    accent,
                    UiAction::CycleTerminalApp,
                );
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Default Web Browser",
                    &config.browser_cmd,
                    accent,
                    UiAction::CycleBrowserApp,
                );
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Default File Manager",
                    &config.files_cmd,
                    accent,
                    UiAction::CycleFilesApp,
                );
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Auto-Launch Terminal on Login",
                    config.auto_launch_terminal_on_login,
                    accent,
                    UiAction::ToggleAutoLaunchTerminal,
                );
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Desktop Applications Index",
                    "Rescan",
                    accent,
                    UiAction::RescanApps,
                );
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Running Background Processes",
                    "Manage",
                    accent,
                    UiAction::ToggleActiveApps,
                );
            }
            SettingsTab::SystemKeys => {
                let shortcuts = [
                    ("Win Tap / Win+S / Space", "Start Launcher & Search"),
                    ("Alt+Tab / Win+Tab", "Window Switcher / Task View"),
                    ("Win+Up / Win+Down", "Expand Max / Shrink or Minimize"),
                    ("Win+Left / Win+Right", "Snap Left / Right Half (or Shrink)"),
                    ("Win+D / Win+E / Win+I", "Show Desktop / Files / Settings App"),
                    ("Ctrl+Win+Left / Right", "Switch Virtual Desktop"),
                    ("Win+A / V / N / U", "Quick / Audio / Calendar / Notifs"),
                    ("Ctrl+Win+Shift+R", "Seamless Reload Updated WM"),
                    ("Alt+F4 / Win+T / Shift+Esc", "Close / Pin on Top / Bg Apps"),
                ];
                for (k, desc) in shortcuts {
                    self.painter
                        .draw_text(buf, w, h, cx, y, 215, k, 11.0, [accent[0], accent[1], accent[2], 255]);
                    self.painter
                        .draw_text(buf, w, h, cx + 220, y, cw - 220, desc, 11.0, [214, 224, 248, 255]);
                    y += 22;
                }
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Windows-Style Shortcuts (Win+Arrows/D/E/I/Tab, Alt+F4)",
                    config.windows_shortcuts,
                    accent,
                    UiAction::ToggleWindowsShortcuts,
                );
                let primary_out = self
                    .display_telemetry
                    .outputs
                    .iter()
                    .find(|o| o.primary)
                    .or_else(|| self.display_telemetry.outputs.first());
                let hz_lbl = if config.output_refresh_hz == 0 {
                    primary_out
                        .map(|o| format!("Auto ({} Hz)", o.current_hz))
                        .unwrap_or_else(|| "Auto Max".to_string())
                } else {
                    format!("{} Hz", config.output_refresh_hz)
                };
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Monitor Refresh Rate (Confirm Prompt)",
                    &hz_lbl,
                    accent,
                    UiAction::CycleRefreshHz,
                );
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Variable Refresh Rate (VRR)",
                    config.vrr_mode.label(),
                    accent,
                    UiAction::CycleVrrMode,
                );
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Video & Idle Battery Saver (30Hz video / 10-24Hz idle)",
                    config.video_battery_saver,
                    accent,
                    UiAction::ToggleVideoBatterySaver,
                );
                let idle_lbl = if config.idle_refresh_hz == 0 {
                    "Off".to_string()
                } else {
                    format!("{} Hz", config.idle_refresh_hz)
                };
                self.draw_cc_choice(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Idle Downclock Refresh Target",
                    &idle_lbl,
                    accent,
                    UiAction::CycleIdleRefreshHz,
                );
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Non-VRR Hardware Modeset Saver",
                    config.battery_saver_modeset,
                    accent,
                    UiAction::ToggleBatterySaverModeset,
                );
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Dim Unfocused Windows (Keeps Active Bright)",
                    config.dim_unfocused_windows,
                    accent,
                    UiAction::ToggleDimUnfocused,
                );
                self.draw_cc_toggle(
                    buf,
                    w,
                    h,
                    ox,
                    oy,
                    cx,
                    cw,
                    &mut y,
                    &mut hit_id,
                    "Freeze Off-Screen Window Views (~1Hz App Keepalive)",
                    config.freeze_hidden_windows,
                    accent,
                    UiAction::ToggleFreezeHidden,
                );
                y += 6;
                let btns = [
                    ("Full Settings App", UiAction::LaunchSystemSettings, 134, [accent[0], accent[1], accent[2], 235]),
                    ("Trim RAM", UiAction::CleanRamCacheNow { deep_kernel: false }, 84, [38, 92, 82, 225]),
                    ("Save", UiAction::SaveConfigNow, 56, [40, 56, 92, 225]),
                    ("Reload", UiAction::ReloadConfig, 64, [32, 42, 68, 225]),
                    ("Reset", UiAction::ResetConfig, 58, [88, 46, 62, 225]),
                    ("Exit", UiAction::QuitSession, 52, [156, 46, 66, 230]),
                ];
                let mut bx = cx;
                for (label, act, bw, base_col) in btns {
                    if bx + bw > w - 12 {
                        break;
                    }
                    let hovered = self.hovered_hit_id == Some(hit_id);
                    let bg = if hovered {
                        [accent[0], accent[1], accent[2], 245]
                    } else {
                        base_col
                    };
                    fill_rounded_rect(buf, w, h, bx, y, bw, 30, 7, bg);
                    let tx = bx + (bw - self.painter.measure_text(label, 12.0)) / 2;
                    self.painter
                        .draw_text(buf, w, h, tx, y + 8, bw, label, 12.0, [244, 247, 255, 255]);
                    self.popup_hits.push(HitRegion {
                        rect: Rectangle::new((ox + bx, oy + y).into(), (bw, 30).into()),
                        action: act,
                        right_action: None,
                        scroll_action: None,
                        id: hit_id,
                    });
                    hit_id += 1;
                    bx += bw + 8;
                }
            }
        }

        // Persistent Disk Status Bar at Bottom
        let status_y = h - 34;
        fill_rounded_rect(buf, w, h, cx, status_y - 4, cw, 1, 0, [62, 80, 120, 100]);
        let msg = if saved_banner {
            "Saved preferences to ~/.config/velox-wm/config.toml"
        } else {
            "Preferences auto-save to ~/.config/velox-wm/config.toml"
        };
        self.painter.draw_text(
            buf,
            w,
            h,
            cx,
            status_y + 4,
            cw - 110,
            msg,
            11.0,
            if saved_banner {
                [88, 226, 158, 255]
            } else {
                [142, 156, 188, 220]
            },
        );
        let s_hover = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            w - 114,
            status_y,
            96,
            24,
            6,
            if s_hover {
                [accent[0], accent[1], accent[2], 240]
            } else {
                [32, 44, 72, 225]
            },
        );
        self.painter.draw_text(
            buf,
            w,
            h,
            w - 102,
            status_y + 5,
            80,
            "Save Prefs",
            11.5,
            [244, 247, 255, 255],
        );
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + w - 114, oy + status_y).into(), (96, 24).into()),
            action: UiAction::SaveConfigNow,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
    }

    fn draw_display_confirm(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 9800usize;
        let Some(confirm) = self.display_confirm.clone() else {
            return;
        };

        let pending_txt = if confirm.pending_hz == 0 {
            "Auto Max Native".to_string()
        } else {
            format!("{} Hz", confirm.pending_hz)
        };
        let prev_txt = if confirm.prev_hz == 0 {
            "Auto Max".to_string()
        } else {
            format!("{} Hz", confirm.prev_hz)
        };

        let (title, desc, primary_lbl, primary_act, cancel_lbl) =
            if let Some(deadline) = confirm.verify_deadline {
                let rem = deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs()
                    .max(1);
                (
                    "Keep These Display Settings?",
                    format!("Now running {pending_txt}. Reverting to {prev_txt} in {rem}s..."),
                    "Keep Changes",
                    UiAction::KeepDisplayChange,
                    "Revert Now",
                )
            } else {
                (
                    "Apply Display Change?",
                    format!("Switch monitor refresh rate from {prev_txt} to {pending_txt}?"),
                    "Apply",
                    UiAction::ConfirmDisplayChange,
                    "Cancel",
                )
            };

        self.painter
            .draw_text(buf, w, h, 22, 22, w - 44, title, 16.5, [244, 248, 255, 255]);
        self.painter
            .draw_text(buf, w, h, 22, 58, w - 44, &desc, 12.5, [186, 204, 236, 245]);

        let bw = (w - 58) / 2;
        let by = h - 58;
        let h0 = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            22,
            by,
            bw,
            36,
            9,
            [accent[0], accent[1], accent[2], if h0 { 250 } else { 225 }],
        );
        let t0 = 22 + (bw - self.painter.measure_text(primary_lbl, 12.5)) / 2;
        self.painter
            .draw_text(buf, w, h, t0, by + 11, bw - 8, primary_lbl, 12.5, [250, 252, 255, 255]);
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + 22, oy + by).into(), (bw, 36).into()),
            action: primary_act,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
        hit_id += 1;

        let bx1 = 22 + bw + 14;
        let h1 = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            bx1,
            by,
            bw,
            36,
            9,
            if h1 { [156, 52, 72, 235] } else { [34, 42, 64, 225] },
        );
        let t1 = bx1 + (bw - self.painter.measure_text(cancel_lbl, 12.5)) / 2;
        self.painter
            .draw_text(buf, w, h, t1, by + 11, bw - 8, cancel_lbl, 12.5, [242, 246, 255, 255]);
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + bx1, oy + by).into(), (bw, 36).into()),
            action: UiAction::CancelRevertDisplayChange,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_reload_confirm(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        reason: &str,
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 9850usize;

        fill_rounded_rect(buf, w, h, 20, 18, w - 40, 38, 9, [48, 36, 18, 225]);
        stroke_rounded_rect(buf, w, h, 20, 18, w - 40, 38, 9, 1, [242, 184, 72, 185]);
        self.painter.draw_text(
            buf,
            w,
            h,
            32,
            29,
            w - 64,
            "Confirm Seamless Compositor Reload",
            15.5,
            [255, 240, 206, 255],
        );

        let sub = if reason.trim().is_empty() {
            "Snapshot & Session Reload Notice".to_string()
        } else {
            reason.to_string()
        };
        self.painter
            .draw_text(buf, w, h, 22, 66, w - 44, &sub, 12.0, [accent[0], accent[1], accent[2], 255]);

        let lines = [
            ("1. Open apps will briefly close & relaunch as Velox restarts in place.", [232, 240, 255, 250]),
            ("2. SAVE your unsaved work / files / games first to prevent data loss!", [255, 198, 120, 255]),
            ("3. A session snapshot will save your workspaces, multitasking tiling,", [194, 212, 242, 245]),
            ("   window snap zones, monitor positions, and notifications.", [194, 212, 242, 245]),
            ("4. Window placement & multitasking layout restore automatically,", [176, 230, 205, 245]),
            ("   but you will need to resume your active work inside apps manually.", [176, 230, 205, 245]),
        ];
        let mut ly = 92;
        for (txt, col) in lines {
            self.painter.draw_text(buf, w, h, 22, ly, w - 44, txt, 11.8, col);
            ly += 24;
        }

        let bw = (w - 58) / 2;
        let by = h - 54;
        let h0 = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            22,
            by,
            bw,
            36,
            9,
            if h0 { [48, 198, 134, 250] } else { [36, 168, 114, 230] },
        );
        let l0 = "Create Snapshot & Reload";
        let t0 = 22 + (bw - self.painter.measure_text(l0, 12.2)) / 2;
        self.painter
            .draw_text(buf, w, h, t0, by + 11, bw - 8, l0, 12.2, [246, 255, 250, 255]);
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + 22, oy + by).into(), (bw, 36).into()),
            action: UiAction::SeamlessReloadWm,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
        hit_id += 1;

        let bx1 = 22 + bw + 14;
        let h1 = self.hovered_hit_id == Some(hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            bx1,
            by,
            bw,
            36,
            9,
            if h1 { [156, 52, 72, 235] } else { [34, 42, 64, 225] },
        );
        let l1 = "Cancel";
        let t1 = bx1 + (bw - self.painter.measure_text(l1, 12.2)) / 2;
        self.painter
            .draw_text(buf, w, h, t1, by + 11, bw - 8, l1, 12.2, [242, 246, 255, 255]);
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + bx1, oy + by).into(), (bw, 36).into()),
            action: UiAction::CancelReloadConfirm,
            right_action: None,
            scroll_action: None,
            id: hit_id,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_notifications_drawer(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        apps: &[DesktopApp],
        notifs: &NotificationSnapshot,
        updates: &UpdateStatus,
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 9900usize;
        let mut y = 14;

        let sub = if notifs.cooldown_active {
            format!(
                "Cooldown {}s • {} received • Volume quieted",
                notifs.cooldown_remaining_secs, notifs.cooldown_received_count
            )
        } else if notifs.dbus_active {
            format!(
                "{} active • {} silent • D-Bus active",
                notifs.items.len(),
                notifs.quiet_unread_count
            )
        } else {
            format!("{} notifications", notifs.items.len())
        };
        self.painter
            .draw_text(buf, w, h, 18, y, 220, "Notifications", 15.5, [244, 247, 255, 255]);
        self.painter
            .draw_text(buf, w, h, 18, y + 20, 270, &sub, 10.5, [156, 174, 206, 235]);
        self.draw_close_btn(buf, w, h, ox, oy, y - 2, &mut hit_id);

        // Quick header action pills: DND, Updates, Clear
        let clear_act = if self.notif_tab == NotifDrawerTab::History {
            UiAction::ClearNotificationHistory
        } else {
            UiAction::ClearNotifications
        };
        let hdr_btns: [(&str, bool, UiAction, i32); 3] = [
            (
                if config.notifications_dnd { "DND: ON" } else { "DND" },
                config.notifications_dnd,
                UiAction::ToggleDnd,
                56,
            ),
            (
                if updates.checking { "..." } else { "Updates" },
                updates.local_update_ready || updates.github_latest_tag.is_some(),
                UiAction::CheckUpdatesNow,
                60,
            ),
            (
                if self.notif_tab == NotifDrawerTab::History {
                    "Clear H"
                } else {
                    "Clear"
                },
                false,
                clear_act,
                52,
            ),
        ];
        let mut hx = w - 48;
        for (lbl, active, act, bw) in hdr_btns.into_iter().rev() {
            hx -= bw + 6;
            let hov = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                hx,
                y - 2,
                bw,
                24,
                6,
                if active {
                    [accent[0], accent[1], accent[2], 225]
                } else if hov {
                    [48, 60, 92, 225]
                } else {
                    [26, 34, 54, 220]
                },
            );
            let tx = hx + (bw - self.painter.measure_text(lbl, 10.5)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 4, bw - 6, lbl, 10.5, [238, 244, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + hx, oy + y - 2).into(), (bw, 24).into()),
                action: act,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
        y += 38;

        // Pixel-OS sub-tabs row: Inbox | Priority | Snoozed | History | Apps
        let mut tx_tab = 14;
        for tab in NotifDrawerTab::ALL {
            let count_suffix = match tab {
                NotifDrawerTab::Snoozed if !notifs.snoozed.is_empty() => {
                    format!(" ({})", notifs.snoozed.len())
                }
                NotifDrawerTab::Priority => {
                    let pc = notifs.items.iter().filter(|n| n.is_priority).count();
                    if pc > 0 { format!(" ({pc})") } else { String::new() }
                }
                _ => String::new(),
            };
            let lbl = format!("{}{}", tab.label(), count_suffix);
            let bw = (self.painter.measure_text(&lbl, 10.5) + 16).max(54);
            let active = self.notif_tab == *tab;
            let hov = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                tx_tab,
                y,
                bw,
                23,
                7,
                if active {
                    [accent[0], accent[1], accent[2], 228]
                } else if hov {
                    [42, 54, 84, 220]
                } else {
                    [20, 27, 46, 220]
                },
            );
            let ttx = tx_tab + (bw - self.painter.measure_text(&lbl, 10.5)) / 2;
            self.painter.draw_text(
                buf,
                w,
                h,
                ttx,
                y + 5,
                bw - 6,
                &lbl,
                10.5,
                [238, 244, 255, 255],
            );
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + tx_tab, oy + y).into(), (bw, 23).into()),
                action: UiAction::SelectNotifTab(*tab),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            tx_tab += bw + 6;
        }
        y += 30;

        // Update / Seamless Live Reload Card
        let update_available = updates.local_update_ready || updates.github_latest_tag.is_some();
        if update_available && self.notif_tab == NotifDrawerTab::All {
            fill_rounded_rect(buf, w, h, 14, y, w - 28, 56, 10, [24, 52, 56, 235]);
            stroke_rounded_rect(buf, w, h, 14, y, w - 28, 56, 10, 1, [68, 212, 148, 170]);
            let utitle = if updates.local_update_ready {
                "Update Installed — Ready to Reload"
            } else {
                "GitHub Release Available"
            };
            let udesc = if let Some(ref tag) = updates.github_latest_tag {
                format!("Version {tag} • {}", updates.last_check_note)
            } else {
                updates.last_check_note.clone()
            };
            self.painter.draw_text(
                buf,
                w,
                h,
                24,
                y + 8,
                w - 156,
                utitle,
                12.0,
                [235, 255, 246, 255],
            );
            self.painter.draw_text(
                buf,
                w,
                h,
                24,
                y + 27,
                w - 156,
                &udesc,
                10.0,
                [172, 224, 200, 245],
            );
            let rb_x = w - 118;
            let rhov = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                rb_x,
                y + 13,
                94,
                30,
                8,
                if rhov { [56, 206, 138, 245] } else { [42, 174, 118, 230] },
            );
            let rlbl = "Reload WM";
            let rtx = rb_x + (94 - self.painter.measure_text(rlbl, 11.0)) / 2;
            self.painter
                .draw_text(buf, w, h, rtx, y + 22, 88, rlbl, 11.0, [246, 255, 250, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + rb_x, oy + y + 13).into(), (94, 30).into()),
                action: UiAction::PromptReloadConfirm(utitle.to_string()),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            y += 64;
        }

        // Cooldown Banner when active
        if notifs.cooldown_active && matches!(self.notif_tab, NotifDrawerTab::All | NotifDrawerTab::Priority) {
            fill_rounded_rect(buf, w, h, 14, y, w - 28, 36, 8, [44, 36, 68, 235]);
            stroke_rounded_rect(buf, w, h, 14, y, w - 28, 36, 8, 1, [242, 182, 74, 150]);
            let ctxt = format!(
                "Cooldown active ({}s): alerts & volume lowered • {} quieted items below",
                notifs.cooldown_remaining_secs, notifs.cooldown_received_count
            );
            self.painter.draw_text(buf, w, h, 24, y + 11, w - 48, &ctxt, 10.5, [252, 224, 162, 255]);
            y += 42;
        }

        match self.notif_tab {
            NotifDrawerTab::Snoozed => {
                if notifs.snoozed.is_empty() {
                    fill_rounded_rect(buf, w, h, 14, y, w - 28, 66, 10, [20, 27, 46, 220]);
                    self.painter.draw_text(buf, w, h, 26, y + 14, w - 52, "No snoozed notifications", 12.5, [190, 206, 236, 240]);
                    self.painter.draw_text(buf, w, h, 26, y + 36, w - 52, "Use 'Zz' on any notification to remind yourself later", 10.5, [136, 154, 188, 225]);
                    return;
                }
                let now = notif_unix_now();
                for s in notifs.snoozed.iter().skip(self.notif_scroll.min(notifs.snoozed.len().saturating_sub(1))) {
                    if y + 66 > h - 12 { break; }
                    let rem_m = (s.snooze_until_epoch.saturating_sub(now) + 59) / 60;
                    fill_rounded_rect(buf, w, h, 14, y, w - 28, 60, 10, [22, 30, 50, 232]);
                    let shdr = format!("{} • wakes in {}m", s.app_name, rem_m.max(1));
                    self.painter.draw_text(buf, w, h, 24, y + 8, w - 130, &shdr, 10.5, [242, 198, 114, 245]);
                    self.painter.draw_text(buf, w, h, 24, y + 26, w - 130, &s.summary, 12.0, [240, 246, 255, 255]);
                    let rx = w - 98;
                    fill_rounded_rect(buf, w, h, rx, y + 16, 74, 26, 6, [accent[0], accent[1], accent[2], 220]);
                    self.painter.draw_text(buf, w, h, rx + 12, y + 23, 56, "Restore", 10.5, [246, 250, 255, 255]);
                    self.popup_hits.push(HitRegion {
                        rect: Rectangle::new((ox + rx, oy + y + 16).into(), (74, 26).into()),
                        action: UiAction::UnsnoozeNotification(s.id),
                        right_action: None,
                        scroll_action: None,
                        id: hit_id,
                    });
                    hit_id += 1;
                    y += 68;
                }
                return;
            }
            NotifDrawerTab::History => {
                if !config.notification_history_enabled {
                    fill_rounded_rect(buf, w, h, 14, y, w - 28, 62, 10, [20, 27, 46, 220]);
                    self.painter.draw_text(buf, w, h, 26, y + 22, w - 52, "Notification History is turned off in Settings", 11.5, [196, 210, 236, 240]);
                    return;
                }
                if notifs.history.is_empty() {
                    fill_rounded_rect(buf, w, h, 14, y, w - 28, 62, 10, [20, 27, 46, 220]);
                    self.painter.draw_text(buf, w, h, 26, y + 22, w - 52, "No recent notifications in history yet", 11.5, [196, 210, 236, 240]);
                    return;
                }
                for entry in notifs.history.iter().skip(self.notif_scroll.min(notifs.history.len().saturating_sub(1))) {
                    if y + 58 > h - 12 { break; }
                    fill_rounded_rect(buf, w, h, 14, y, w - 28, 52, 8, [20, 27, 46, 226]);
                    let line1 = format!(
                        "{} • {} • {} [{}]",
                        entry.app_name,
                        entry.category.short_tag(),
                        format_relative_time(entry.timestamp_epoch),
                        entry.status
                    );
                    self.painter.draw_text(buf, w, h, 24, y + 8, w - 48, &line1, 10.0, [156, 178, 214, 235]);
                    let line2 = entry.summary_recap.as_deref().unwrap_or(&entry.summary);
                    self.painter.draw_text(buf, w, h, 24, y + 26, w - 48, line2, 11.5, [236, 242, 255, 255]);
                    y += 58;
                }
                return;
            }
            NotifDrawerTab::Apps => {
                self.painter.draw_text(
                    buf,
                    w,
                    h,
                    18,
                    y,
                    w - 36,
                    "Per-app notification permissions (Allowed / Silent / Off)",
                    11.0,
                    [172, 192, 226, 240],
                );
                y += 22;
                let mut app_names: Vec<String> = notifs.seen_apps.clone();
                for app in apps.iter().take(24) {
                    if !app_names.iter().any(|n| n.eq_ignore_ascii_case(&app.name)) {
                        app_names.push(app.name.clone());
                    }
                }
                for app_name in app_names.into_iter().skip(self.notif_scroll) {
                    if y + 40 > h - 12 { break; }
                    let key = normalize_app_key(&app_name);
                    let disabled = config
                        .disabled_notification_apps
                        .iter()
                        .any(|a| normalize_app_key(a) == key);
                    let silent = !disabled
                        && config
                            .silent_notification_apps
                            .iter()
                            .any(|a| normalize_app_key(a) == key);
                    fill_rounded_rect(buf, w, h, 14, y, w - 28, 34, 8, [20, 27, 46, 226]);
                    self.painter.draw_text(buf, w, h, 24, y + 10, w - 180, &app_name, 11.5, [238, 244, 255, 255]);

                    let sil_x = w - 142;
                    fill_rounded_rect(
                        buf,
                        w,
                        h,
                        sil_x,
                        y + 5,
                        56,
                        24,
                        6,
                        if silent { [112, 92, 168, 225] } else { [34, 44, 68, 220] },
                    );
                    self.painter.draw_text(buf, w, h, sil_x + 11, y + 11, 44, "Silent", 10.0, [236, 242, 255, 255]);
                    self.popup_hits.push(HitRegion {
                        rect: Rectangle::new((ox + sil_x, oy + y + 5).into(), (56, 24).into()),
                        action: UiAction::ToggleAppNotifSilent(app_name.clone()),
                        right_action: None,
                        scroll_action: None,
                        id: hit_id,
                    });
                    hit_id += 1;

                    let dis_x = w - 78;
                    let on_lbl = if disabled { "Off" } else { "On" };
                    fill_rounded_rect(
                        buf,
                        w,
                        h,
                        dis_x,
                        y + 5,
                        54,
                        24,
                        6,
                        if disabled { [146, 54, 72, 225] } else { [42, 168, 116, 225] },
                    );
                    self.painter.draw_text(buf, w, h, dis_x + 17, y + 11, 36, on_lbl, 10.5, [246, 250, 255, 255]);
                    self.popup_hits.push(HitRegion {
                        rect: Rectangle::new((ox + dis_x, oy + y + 5).into(), (54, 24).into()),
                        action: UiAction::ToggleAppNotifDisabled(app_name),
                        right_action: None,
                        scroll_action: None,
                        id: hit_id,
                    });
                    hit_id += 1;
                    y += 40;
                }
                return;
            }
            NotifDrawerTab::Priority | NotifDrawerTab::All => {}
        }

        // Group conversation summaries banner in Inbox/Priority when enabled & multi-message chats exist
        if config.notification_summaries_enabled
            && !notifs.conversation_groups.is_empty()
            && self.notif_scroll == 0
        {
            if let Some(grp) = notifs
                .conversation_groups
                .iter()
                .find(|g| g.count >= 2 && (self.notif_tab != NotifDrawerTab::Priority || g.is_priority))
            {
                if y + 52 <= h - 14 {
                    fill_rounded_rect(buf, w, h, 14, y, w - 28, 46, 9, [24, 34, 58, 232]);
                    stroke_rounded_rect(buf, w, h, 14, y, w - 28, 46, 9, 1, [accent[0], accent[1], accent[2], 120]);
                    let hdr = format!("Summary • {} ({} msgs · on-device AI)", grp.title, grp.count);
                    self.painter.draw_text(buf, w, h, 24, y + 7, w - 48, &hdr, 10.0, [accent[0], accent[1], accent[2], 255]);
                    self.painter.draw_text(buf, w, h, 24, y + 24, w - 48, &grp.recap, 10.5, [218, 230, 250, 250]);
                    y += 52;
                }
            }
        }

        let priority_only = self.notif_tab == NotifDrawerTab::Priority;
        let primary_items: Vec<&NotificationItem> = notifs
            .items
            .iter()
            .filter(|n| {
                if priority_only {
                    n.is_priority
                } else {
                    !(config.notification_organizer_enabled
                        && n.is_silent
                        && !n.is_priority
                        && ORGANIZER_SILENCEABLE_CATEGORIES
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(n.category.label())))
                }
            })
            .collect();
        let bundled_items: Vec<&NotificationItem> = if priority_only || !config.notification_organizer_enabled {
            Vec::new()
        } else {
            notifs
                .items
                .iter()
                .filter(|n| {
                    n.is_silent
                        && !n.is_priority
                        && ORGANIZER_SILENCEABLE_CATEGORIES
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(n.category.label()))
                })
                .collect()
        };

        if primary_items.is_empty() && bundled_items.is_empty() {
            fill_rounded_rect(buf, w, h, 14, y, w - 28, 72, 10, [20, 27, 46, 220]);
            let empty_msg = if priority_only {
                "No Priority Conversations yet — tap ★ on any chat card"
            } else {
                "All caught up — no unread notifications"
            };
            self.painter.draw_text(buf, w, h, 28, y + 18, w - 56, empty_msg, 12.0, [188, 204, 234, 240]);
            self.painter.draw_text(buf, w, h, 28, y + 40, w - 56, "Listening on org.freedesktop.Notifications • Win+U to toggle", 10.0, [134, 152, 188, 225]);
            return;
        }

        let start = self.notif_scroll.min(primary_items.len().saturating_sub(1));
        for n in primary_items.iter().skip(start) {
            if !self.draw_notif_card(buf, w, h, ox, oy, &mut y, &mut hit_id, config, n) {
                return;
            }
        }

        // Pixel-OS On-Device AI Organizer Bundle card
        if !bundled_items.is_empty() && y + 58 <= h - 12 {
            let bh = 50;
            let hov = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                14,
                y,
                w - 28,
                bh,
                10,
                if hov { [32, 42, 68, 232] } else { [22, 29, 48, 228] },
            );
            stroke_rounded_rect(buf, w, h, 14, y, w - 28, bh, 10, 1, [96, 120, 170, 120]);
            let b_lbl = format!(
                "Bundled by Organizer ({} silent) · {}",
                bundled_items.len(),
                if self.notif_organizer_expanded { "Hide" } else { "Show" }
            );
            self.painter.draw_text(buf, w, h, 24, y + 8, w - 52, &b_lbl, 11.5, [218, 230, 252, 255]);
            self.painter.draw_text(
                buf,
                w,
                h,
                24,
                y + 27,
                w - 52,
                "Uses AI on your device (may make mistakes) · Never sent to Google",
                9.5,
                [144, 166, 204, 225],
            );
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + 14, oy + y).into(), (w - 28, bh).into()),
                action: UiAction::ToggleOrganizerExpanded,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            y += bh + 6;

            if self.notif_organizer_expanded {
                if y + 28 <= h - 12 {
                    let mut cx_cat = 18;
                    for cat in ORGANIZER_SILENCEABLE_CATEGORIES {
                        let cw = (self.painter.measure_text(cat, 10.0) + 14).max(58);
                        if cx_cat + cw > w - 18 {
                            break;
                        }
                        let active = config
                            .organized_silenced_categories
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(cat));
                        fill_rounded_rect(
                            buf,
                            w,
                            h,
                            cx_cat,
                            y,
                            cw,
                            21,
                            6,
                            if active {
                                [accent[0], accent[1], accent[2], 220]
                            } else {
                                [28, 38, 62, 220]
                            },
                        );
                        let ttx = cx_cat + (cw - self.painter.measure_text(cat, 10.0)) / 2;
                        self.painter.draw_text(
                            buf,
                            w,
                            h,
                            ttx,
                            y + 4,
                            cw - 4,
                            cat,
                            10.0,
                            [238, 244, 255, 255],
                        );
                        self.popup_hits.push(HitRegion {
                            rect: Rectangle::new((ox + cx_cat, oy + y).into(), (cw, 21).into()),
                            action: UiAction::ToggleOrganizerSilenceCategory((*cat).to_string()),
                            right_action: None,
                            scroll_action: None,
                            id: hit_id,
                        });
                        hit_id += 1;
                        cx_cat += cw + 6;
                    }
                    y += 26;
                }
                for n in bundled_items {
                    if !self.draw_notif_card(buf, w, h, ox, oy, &mut y, &mut hit_id, config, n) {
                        break;
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_notif_card(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        y: &mut i32,
        hit_id: &mut usize,
        config: &Config,
        n: &NotificationItem,
    ) -> bool {
        let accent = config.accent_rgb();
        let editing_reply = self.notif_reply_target == Some(n.id);
        let has_chips = !n.actions.is_empty() || !n.suggested_actions.is_empty();
        let has_recap = config.notification_summaries_enabled && n.summary_recap.is_some();
        let mut card_h = 54;
        if !n.body.is_empty() || has_recap {
            card_h += 18;
        }
        if has_recap {
            card_h += 12;
        }
        if editing_reply {
            card_h += 32;
        }
        if has_chips {
            card_h += 28;
        }
        if *y + card_h > h - 10 {
            return false;
        }

        let bar_col = if n.is_priority {
            [248, 198, 72, 255]
        } else {
            match n.urgency {
                NotificationUrgency::Critical => [232, 86, 104, 255],
                NotificationUrgency::Normal => [accent[0], accent[1], accent[2], 255],
                NotificationUrgency::Low => [110, 128, 166, 230],
            }
        };
        fill_rounded_rect(buf, w, h, 14, *y, w - 28, card_h, 10, [22, 30, 50, 232]);
        fill_rounded_rect(buf, w, h, 14, *y + 6, 3, card_h - 12, 2, bar_col);

        let tag = if n.is_priority {
            "★ Priority".to_string()
        } else if n.arrived_in_cooldown {
            "Cooldown".to_string()
        } else {
            n.category.short_tag().to_string()
        };
        let hdr = format!(
            "{} • {} • {}",
            n.app_name,
            tag,
            format_relative_time(n.timestamp_epoch)
        );
        self.painter
            .draw_text(buf, w, h, 26, *y + 8, w - 150, &hdr, 10.5, [156, 180, 218, 240]);
        self.painter
            .draw_text(buf, w, h, 26, *y + 24, w - 82, &n.summary, 12.5, [242, 247, 255, 255]);

        if let Some(ref recap) = n.summary_recap.as_ref().filter(|_| has_recap) {
            self.painter
                .draw_text(buf, w, h, 26, *y + 42, w - 52, recap, 10.5, [164, 224, 198, 248]);
            self.painter.draw_text(
                buf,
                w,
                h,
                26,
                *y + 56,
                w - 52,
                "On-device AI summary (may make mistakes)",
                9.0,
                [130, 150, 184, 215],
            );
        } else if !n.body.is_empty() {
            self.painter
                .draw_text(buf, w, h, 26, *y + 42, w - 52, &n.body, 11.0, [186, 202, 232, 240]);
        }

        // Top-right card action buttons: [x] Dismiss, [Zz] Snooze, [★] Priority, [Reply]
        let mut bx = w - 40;
        let draw_micro_btn = |hits: &mut Vec<HitRegion>,
                              painter: &mut UiPainter,
                              buf: &mut [u8],
                              id_ctr: &mut usize,
                              x: i32,
                              bw: i32,
                              lbl: &str,
                              bg: [u8; 4],
                              act: UiAction| {
            fill_rounded_rect(buf, w, h, x, *y + 7, bw, 19, 5, bg);
            let tx = x + (bw - painter.measure_text(lbl, 9.5)) / 2;
            painter.draw_text(buf, w, h, tx, *y + 11, bw - 4, lbl, 9.5, [238, 244, 255, 255]);
            hits.push(HitRegion {
                rect: Rectangle::new((ox + x, oy + *y + 7).into(), (bw, 19).into()),
                action: act,
                right_action: None,
                scroll_action: None,
                id: *id_ctr,
            });
            *id_ctr += 1;
        };

        draw_micro_btn(
            &mut self.popup_hits,
            &mut self.painter,
            buf,
            hit_id,
            bx,
            20,
            "x",
            if self.hovered_hit_id == Some(*hit_id) {
                [180, 58, 78, 235]
            } else {
                [32, 42, 66, 210]
            },
            UiAction::DismissNotification(n.id),
        );
        if config.allow_notification_snooze {
            bx -= 34;
            draw_micro_btn(
                &mut self.popup_hits,
                &mut self.painter,
                buf,
                hit_id,
                bx,
                30,
                "15m",
                if self.hovered_hit_id == Some(*hit_id) {
                    [78, 92, 134, 235]
                } else {
                    [32, 42, 66, 210]
                },
                UiAction::SnoozeNotification {
                    id: n.id,
                    secs: 900,
                },
            );
        }
        if let Some(ref ckey) = n.conversation_key {
            bx -= 26;
            draw_micro_btn(
                &mut self.popup_hits,
                &mut self.painter,
                buf,
                hit_id,
                bx,
                22,
                "★",
                if n.is_priority {
                    [202, 146, 36, 235]
                } else {
                    [32, 42, 66, 210]
                },
                UiAction::TogglePriorityConversation(ckey.clone()),
            );
        }
        if n.is_conversation || n.supports_inline_reply {
            bx -= 44;
            draw_micro_btn(
                &mut self.popup_hits,
                &mut self.painter,
                buf,
                hit_id,
                bx,
                40,
                "Reply",
                if editing_reply {
                    [accent[0], accent[1], accent[2], 235]
                } else {
                    [32, 42, 66, 210]
                },
                UiAction::OpenInlineReply(n.id),
            );
        }

        // Inline reply composer box
        if editing_reply {
            let ry = *y + card_h - if has_chips { 58 } else { 32 };
            fill_rounded_rect(buf, w, h, 24, ry, w - 108, 24, 6, [12, 18, 32, 245]);
            stroke_rounded_rect(buf, w, h, 24, ry, w - 108, 24, 6, 1, [accent[0], accent[1], accent[2], 190]);
            let prompt = if self.notif_reply_draft.is_empty() {
                "Type reply + Enter...".to_string()
            } else {
                format!("{}_", self.notif_reply_draft)
            };
            self.painter.draw_text(buf, w, h, 30, ry + 6, w - 120, &prompt, 10.5, [238, 244, 255, 255]);
            let sx = w - 78;
            fill_rounded_rect(buf, w, h, sx, ry, 52, 24, 6, [accent[0], accent[1], accent[2], 230]);
            self.painter.draw_text(buf, w, h, sx + 12, ry + 6, 36, "Send", 10.5, [246, 250, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + sx, oy + ry).into(), (52, 24).into()),
                action: UiAction::SendNotificationReply {
                    id: n.id,
                    text: self.notif_reply_draft.clone(),
                },
                right_action: None,
                scroll_action: None,
                id: *hit_id,
            });
            *hit_id += 1;
        }

        // Action + Smart Suggested Chips row
        if has_chips {
            let mut ax = 26;
            let row_y = *y + card_h - 27;
            for act in n.actions.iter().take(2) {
                let aw = (self.painter.measure_text(&act.label, 10.5) + 16).clamp(56, 134);
                if ax + aw > w - 20 { break; }
                let hov = self.hovered_hit_id == Some(*hit_id);
                fill_rounded_rect(
                    buf,
                    w,
                    h,
                    ax,
                    row_y,
                    aw,
                    21,
                    6,
                    if hov { [accent[0], accent[1], accent[2], 235] } else { [38, 52, 84, 225] },
                );
                let atx = ax + (aw - self.painter.measure_text(&act.label, 10.5)) / 2;
                self.painter.draw_text(buf, w, h, atx, row_y + 4, aw - 6, &act.label, 10.5, [244, 248, 255, 255]);
                self.popup_hits.push(HitRegion {
                    rect: Rectangle::new((ox + ax, oy + row_y).into(), (aw, 21).into()),
                    action: UiAction::InvokeNotificationAction {
                        id: n.id,
                        key: act.key.clone(),
                    },
                    right_action: None,
                    scroll_action: None,
                    id: *hit_id,
                });
                *hit_id += 1;
                ax += aw + 6;
            }
            for sug in n.suggested_actions.iter().take(3) {
                let aw = (self.painter.measure_text(&sug.label, 10.5) + 16).clamp(58, 136);
                if ax + aw > w - 20 { break; }
                let hov = self.hovered_hit_id == Some(*hit_id);
                let chip_bg = if hov {
                    [accent[0], accent[1], accent[2], 235]
                } else {
                    match sug.kind {
                        SuggestedActionKind::CopyCode => [40, 104, 86, 225],
                        _ => [30, 44, 74, 225],
                    }
                };
                fill_rounded_rect(buf, w, h, ax, row_y, aw, 21, 6, chip_bg);
                let atx = ax + (aw - self.painter.measure_text(&sug.label, 10.5)) / 2;
                self.painter.draw_text(buf, w, h, atx, row_y + 4, aw - 6, &sug.label, 10.5, [238, 246, 255, 255]);
                self.popup_hits.push(HitRegion {
                    rect: Rectangle::new((ox + ax, oy + row_y).into(), (aw, 21).into()),
                    action: UiAction::RunSuggestedAction {
                        id: n.id,
                        kind: sug.kind,
                        payload: sug.payload.clone(),
                    },
                    right_action: None,
                    scroll_action: None,
                    id: *hit_id,
                });
                *hit_id += 1;
                ax += aw + 6;
            }
        }

        *y += card_h + 7;
        true
    }

    fn ensure_toast(
        &mut self,
        output_geo: Rectangle<i32, Logical>,
        config: &Config,
        notifs: &NotificationSnapshot,
        updates: &UpdateStatus,
        allow_toasts: bool,
    ) {
        self.toast_hits.clear();
        if !allow_toasts || self.active_popup == ActivePopup::Notifications {
            self.toast_buffer = None;
            self.toast_signature.clear();
            return;
        }
        let now = notif_unix_now();
        let active: Option<&NotificationItem> = notifs
            .items
            .iter()
            .find(|n| Self::toast_eligible(n, notifs, config, now));
        let Some(item) = active else {
            self.toast_buffer = None;
            self.toast_signature.clear();
            return;
        };

        let tw = 356.min(output_geo.size.w - 24).max(250);
        let first_sug = item.suggested_actions.first();
        let has_act = !item.actions.is_empty() || updates.local_update_ready || first_sug.is_some();
        let th = if has_act { 98 } else { 76 };
        let ph = config.panel_height.clamp(34, 56);
        let pad = if config.shelf_style == ShelfStyle::Floating { 16 } else { 10 };
        let tx = output_geo.loc.x + (output_geo.size.w - tw - 14).max(10);
        let ty = output_geo.loc.y
            + match config.panel_position {
                PanelPosition::Top => ph + pad,
                PanelPosition::Bottom => (output_geo.size.h - ph - th - pad).max(10),
            };
        self.toast_rect = Rectangle::new((tx, ty).into(), (tw, th).into());

        let sig = format!(
            "{}x{}@{},{}:{}:{}:{}:{}:{}",
            tw,
            th,
            tx,
            ty,
            item.id,
            notifs.version,
            self.hovered_hit_id.unwrap_or(usize::MAX),
            updates.local_update_ready,
            config.allow_notification_snooze
        );
        let accent = config.accent_rgb();
        if sig != self.toast_signature || self.toast_buffer.is_none() {
            self.toast_signature = sig;
            let needed = (tw * th * 4) as usize;
            if self.toast_pixels.len() != needed {
                self.toast_pixels.resize(needed, 0);
            } else {
                self.toast_pixels.fill(0);
            }
            let mut buf = std::mem::take(&mut self.toast_pixels);
            fill_rounded_rect(&mut buf, tw, th, 0, 0, tw, th, 12, [14, 19, 34, 245]);
            let border_col = if item.is_priority {
                [248, 198, 72, 200]
            } else {
                [accent[0], accent[1], accent[2], 165]
            };
            stroke_rounded_rect(&mut buf, tw, th, 0, 0, tw, th, 12, 1, border_col);
            let pri_prefix = if item.is_priority { "★ Priority • " } else { "" };
            let hdr = format!("{pri_prefix}{} • now", item.app_name);
            self.painter.draw_text(&mut buf, tw, th, 14, 10, tw - 76, &hdr, 10.5, border_col);
            self.painter.draw_text(&mut buf, tw, th, 14, 26, tw - 48, &item.summary, 12.5, [244, 248, 255, 255]);
            let preview = if config.notification_summaries_enabled {
                item.summary_recap.as_deref().unwrap_or(&item.body)
            } else {
                &item.body
            };
            if !preview.is_empty() {
                self.painter.draw_text(&mut buf, tw, th, 14, 44, tw - 32, preview, 11.0, [186, 204, 234, 240]);
            }
            if config.allow_notification_snooze {
                fill_rounded_rect(&mut buf, tw, th, tw - 66, 8, 30, 20, 6, [34, 44, 70, 220]);
                self.painter.draw_text(&mut buf, tw, th, tw - 62, 12, 24, "15m", 10.0, [222, 234, 255, 255]);
            }
            fill_rounded_rect(&mut buf, tw, th, tw - 30, 8, 22, 20, 6, [34, 44, 70, 220]);
            self.painter.draw_text(&mut buf, tw, th, tw - 23, 12, 16, "x", 11.0, [238, 244, 255, 255]);
            if has_act {
                let lbl = item
                    .actions
                    .first()
                    .map(|a| a.label.as_str())
                    .or_else(|| first_sug.map(|s| s.label.as_str()))
                    .unwrap_or("Reload WM Now");
                fill_rounded_rect(&mut buf, tw, th, 14, th - 30, 146, 22, 6, [accent[0], accent[1], accent[2], 225]);
                let atx = 14 + (146 - self.painter.measure_text(lbl, 11.0)) / 2;
                self.painter.draw_text(&mut buf, tw, th, atx, th - 25, 138, lbl, 11.0, [246, 250, 255, 255]);
            }
            update_memory_buffer(&mut self.toast_buffer, &mut self.toast_buf_size, (tw, th), &buf);
            self.toast_pixels = buf;
        }

        self.toast_hits.push(HitRegion {
            rect: Rectangle::new((tx, ty).into(), (tw, th).into()),
            action: UiAction::ToggleNotifications,
            right_action: Some(UiAction::DismissNotification(item.id)),
            scroll_action: None,
            id: 9990,
        });
        if has_act {
            let act = if let Some(first) = item.actions.first() {
                UiAction::InvokeNotificationAction {
                    id: item.id,
                    key: first.key.clone(),
                }
            } else if let Some(sug) = first_sug {
                UiAction::RunSuggestedAction {
                    id: item.id,
                    kind: sug.kind,
                    payload: sug.payload.clone(),
                }
            } else {
                UiAction::PromptReloadConfirm("New compositor update ready to reload".to_string())
            };
            self.toast_hits.push(HitRegion {
                rect: Rectangle::new((tx + 14, ty + th - 30).into(), (146, 22).into()),
                action: act,
                right_action: None,
                scroll_action: None,
                id: 9991,
            });
        }
        if config.allow_notification_snooze {
            self.toast_hits.push(HitRegion {
                rect: Rectangle::new((tx + tw - 66, ty + 8).into(), (30, 20).into()),
                action: UiAction::SnoozeNotification {
                    id: item.id,
                    secs: 900,
                },
                right_action: None,
                scroll_action: None,
                id: 9993,
            });
        }
        self.toast_hits.push(HitRegion {
            rect: Rectangle::new((tx + tw - 30, ty + 8).into(), (22, 20).into()),
            action: UiAction::DismissNotification(item.id),
            right_action: None,
            scroll_action: None,
            id: 9992,
        });
    }

    fn ensure_saver_overlay(
        &mut self,
        output_geo: Rectangle<i32, Logical>,
        config: &Config,
        allow_toasts: bool,
    ) {
        if !allow_toasts || !config.show_refresh_overlay {
            self.saver_overlay_buffer = None;
            self.saver_overlay_signature.clear();
            return;
        }
        let Some(out) = self
            .display_telemetry
            .outputs
            .iter()
            .find(|o| o.primary)
            .or_else(|| self.display_telemetry.outputs.first())
        else {
            self.saver_overlay_buffer = None;
            self.saver_overlay_signature.clear();
            return;
        };

        let pace_hz = if out.active_pacing_hz > 0 {
            out.active_pacing_hz
        } else {
            out.current_hz
        };
        let state_label = out.battery_saver_state.trim();
        let saver_active = !state_label.is_empty()
            && state_label != "Active"
            && state_label != "Normal"
            && state_label != "Fixed";
        let tag = if self.display_telemetry.detected_video_fps > 0 && config.video_battery_saver {
            format!("Video {}fps", self.display_telemetry.detected_video_fps)
        } else if saver_active {
            state_label.to_string()
        } else if out.vrr_enabled {
            "VRR".to_string()
        } else if config.seamless_refresh_overlay {
            "Seamless".to_string()
        } else {
            "KMS".to_string()
        };

        let label = if out.current_hz > 0 && pace_hz > 0 && pace_hz != out.current_hz {
            format!("{} • {}Hz -> {}Hz • {}", out.name, out.current_hz, pace_hz, tag)
        } else if pace_hz > 0 {
            format!("{} • {}Hz • {}", out.name, pace_hz, tag)
        } else {
            format!("{} • {}", out.name, tag)
        };

        let ow = (self.painter.measure_text(&label, 11.0) + 28)
            .clamp(124, (output_geo.size.w - 28).max(124));
        let oh = 26;
        let ph = config.panel_height.clamp(34, 56);
        let pad = if config.shelf_style == ShelfStyle::Floating { 16 } else { 10 };
        let toast_offset = if self.toast_buffer.is_some() {
            self.toast_rect.size.h + 8
        } else {
            0
        };
        let ox = output_geo.loc.x + (output_geo.size.w - ow - 14).max(10);
        let oy = output_geo.loc.y
            + match config.panel_position {
                PanelPosition::Top => ph + pad + toast_offset,
                PanelPosition::Bottom => (output_geo.size.h - ph - oh - pad - toast_offset).max(10),
            };
        self.saver_overlay_rect = Rectangle::new((ox, oy).into(), (ow, oh).into());

        let accent = config.accent_rgb();
        let dot_col = if config.battery_saver_modeset && saver_active {
            [245, 188, 66, 255]
        } else if saver_active || config.seamless_refresh_overlay {
            [72, 226, 152, 255]
        } else {
            [accent[0], accent[1], accent[2], 255]
        };
        let sig = format!(
            "{}x{}@{},{}:{}:{:02x}{:02x}{:02x}",
            ow, oh, ox, oy, label, dot_col[0], dot_col[1], dot_col[2]
        );
        if sig != self.saver_overlay_signature || self.saver_overlay_buffer.is_none() {
            self.saver_overlay_signature = sig;
            let needed = (ow * oh * 4) as usize;
            if self.saver_overlay_pixels.len() != needed {
                self.saver_overlay_pixels.resize(needed, 0);
            } else {
                self.saver_overlay_pixels.fill(0);
            }
            let mut buf = std::mem::take(&mut self.saver_overlay_pixels);
            fill_rounded_rect(&mut buf, ow, oh, 0, 0, ow, oh, 10, [12, 17, 30, 198]);
            stroke_rounded_rect(
                &mut buf,
                ow,
                oh,
                0,
                0,
                ow,
                oh,
                10,
                1,
                [accent[0], accent[1], accent[2], 110],
            );
            fill_circle(&mut buf, ow, oh, 11, oh / 2, 3, dot_col);
            self.painter.draw_text(
                &mut buf,
                ow,
                oh,
                19,
                7,
                ow - 24,
                &label,
                11.0,
                [232, 240, 255, 245],
            );
            update_memory_buffer(
                &mut self.saver_overlay_buffer,
                &mut self.saver_overlay_buf_size,
                (ow, oh),
                &buf,
            );
            self.saver_overlay_pixels = buf;
        }
    }

    fn draw_close_btn(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        y: i32,
        hit_id: &mut usize,
    ) {
        let hovered = self.hovered_hit_id == Some(*hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            w - 42,
            y,
            26,
            24,
            6,
            if hovered { [190, 62, 82, 235] } else { [34, 42, 64, 220] },
        );
        self.painter
            .draw_text(buf, w, h, w - 33, y + 5, 20, "X", 12.0, [242, 246, 255, 255]);
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + w - 42, oy + y).into(), (26, 24).into()),
            action: UiAction::ClosePopup,
            right_action: None,
            scroll_action: None,
            id: *hit_id,
        });
        *hit_id += 1;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cc_toggle(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        cx: i32,
        cw: i32,
        y: &mut i32,
        hit_id: &mut usize,
        label: &str,
        enabled: bool,
        accent: [u8; 3],
        action: UiAction,
    ) {
        self.painter
            .draw_text(buf, w, h, cx, *y + 6, cw - 82, label, 12.5, [212, 222, 246, 255]);
        let bw = 66;
        let bx = cx + cw - bw;
        let hovered = self.hovered_hit_id == Some(*hit_id);
        let bg = if enabled {
            [accent[0], accent[1], accent[2], if hovered { 250 } else { 220 }]
        } else if hovered {
            [52, 62, 92, 225]
        } else {
            [32, 40, 62, 215]
        };
        fill_rounded_rect(buf, w, h, bx, *y, bw, 26, 7, bg);
        let txt = if enabled { "ON" } else { "OFF" };
        let tx = bx + (bw - self.painter.measure_text(txt, 12.0)) / 2;
        self.painter
            .draw_text(buf, w, h, tx, *y + 7, bw, txt, 12.0, [245, 248, 255, 255]);
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + bx, oy + *y).into(), (bw, 26).into()),
            action,
            right_action: None,
            scroll_action: None,
            id: *hit_id,
        });
        *hit_id += 1;
        *y += 33;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cc_choice(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        cx: i32,
        cw: i32,
        y: &mut i32,
        hit_id: &mut usize,
        label: &str,
        value: &str,
        accent: [u8; 3],
        action: UiAction,
    ) {
        self.painter
            .draw_text(buf, w, h, cx, *y + 6, cw - 124, label, 12.5, [212, 222, 246, 255]);
        let bw = 112;
        let bx = cx + cw - bw;
        let hovered = self.hovered_hit_id == Some(*hit_id);
        let bg = if hovered {
            [accent[0], accent[1], accent[2], 220]
        } else {
            [32, 42, 68, 220]
        };
        fill_rounded_rect(buf, w, h, bx, *y, bw, 26, 7, bg);
        let tx = bx + (bw - self.painter.measure_text(value, 11.5)) / 2;
        self.painter
            .draw_text(buf, w, h, tx, *y + 7, bw, value, 11.5, [245, 248, 255, 255]);
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + bx, oy + *y).into(), (bw, 26).into()),
            action,
            right_action: None,
            scroll_action: None,
            id: *hit_id,
        });
        *hit_id += 1;
        *y += 33;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cc_stepper(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        cx: i32,
        cw: i32,
        y: &mut i32,
        hit_id: &mut usize,
        label: &str,
        value: &str,
        accent: [u8; 3],
        dec_action: UiAction,
        inc_action: UiAction,
    ) {
        self.painter
            .draw_text(buf, w, h, cx, *y + 6, cw - 140, label, 12.5, [212, 222, 246, 255]);
        let right = cx + cw;
        let inc_x = right - 28;
        let val_x = inc_x - 64;
        let dec_x = val_x - 32;

        let dec_hover = self.hovered_hit_id == Some(*hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            dec_x,
            *y,
            28,
            26,
            6,
            if dec_hover { [accent[0], accent[1], accent[2], 220] } else { [32, 42, 68, 220] },
        );
        self.painter
            .draw_text(buf, w, h, dec_x + 11, *y + 6, 20, "-", 14.0, [245, 248, 255, 255]);
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + dec_x, oy + *y).into(), (28, 26).into()),
            action: dec_action,
            right_action: None,
            scroll_action: None,
            id: *hit_id,
        });
        *hit_id += 1;

        let vtx = val_x + (60 - self.painter.measure_text(value, 12.5)) / 2;
        self.painter
            .draw_text(buf, w, h, vtx, *y + 7, 60, value, 12.5, [238, 244, 255, 255]);

        let inc_hover = self.hovered_hit_id == Some(*hit_id);
        fill_rounded_rect(
            buf,
            w,
            h,
            inc_x,
            *y,
            28,
            26,
            6,
            if inc_hover { [accent[0], accent[1], accent[2], 220] } else { [32, 42, 68, 220] },
        );
        self.painter
            .draw_text(buf, w, h, inc_x + 10, *y + 6, 20, "+", 14.0, [245, 248, 255, 255]);
        self.popup_hits.push(HitRegion {
            rect: Rectangle::new((ox + inc_x, oy + *y).into(), (28, 26).into()),
            action: inc_action,
            right_action: None,
            scroll_action: None,
            id: *hit_id,
        });
        *hit_id += 1;
        *y += 33;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cc_slider(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        cx: i32,
        cw: i32,
        y: &mut i32,
        hit_id: &mut usize,
        label: &str,
        val: i32,
        min: i32,
        max: i32,
        accent: [u8; 3],
        target: SliderTarget,
    ) {
        self.painter
            .draw_text(buf, w, h, cx, *y + 5, 225, label, 12.5, [212, 222, 246, 255]);
        let sw = (cw - 230).max(120);
        self.draw_slider(
            buf,
            w,
            h,
            ox,
            oy,
            cx + 230,
            *y,
            sw,
            val,
            min,
            max,
            accent,
            target,
            hit_id,
        );
        *y += 33;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_start_menu(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
        apps: &[DesktopApp],
    ) {
        let accent = config.accent_rgb();
        let mut hit_id = 1000usize;
        let mut y = 14;

        self.painter
            .draw_text(buf, w, h, 18, y, 220, "Applications", 16.0, [242, 246, 255, 255]);

        let quick_buttons = [
            ("Terminal", UiAction::LaunchQuickTerminal, 76),
            ("Files", UiAction::LaunchQuickFiles, 62),
            ("Browser", UiAction::LaunchQuickBrowser, 74),
            ("Control", UiAction::ToggleSettings, 74),
        ];
        let mut qx = w - 16;
        for (label, action, bw) in quick_buttons.iter().rev() {
            qx -= *bw;
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if hovered {
                [accent[0], accent[1], accent[2], 215]
            } else {
                [28, 36, 58, 225]
            };
            fill_rounded_rect(buf, w, h, qx, y - 3, *bw, 26, 7, bg);
            let tx = qx + (*bw - self.painter.measure_text(label, 12.0)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, y + 3, *bw, label, 12.0, [238, 244, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + qx, oy + y - 3).into(), (*bw, 26).into()),
                action: action.clone(),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            qx -= 6;
        }
        y += 34;

        let search_w = w - 32;
        fill_rounded_rect(buf, w, h, 16, y, search_w, 34, 8, [20, 26, 44, 240]);
        stroke_rounded_rect(
            buf,
            w,
            h,
            16,
            y,
            search_w,
            34,
            8,
            1,
            [accent[0], accent[1], accent[2], 150],
        );
        if self.search_query.is_empty() {
            self.painter.draw_text(
                buf,
                w,
                h,
                28,
                y + 10,
                search_w - 60,
                "Type to search apps...",
                13.5,
                [128, 142, 174, 220],
            );
        } else {
            self.painter.draw_text(
                buf,
                w,
                h,
                28,
                y + 10,
                search_w - 72,
                &self.search_query,
                13.5,
                [245, 248, 255, 255],
            );
            let clr_x = w - 58;
            let hovered = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                clr_x,
                y + 5,
                36,
                24,
                6,
                if hovered { [72, 84, 118, 230] } else { [38, 46, 72, 220] },
            );
            self.painter
                .draw_text(buf, w, h, clr_x + 7, y + 10, 28, "Clr", 11.5, [232, 238, 252, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + clr_x, oy + y + 5).into(), (36, 24).into()),
                action: UiAction::ClearSearch,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
        }
        y += 44;

        let mut custom_cats: Vec<String> = Vec::new();
        for app in apps {
            if !app.hidden {
                if let Some(ref cc) = app.custom_category {
                    let trimmed = cc.trim();
                    if !trimmed.is_empty()
                        && !custom_cats.iter().any(|c| c.eq_ignore_ascii_case(trimmed))
                    {
                        custom_cats.push(trimmed.to_string());
                    }
                }
            }
        }
        custom_cats.sort_by_key(|c| c.to_ascii_lowercase());

        let mut cx = 16;
        let row_limit = w - 16;
        for &cat in AppCategory::TABS {
            let label = cat.label();
            let cw = self.painter.measure_text(label, 11.5) + 18;
            if cx + cw > row_limit && cx > 16 {
                cx = 16;
                y += 30;
            }
            let active = self.selected_custom_category.is_none() && self.selected_category == cat;
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if active {
                [accent[0], accent[1], accent[2], 230]
            } else if hovered {
                [44, 56, 86, 225]
            } else {
                [24, 30, 48, 210]
            };
            fill_rounded_rect(buf, w, h, cx, y, cw, 25, 7, bg);
            self.painter.draw_text(
                buf,
                w,
                h,
                cx + 9,
                y + 6,
                cw - 12,
                label,
                11.5,
                if active { [250, 252, 255, 255] } else { [192, 204, 230, 240] },
            );
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + cx, oy + y).into(), (cw, 25).into()),
                action: UiAction::SelectCategory(cat),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            cx += cw + 6;
        }
        for cc in custom_cats {
            let cw = self.painter.measure_text(&cc, 11.5) + 18;
            if cx + cw > row_limit && cx > 16 {
                cx = 16;
                y += 30;
            }
            let active = self
                .selected_custom_category
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(&cc));
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if active {
                [accent[0], accent[1], accent[2], 230]
            } else if hovered {
                [44, 56, 86, 225]
            } else {
                [30, 38, 60, 215]
            };
            fill_rounded_rect(buf, w, h, cx, y, cw, 25, 7, bg);
            self.painter.draw_text(
                buf,
                w,
                h,
                cx + 9,
                y + 6,
                cw - 12,
                &cc,
                11.5,
                if active { [250, 252, 255, 255] } else { [200, 214, 240, 240] },
            );
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + cx, oy + y).into(), (cw, 25).into()),
                action: UiAction::SelectCustomCategory(cc),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            cx += cw + 6;
        }
        y += 32;

        let filtered = self.filtered_app_indices(apps);
        let in_filtered_category =
            self.selected_category != AppCategory::All || self.selected_custom_category.is_some();
        let all_match_count = if in_filtered_category {
            self.filtered_app_indices_all(apps).len()
        } else {
            filtered.len()
        };

        if in_filtered_category && !self.search_query.trim().is_empty() && all_match_count > 0 {
            let btn_lbl = format!("Search in All Categories ({all_match_count})");
            let bw = (self.painter.measure_text(&btn_lbl, 11.5) + 22).clamp(170, w - 32);
            let hov = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                16,
                y,
                bw,
                25,
                7,
                if hov {
                    [accent[0], accent[1], accent[2], 240]
                } else {
                    [34, 52, 86, 228]
                },
            );
            self.painter
                .draw_text(buf, w, h, 27, y + 6, bw - 16, &btn_lbl, 11.5, [238, 246, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + 16, oy + y).into(), (bw, 25).into()),
                action: UiAction::SearchInAllCategories,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            y += 31;
        } else if let Some((prev_cat, ref prev_custom)) = self.auto_switched_from_category {
            let prev_lbl = prev_custom.as_deref().unwrap_or_else(|| prev_cat.label());
            let info = format!("Switched from '{prev_lbl}' to All Apps");
            self.painter
                .draw_text(buf, w, h, 20, y + 5, w - 180, &info, 11.0, [166, 198, 236, 235]);
            let back_lbl = format!("Back to {prev_lbl}");
            let bw = (self.painter.measure_text(&back_lbl, 11.0) + 18).clamp(88, 160);
            let bx = w - 16 - bw;
            let hov = self.hovered_hit_id == Some(hit_id);
            fill_rounded_rect(
                buf,
                w,
                h,
                bx,
                y,
                bw,
                23,
                6,
                if hov { [54, 72, 112, 235] } else { [28, 38, 62, 225] },
            );
            let btx = bx + (bw - self.painter.measure_text(&back_lbl, 11.0)) / 2;
            self.painter
                .draw_text(buf, w, h, btx, y + 5, bw - 8, &back_lbl, 11.0, [236, 244, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + bx, oy + y).into(), (bw, 23).into()),
                action: UiAction::RestorePreviousCategory,
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            y += 29;
        }

        let footer_h = 46;
        let list_h = (h - y - footer_h - 8).max(48);
        let row_h = 44;
        let visible_rows = (list_h / row_h).max(1) as usize;
        let max_scroll = filtered.len().saturating_sub(visible_rows);
        if self.app_scroll > max_scroll {
            self.app_scroll = max_scroll;
        }

        if filtered.is_empty() {
            self.painter.draw_text(
                buf,
                w,
                h,
                24,
                y + 18,
                w - 48,
                "No matching applications in this category.",
                13.5,
                [148, 160, 190, 220],
            );
            if in_filtered_category && all_match_count > 0 {
                let lbl = format!("Search in All Categories ({all_match_count})");
                let bw = (self.painter.measure_text(&lbl, 12.5) + 28).clamp(180, 320);
                fill_rounded_rect(buf, w, h, 24, y + 48, bw, 32, 8, [accent[0], accent[1], accent[2], 235]);
                let tx = 24 + (bw - self.painter.measure_text(&lbl, 12.5)) / 2;
                self.painter.draw_text(buf, w, h, tx, y + 56, bw - 12, &lbl, 12.5, [246, 250, 255, 255]);
                self.popup_hits.push(HitRegion {
                    rect: Rectangle::new((ox + 24, oy + y + 48).into(), (bw, 32).into()),
                    action: UiAction::SearchInAllCategories,
                    right_action: None,
                    scroll_action: None,
                    id: hit_id,
                });
                hit_id += 1;
            }
        } else {
            for (vis_idx, &app_idx) in filtered
                .iter()
                .skip(self.app_scroll)
                .take(visible_rows)
                .enumerate()
            {
                let app = &apps[app_idx];
                let ry = y + (vis_idx as i32) * row_h;
                let hovered = self.hovered_hit_id == Some(hit_id);
                let kbd_sel = self.app_scroll + vis_idx == self.selected_app_row;

                let row_bg = if hovered {
                    [accent[0], accent[1], accent[2], 95]
                } else if kbd_sel {
                    [32, 42, 68, 215]
                } else {
                    [18, 24, 42, 160]
                };
                fill_rounded_rect(buf, w, h, 16, ry, w - 32, row_h - 4, 9, row_bg);

                let bc = app.badge_color;
                fill_rounded_rect(buf, w, h, 24, ry + 6, 28, 28, 8, [bc[0], bc[1], bc[2], 235]);
                let initial = app.name.chars().next().unwrap_or('A').to_uppercase().to_string();
                let itx = 24 + (28 - self.painter.measure_text(&initial, 14.0)) / 2;
                self.painter
                    .draw_text(buf, w, h, itx, ry + 13, 24, &initial, 14.0, [252, 254, 255, 255]);

                self.painter.draw_text(
                    buf,
                    w,
                    h,
                    62,
                    ry + 6,
                    w - 140,
                    &app.name,
                    13.5,
                    [242, 246, 255, 255],
                );
                let sub = if app.comment.is_empty() {
                    app.effective_category_label()
                } else {
                    &app.comment
                };
                self.painter.draw_text(
                    buf,
                    w,
                    h,
                    62,
                    ry + 23,
                    w - 140,
                    sub,
                    11.0,
                    [146, 160, 192, 230],
                );

                let tag = if app.is_terminal_app() {
                    "Term"
                } else {
                    app.effective_category_label()
                };
                let tw = self.painter.measure_text(tag, 10.5) + 14;
                fill_rounded_rect(buf, w, h, w - 26 - tw, ry + 11, tw, 18, 5, [28, 36, 56, 200]);
                self.painter.draw_text(
                    buf,
                    w,
                    h,
                    w - 19 - tw,
                    ry + 14,
                    tw,
                    tag,
                    10.5,
                    [172, 188, 220, 235],
                );

                self.popup_hits.push(HitRegion {
                    rect: Rectangle::new((ox + 16, oy + ry).into(), (w - 32, row_h - 4).into()),
                    action: UiAction::LaunchApp(app_idx),
                    right_action: Some(UiAction::OpenContextMenu {
                        target: ContextTarget::LibraryApp(app_idx),
                        pos: (0, 0),
                    }),
                    scroll_action: None,
                    id: hit_id,
                });
                hit_id += 1;
            }
        }

        let fy = h - footer_h;
        fill_rounded_rect(buf, w, h, 16, fy, w - 32, 1, 0, [62, 80, 120, 110]);
        let count_text = format!("{} apps", filtered.len());
        self.painter
            .draw_text(buf, w, h, 20, fy + 15, 110, &count_text, 12.0, [152, 166, 196, 230]);

        let footer_btns: [(&str, UiAction, i32, [u8; 4]); 4] = [
            ("Rescan", UiAction::RescanApps, 66, [30, 38, 60, 220]),
            ("Exit Login", UiAction::QuitSession, 84, [42, 48, 72, 225]),
            ("Reboot", UiAction::SystemReboot, 66, [66, 42, 58, 225]),
            ("Power Off", UiAction::SystemPoweroff, 80, [145, 44, 62, 230]),
        ];
        let mut fx = w - 16;
        for (label, act, bw, base_col) in footer_btns.iter().rev() {
            fx -= *bw;
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if hovered {
                [
                    base_col[0].saturating_add(32),
                    base_col[1].saturating_add(32),
                    base_col[2].saturating_add(32),
                    245,
                ]
            } else {
                *base_col
            };
            fill_rounded_rect(buf, w, h, fx, fy + 10, *bw, 26, 7, bg);
            let tx = fx + (*bw - self.painter.measure_text(label, 11.5)) / 2;
            self.painter
                .draw_text(buf, w, h, tx, fy + 17, *bw, label, 11.5, [242, 245, 255, 255]);
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + fx, oy + fy + 10).into(), (*bw, 26).into()),
                action: act.clone(),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            hit_id += 1;
            fx -= 6;
        }
    }

    fn draw_power_menu(
        &mut self,
        buf: &mut [u8],
        w: i32,
        h: i32,
        ox: i32,
        oy: i32,
        config: &Config,
    ) {
        let accent = config.accent_rgb();
        let mut y = 14;
        let items = [
            ("Lock Screen (Win+L)", UiAction::LockScreen, [40, 60, 100, 230]),
            ("Exit to LightDM", UiAction::QuitSession, [52, 64, 96, 220]),
            ("Reload Config & Apps", UiAction::ReloadConfig, [30, 40, 66, 220]),
            ("Suspend", UiAction::SystemSuspend, [30, 40, 66, 220]),
            ("Reboot", UiAction::SystemReboot, [84, 48, 66, 220]),
            ("Shut Down", UiAction::SystemPoweroff, [156, 46, 66, 230]),
        ];

        for (idx, (label, act, base_bg)) in items.iter().enumerate() {
            let hit_id = 3000 + idx;
            let hovered = self.hovered_hit_id == Some(hit_id);
            let bg = if hovered {
                [accent[0], accent[1], accent[2], 225]
            } else {
                *base_bg
            };
            fill_rounded_rect(buf, w, h, 14, y, w - 28, 34, 8, bg);
            self.painter.draw_text(
                buf,
                w,
                h,
                26,
                y + 10,
                w - 48,
                label,
                13.0,
                [244, 247, 255, 255],
            );
            self.popup_hits.push(HitRegion {
                rect: Rectangle::new((ox + 14, oy + y).into(), (w - 28, 34).into()),
                action: act.clone(),
                right_action: None,
                scroll_action: None,
                id: hit_id,
            });
            y += 40;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_lock_screen(
        &mut self,
        renderer: &mut GlesRenderer,
        out_geo: Rectangle<i32, Logical>,
        is_primary_ui: bool,
        scale: f64,
        config: &Config,
        password_input: &str,
        error_msg: &str,
        verifying: bool,
    ) -> Vec<MemoryRenderBufferRenderElement<GlesRenderer>> {
        let mut out = Vec::new();
        if !is_primary_ui || out_geo.size.w <= 0 || out_geo.size.h <= 0 {
            return out;
        }
        let now_sec = unix_secs_now();
        if now_sec != self.cached_clock_sec {
            self.cached_clock_sec = now_sec;
            self.cached_clock = current_local_clock(now_sec);
        }
        let clock_str = self.cached_clock.0.clone();
        let date_str = self.cached_clock.1.clone();
        let user = std::env::var("USER").unwrap_or_else(|_| "User".to_string());

        let cw = 396.min(out_geo.size.w - 32).max(280);
        let ch = 264;
        let accent = config.accent_rgb();
        let sig = format!(
            "{}x{}:{}:{}:{}:{}:{}:{}",
            cw,
            ch,
            clock_str,
            date_str,
            password_input.chars().count(),
            error_msg,
            verifying,
            config.accent_theme
        );
        if sig != self.lock_signature || self.lock_buffer.is_none() {
            self.lock_signature = sig;
            let needed = (cw * ch * 4) as usize;
            if self.lock_pixels.len() != needed {
                self.lock_pixels.resize(needed, 0);
            } else {
                self.lock_pixels.fill(0);
            }
            let mut buf = std::mem::take(&mut self.lock_pixels);
            fill_rounded_rect(&mut buf, cw, ch, 0, 0, cw, ch, 16, [12, 17, 32, 245]);
            stroke_rounded_rect(&mut buf, cw, ch, 0, 0, cw, ch, 16, 1, [accent[0], accent[1], accent[2], 190]);

            let clock_w = self.painter.measure_text(&clock_str, 40.0);
            self.painter.draw_text(
                &mut buf,
                cw,
                ch,
                ((cw - clock_w) / 2).max(16),
                20,
                cw - 32,
                &clock_str,
                40.0,
                [244, 248, 255, 255],
            );
            let date_w = self.painter.measure_text(&date_str, 13.0);
            self.painter.draw_text(
                &mut buf,
                cw,
                ch,
                ((cw - date_w) / 2).max(16),
                72,
                cw - 32,
                &date_str,
                13.0,
                [164, 184, 222, 240],
            );

            let user_lbl = format!("🔒 Locked • {user}");
            let uw = self.painter.measure_text(&user_lbl, 13.5);
            self.painter.draw_text(
                &mut buf,
                cw,
                ch,
                ((cw - uw) / 2).max(16),
                110,
                cw - 32,
                &user_lbl,
                13.5,
                [accent[0], accent[1], accent[2], 255],
            );

            // Password input box
            fill_rounded_rect(&mut buf, cw, ch, 32, 144, cw - 64, 38, 10, [18, 26, 46, 245]);
            stroke_rounded_rect(
                &mut buf,
                cw,
                ch,
                32,
                144,
                cw - 64,
                38,
                10,
                1,
                if !error_msg.is_empty() {
                    [236, 86, 106, 220]
                } else {
                    [accent[0], accent[1], accent[2], 175]
                },
            );
            let mask_len = password_input.chars().count();
            let pwd_disp = if mask_len == 0 {
                "Enter password to unlock".to_string()
            } else {
                "•".repeat(mask_len.min(28))
            };
            let pwd_col = if mask_len == 0 {
                [132, 148, 182, 225]
            } else {
                [244, 248, 255, 255]
            };
            let pw_w = self.painter.measure_text(&pwd_disp, 14.0);
            self.painter.draw_text(
                &mut buf,
                cw,
                ch,
                ((cw - pw_w) / 2).max(42),
                156,
                cw - 84,
                &pwd_disp,
                14.0,
                pwd_col,
            );

            let hint = if verifying {
                "Checking password..."
            } else if !error_msg.is_empty() {
                error_msg
            } else {
                "Press Enter to unlock • Esc clears input"
            };
            let hint_col = if !error_msg.is_empty() {
                [246, 118, 134, 255]
            } else {
                [148, 168, 206, 230]
            };
            let hw = self.painter.measure_text(hint, 11.5);
            self.painter.draw_text(
                &mut buf,
                cw,
                ch,
                ((cw - hw) / 2).max(20),
                204,
                cw - 40,
                hint,
                11.5,
                hint_col,
            );

            update_memory_buffer(&mut self.lock_buffer, &mut self.lock_buf_size, (cw, ch), &buf);
            self.lock_pixels = buf;
        }

        if let Some(ref buf) = self.lock_buffer {
            let lx = ((out_geo.size.w - cw) / 2).max(0);
            let ly = ((out_geo.size.h - ch) / 2).max(0);
            let pos = Point::<f64, Logical>::from((lx as f64, ly as f64))
                .to_physical_precise_round::<f64, i32>(scale);
            if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                (pos.x as f64, pos.y as f64),
                buf,
                None,
                None,
                None,
                Kind::Unspecified,
            ) {
                out.push(elem);
            }
        }
        out
    }

    pub fn render_minimal_card_pixels(
        &mut self,
        w: i32,
        h: i32,
        title: &str,
        is_terminal: bool,
        focused: bool,
        config: &Config,
    ) -> Vec<u8> {
        let rw = w.max(48);
        let rh = h.max(36);
        let mut buf = vec![0u8; (rw * rh * 4) as usize];
        let accent = config.accent_rgb();
        let rad = config.corner_radius.clamp(6, 14).min(rw / 4).min(rh / 4);
        let bg = if focused {
            [18, 25, 44, 244]
        } else {
            [13, 18, 32, 236]
        };
        let border = if focused {
            [accent[0], accent[1], accent[2], 235]
        } else {
            [74, 96, 138, 170]
        };
        fill_rounded_rect(&mut buf, rw, rh, 0, 0, rw, rh, rad, bg);
        stroke_rounded_rect(&mut buf, rw, rh, 0, 0, rw, rh, rad, 1, border);

        let badge_sz = (rh - 16).clamp(18, 30);
        let bx = 10;
        let by = if rh >= 96 { 12 } else { (rh - badge_sz) / 2 };
        let badge_col = if focused {
            [accent[0], accent[1], accent[2], 235]
        } else if is_terminal {
            [32, 96, 114, 230]
        } else {
            [44, 58, 92, 230]
        };
        fill_rounded_rect(&mut buf, rw, rh, bx, by, badge_sz, badge_sz, 7, badge_col);
        let badge_txt = if is_terminal {
            ">_".to_string()
        } else {
            title
                .chars()
                .next()
                .unwrap_or('W')
                .to_uppercase()
                .to_string()
        };
        let bfont = if is_terminal { 10.5 } else { 12.0 };
        let btx = bx + ((badge_sz - self.painter.measure_text(&badge_txt, bfont)) / 2).max(1);
        self.painter.draw_text(
            &mut buf,
            rw,
            rh,
            btx,
            by + ((badge_sz - 12) / 2).max(2),
            badge_sz,
            &badge_txt,
            bfont,
            [248, 251, 255, 255],
        );

        let tx = bx + badge_sz + 8;
        let max_tw = (rw - tx - 8).max(16);
        if rh < 96 {
            self.painter.draw_text(
                &mut buf,
                rw,
                rh,
                tx,
                (rh - 12) / 2,
                max_tw,
                title,
                11.5,
                [238, 244, 255, 255],
            );
        } else {
            self.painter.draw_text(
                &mut buf,
                rw,
                rh,
                tx,
                by + 2,
                max_tw,
                title,
                12.5,
                [242, 247, 255, 255],
            );
            let meta_lbl = format!(
                "Minimal View  {}x{}",
                rw, rh
            );
            self.painter.draw_text(
                &mut buf,
                rw,
                rh,
                tx,
                by + 20,
                max_tw,
                &meta_lbl,
                10.5,
                [150, 174, 212, 225],
            );
            if rh >= 82 {
                self.painter.draw_text(
                    &mut buf,
                    rw,
                    rh,
                    10,
                    rh - 22,
                    rw - 20,
                    "Double-click or right-click to expand",
                    10.5,
                    [168, 188, 222, 220],
                );
            }
        }
        buf
    }
}

fn update_memory_buffer(
    slot: &mut Option<MemoryRenderBuffer>,
    cached_size: &mut (i32, i32),
    size: (i32, i32),
    pixels: &[u8],
) {
    if size.0 <= 0 || size.1 <= 0 {
        return;
    }
    if *cached_size != size || slot.is_none() {
        *slot = Some(MemoryRenderBuffer::new(
            DrmFourcc::Abgr8888,
            size,
            1,
            Transform::Normal,
            None,
        ));
        *cached_size = size;
    }
    if let Some(buf) = slot {
        let mut ctx = buf.render();
        let _ = ctx.draw(|dst| {
            let len = dst.len().min(pixels.len());
            dst[..len].copy_from_slice(&pixels[..len]);
            Ok::<_, ()>(vec![Rectangle::from_size(size.into())])
        });
    }
}

fn read_system_uptime() -> String {
    let Ok(raw) = std::fs::read_to_string("/proc/uptime") else {
        return "0m".to_string();
    };
    let secs = raw
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0) as u64;
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

fn unix_secs_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn current_local_clock(secs: u64) -> (String, String, String) {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&t, &mut tm);
    }

    let weekdays = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let wd = weekdays.get(tm.tm_wday as usize).copied().unwrap_or("Day");
    let mo = months.get(tm.tm_mon as usize).copied().unwrap_or("Mon");

    let clock = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    let clock_full = format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec);
    let date = format!("{wd} {:02} {mo}", tm.tm_mday);
    (clock, date, clock_full)
}

fn current_local_ymd(secs: u64) -> (i32, u32, u32) {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&t, &mut tm);
    }
    (tm.tm_year + 1900, (tm.tm_mon + 1).max(1) as u32, tm.tm_mday.max(1) as u32)
}

pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap_year(year) { 29 } else { 28 },
        _ => 30,
    }
}

fn add_months(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let total = year * 12 + (month as i32 - 1) + delta;
    let ny = total.div_euclid(12);
    let nm = (total.rem_euclid(12) + 1) as u32;
    (ny, nm)
}

/// Howard Hinnant civil date -> Monday=0..Sunday=6 day-of-week.
pub fn weekday_mon0(year: i32, month: u32, day: u32) -> u32 {
    let m = month as i32;
    let y = year - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    // 1970-01-01 was Thursday (index 3 in Mon=0..Sun=6)
    (days + 3).rem_euclid(7) as u32
}

fn month_name(month: u32) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "Month",
    }
}

fn format_full_date(year: i32, month: u32, day: u32) -> String {
    let wd_names = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    let wd = wd_names
        .get(weekday_mon0(year, month, day) as usize)
        .copied()
        .unwrap_or("Day");
    format!("{wd}, {} {day:02}, {year}", month_name(month))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::DesktopApp;

    fn sample_apps() -> Vec<DesktopApp> {
        vec![
            DesktopApp {
                id: "alacritty.desktop".into(),
                name: "Alacritty".into(),
                comment: "Terminal emulator".into(),
                exec_args: vec!["alacritty".into()],
                terminal: false,
                category: AppCategory::System,
                badge_color: [66, 135, 245],
                ..DesktopApp::default()
            },
            DesktopApp {
                id: "firefox.desktop".into(),
                name: "Firefox".into(),
                comment: "Web Browser".into(),
                exec_args: vec!["firefox".into()],
                terminal: false,
                category: AppCategory::Internet,
                badge_color: [240, 120, 60],
                ..DesktopApp::default()
            },
        ]
    }

    #[test]
    fn filters_and_scrolls_start_menu_apps() {
        let mut ui = ShellUi::new();
        let apps = sample_apps();
        assert_eq!(ui.filtered_app_indices(&apps), vec![0, 1]);

        ui.selected_category = AppCategory::Internet;
        assert_eq!(ui.filtered_app_indices(&apps), vec![1]);

        // Smart switch moves to All automatically when typing a query absent in current category
        ui.search_query = "term".into();
        assert!(ui.apply_search_smart_switch(&apps));
        assert_eq!(ui.selected_category, AppCategory::All);
        assert_eq!(ui.filtered_app_indices(&apps), vec![0]);
    }

    #[test]
    fn chromeos_shelf_active_apps_calendar_and_alt_tab_work() {
        let mut ui = ShellUi::new();
        let mut cfg = Config::default();
        let apps = sample_apps();
        let sys = SystemSnapshot::default();
        let ws_counts = [1, 0, 0, 0, 0, 0, 0, 0, 0];
        let tasks = vec![TaskWindowInfo {
            title: "Alacritty".into(),
            focused: true,
            floating: false,
            always_on_top: true,
            minimized: false,
            maximized: false,
            is_terminal: true,
            minimal_view: false,
        }];
        let globals = vec![GlobalWindowInfo {
            workspace: 0,
            index_in_ws: 0,
            title: "Alacritty".into(),
            focused: true,
            floating: false,
            always_on_top: true,
            minimized: false,
            maximized: false,
            is_terminal: true,
            minimal_view: false,
        }];

        let notifs = NotificationSnapshot::default();
        let updates = UpdateStatus::default();
        let primary_geo = Rectangle::new((0, 0).into(), (1920, 1080).into());
        let secondary_geo = Rectangle::new((1920, 0).into(), (2560, 1440).into());
        ui.ensure_panel(
            primary_geo,
            &cfg,
            0,
            &ws_counts,
            &tasks,
            globals.len(),
            &sys,
            &notifs,
            &updates,
            "14:30",
            "Thu 26 Nov",
        );

        let notif_hit = ui
            .panel_hits
            .iter()
            .find(|h| h.action == UiAction::ToggleNotifications)
            .expect("notifications pill hit");
        assert_eq!(
            ui.click_at(
                ((notif_hit.rect.loc.x + 10) as f64, (notif_hit.rect.loc.y + 10) as f64).into(),
                false
            ),
            Some(UiAction::ToggleNotifications)
        );
        ui.toggle_notifications();
        ui.ensure_popup(primary_geo, primary_geo, &cfg, &ws_counts, &tasks, &globals, &apps, &sys, &notifs, &updates, "14:30:00");
        assert!((ui.popup_rect.loc.x + ui.popup_rect.size.w / 2 - ui.anchor_notif).abs() <= 140);
        ui.close_popup();

        let active_hit = ui
            .panel_hits
            .iter()
            .find(|h| h.action == UiAction::ToggleActiveApps)
            .expect("active apps pill hit");
        assert_eq!(
            ui.click_at(
                ((active_hit.rect.loc.x + 10) as f64, (active_hit.rect.loc.y + 10) as f64).into(),
                false
            ),
            Some(UiAction::ToggleActiveApps)
        );

        let cal_hit = ui
            .panel_hits
            .iter()
            .find(|h| h.action == UiAction::ToggleCalendar)
            .expect("calendar pill hit");
        assert_eq!(
            ui.click_at(
                ((cal_hit.rect.loc.x + 10) as f64, (cal_hit.rect.loc.y + 10) as f64).into(),
                false
            ),
            Some(UiAction::ToggleCalendar)
        );

        // Verify pill anchoring: Network pill popup opens near the Network pill, not screen center!
        ui.toggle_network_popup();
        ui.ensure_popup(primary_geo, primary_geo, &cfg, &ws_counts, &tasks, &globals, &apps, &sys, &notifs, &updates, "14:30:00");
        assert!((ui.popup_rect.loc.x + ui.popup_rect.size.w / 2 - ui.anchor_net).abs() <= 140);

        ui.step_alt_tab(2, true);
        assert!(ui.is_alt_tab_open());
        assert_eq!(ui.alt_tab_index, 1);
        ui.step_alt_tab(2, true);
        assert_eq!(ui.alt_tab_index, 0);

        ui.toggle_start_menu();
        ui.ensure_popup(primary_geo, primary_geo, &cfg, &ws_counts, &tasks, &globals, &apps, &sys, &notifs, &updates, "14:30:00");
        let app_hit = ui
            .popup_hits
            .iter()
            .find(|h| matches!(h.action, UiAction::LaunchApp(0)))
            .expect("first app hit");
        assert_eq!(
            ui.click_at(
                ((app_hit.rect.loc.x + 10) as f64, (app_hit.rect.loc.y + 10) as f64).into(),
                false
            ),
            Some(UiAction::LaunchApp(0))
        );

        // Calendar math: 2024 leap year Feb = 29, 2026-11-26 = Thursday (Mon=0 -> 3)
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(weekday_mon0(2026, 11, 26), 3);

        // Display confirmation: immediate zero-blink apply & clean cancel
        ui.prompt_display_confirm(60, 0, VrrMode::Off);
        assert_eq!(ui.active_popup, ActivePopup::DisplayConfirm);
        assert_eq!(ui.confirm_and_apply_display_immediately(), Some(60));
        assert_eq!(ui.active_popup, ActivePopup::None);
        ui.trim_caches();

        ui.prompt_display_confirm(60, 0, VrrMode::Auto);
        assert_eq!(ui.active_popup, ActivePopup::DisplayConfirm);
        assert_eq!(ui.revert_display_confirm(), None);
        assert_eq!(ui.active_popup, ActivePopup::None);

        // Reload confirmation modal warns before snapshot reload and provides Confirm/Cancel hits.
        ui.prompt_reload_confirm("Update installed");
        assert!(matches!(ui.active_popup, ActivePopup::ReloadConfirm { .. }));
        ui.ensure_popup(primary_geo, primary_geo, &cfg, &ws_counts, &tasks, &globals, &apps, &sys, &notifs, &updates, "14:30:00");
        let confirm_hit = ui
            .popup_hits
            .iter()
            .find(|h| h.action == UiAction::SeamlessReloadWm)
            .expect("reload confirm button hit");
        assert_eq!(
            ui.click_at(
                ((confirm_hit.rect.loc.x + 10) as f64, (confirm_hit.rect.loc.y + 10) as f64).into(),
                false
            ),
            Some(UiAction::SeamlessReloadWm)
        );
        let cancel_hit = ui
            .popup_hits
            .iter()
            .find(|h| h.action == UiAction::CancelReloadConfirm)
            .expect("reload cancel button hit");
        assert_eq!(
            ui.click_at(
                ((cancel_hit.rect.loc.x + 10) as f64, (cancel_hit.rect.loc.y + 10) as f64).into(),
                false
            ),
            Some(UiAction::CancelReloadConfirm)
        );
        ui.close_popup();

        // When shelf_visible is false (fullscreen taskbar locked), panel pointer hits are disabled.
        ui.shelf_visible = false;
        let panel_pt: Point<f64, Logical> = (
            (ui.panel_rect.loc.x + 20) as f64,
            (ui.panel_rect.loc.y + 12) as f64,
        )
            .into();
        assert!(!ui.captures_pointer(panel_pt));
        assert_eq!(ui.click_at(panel_pt, false), None);

        // Taskbar right-click menu exposes Pin on Top and Open Another Window alongside Close Window.
        ui.shelf_visible = true;
        ui.open_context_menu(ContextTarget::TaskbarWindow(0), (200, 1020));
        ui.ensure_popup(primary_geo, primary_geo, &cfg, &ws_counts, &tasks, &globals, &apps, &sys, &notifs, &updates, "14:30:00");
        assert!(ui.popup_hits.iter().any(|h| h.action == UiAction::ToggleAlwaysOnTopWindow(0)));
        assert!(ui.popup_hits.iter().any(|h| h.action == UiAction::OpenNewInstanceWindow(0)));
        assert!(ui.popup_hits.iter().any(|h| h.action == UiAction::CloseWindow(0)));

        // Built-in terminal right-click context menu exposes Copy, Paste, and Open Another Window.
        ui.open_context_menu(ContextTarget::Terminal, (320, 260));
        ui.ensure_popup(primary_geo, primary_geo, &cfg, &ws_counts, &tasks, &globals, &apps, &sys, &notifs, &updates, "14:30:00");
        assert!(ui.popup_hits.iter().any(|h| h.action == UiAction::TerminalCopyPrimary));
        assert!(ui.popup_hits.iter().any(|h| h.action == UiAction::TerminalPasteClipboard));
        assert!(ui.popup_hits.iter().any(|h| h.action == UiAction::OpenNewTerminalForFocused));

        // Multi-monitor context menu stays on the second monitor instead of clamping onto primary monitor edge.
        ui.open_context_menu(ContextTarget::Desktop, (2800, 600));
        ui.ensure_popup(primary_geo, secondary_geo, &cfg, &ws_counts, &tasks, &globals, &apps, &sys, &notifs, &updates, "14:30:00");
        assert_eq!(ui.popup_rect.loc, (2800, 600).into());
        assert!(secondary_geo.overlaps(ui.popup_rect));
        assert!(!primary_geo.overlaps(ui.popup_rect));
        assert!(ui.captures_pointer((2820.0, 620.0).into()));
        assert_eq!(ui.click_at((2820.0, 620.0).into(), false), Some(UiAction::LaunchQuickTerminal));
        ui.open_context_menu(ContextTarget::Desktop, (4460, 1430));
        ui.ensure_popup(primary_geo, secondary_geo, &cfg, &ws_counts, &tasks, &globals, &apps, &sys, &notifs, &updates, "14:30:00");
        assert!(ui.popup_rect.loc.x + ui.popup_rect.size.w <= secondary_geo.loc.x + secondary_geo.size.w - 10);
        assert!(ui.popup_rect.loc.y + ui.popup_rect.size.h <= secondary_geo.loc.y + secondary_geo.size.h - 16);
        ui.close_popup();

        // Zero-black-screen seamless refresh overlay renders pass-through HUD when enabled.
        ui.display_telemetry.outputs.push(crate::config::OutputTelemetry {
            name: "DP-1".to_string(),
            width: 1920,
            height: 1080,
            current_hz: 144,
            preferred_hz: 144,
            available_hz: vec![144, 120, 60],
            resolution_modes: Vec::new(),
            scale_x100: 100,
            primary: true,
            vrr_capable: true,
            vrr_requires_modeset: false,
            vrr_enabled: false,
            battery_saver_state: "Idle 24Hz".to_string(),
            active_pacing_hz: 24,
        });
        cfg.show_refresh_overlay = true;
        ui.ensure_saver_overlay(primary_geo, &cfg, true);
        assert!(ui.saver_overlay_buffer.is_some());
        assert!(!ui.captures_pointer((ui.saver_overlay_rect.loc.x as f64 + 5.0, ui.saver_overlay_rect.loc.y as f64 + 5.0).into()));
        cfg.show_refresh_overlay = false;
        ui.ensure_saver_overlay(primary_geo, &cfg, true);
        assert!(ui.saver_overlay_buffer.is_none());
    }

    #[test]
    fn start_menu_right_click_opens_library_context_without_launching() {
        let mut ui = ShellUi::new();
        let cfg = Config::default();
        let apps = sample_apps();
        let sys = SystemSnapshot::default();
        let ws_counts = [0usize; 9];
        let tasks = Vec::new();
        let globals = Vec::new();
        let notifs = NotificationSnapshot::default();
        let updates = UpdateStatus::default();
        let primary_geo = Rectangle::new((0, 0).into(), (1920, 1080).into());

        ui.ensure_panel(primary_geo, &cfg, 0, &ws_counts, &tasks, 0, &sys, &notifs, &updates, "14:30", "Thu 26 Nov");
        ui.open_popup(ActivePopup::StartMenu);
        ui.ensure_popup(primary_geo, primary_geo, &cfg, &ws_counts, &tasks, &globals, &apps, &sys, &notifs, &updates, "14:30:00");

        let app_hit = ui
            .popup_hits
            .iter()
            .find(|h| h.action == UiAction::LaunchApp(0))
            .expect("start menu app hit");
        let click_pt: Point<f64, Logical> =
            ((app_hit.rect.loc.x + 24) as f64, (app_hit.rect.loc.y + 16) as f64).into();

        // Left-click launches the app.
        assert_eq!(ui.click_at(click_pt, false), Some(UiAction::LaunchApp(0)));
        // Right-click opens LibraryApp context menu at cursor coordinates instead of launching.
        assert_eq!(
            ui.click_at(click_pt, true),
            Some(UiAction::OpenContextMenu {
                target: ContextTarget::LibraryApp(0),
                pos: (click_pt.x.round() as i32, click_pt.y.round() as i32),
            })
        );
        // Right-clicking a button without right_action returns None instead of running left action.
        let rescan_hit = ui
            .popup_hits
            .iter()
            .find(|h| h.action == UiAction::RescanApps)
            .expect("rescan button hit");
        let rescan_pt: Point<f64, Logical> =
            ((rescan_hit.rect.loc.x + 8) as f64, (rescan_hit.rect.loc.y + 8) as f64).into();
        assert_eq!(ui.click_at(rescan_pt, true), None);

        ui.open_context_menu(ContextTarget::LibraryApp(0), (400, 420));
        ui.ensure_popup(primary_geo, primary_geo, &cfg, &ws_counts, &tasks, &globals, &apps, &sys, &notifs, &updates, "14:30:00");
        assert!(ui.popup_hits.iter().any(|h| h.action == UiAction::EditLibraryAppInSettings(0)));
        assert!(ui.popup_hits.iter().any(|h| h.action == UiAction::ToggleLibraryAppTerminal(0)));
    }

    #[test]
    fn customizable_taskbar_quick_tiles_reorder_and_toggle_anchors() {
        let mut ui = ShellUi::new();
        let mut cfg = Config::default();
        let apps = sample_apps();
        let sys = SystemSnapshot::default();
        let ws_counts = [0usize; 9];
        let tasks = Vec::new();
        let globals = Vec::new();
        let notifs = NotificationSnapshot::default();
        let updates = UpdateStatus::default();
        let primary_geo = Rectangle::new((0, 0).into(), (1920, 1080).into());

        ui.ensure_panel(primary_geo, &cfg, 0, &ws_counts, &tasks, 0, &sys, &notifs, &updates, "14:30", "Thu 26 Nov");
        let orig_vol = ui.anchor_vol;
        let orig_notif = ui.anchor_notif;
        assert!(orig_vol < orig_notif);

        // Move Volume rightward past Notif and hide Brightness pill.
        for _ in 0..5 {
            assert!(cfg.move_taskbar_tile("vol", 1));
        }
        assert!(cfg.toggle_taskbar_tile_visible("bright"));
        ui.ensure_panel(primary_geo, &cfg, 0, &ws_counts, &tasks, 0, &sys, &notifs, &updates, "14:30", "Thu 26 Nov");
        assert!(ui.anchor_vol > ui.anchor_notif);
        assert!(!ui.panel_hits.iter().any(|h| h.action == UiAction::ToggleBrightnessPopup));

        ui.open_popup(ActivePopup::QuickSettings);
        ui.customize_quick_tiles = true;
        ui.ensure_popup(primary_geo, primary_geo, &cfg, &ws_counts, &tasks, &globals, &apps, &sys, &notifs, &updates, "14:30:00");
        assert!(ui.popup_hits.iter().any(|h| matches!(h.action, UiAction::MoveTaskbarQuickTile(_, -1))));
        assert!(ui.popup_hits.iter().any(|h| h.action == UiAction::ResetTaskbarQuickTiles));
    }
}
