use std::{
    borrow::Cow,
    cell::RefCell,
    os::unix::io::{FromRawFd, OwnedFd},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::{dmabuf::Dmabuf, Buffer, Format, Modifier},
        input::{ButtonState, KeyState},
        renderer::{
            element::{memory::MemoryRenderBuffer, solid::SolidColorBuffer},
            Color32F,
        },
    },
    delegate_dmabuf, delegate_drm_syncobj, delegate_fractional_scale, delegate_pointer_constraints,
    delegate_relative_pointer, delegate_viewporter,
    desktop::{layer_map_for_output, PopupManager, Space, Window, WindowSurfaceType},
    input::{
        keyboard::{KeyboardHandle, KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, CursorImageStatus, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent,
            PointerHandle, PointerTarget, RelativeMotionEvent,
        },
        Seat, SeatState,
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason, ObjectId},
            protocol::wl_surface::WlSurface,
            Client, DisplayHandle, Resource,
        },
    },
    utils::{IsAlive, Logical, Point, Rectangle, Serial, Size, SERIAL_COUNTER},
    wayland::{
        compositor::{get_parent, with_states, CompositorClientState, CompositorState},
        dmabuf::{DmabufFeedback, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        drm_syncobj::{DrmSyncPointSource, DrmSyncobjHandler, DrmSyncobjState},
        fractional_scale::{
            with_fractional_scale, FractionalScaleHandler, FractionalScaleManagerState,
        },
        output::OutputManagerState,
        pointer_constraints::{
            with_pointer_constraint, PointerConstraint, PointerConstraintsHandler,
            PointerConstraintsState,
        },
        relative_pointer::RelativePointerManagerState,
        seat::WaylandFocus,
        selection::{
            data_device::{
                current_data_device_selection_userdata, request_data_device_client_selection,
                set_data_device_selection, DataDeviceState,
                SelectionRequestError as DataRequestError,
            },
            ext_data_control::DataControlState as ExtDataControlState,
            primary_selection::{
                current_primary_selection_userdata, request_primary_client_selection,
                set_primary_selection, PrimarySelectionState,
                SelectionRequestError as PrimaryRequestError,
            },
            wlr_data_control::DataControlState as WlrDataControlState,
            SelectionTarget,
        },
        shell::{
            wlr_layer::WlrLayerShellState,
            xdg::{SurfaceCachedState, XdgShellState, XdgToplevelSurfaceData},
        },
        shm::ShmState,
        viewporter::ViewporterState,
        xwayland_shell::XWaylandShellState,
    },
    xwayland::{X11Surface, X11Wm},
};

use crate::{
    animation::WindowAnimator,
    apps::{
        executable_exists, is_terminal_emulator_id, scan_desktop_apps, AppCategory,
        AppLibraryStore, AppOverrideEntry, DesktopApp,
    },
    config::{
        Config, DisplayTelemetry, LayoutMode, PanelPosition, ShelfStyle, WALLPAPER_PRESETS,
    },
    grabs::{rect_for_snap_zone, SnapZone},
    handlers::input::spawn_detached,
    mem_cache::trim_process_heaps,
    notifications::{
        copy_text_to_clipboard, normalize_app_key, NotificationService, NotificationSnapshot,
        NotificationUrgency, SuggestedActionKind,
    },
    shell_ui::{
        ActivePopup, GlobalWindowInfo, ShellUi, SliderTarget, TaskWindowInfo, UiAction,
    },
    system_ctrl::{collect_process_tree, SysCommand, SystemController, SystemSnapshot},
    updates::{
        proc_ancestors_include, read_proc_cmdline, save_reload_snapshot, send_settings_cmd,
        take_external_cmd, take_fresh_reload_snapshot, ReloadSessionSnapshot, ReloadWindowEntry,
        UpdateStatus, UpdateWatcher,
    },
};

pub const NUM_WORKSPACES: usize = 9;

/// Seat focus target routing Wayland surfaces directly and XWayland surfaces through `X11Surface`
/// so X11 games/apps receive `XSetInputFocus`, `WM_TAKE_FOCUS`, and keyboard events cleanly.
#[derive(Debug, Clone, PartialEq)]
pub enum FocusTarget {
    Wayland(WlSurface),
    X11(X11Surface),
}

impl From<WlSurface> for FocusTarget {
    fn from(surface: WlSurface) -> Self {
        Self::Wayland(surface)
    }
}

impl From<smithay::desktop::PopupKind> for FocusTarget {
    fn from(popup: smithay::desktop::PopupKind) -> Self {
        Self::Wayland(popup.wl_surface().clone())
    }
}

impl IsAlive for FocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::Wayland(s) => s.alive(),
            Self::X11(x) => x.alive(),
        }
    }
}

impl WaylandFocus for FocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Wayland(s) => Some(Cow::Borrowed(s)),
            Self::X11(x) => x.wl_surface().map(Cow::Owned),
        }
    }

    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            Self::Wayland(s) => s.id().same_client_as(object_id),
            Self::X11(x) => x.wl_surface().is_some_and(|s| s.id().same_client_as(object_id)),
        }
    }
}

impl KeyboardTarget<VeloxState> for FocusTarget {
    fn enter(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        match self {
            Self::Wayland(s) => KeyboardTarget::enter(s, seat, data, keys, serial),
            Self::X11(x) => KeyboardTarget::enter(x, seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, serial: Serial) {
        match self {
            Self::Wayland(s) => KeyboardTarget::leave(s, seat, data, serial),
            Self::X11(x) => KeyboardTarget::leave(x, seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<VeloxState>,
        data: &mut VeloxState,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match self {
            Self::Wayland(s) => KeyboardTarget::key(s, seat, data, key, state, serial, time),
            Self::X11(x) => KeyboardTarget::key(x, seat, data, key, state, serial, time),
        }
    }

    fn modifiers(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, modifiers: ModifiersState, serial: Serial) {
        match self {
            Self::Wayland(s) => KeyboardTarget::modifiers(s, seat, data, modifiers, serial),
            Self::X11(x) => KeyboardTarget::modifiers(x, seat, data, modifiers, serial),
        }
    }
}

impl PointerTarget<VeloxState> for FocusTarget {
    fn enter(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &MotionEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::enter(s, seat, data, event),
            Self::X11(x) => PointerTarget::enter(x, seat, data, event),
        }
    }

    fn motion(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &MotionEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::motion(s, seat, data, event),
            Self::X11(x) => PointerTarget::motion(x, seat, data, event),
        }
    }

    fn relative_motion(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &RelativeMotionEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::relative_motion(s, seat, data, event),
            Self::X11(x) => PointerTarget::relative_motion(x, seat, data, event),
        }
    }

    fn button(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &ButtonEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::button(s, seat, data, event),
            Self::X11(x) => PointerTarget::button(x, seat, data, event),
        }
    }

    fn axis(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, frame: AxisFrame) {
        match self {
            Self::Wayland(s) => PointerTarget::axis(s, seat, data, frame),
            Self::X11(x) => PointerTarget::axis(x, seat, data, frame),
        }
    }

    fn frame(&self, seat: &Seat<VeloxState>, data: &mut VeloxState) {
        match self {
            Self::Wayland(s) => PointerTarget::frame(s, seat, data),
            Self::X11(x) => PointerTarget::frame(x, seat, data),
        }
    }

    fn gesture_swipe_begin(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &GestureSwipeBeginEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_swipe_begin(s, seat, data, event),
            Self::X11(x) => PointerTarget::gesture_swipe_begin(x, seat, data, event),
        }
    }

    fn gesture_swipe_update(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &GestureSwipeUpdateEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_swipe_update(s, seat, data, event),
            Self::X11(x) => PointerTarget::gesture_swipe_update(x, seat, data, event),
        }
    }

    fn gesture_swipe_end(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &GestureSwipeEndEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_swipe_end(s, seat, data, event),
            Self::X11(x) => PointerTarget::gesture_swipe_end(x, seat, data, event),
        }
    }

    fn gesture_pinch_begin(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &GesturePinchBeginEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_pinch_begin(s, seat, data, event),
            Self::X11(x) => PointerTarget::gesture_pinch_begin(x, seat, data, event),
        }
    }

    fn gesture_pinch_update(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &GesturePinchUpdateEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_pinch_update(s, seat, data, event),
            Self::X11(x) => PointerTarget::gesture_pinch_update(x, seat, data, event),
        }
    }

    fn gesture_pinch_end(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &GesturePinchEndEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_pinch_end(s, seat, data, event),
            Self::X11(x) => PointerTarget::gesture_pinch_end(x, seat, data, event),
        }
    }

    fn gesture_hold_begin(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &GestureHoldBeginEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_hold_begin(s, seat, data, event),
            Self::X11(x) => PointerTarget::gesture_hold_begin(x, seat, data, event),
        }
    }

    fn gesture_hold_end(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, event: &GestureHoldEndEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_hold_end(s, seat, data, event),
            Self::X11(x) => PointerTarget::gesture_hold_end(x, seat, data, event),
        }
    }

    fn leave(&self, seat: &Seat<VeloxState>, data: &mut VeloxState, serial: Serial, time: u32) {
        match self {
            Self::Wayland(s) => PointerTarget::leave(s, seat, data, serial, time),
            Self::X11(x) => PointerTarget::leave(x, seat, data, serial, time),
        }
    }
}

#[derive(Debug)]
pub struct WindowMeta {
    pub floating: bool,
    pub always_on_top: bool,
    pub fullscreen: bool,
    pub maximized: bool,
    pub minimized: bool,
    pub show_desktop_hidden: bool,
    pub snapped: Option<SnapZone>,
    pub restore_rect: Option<Rectangle<i32, Logical>>,
    pub override_redirect: bool,
    pub last_configured_rect: Option<Rectangle<i32, Logical>>,
    pub order_hint: Option<usize>,
    pub taskbar_order: u64,
    pub is_game: Option<bool>,
    pub restore_fallback_checks: u8,
    pub restore_clamp_commits: u8,
    pub minimal_view: bool,
    pub minimal_slot_size: Size<i32, Logical>,
    pub minimal_card_buf: Option<MemoryRenderBuffer>,
    pub minimal_card_size: (i32, i32),
    pub minimal_card_sig: String,
    pub unfullscreen_hold_until: Option<Instant>,
    pub last_minimal_click_at: Option<Instant>,
    pub border_bufs: [SolidColorBuffer; 4],
    pub border_color: [f32; 4],
    pub border_geom: (i32, i32, i32),
    pub dim_buf: SolidColorBuffer,
    pub dim_size: Size<i32, Logical>,
    pub dim_color: [f32; 4],
    pub last_bg_frame_at: Instant,
}

impl Default for WindowMeta {
    fn default() -> Self {
        Self {
            floating: false,
            always_on_top: false,
            fullscreen: false,
            maximized: false,
            minimized: false,
            show_desktop_hidden: false,
            snapped: None,
            restore_rect: None,
            override_redirect: false,
            last_configured_rect: None,
            order_hint: None,
            taskbar_order: 0,
            is_game: None,
            restore_fallback_checks: 0,
            restore_clamp_commits: 0,
            minimal_view: false,
            minimal_slot_size: (0, 0).into(),
            minimal_card_buf: None,
            minimal_card_size: (0, 0),
            minimal_card_sig: String::new(),
            unfullscreen_hold_until: None,
            last_minimal_click_at: None,
            border_bufs: std::array::from_fn(|_| SolidColorBuffer::default()),
            border_color: [0.0, 0.0, 0.0, 0.0],
            border_geom: (0, 0, 0),
            dim_buf: SolidColorBuffer::default(),
            dim_size: (0, 0).into(),
            dim_color: [0.0, 0.0, 0.0, 0.0],
            last_bg_frame_at: Instant::now(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingWindowRestore {
    pub entry: ReloadWindowEntry,
    pub spawned_pid: Option<u32>,
    pub applied: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum PointerLockMode {
    Free,
    Locked,
    Confined(Rectangle<i32, Logical>),
}

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone)]
pub struct SurfaceCadenceSlot {
    pub surf_id: u64,
    pub last_commit_at: Instant,
    pub ema_dt_ms: f32,
    pub steady_count: u32,
    pub detected_video_fps: u32,
    pub area: i32,
}

#[derive(Debug, Default)]
pub struct CommitCadenceTracker {
    pub last_surface_id: u64,
    pub last_commit_at: Option<Instant>,
    pub ema_dt_ms: f32,
    pub steady_count: u32,
    pub detected_video_fps: u32,
    pub slots: Vec<SurfaceCadenceSlot>,
}

impl CommitCadenceTracker {
    pub fn classify_dt_ms(ema_dt_ms: f32) -> u32 {
        match ema_dt_ms {
            d if (37.5..=46.0).contains(&d) => {
                if d < 40.9 { 25 } else { 24 }
            }
            d if (28.5..37.5).contains(&d) => 30,
            d if (19.6..=23.5).contains(&d) => 50,
            // Include 13.5..=19.6ms so 60fps video quantized to 144Hz vsync (2 ticks = 13.89ms,
            // avg 16.67ms) is reliably detected instead of falling into a dead zone.
            d if (13.5..19.6).contains(&d) => 60,
            _ => 0,
        }
    }

    pub fn record_commit_at(
        &mut self,
        surf_id: u64,
        area: i32,
        animating: bool,
        is_game: bool,
        now: Instant,
    ) {
        if is_game {
            self.slots.retain(|s| s.surf_id != surf_id);
            if self.slots.is_empty() {
                self.detected_video_fps = 0;
                self.steady_count = 0;
            }
            return;
        }
        if animating || area < 240 * 140 {
            return;
        }

        // Track up to 8 independent visible surfaces so UI chrome/subtitle commits do not wipe
        // the steady frame-rate cadence of an active video surface.
        if let Some(pos) = self.slots.iter().position(|s| s.surf_id == surf_id) {
            let dt_ms = now
                .saturating_duration_since(self.slots[pos].last_commit_at)
                .as_secs_f32()
                * 1000.0;
            // Ignore duplicate back-to-back damage/sync commits within the same frame window.
            if dt_ms < 5.5 {
                return;
            }
            let slot = &mut self.slots[pos];
            slot.last_commit_at = now;
            slot.area = area;
            if (13.5..=66.0).contains(&dt_ms) {
                if slot.steady_count == 0 {
                    slot.ema_dt_ms = dt_ms;
                    slot.steady_count = 1;
                } else {
                    slot.ema_dt_ms = slot.ema_dt_ms * 0.80 + dt_ms * 0.20;
                    if (dt_ms - slot.ema_dt_ms).abs() <= 11.0 {
                        slot.steady_count = slot.steady_count.saturating_add(1);
                    } else {
                        slot.steady_count /= 2;
                    }
                }
                if slot.steady_count >= 12 {
                    slot.detected_video_fps = Self::classify_dt_ms(slot.ema_dt_ms);
                }
            } else if dt_ms > 180.0 {
                slot.steady_count = 0;
                slot.detected_video_fps = 0;
            }
        } else {
            if self.slots.len() >= 8 {
                if let Some((oldest_idx, _)) = self
                    .slots
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, s)| s.last_commit_at)
                {
                    self.slots.swap_remove(oldest_idx);
                }
            }
            self.slots.push(SurfaceCadenceSlot {
                surf_id,
                last_commit_at: now,
                ema_dt_ms: 0.0,
                steady_count: 0,
                detected_video_fps: 0,
                area,
            });
        }

        self.refresh_summary(surf_id, now);
    }

    pub fn record_commit(&mut self, surf_id: u64, area: i32, animating: bool, is_game: bool) {
        self.record_commit_at(surf_id, area, animating, is_game, Instant::now());
    }

    fn refresh_summary(&mut self, active_surf_id: u64, now: Instant) {
        self.slots
            .retain(|s| now.saturating_duration_since(s.last_commit_at).as_secs_f32() <= 2.8);
        if let Some(best) = self
            .slots
            .iter()
            .filter(|s| s.detected_video_fps > 0)
            .max_by_key(|s| (s.area, s.steady_count))
        {
            self.last_surface_id = best.surf_id;
            self.last_commit_at = Some(best.last_commit_at);
            self.ema_dt_ms = best.ema_dt_ms;
            self.steady_count = best.steady_count;
            self.detected_video_fps = best.detected_video_fps;
        } else if let Some(cur) = self.slots.iter().find(|s| s.surf_id == active_surf_id) {
            self.last_surface_id = cur.surf_id;
            self.last_commit_at = Some(cur.last_commit_at);
            self.ema_dt_ms = cur.ema_dt_ms;
            self.steady_count = cur.steady_count;
            self.detected_video_fps = 0;
        }
    }

    pub fn expire_if_idle(&mut self) -> bool {
        let now = Instant::now();
        let prev = self.detected_video_fps;
        self.slots
            .retain(|s| now.saturating_duration_since(s.last_commit_at).as_secs_f32() <= 2.5);
        self.detected_video_fps = self
            .slots
            .iter()
            .filter(|s| s.detected_video_fps > 0)
            .max_by_key(|s| (s.area, s.steady_count))
            .map(|s| {
                self.last_surface_id = s.surf_id;
                self.last_commit_at = Some(s.last_commit_at);
                self.ema_dt_ms = s.ema_dt_ms;
                self.steady_count = s.steady_count;
                s.detected_video_fps
            })
            .unwrap_or(0);
        if self.detected_video_fps == 0 {
            self.steady_count = 0;
        }
        prev != 0 && self.detected_video_fps == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum VeloxSelectionData {
    #[default]
    X11,
    ServerBuffer(u64),
}

#[derive(Debug, Default)]
pub struct ServerSelectionSlot {
    pub seq: AtomicU64,
    pub data: Mutex<Option<(u64, Vec<u8>)>>,
    pub ready: Condvar,
}

impl ServerSelectionSlot {
    pub fn next_id(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn store(&self, id: u64, bytes: Vec<u8>) {
        *self.data.lock().unwrap() = Some((id, bytes));
        self.ready.notify_all();
    }

    pub fn cached_bytes(&self, expected_id: u64) -> Option<Vec<u8>> {
        let guard = self.data.lock().unwrap();
        match guard.as_ref() {
            Some((id, buf)) if *id == expected_id => Some(buf.clone()),
            _ => None,
        }
    }

    pub fn write_to_fd_async(self: &Arc<Self>, expected_id: u64, fd: OwnedFd) {
        let slot = Arc::clone(self);
        std::thread::spawn(move || {
            use std::io::Write;
            let mut guard = slot.data.lock().unwrap();
            let deadline = Instant::now() + Duration::from_millis(450);
            while guard.as_ref().is_none_or(|(id, _)| *id != expected_id) {
                let now = Instant::now();
                if now >= deadline {
                    return;
                }
                let (g, _) = slot.ready.wait_timeout(guard, deadline - now).unwrap();
                guard = g;
            }
            let bytes = match guard.as_ref() {
                Some((id, buf)) if *id == expected_id => buf.clone(),
                _ => return,
            };
            drop(guard);
            let mut file = std::fs::File::from(fd);
            let _ = file.write_all(&bytes);
        });
    }
}

fn create_pipe_pair() -> Option<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    // SAFETY: fds is a 2-element output array and O_CLOEXEC avoids leaking descriptors into children.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return None;
    }
    // SAFETY: pipe2 succeeded so both file descriptors are valid open descriptors.
    unsafe { Some((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))) }
}

pub struct VeloxState {
    pub display_handle: DisplayHandle,
    pub config: Config,
    pub should_quit: bool,
    pub reload_requested: bool,
    pub wallpaper_dirty: bool,
    pub needs_redraw: bool,
    pub cursor_dirty: bool,
    pub display_config_dirty: bool,
    pub last_input_at: Instant,
    pub last_visible_commit_at: Instant,
    pub cadence: CommitCadenceTracker,
    pub display_telemetry: DisplayTelemetry,

    pub wayland_display: Option<String>,
    pub x11_display: Option<String>,

    pub workspaces: [Space<Window>; NUM_WORKSPACES],
    pub active_workspace: usize,
    pub outputs: Vec<Output>,
    pub popups: PopupManager,

    pub apps: Vec<DesktopApp>,
    pub ui: ShellUi,
    pub animator: WindowAnimator,
    pub sys: SystemController,
    pub sys_snap: SystemSnapshot,
    pub notif_svc: Arc<NotificationService>,
    pub notif_snap: NotificationSnapshot,
    pub cooldown_saved_volume: Option<u8>,
    pub update_watcher: UpdateWatcher,
    pub update_status: UpdateStatus,
    pub update_seen_ver: u64,
    pub pending_restores: Vec<PendingWindowRestore>,
    pub restore_deadline: Option<Instant>,
    pub next_window_seq: u64,
    pub snap_preview: Option<Rectangle<i32, Logical>>,
    pub snap_preview_buf: SolidColorBuffer,
    pub ignore_reload_mtime: u128,
    pub last_app_library_mtime: u128,
    pub hardware_dirty: bool,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    pub xwayland_shell_state: XWaylandShellState,
    pub xwm: Option<X11Wm>,
    pub shm_state: ShmState,
    #[allow(dead_code)]
    pub viewporter_state: ViewporterState,
    #[allow(dead_code)]
    pub fractional_scale_manager_state: FractionalScaleManagerState,
    #[allow(dead_code)]
    pub pointer_constraints_state: PointerConstraintsState,
    #[allow(dead_code)]
    pub relative_pointer_manager_state: RelativePointerManagerState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    pub dmabuf_formats: Vec<Format>,
    pub syncobj_state: Option<DrmSyncobjState>,
    pub pending_sync_sources: Vec<(Client, DrmSyncPointSource)>,
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub data_control_state: WlrDataControlState,
    pub ext_data_control_state: ExtDataControlState,
    pub pending_x11_selections: Vec<(SelectionTarget, String, OwnedFd)>,
    pub server_clipboard: Arc<ServerSelectionSlot>,
    pub server_primary: Arc<ServerSelectionSlot>,
    pub last_terminal_right_click: Option<(Window, Point<f64, Logical>)>,

    #[allow(dead_code)]
    pub seat: Seat<Self>,
    pub pointer: PointerHandle<Self>,
    pub keyboard: KeyboardHandle<Self>,
    pub pointer_location: Point<f64, Logical>,
    pub pointer_button_down: bool,
    pub cursor_status: CursorImageStatus,
    pub super_only_pressed: bool,
    pub screen_locked: bool,
    pub lock_password_input: String,
    pub lock_error_msg: String,
    pub lock_auth_pending: Option<Arc<std::sync::Mutex<Option<bool>>>>,
    pub idle_dimmed: bool,
    pub idle_asleep: bool,
    pub idle_dim_buf: SolidColorBuffer,
    pub idle_sleep_buf: SolidColorBuffer,
}

impl VeloxState {
    pub fn new(display_handle: DisplayHandle, config: Config) -> Self {
        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&display_handle);
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let viewporter_state = ViewporterState::new::<Self>(&display_handle);
        let fractional_scale_manager_state =
            FractionalScaleManagerState::new::<Self>(&display_handle);
        let pointer_constraints_state = PointerConstraintsState::new::<Self>(&display_handle);
        let relative_pointer_manager_state = RelativePointerManagerState::new::<Self>(&display_handle);
        let dmabuf_state = DmabufState::new();
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&display_handle);
        let data_control_state = WlrDataControlState::new::<Self, _>(
            &display_handle,
            Some(&primary_selection_state),
            |_| true,
        );
        let ext_data_control_state = ExtDataControlState::new::<Self, _>(
            &display_handle,
            Some(&primary_selection_state),
            |_| true,
        );

        let mut seat = seat_state.new_wl_seat(&display_handle, "velox-seat");
        let keyboard = seat
            .add_keyboard(Default::default(), 200, 25)
            .expect("Failed to initialize keyboard on seat");
        let pointer = seat.add_pointer();

        let apps = scan_desktop_apps();
        let sys = SystemController::start(
            config.comms_output_device.clone(),
            config.media_output_device.clone(),
            config.auto_route_audio_roles,
        );
        sys.send(SysCommand::SetShowSystemProcesses {
            enabled: config.show_system_processes,
        });
        sys.restore_saved_hardware(&config);
        let sys_snap = sys.snapshot();
        let notif_svc = NotificationService::start(true);
        notif_svc.apply_config(&config);
        let mut active_workspace = 0usize;
        let mut pending_restores = Vec::new();
        let mut restore_deadline = None;
        if let Some(restored) = take_fresh_reload_snapshot() {
            active_workspace = restored.active_workspace.min(NUM_WORKSPACES - 1);
            notif_svc.restore_items(restored.notifications);
            let count = restored.windows.len();
            notif_svc.mark_reloaded(env!("CARGO_PKG_VERSION"), count);
            pending_restores = restored
                .windows
                .into_iter()
                .map(|entry| PendingWindowRestore {
                    entry,
                    spawned_pid: None,
                    applied: false,
                })
                .collect();
            if !pending_restores.is_empty() {
                restore_deadline = Some(Instant::now() + Duration::from_secs(30));
            }
        }
        let notif_snap = notif_svc.snapshot();
        let update_watcher = UpdateWatcher::start(
            config.github_update_repo.clone(),
            config.check_github_updates,
            notif_svc.clone(),
        );
        let update_status = update_watcher.status();
        let update_seen_ver = update_watcher.version();
        tracing::info!(count = apps.len(), "Indexed installed desktop applications");

        Self {
            display_handle,
            config,
            should_quit: false,
            reload_requested: false,
            wallpaper_dirty: false,
            needs_redraw: true,
            cursor_dirty: true,
            display_config_dirty: true,
            last_input_at: Instant::now(),
            last_visible_commit_at: Instant::now(),
            cadence: CommitCadenceTracker::default(),
            display_telemetry: DisplayTelemetry::default(),
            wayland_display: None,
            x11_display: None,
            workspaces: std::array::from_fn(|_| Space::default()),
            active_workspace,
            outputs: Vec::new(),
            popups: PopupManager::default(),
            apps,
            ui: ShellUi::new(),
            animator: WindowAnimator::default(),
            sys,
            sys_snap,
            notif_svc,
            notif_snap,
            cooldown_saved_volume: None,
            update_watcher,
            update_status,
            update_seen_ver,
            pending_restores,
            restore_deadline,
            next_window_seq: 1,
            snap_preview: None,
            snap_preview_buf: SolidColorBuffer::default(),
            ignore_reload_mtime: Config::config_modified_nanos(),
            last_app_library_mtime: AppLibraryStore::modified_nanos(),
            hardware_dirty: false,
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            xwayland_shell_state,
            xwm: None,
            shm_state,
            viewporter_state,
            fractional_scale_manager_state,
            pointer_constraints_state,
            relative_pointer_manager_state,
            dmabuf_state,
            dmabuf_global: None,
            dmabuf_formats: Vec::new(),
            syncobj_state: None,
            pending_sync_sources: Vec::new(),
            output_manager_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            data_control_state,
            ext_data_control_state,
            pending_x11_selections: Vec::new(),
            server_clipboard: Arc::new(ServerSelectionSlot::default()),
            server_primary: Arc::new(ServerSelectionSlot::default()),
            last_terminal_right_click: None,
            seat,
            pointer,
            keyboard,
            pointer_location: (640.0, 400.0).into(),
            pointer_button_down: false,
            cursor_status: CursorImageStatus::default_named(),
            super_only_pressed: false,
            screen_locked: false,
            lock_password_input: String::new(),
            lock_error_msg: String::new(),
            lock_auth_pending: None,
            idle_dimmed: false,
            idle_asleep: false,
            idle_dim_buf: SolidColorBuffer::default(),
            idle_sleep_buf: SolidColorBuffer::default(),
        }
    }

    pub fn effective_idle_secs(&self) -> u64 {
        if self.config.idle_inhibit_on_video
            && self.cadence.detected_video_fps > 0
            && self.last_visible_commit_at.elapsed().as_secs() < 3
        {
            return 0;
        }
        self.last_input_at.elapsed().as_secs()
    }

    pub fn wake_from_user_input(&mut self) {
        self.last_input_at = Instant::now();
        if self.idle_dimmed || self.idle_asleep {
            self.idle_dimmed = false;
            self.idle_asleep = false;
            self.needs_redraw = true;
        }
    }

    pub fn update_idle_power_states(&mut self) -> bool {
        let idle = self.effective_idle_secs();
        let next_dim = self.config.idle_dim_secs > 0 && idle >= self.config.idle_dim_secs as u64;
        let next_sleep =
            self.config.idle_sleep_secs > 0 && idle >= self.config.idle_sleep_secs as u64;
        if next_sleep && self.config.lock_on_sleep && !self.screen_locked {
            self.lock_screen();
        }
        if next_dim != self.idle_dimmed || next_sleep != self.idle_asleep {
            self.idle_dimmed = next_dim;
            self.idle_asleep = next_sleep;
            self.needs_redraw = true;
            return true;
        }
        false
    }

    pub fn lock_screen(&mut self) {
        self.deactivate_focused_pointer_constraint();
        self.ui.close_popup();
        self.screen_locked = true;
        self.lock_password_input.clear();
        self.lock_error_msg.clear();
        self.lock_auth_pending = None;
        self.needs_redraw = true;
    }

    pub fn submit_lock_password(&mut self) {
        if !self.screen_locked || self.lock_auth_pending.is_some() {
            return;
        }
        let pwd = self.lock_password_input.clone();
        self.lock_error_msg.clear();
        let result_slot = Arc::new(std::sync::Mutex::new(None));
        let slot_bg = Arc::clone(&result_slot);
        self.lock_auth_pending = Some(result_slot);
        self.needs_redraw = true;

        std::thread::Builder::new()
            .name("velox-lock-auth".to_string())
            .spawn(move || {
                let ok = verify_user_password_unix_chkpwd(&pwd);
                if let Ok(mut guard) = slot_bg.lock() {
                    *guard = Some(ok);
                }
            })
            .ok();
    }

    pub fn poll_lock_auth(&mut self) {
        let outcome = self
            .lock_auth_pending
            .as_ref()
            .and_then(|slot| slot.lock().ok().and_then(|g| *g));
        if let Some(ok) = outcome {
            self.lock_auth_pending = None;
            self.lock_password_input.clear();
            if ok {
                self.screen_locked = false;
                self.lock_error_msg.clear();
                self.wake_from_user_input();
                self.sync_focused_pointer_constraint();
            } else {
                self.lock_error_msg = "Incorrect password — try again".to_string();
            }
            self.needs_redraw = true;
        }
    }

    pub fn refresh_system_snapshot(&mut self) -> bool {
        let mut changed = false;
        let was_cooling = self.notif_snap.cooldown_active;
        self.notif_svc.tick(&self.config);
        self.poll_lock_auth();
        if self.update_idle_power_states() {
            changed = true;
        }
        if let Some(latest) = self.sys.snapshot_if_changed(self.sys_snap.version) {
            self.sys_snap = latest;
            self.needs_redraw = true;
            changed = true;
        }
        if let Some(latest_notif) = self.notif_svc.snapshot_if_changed(self.notif_snap.version) {
            self.notif_snap = latest_notif;
            self.needs_redraw = true;
            changed = true;
        }
        if !was_cooling && self.notif_snap.cooldown_active {
            let (vol, muted) = self.sys_snap.default_output_vol();
            if !muted && vol > 25 && self.cooldown_saved_volume.is_none() {
                self.cooldown_saved_volume = Some(vol);
                let ducked = (vol / 2).clamp(15, 35);
                self.sys.send(SysCommand::SetSinkVolume {
                    name: "@DEFAULT_SINK@".to_string(),
                    pct: ducked,
                });
            }
        } else if was_cooling && !self.notif_snap.cooldown_active {
            if let Some(orig) = self.cooldown_saved_volume.take() {
                self.sys.send(SysCommand::SetSinkVolume {
                    name: "@DEFAULT_SINK@".to_string(),
                    pct: orig,
                });
            }
        }
        let uver = self.update_watcher.version();
        if uver != self.update_seen_ver {
            self.update_seen_ver = uver;
            self.update_status = self.update_watcher.status();
            self.needs_redraw = true;
            changed = true;
        }
        if self.ui.should_expire_toast(&self.notif_snap, &self.config) {
            self.needs_redraw = true;
            changed = true;
        }
        if let Some(cmd) = take_external_cmd() {
            match cmd.as_str() {
                "confirm-reload" | "reload" => {
                    self.deactivate_focused_pointer_constraint();
                    self.ui.prompt_reload_confirm("Update installed or reload requested");
                }
                "reload-now" => self.trigger_seamless_reload(),
                "check-updates" => self.update_watcher.trigger_check(),
                "lock-screen" => self.lock_screen(),
                "test-notif" => {
                    self.notif_svc.push_internal(
                        "Velox Notifications",
                        "Notification System Active",
                        "D-Bus notifications, toast alerts & update watcher are ready.",
                        NotificationUrgency::Normal,
                        vec![],
                        false,
                        5000,
                    );
                }
                _ => {}
            }
            self.needs_redraw = true;
            changed = true;
        }
        changed
    }

    pub fn has_pending_focused_restore_missing(&self) -> bool {
        if self.restore_deadline.is_some_and(|dl| Instant::now() > dl) {
            return false;
        }
        self.pending_restores
            .iter()
            .any(|r| !r.applied && r.entry.focused && r.entry.workspace == self.active_workspace)
    }

    pub fn is_taskbar_locked(&self) -> bool {
        self.config.lock_fullscreen_taskbar
            && self
                .focused_window()
                .is_some_and(|w| Self::window_meta(&w).fullscreen)
    }

    pub fn is_taskbar_locked_for_output(&self, output: &Output) -> bool {
        self.config.lock_fullscreen_taskbar
            && self.has_fullscreen_on_output(output)
            && self
                .focused_window()
                .is_some_and(|w| Self::window_meta(&w).fullscreen)
    }

    pub fn deactivate_focused_pointer_constraint(&self) {
        let Some(surf) = self
            .focused_window()
            .and_then(|w| Self::window_wl_surface(&w))
        else {
            return;
        };
        with_pointer_constraint(&surf, &self.pointer, |constraint| {
            if let Some(c) = constraint {
                if c.is_active() {
                    c.deactivate();
                }
            }
        });
    }

    pub fn sync_focused_pointer_constraint(&self) {
        if self.ui.is_popup_open() {
            return;
        }
        let Some(surf) = self
            .focused_window()
            .and_then(|w| Self::window_wl_surface(&w))
        else {
            return;
        };
        with_pointer_constraint(&surf, &self.pointer, |constraint| {
            if let Some(c) = constraint {
                if !c.is_active() {
                    c.activate();
                }
            }
        });
    }

    pub fn window_pid(&self, win: &Window) -> Option<u32> {
        win.toplevel()
            .and_then(|tl| self.display_handle.get_client(tl.wl_surface().id()).ok())
            .and_then(|c| c.get_credentials(&self.display_handle).ok())
            .map(|cred| cred.pid as u32)
            .or_else(|| win.x11_surface().and_then(|x| x.pid()))
    }

    pub fn window_app_id(win: &Window) -> String {
        if let Some(tl) = win.toplevel() {
            if let Some(id) = with_states(tl.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|d| d.lock().unwrap().app_id.clone())
            }) {
                if !id.is_empty() {
                    return id;
                }
            }
        }
        if let Some(x11) = win.x11_surface() {
            let class = x11.class();
            if !class.is_empty() {
                return class;
            }
            let inst = x11.instance();
            if !inst.is_empty() {
                return inst;
            }
        }
        String::new()
    }

    pub fn is_game_window(&self, win: &Window) -> bool {
        if let Some(surf) = Self::window_wl_surface(win) {
            let has_constraint = with_pointer_constraint(&surf, &self.pointer, |c| c.is_some());
            if has_constraint {
                return true;
            }
        }
        if let Some(cached) = Self::window_meta(win).is_game {
            return cached;
        }

        let app_id = Self::window_app_id(win).to_ascii_lowercase();
        let title = Self::window_title(win).to_ascii_lowercase();
        let pid = self.window_pid(win);

        let excluded_names = [
            "zen-browser",
            "zen",
            "firefox",
            "chromium",
            "google-chrome",
            "brave",
            "discord",
            "alacritty",
            "kitty",
            "wezterm",
            "foot",
            "velox-settings",
            "thunar",
            "dolphin",
            "nautilus",
            "btop",
            "steamwebhelper",
        ];

        let mut is_game = false;
        let mut explicit_exclude = excluded_names
            .iter()
            .any(|ex| app_id == *ex || app_id.ends_with(&format!(".{ex}")));

        if !explicit_exclude && !app_id.is_empty() {
            if let Some(matched) = self.apps.iter().find(|app| {
                !app.hidden
                    && (app.name.to_ascii_lowercase() == app_id
                        || app
                            .exec_args
                            .first()
                            .and_then(|e| std::path::Path::new(e).file_name())
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.to_ascii_lowercase() == app_id))
            }) {
                if let Some(forced) = matched.is_game_override {
                    is_game = forced;
                    if !forced {
                        explicit_exclude = true;
                    }
                } else if matched.category == AppCategory::Games {
                    is_game = true;
                }
            }
        }

        if let Some(p) = pid {
            let exe_lower = std::fs::read_link(format!("/proc/{p}/exe"))
                .ok()
                .map(|pb| pb.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let cmd_lower = read_proc_cmdline(p)
                .map(|v| v.join(" ").to_ascii_lowercase())
                .unwrap_or_default();
            let proc_base = std::path::Path::new(&exe_lower)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            if excluded_names.iter().any(|ex| proc_base == *ex)
                || proc_base == "steam"
                || app_id == "steam"
            {
                explicit_exclude = true;
            }

            if !explicit_exclude {
                let game_markers = [
                    "steamapps/common",
                    "gamescope",
                    "proton",
                    ".exe",
                    "retroarch",
                    "rpcs3",
                    "dolphin-emu",
                    "pcsx2",
                    "yuzu",
                    "ryujinx",
                    "cemu",
                    "ppsspp",
                    "cities",
                    "transportfever",
                    "factorio",
                    "minecraft",
                    "prismlauncher",
                    "lutris-wrapper",
                ];
                if game_markers
                    .iter()
                    .any(|m| exe_lower.contains(m) || cmd_lower.contains(m))
                {
                    is_game = true;
                } else if let Ok(env_bytes) = std::fs::read(format!("/proc/{p}/environ")) {
                    for entry in env_bytes.split(|&b| b == 0) {
                        let Ok(kv) = std::str::from_utf8(entry) else {
                            continue;
                        };
                        if let Some(val) = kv.strip_prefix("SteamAppId=") {
                            if !val.is_empty() && val != "0" {
                                is_game = true;
                                break;
                            }
                        } else if kv.starts_with("SteamGameId=")
                            || kv.starts_with("PROTON_VERSION=")
                            || kv.starts_with("LUTRIS_GAME_UUID=")
                            || kv.starts_with("HEROIC_APP_NAME=")
                        {
                            is_game = true;
                            break;
                        }
                    }
                }
            }
        }

        if explicit_exclude {
            is_game = false;
        }
        if is_game || pid.is_some() || !app_id.is_empty() || (!title.is_empty() && title != "window") {
            Self::window_meta_mut(win).is_game = Some(is_game);
        }
        is_game
    }

    pub fn active_game_pointer_bounds(&self) -> Option<Rectangle<i32, Logical>> {
        if !self.config.game_pointer_lock || self.ui.is_popup_open() || self.pointer.is_grabbed() {
            return None;
        }
        let win = self.focused_window()?;
        if !self.is_game_window(&win) {
            return None;
        }
        if Self::window_meta(&win).fullscreen {
            return self
                .output_for_window(&win)
                .and_then(|o| self.active_space().output_geometry(&o));
        }
        self.active_space()
            .element_geometry(&win)
            .filter(|r| r.size.w > 0 && r.size.h > 0)
    }

    pub fn pointer_lock_mode(&self) -> PointerLockMode {
        if self.ui.is_popup_open() || self.pointer.is_grabbed() {
            return PointerLockMode::Free;
        }
        if let Some(win) = self.focused_window() {
            if let Some(surf) = Self::window_wl_surface(&win) {
                let win_rect = self.active_space().element_geometry(&win);
                let out_rect = self
                    .output_for_window(&win)
                    .and_then(|o| self.active_space().output_geometry(&o));
                let mode = with_pointer_constraint(&surf, &self.pointer, |constraint| {
                    let Some(c) = constraint else {
                        return None;
                    };
                    if !c.is_active() {
                        return None;
                    }
                    match &*c {
                        PointerConstraint::Locked(_) => Some(PointerLockMode::Locked),
                        PointerConstraint::Confined(_) => {
                            win_rect.or(out_rect).map(PointerLockMode::Confined)
                        }
                    }
                });
                if let Some(m) = mode {
                    return m;
                }
            }
        }
        if let Some(bounds) = self.active_game_pointer_bounds() {
            return PointerLockMode::Confined(bounds);
        }
        PointerLockMode::Free
    }

    pub fn has_fullscreen_on_output(&self, output: &Output) -> bool {
        let Some(out_geo) = self.active_space().output_geometry(output) else {
            return false;
        };
        for w in self.active_space().elements().rev() {
            let meta = Self::window_meta(w);
            if meta.minimized || meta.show_desktop_hidden || meta.override_redirect {
                continue;
            }
            let is_fs = meta.fullscreen;
            drop(meta);
            if self
                .active_space()
                .element_geometry(w)
                .is_some_and(|g| g.overlaps(out_geo))
            {
                return is_fs;
            }
        }
        false
    }

    pub fn trigger_seamless_reload(&mut self) {
        let _ = self.config.save();
        let created_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let focused_win = self.focused_window();
        let mut entries = Vec::new();
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            let mut order_idx = 0usize;
            for win in ws.elements() {
                let meta = Self::window_meta(win);
                if meta.override_redirect {
                    continue;
                }
                let current_order = order_idx;
                order_idx += 1;

                let pid = self.window_pid(win);
                let Some(p) = pid else {
                    continue;
                };
                let Some(cmdline) = read_proc_cmdline(p) else {
                    continue;
                };
                if cmdline
                    .first()
                    .is_some_and(|b| b.ends_with("Xwayland") || b.ends_with("velox-wm"))
                {
                    continue;
                }
                let loc = ws.element_location(win);
                let sz = win.geometry().size;
                let rect = loc.map(|l| [l.x, l.y, sz.w.max(64), sz.h.max(64)]);
                let restore_rect = meta
                    .restore_rect
                    .map(|r| [r.loc.x, r.loc.y, r.size.w, r.size.h]);
                let output_name = self.output_for_window_in_ws(ws_idx, win).map(|o| o.name());

                entries.push(ReloadWindowEntry {
                    workspace: ws_idx,
                    order_index: current_order,
                    cmdline,
                    app_id: Self::window_app_id(win),
                    title: Self::window_title(win),
                    output_name,
                    rect,
                    restore_rect,
                    floating: meta.floating,
                    always_on_top: meta.always_on_top,
                    fullscreen: meta.fullscreen,
                    maximized: meta.maximized,
                    minimized: meta.minimized,
                    show_desktop_hidden: meta.show_desktop_hidden,
                    snapped: meta.snapped,
                    focused: ws_idx == self.active_workspace && focused_win.as_ref() == Some(win),
                });
            }
        }
        save_reload_snapshot(&ReloadSessionSnapshot {
            created_epoch,
            active_workspace: self.active_workspace,
            windows: entries,
            notifications: self.notif_snap.items.clone(),
        });
        self.notif_svc.shutdown();
        self.update_watcher.shutdown();
        self.reload_requested = true;
        self.should_quit = true;
    }

    /// Hot-reloads `config.toml` and `app_library.toml` when modified externally (e.g. from `velox-settings`).
    pub fn hot_reload_config_if_needed(&mut self, last_mtime: &mut u128) {
        let app_mtime = AppLibraryStore::modified_nanos();
        if app_mtime != self.last_app_library_mtime {
            self.last_app_library_mtime = app_mtime;
            self.apps = scan_desktop_apps();
            self.needs_redraw = true;
        }
        let mtime = Config::config_modified_nanos();
        if mtime == 0 || mtime == *last_mtime {
            return;
        }
        *last_mtime = mtime;
        if mtime == self.ignore_reload_mtime {
            return;
        }
        let next = Config::load_or_create();
        let wp_changed = next.wallpaper_preset != self.config.wallpaper_preset
            || next.wallpaper_path != self.config.wallpaper_path
            || next.use_custom_wallpaper != self.config.use_custom_wallpaper;
        let layout_changed = next.panel_position != self.config.panel_position
            || next.shelf_style != self.config.shelf_style
            || next.panel_height != self.config.panel_height
            || next.gap_px != self.config.gap_px
            || (next.master_ratio - self.config.master_ratio).abs() > 0.005
            || next.layout_mode != self.config.layout_mode
            || next.auto_tile != self.config.auto_tile;

        self.sys.send(SysCommand::SetAutoRouteRoles {
            enabled: next.auto_route_audio_roles,
        });
        self.sys.send(SysCommand::SetShowSystemProcesses {
            enabled: next.show_system_processes,
        });
        self.sys.send(SysCommand::RouteRole {
            is_comms: true,
            target_sink: next.comms_output_device.clone(),
        });
        self.sys.send(SysCommand::RouteRole {
            is_comms: false,
            target_sink: next.media_output_device.clone(),
        });
        if next.disabled_audio_sinks != self.config.disabled_audio_sinks
            || next.disabled_audio_sources != self.config.disabled_audio_sources
        {
            self.sys.send(SysCommand::SetDisabledAudioLists {
                sinks: next.disabled_audio_sinks.clone(),
                sources: next.disabled_audio_sources.clone(),
            });
            for name in &next.disabled_audio_sinks {
                if !self.config.disabled_audio_sinks.contains(name) {
                    self.sys.send(SysCommand::SetAudioDeviceDisabled {
                        is_input: false,
                        name: name.clone(),
                        disabled: true,
                    });
                }
            }
            for name in &self.config.disabled_audio_sinks {
                if !next.disabled_audio_sinks.contains(name) {
                    self.sys.send(SysCommand::SetAudioDeviceDisabled {
                        is_input: false,
                        name: name.clone(),
                        disabled: false,
                    });
                }
            }
            for name in &next.disabled_audio_sources {
                if !self.config.disabled_audio_sources.contains(name) {
                    self.sys.send(SysCommand::SetAudioDeviceDisabled {
                        is_input: true,
                        name: name.clone(),
                        disabled: true,
                    });
                }
            }
            for name in &self.config.disabled_audio_sources {
                if !next.disabled_audio_sources.contains(name) {
                    self.sys.send(SysCommand::SetAudioDeviceDisabled {
                        is_input: true,
                        name: name.clone(),
                        disabled: false,
                    });
                }
            }
        }

        let display_changed = next.output_refresh_hz != self.config.output_refresh_hz
            || next.vrr_mode != self.config.vrr_mode
            || next.video_battery_saver != self.config.video_battery_saver
            || next.idle_refresh_hz != self.config.idle_refresh_hz
            || next.battery_saver_modeset != self.config.battery_saver_modeset
            || next.seamless_refresh_overlay != self.config.seamless_refresh_overlay
            || next.show_refresh_overlay != self.config.show_refresh_overlay
            || next.monitors != self.config.monitors;
        let mem_changed = next.auto_clean_ram_cache != self.config.auto_clean_ram_cache
            || next.ram_cache_threshold_pct != self.config.ram_cache_threshold_pct
            || next.app_prewarm_cache != self.config.app_prewarm_cache;

        let freeze_changed = next.freeze_hidden_windows != self.config.freeze_hidden_windows;
        let update_changed = next.github_update_repo != self.config.github_update_repo
            || next.check_github_updates != self.config.check_github_updates;
        self.config = next;
        self.notif_svc.apply_config(&self.config);
        self.notif_snap = self.notif_svc.snapshot();
        if freeze_changed {
            self.wake_workspace_views();
        }
        if update_changed {
            self.update_watcher.configure(
                self.config.github_update_repo.clone(),
                self.config.check_github_updates,
            );
        }
        if mem_changed {
            self.sys.send(SysCommand::ConfigureMemoryPolicy {
                auto_clean: self.config.auto_clean_ram_cache,
                threshold_pct: self.config.ram_cache_threshold_pct,
                prewarm: self.config.app_prewarm_cache,
                prewarm_bins: vec![
                    self.config.terminal_cmd.clone(),
                    self.config.browser_cmd.clone(),
                    self.config.files_cmd.clone(),
                    self.config.settings_cmd.clone(),
                ],
            });
        }
        if wp_changed {
            self.wallpaper_dirty = true;
        }
        if display_changed {
            if let Some(primary) = self
                .config
                .monitors
                .iter()
                .find(|m| m.primary)
                .map(|m| m.name.clone())
            {
                self.promote_primary_output(&primary);
            }
            self.display_config_dirty = true;
        }
        if layout_changed {
            for win in self.active_space().elements() {
                Self::window_meta_mut(win).last_configured_rect = None;
            }
            self.relayout_active_workspace();
        }
        self.needs_redraw = true;
    }

    pub fn refresh_notifications_and_ui_flags(&mut self) {
        self.notif_snap = self.notif_svc.snapshot();
        self.update_status = self.update_watcher.status();
        self.needs_redraw = true;
    }

    fn spawn_command_with_env(&self, cmd: &[String]) -> Option<u32> {
        let bin = cmd.first()?;
        let mut command = std::process::Command::new(bin);
        command
            .args(cmd.iter().skip(1))
            .env("XDG_CURRENT_DESKTOP", "velox")
            .env("GDK_BACKEND", "wayland,x11")
            .env("QT_QPA_PLATFORM", "wayland;xcb")
            .env("SDL_VIDEODRIVER", "wayland,x11")
            .env("MOZ_ENABLE_WAYLAND", "1")
            .env("ELECTRON_OZONE_PLATFORM_HINT", "auto")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(ref wd) = self.wayland_display {
            command.env("WAYLAND_DISPLAY", wd);
        }
        if let Some(ref xd) = self.x11_display {
            command.env("DISPLAY", xd);
        }
        command.spawn().ok().map(|child| child.id())
    }

    pub fn spawn_restored_windows(&mut self) {
        if self.pending_restores.is_empty() {
            return;
        }
        for i in 0..self.pending_restores.len() {
            let cmd = self.pending_restores[i].entry.cmdline.clone();
            if let Some(pid) = self.spawn_command_with_env(&cmd) {
                self.pending_restores[i].spawned_pid = Some(pid);
            }
        }
    }

    pub fn open_new_instance_for_window(&self, win: &Window) -> bool {
        let app_id = Self::window_app_id(win).to_ascii_lowercase();
        let pid = self.window_pid(win);
        let proc_base = pid
            .and_then(|p| std::fs::read_link(format!("/proc/{p}/exe")).ok())
            .and_then(|pb| pb.file_name().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()))
            .unwrap_or_default();

        let is_browser_bin = |bin: &str| {
            let b = std::path::Path::new(bin)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(bin)
                .to_ascii_lowercase();
            matches!(
                b.as_str(),
                "firefox"
                    | "zen"
                    | "zen-browser"
                    | "librewolf"
                    | "chromium"
                    | "chromium-browser"
                    | "google-chrome"
                    | "google-chrome-stable"
                    | "brave"
                    | "brave-browser"
                    | "vivaldi"
                    | "vivaldi-stable"
                    | "microsoft-edge"
            )
        };

        let matched_app = self.apps.iter().find(|app| {
            let desktop_stem = app
                .id
                .strip_suffix(".desktop")
                .unwrap_or(&app.id)
                .to_ascii_lowercase();
            let exec_base = app
                .exec_args
                .first()
                .and_then(|e| std::path::Path::new(e).file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            (!app_id.is_empty()
                && (desktop_stem == app_id
                    || exec_base == app_id
                    || app.name.to_ascii_lowercase() == app_id))
                || (!proc_base.is_empty() && !exec_base.is_empty() && exec_base == proc_base)
        });

        if let Some(app) = matched_app {
            if !app.terminal
                && app
                    .exec_args
                    .first()
                    .is_some_and(|e| is_browser_bin(e))
            {
                let mut args = app.exec_args.clone();
                if !args.iter().any(|a| a == "--new-window") {
                    args.push("--new-window".to_string());
                }
                if self.spawn_command_with_env(&args).is_some() {
                    return true;
                }
            }
            app.launch(
                &self.config.terminal_cmd,
                self.wayland_display.as_deref(),
                self.x11_display.as_deref(),
            );
            return true;
        }

        let mut cmd = pid
            .and_then(read_proc_cmdline)
            .unwrap_or_default();
        if cmd.is_empty() {
            if let Some(p) = pid {
                if let Ok(exe) = std::fs::read_link(format!("/proc/{p}/exe")) {
                    cmd.push(exe.to_string_lossy().to_string());
                }
            }
        }
        if let Some(first) = cmd.first().cloned() {
            if is_browser_bin(&first) && !cmd.iter().any(|a| a == "--new-window") {
                cmd.push("--new-window".to_string());
            }
            if self.spawn_command_with_env(&cmd).is_some() {
                return true;
            }
        }
        if !app_id.is_empty() && executable_exists(&app_id) {
            return self.spawn_command_with_env(&[app_id]).is_some();
        }
        false
    }

    pub fn is_terminal_window(&self, win: &Window) -> bool {
        let app_id = Self::window_app_id(win);
        if is_terminal_emulator_id(&app_id) {
            return true;
        }
        let app_lower = app_id.to_ascii_lowercase();
        let pid = self.window_pid(win);
        let proc_base = pid
            .and_then(|p| std::fs::read_link(format!("/proc/{p}/exe")).ok())
            .and_then(|pb| {
                pb.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_ascii_lowercase())
            })
            .unwrap_or_default();
        if !proc_base.is_empty() && is_terminal_emulator_id(&proc_base) {
            return true;
        }
        let desktop_stem = app_lower.strip_suffix(".desktop").unwrap_or(&app_lower);
        if let Some(matched) = self.apps.iter().find(|app| {
            let app_stem = app
                .id
                .strip_suffix(".desktop")
                .unwrap_or(&app.id)
                .to_ascii_lowercase();
            let exec_base = app
                .exec_args
                .first()
                .and_then(|e| std::path::Path::new(e).file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            (!desktop_stem.is_empty()
                && (app_stem == desktop_stem
                    || exec_base == desktop_stem
                    || app.name.to_ascii_lowercase() == desktop_stem))
                || (!proc_base.is_empty() && !exec_base.is_empty() && exec_base == proc_base)
        }) {
            return matched.is_terminal_app();
        }
        false
    }

    pub fn is_builtin_menu_terminal(&self, win: &Window) -> bool {
        self.is_terminal_window(win)
    }

    fn read_selection_into_slot(
        &mut self,
        from_primary: bool,
        slot: &Arc<ServerSelectionSlot>,
        target_clipboard: bool,
    ) -> bool {
        let mimes = vec![
            "text/plain;charset=utf-8".to_string(),
            "text/plain".to_string(),
            "UTF8_STRING".to_string(),
            "STRING".to_string(),
            "TEXT".to_string(),
        ];
        let id = slot.next_id();

        let server_data = if from_primary {
            current_primary_selection_userdata(&self.seat)
                .as_deref()
                .cloned()
        } else {
            current_data_device_selection_userdata(&self.seat)
                .as_deref()
                .cloned()
        };

        let mut source_ok = false;
        match server_data {
            Some(VeloxSelectionData::ServerBuffer(src_id)) => {
                let src_slot = if from_primary {
                    &self.server_primary
                } else {
                    &self.server_clipboard
                };
                if let Some(bytes) = src_slot.cached_bytes(src_id) {
                    slot.store(id, bytes);
                    source_ok = true;
                }
            }
            Some(VeloxSelectionData::X11) => {
                if let Some((read_fd, write_fd)) = create_pipe_pair() {
                    let src_ty = if from_primary {
                        SelectionTarget::Primary
                    } else {
                        SelectionTarget::Clipboard
                    };
                    self.pending_x11_selections.push((
                        src_ty,
                        "text/plain;charset=utf-8".to_string(),
                        write_fd,
                    ));
                    spawn_pipe_reader_into_slot(read_fd, Arc::clone(slot), id);
                    source_ok = true;
                }
            }
            None => {
                if let Some((read_fd, write_fd)) = create_pipe_pair() {
                    for mime in &mimes {
                        let Ok(dup_write) = write_fd.try_clone() else {
                            break;
                        };
                        let invalid_mime = if from_primary {
                            match request_primary_client_selection(&self.seat, mime.clone(), dup_write) {
                                Ok(()) => {
                                    source_ok = true;
                                    break;
                                }
                                Err(PrimaryRequestError::InvalidMimetype) => true,
                                Err(_) => false,
                            }
                        } else {
                            match request_data_device_client_selection(&self.seat, mime.clone(), dup_write) {
                                Ok(()) => {
                                    source_ok = true;
                                    break;
                                }
                                Err(DataRequestError::InvalidMimetype) => true,
                                Err(_) => false,
                            }
                        };
                        if !invalid_mime {
                            break;
                        }
                    }
                    drop(write_fd);
                    if source_ok {
                        spawn_pipe_reader_into_slot(read_fd, Arc::clone(slot), id);
                    }
                }
            }
        }

        if !source_ok {
            return false;
        }

        let target_ty = if target_clipboard {
            set_data_device_selection(
                &self.display_handle,
                &self.seat,
                mimes.clone(),
                VeloxSelectionData::ServerBuffer(id),
            );
            SelectionTarget::Clipboard
        } else {
            set_primary_selection(
                &self.display_handle,
                &self.seat,
                mimes.clone(),
                VeloxSelectionData::ServerBuffer(id),
            );
            SelectionTarget::Primary
        };
        if let Some(xwm) = self.xwm.as_mut() {
            let _ = xwm.new_selection(target_ty, Some(mimes));
        }
        true
    }

    pub fn terminal_copy_primary(&mut self) {
        let slot = Arc::clone(&self.server_clipboard);
        let _ = self.read_selection_into_slot(true, &slot, true);
        let _ = self.display_handle.flush_clients();
    }

    fn synthesize_middle_paste(&mut self, win: Window, pos: Point<f64, Logical>) {
        self.focus_window(Some(win));
        self.pointer_location = pos;
        let serial1 = SERIAL_COUNTER.next_serial();
        let serial2 = SERIAL_COUNTER.next_serial();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u32)
            .unwrap_or(0);
        let under = self.surface_under(pos);
        let pointer = self.pointer.clone();
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial: serial1,
                time,
            },
        );
        pointer.frame(self);
        pointer.button(
            self,
            &ButtonEvent {
                button: 0x112,
                state: ButtonState::Pressed,
                serial: serial1,
                time,
            },
        );
        pointer.frame(self);
        pointer.button(
            self,
            &ButtonEvent {
                button: 0x112,
                state: ButtonState::Released,
                serial: serial2,
                time: time.saturating_add(1),
            },
        );
        pointer.frame(self);
        self.needs_redraw = true;
    }

    pub fn terminal_paste_clipboard(&mut self) {
        let target = self
            .last_terminal_right_click
            .take()
            .or_else(|| self.focused_window().map(|w| (w, self.pointer_location)));
        let slot = Arc::clone(&self.server_primary);
        let _ = self.read_selection_into_slot(false, &slot, false);
        let _ = self.display_handle.flush_clients();
        if let Some((win, pos)) = target {
            self.synthesize_middle_paste(win, pos);
        }
    }

    pub fn terminal_paste_selection(&mut self) {
        if let Some((win, pos)) = self
            .last_terminal_right_click
            .take()
            .or_else(|| self.focused_window().map(|w| (w, self.pointer_location)))
        {
            self.synthesize_middle_paste(win, pos);
        }
    }

    pub fn try_apply_window_restore(&mut self, win: &Window, allow_fallback: bool) -> bool {
        if self.pending_restores.is_empty() {
            return false;
        }
        if self
            .restore_deadline
            .is_some_and(|dl| Instant::now() > dl)
        {
            self.pending_restores.clear();
            self.restore_deadline = None;
            return false;
        }
        if Self::window_meta(win).override_redirect {
            return false;
        }

        let win_pid = self.window_pid(win);
        let mut matched_idx = win_pid.and_then(|wp| {
            self.pending_restores
                .iter()
                .position(|r| !r.applied && r.spawned_pid.is_some_and(|sp| proc_ancestors_include(wp, sp)))
        });

        if matched_idx.is_none() && allow_fallback {
            let app_id = Self::window_app_id(win).to_ascii_lowercase();
            let title = Self::window_title(win).to_ascii_lowercase();
            let has_meta = !app_id.is_empty() || (!title.is_empty() && title != "window");
            if has_meta {
                matched_idx = self.pending_restores.iter().position(|r| {
                    if r.applied {
                        return false;
                    }
                    let e_id = r.entry.app_id.to_ascii_lowercase();
                    let e_bin = r
                        .entry
                        .cmdline
                        .first()
                        .and_then(|c| std::path::Path::new(c).file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let e_title = r.entry.title.to_ascii_lowercase();
                    (!e_id.is_empty() && e_id == app_id)
                        || (!e_bin.is_empty()
                            && (!app_id.is_empty() && (app_id == e_bin || app_id.contains(&e_bin) || e_bin.contains(&app_id))))
                        || (!e_title.is_empty() && !title.is_empty() && title == e_title)
                });
            }
        }

        let Some(idx) = matched_idx else {
            return false;
        };

        self.pending_restores[idx].applied = true;
        let entry = self.pending_restores[idx].entry.clone();
        self.pending_restores.retain(|r| !r.applied);
        if self.pending_restores.is_empty() {
            self.restore_deadline = None;
        }

        let target_ws = entry.workspace.min(NUM_WORKSPACES - 1);
        for ws in &mut self.workspaces {
            ws.unmap_elem(win);
        }

        let target_out = entry
            .output_name
            .as_deref()
            .and_then(|name| self.outputs.iter().find(|o| o.name() == name))
            .cloned()
            .or_else(|| self.outputs.first().cloned());

        let out_geo = target_out
            .as_ref()
            .and_then(|o| self.workspaces[target_ws].output_geometry(o))
            .unwrap_or_else(|| Rectangle::new((0, 0).into(), (1920, 1080).into()));
        let usable = target_out
            .as_ref()
            .map(|o| self.usable_rect_for_output(o))
            .unwrap_or(out_geo);

        let seq = self.alloc_window_seq();
        {
            let mut meta = Self::window_meta_mut(win);
            meta.floating = entry.floating;
            meta.always_on_top = entry.always_on_top;
            meta.fullscreen = entry.fullscreen;
            meta.maximized = entry.maximized;
            meta.minimized = entry.minimized;
            meta.show_desktop_hidden = entry.show_desktop_hidden;
            meta.snapped = entry.snapped;
            meta.restore_rect = entry.restore_rect.map(|[x, y, w, h]| {
                clamp_rect_to_bounds(Rectangle::new((x, y).into(), (w, h).into()), usable)
            });
            meta.order_hint = Some(entry.order_index);
            if meta.taskbar_order == 0 {
                meta.taskbar_order = seq;
            }
            meta.last_configured_rect = None;
            meta.is_game = None;
        }

        let initial_loc = if let Some([rx, ry, _, _]) = entry.rect {
            (rx, ry).into()
        } else {
            (usable.loc.x + 32, usable.loc.y + 32).into()
        };

        Self::reinsert_window_ordered(
            &mut self.workspaces[target_ws],
            win.clone(),
            entry.order_index,
            initial_loc,
        );

        if entry.fullscreen {
            if let Some(tl) = win.toplevel() {
                tl.with_pending_state(|s| s.states.set(xdg_toplevel::State::Fullscreen));
            }
            if let Some(x11) = win.x11_surface() {
                let _ = x11.set_fullscreen(true);
            }
            self.apply_window_bounds_in_ws(
                target_ws,
                win,
                (out_geo.loc.x, out_geo.loc.y),
                (out_geo.size.w, out_geo.size.h),
            );
        } else if entry.maximized {
            if let Some(tl) = win.toplevel() {
                tl.with_pending_state(|s| s.states.set(xdg_toplevel::State::Maximized));
            }
            if let Some(x11) = win.x11_surface() {
                let _ = x11.set_maximized(true);
            }
            let gap = if entry.floating || self.config.layout_mode == LayoutMode::Floating {
                0
            } else {
                self.config.gap_px.clamp(0, 32)
            };
            let rect = rect_for_snap_zone(SnapZone::Maximize, usable, gap);
            self.apply_window_bounds_in_ws(
                target_ws,
                win,
                (rect.loc.x, rect.loc.y),
                (rect.size.w, rect.size.h),
            );
        } else if let Some(zone) = entry.snapped {
            let rect = entry
                .rect
                .filter(|r| r[2] >= 64 && r[3] >= 64)
                .map(|[x, y, w, h]| {
                    clamp_rect_to_bounds(Rectangle::new((x, y).into(), (w, h).into()), usable)
                })
                .unwrap_or_else(|| rect_for_snap_zone(zone, usable, self.config.gap_px.clamp(0, 32)));
            self.apply_window_bounds_in_ws(
                target_ws,
                win,
                (rect.loc.x, rect.loc.y),
                (rect.size.w, rect.size.h),
            );
        } else if entry.floating {
            let rect = entry
                .rect
                .filter(|r| r[2] >= 64 && r[3] >= 64)
                .map(|[x, y, w, h]| {
                    clamp_rect_to_bounds(Rectangle::new((x, y).into(), (w, h).into()), usable)
                })
                .unwrap_or_else(|| fallback_rect_for_usable(usable));
            self.apply_window_bounds_in_ws(
                target_ws,
                win,
                (rect.loc.x, rect.loc.y),
                (rect.size.w, rect.size.h),
            );
        } else {
            self.relayout_workspace(target_ws);
        }

        if target_ws == self.active_workspace && !entry.minimized {
            if entry.focused || self.focused_window().is_none() {
                self.focus_window(Some(win.clone()));
            }
        }
        self.wake_workspace_views();
        self.needs_redraw = true;
        true
    }

    fn reinsert_window_ordered(
        space: &mut Space<Window>,
        win: Window,
        target_order: usize,
        fallback_loc: Point<i32, Logical>,
    ) {
        Self::window_meta_mut(&win).order_hint = Some(target_order);
        if !space.elements().any(|w| w == &win) {
            space.map_element(win.clone(), fallback_loc, false);
        }
        let mut ordered: Vec<(usize, usize, Window, Point<i32, Logical>)> = space
            .elements()
            .enumerate()
            .map(|(seq, w)| {
                let hint = Self::window_meta(w)
                    .order_hint
                    .unwrap_or(usize::MAX.saturating_sub(2048).saturating_add(seq));
                let loc = space.element_location(w).unwrap_or(fallback_loc);
                (hint, seq, w.clone(), loc)
            })
            .collect();
        ordered.sort_by_key(|(hint, seq, _, _)| (*hint, *seq));
        for (_, _, w, _) in &ordered {
            space.unmap_elem(w);
        }
        for (_, _, w, loc) in ordered {
            space.map_element(w, loc, false);
        }
    }

    pub fn persist_hardware_if_dirty(&mut self) {
        if std::mem::take(&mut self.hardware_dirty) {
            self.save_config_with_feedback();
        }
    }

    pub fn sync_display_confirm_timeout(&mut self) {
        if let Some((prev_hz, prev_vrr)) = self.ui.tick_display_confirm_expired() {
            self.config.output_refresh_hz = prev_hz;
            self.config.vrr_mode = prev_vrr;
            self.display_config_dirty = true;
            self.save_config_with_feedback();
            self.needs_redraw = true;
        }
    }

    pub fn active_space(&self) -> &Space<Window> {
        &self.workspaces[self.active_workspace]
    }

    pub fn active_space_mut(&mut self) -> &mut Space<Window> {
        &mut self.workspaces[self.active_workspace]
    }

    pub fn add_output(&mut self, output: Output, location: Point<i32, Logical>) {
        if !self.outputs.iter().any(|o| o == &output) {
            self.outputs.push(output.clone());
        }
        for ws in &mut self.workspaces {
            ws.map_output(&output, location);
        }
        self.needs_redraw = true;
    }

    pub fn remove_output(&mut self, output: &Output) {
        self.outputs.retain(|o| o != output);
        for ws in &mut self.workspaces {
            ws.unmap_output(output);
        }
        self.relayout_outputs();
        if let Some(primary) = self.outputs.first().cloned() {
            if let Some(p_usable) = Some(self.usable_rect_for_output(&primary)) {
                for ws in &mut self.workspaces {
                    let wins: Vec<Window> = ws.elements().cloned().collect();
                    for win in wins {
                        let loc = ws.element_location(&win).unwrap_or_default();
                        let still_visible = self.outputs.iter().any(|o| {
                            ws.output_geometry(o)
                                .is_some_and(|g| g.contains(loc))
                        });
                        if !still_visible {
                            Self::window_meta_mut(&win).last_configured_rect = None;
                            ws.map_element(win, (p_usable.loc.x + 24, p_usable.loc.y + 24), false);
                        }
                    }
                }
            }
        }
        self.relayout_active_workspace();
        self.wake_workspace_views();
        self.needs_redraw = true;
    }

    /// Moves the named monitor to primary slot (index 0) and repacks layout when order changes.
    pub fn promote_primary_output(&mut self, name: &str) {
        if let Some(pos) = self.outputs.iter().position(|o| o.name() == name) {
            if pos != 0 {
                let out = self.outputs.remove(pos);
                self.outputs.insert(0, out);
                self.relayout_outputs();
                self.relayout_active_workspace();
                self.wallpaper_dirty = true;
                self.needs_redraw = true;
            }
        }
    }

    /// Re-packs connected outputs horizontally side-by-side cleanly after hotplug or mode change.
    pub fn relayout_outputs(&mut self) {
        let mut next_x = 0i32;
        for out in self.outputs.clone() {
            let loc: Point<i32, Logical> = (next_x, 0).into();
            for ws in &mut self.workspaces {
                ws.map_output(&out, loc);
            }
            let w = out
                .current_mode()
                .map(|m| {
                    let s = out.current_scale().fractional_scale();
                    (m.size.w as f64 / s).round() as i32
                })
                .unwrap_or(1920)
                .max(320);
            next_x += w;
        }
    }

    pub fn output_at_point(&self, pos: Point<f64, Logical>) -> Option<Output> {
        let p = pos.to_i32_round();
        for out in &self.outputs {
            if let Some(geo) = self.active_space().output_geometry(out) {
                if geo.contains(p) {
                    return Some(out.clone());
                }
            }
        }
        self.outputs.first().cloned()
    }

    pub fn output_for_window_in_ws(&self, ws_idx: usize, win: &Window) -> Option<Output> {
        let ws = self.workspaces.get(ws_idx)?;
        let loc = ws.element_location(win)?;
        let size = win.geometry().size;
        let center: Point<f64, Logical> = (
            (loc.x + size.w / 2) as f64,
            (loc.y + size.h / 2) as f64,
        )
            .into();
        self.output_at_point(center)
    }

    pub fn output_for_window(&self, win: &Window) -> Option<Output> {
        self.output_for_window_in_ws(self.active_workspace, win)
    }

    pub fn focused_output(&self) -> Option<Output> {
        self.output_at_point(self.pointer_location)
            .or_else(|| self.focused_window().and_then(|w| self.output_for_window(&w)))
            .or_else(|| self.outputs.first().cloned())
    }

    pub fn usable_rect_for_output(&self, output: &Output) -> Rectangle<i32, Logical> {
        let Some(out_geo) = self.active_space().output_geometry(output) else {
            return Rectangle::new((0, 0).into(), (1280, 720).into());
        };
        let non_exclusive = layer_map_for_output(output).non_exclusive_zone();
        let is_primary = self.outputs.first() == Some(output);
        let (panel_top, panel_bottom) = if is_primary {
            let extra = if self.config.shelf_style == ShelfStyle::Floating {
                10
            } else {
                0
            };
            let panel_h = self.config.panel_height.clamp(34, 56) + extra;
            match self.config.panel_position {
                PanelPosition::Top => (panel_h, 0),
                PanelPosition::Bottom => (0, panel_h),
            }
        } else {
            (0, 0)
        };

        let usable_x = out_geo.loc.x + non_exclusive.loc.x;
        let usable_y = out_geo.loc.y + non_exclusive.loc.y.max(panel_top);
        let usable_w = non_exclusive
            .size
            .w
            .min(out_geo.size.w - non_exclusive.loc.x)
            .max(160);
        let usable_h = (out_geo.loc.y + out_geo.size.h - usable_y - panel_bottom)
            .min(
                non_exclusive.size.h - panel_top.max(non_exclusive.loc.y) + non_exclusive.loc.y
                    - panel_bottom,
            )
            .max(160);
        Rectangle::new((usable_x, usable_y).into(), (usable_w, usable_h).into())
    }

    pub fn usable_workspace_rect(&self) -> Rectangle<i32, Logical> {
        let Some(output) = self.focused_output() else {
            return Rectangle::new((0, 0).into(), (1280, 720).into());
        };
        self.usable_rect_for_output(&output)
    }

    pub fn usable_rect_for_window(&self, win: &Window) -> Rectangle<i32, Logical> {
        self.output_for_window(win)
            .map(|o| self.usable_rect_for_output(&o))
            .unwrap_or_else(|| self.usable_workspace_rect())
    }

    pub fn usable_workspace_rect_at(&self, pos: Point<f64, Logical>) -> Rectangle<i32, Logical> {
        self.output_at_point(pos)
            .map(|o| self.usable_rect_for_output(&o))
            .unwrap_or_else(|| self.usable_workspace_rect())
    }

    /// Clamps a target rectangle to whichever connected output contains its center, or else `win`'s output.
    pub fn clamp_rect_to_best_output(
        &self,
        win: &Window,
        rect: Rectangle<i32, Logical>,
        fullscreen: bool,
    ) -> Rectangle<i32, Logical> {
        let center: Point<i32, Logical> = (
            rect.loc.x + rect.size.w / 2,
            rect.loc.y + rect.size.h / 2,
        )
            .into();
        let target_out = self
            .outputs
            .iter()
            .find(|o| {
                self.active_space()
                    .output_geometry(o)
                    .is_some_and(|g| g.contains(center))
            })
            .cloned()
            .or_else(|| self.output_for_window(win))
            .or_else(|| self.focused_output());

        let bounds = match target_out {
            Some(out) if fullscreen => self
                .active_space()
                .output_geometry(&out)
                .unwrap_or_else(|| self.usable_rect_for_output(&out)),
            Some(out) => self.usable_rect_for_output(&out),
            None => self.usable_rect_for_window(win),
        };
        clamp_rect_to_bounds(rect, bounds)
    }

    /// Constrains interactive window drag positions so the titlebar/grab strip stays reachable on outputs.
    pub fn clamp_move_location(
        &self,
        size: Size<i32, Logical>,
        loc: Point<i32, Logical>,
        pointer_pos: Point<f64, Logical>,
    ) -> Point<i32, Logical> {
        let pointer_usable = self.usable_workspace_rect_at(pointer_pos);
        let mut min_x = pointer_usable.loc.x;
        let mut max_x = pointer_usable.loc.x + pointer_usable.size.w;
        let mut min_y = pointer_usable.loc.y;
        let mut max_y = pointer_usable.loc.y + pointer_usable.size.h;
        for out in &self.outputs {
            if let Some(geo) = self.active_space().output_geometry(out) {
                min_x = min_x.min(geo.loc.x);
                max_x = max_x.max(geo.loc.x + geo.size.w);
                min_y = min_y.min(geo.loc.y);
                max_y = max_y.max(geo.loc.y + geo.size.h);
            }
        }
        let w = size.w.max(80);
        let h = size.h.max(40);
        let keep_x = 80.min(w);
        let keep_y = 40.min(h);
        let low_x = min_x - (w - keep_x);
        let high_x = (max_x - keep_x).max(low_x);
        let low_y = pointer_usable.loc.y.max(min_y);
        let p_bottom = pointer_usable.loc.y + pointer_usable.size.h;
        let eff_max_y = if max_y > p_bottom + 80 { max_y } else { p_bottom };
        let high_y = (eff_max_y - keep_y).max(low_y);
        Point::from((loc.x.clamp(low_x, high_x), loc.y.clamp(low_y, high_y)))
    }

    /// Clamps post-unfullscreen/unmaximize Wayland commits back inside display borders if client min_size or lag overflows.
    pub fn clamp_committed_window_after_restore(&mut self, win: &Window) {
        let (clamp_commits, is_fs, is_or, is_max, is_tiled, configured) = {
            let meta = Self::window_meta(win);
            (
                meta.restore_clamp_commits,
                meta.fullscreen,
                meta.override_redirect,
                meta.maximized,
                !meta.floating
                    && self.config.auto_tile
                    && self.config.layout_mode != LayoutMode::Floating,
                meta.last_configured_rect,
            )
        };
        if clamp_commits == 0 || is_fs || is_or {
            return;
        }
        let geom = win.geometry();
        if geom.size.w <= 0 || geom.size.h <= 0 {
            return;
        }
        Self::window_meta_mut(win).restore_clamp_commits = clamp_commits - 1;

        let Some(loc) = self.active_space().element_location(win) else {
            return;
        };
        let usable = self.usable_rect_for_window(win);
        let out_geo = self
            .output_for_window(win)
            .and_then(|o| self.active_space().output_geometry(&o))
            .unwrap_or(usable);

        // Skip pulling origin during an in-flight pre-unfullscreen frame while resize configure is pending.
        let waiting_for_resize_ack = configured.is_some_and(|c| c.size != geom.size)
            && geom.size.w >= out_geo.size.w
            && geom.size.h >= out_geo.size.h;
        if waiting_for_resize_ack {
            Self::window_meta_mut(win).restore_clamp_commits = clamp_commits.max(2);
            return;
        }
        Self::window_meta_mut(win).unfullscreen_hold_until = None;

        let current_rect = Rectangle::new(loc, geom.size);
        let clamped = clamp_rect_to_bounds(current_rect, usable);
        if clamped.loc != loc {
            self.active_space_mut()
                .map_element(win.clone(), clamped.loc, false);
            self.needs_redraw = true;
        }
        if !is_max && !is_tiled && clamped.size != geom.size {
            Self::window_meta_mut(win).last_configured_rect = Some(clamped);
            if let Some(tl) = win.toplevel() {
                tl.with_pending_state(|s| {
                    s.size = Some(clamped.size);
                });
                tl.send_pending_configure();
            }
            if let Some(x11) = win.x11_surface() {
                let _ = x11.configure(clamped);
            }
        }
    }

    pub fn window_meta(window: &Window) -> std::cell::Ref<'_, WindowMeta> {
        window
            .user_data()
            .get_or_insert(|| RefCell::new(WindowMeta::default()))
            .borrow()
    }

    pub fn window_meta_mut(window: &Window) -> std::cell::RefMut<'_, WindowMeta> {
        window
            .user_data()
            .get_or_insert(|| RefCell::new(WindowMeta::default()))
            .borrow_mut()
    }

    pub fn update_border_buffers(
        window: &Window,
        win_size: Size<i32, Logical>,
        bw: i32,
        color: [f32; 4],
    ) -> [SolidColorBuffer; 4] {
        let mut meta = Self::window_meta_mut(window);
        let geom = (win_size.w, win_size.h, bw);
        if meta.border_geom != geom {
            meta.border_geom = geom;
            let specs = [
                (win_size.w + bw * 2, bw),
                (win_size.w + bw * 2, bw),
                (bw, win_size.h),
                (bw, win_size.h),
            ];
            for (i, s) in specs.into_iter().enumerate() {
                meta.border_bufs[i].resize(s);
            }
        }
        if meta.border_color != color {
            meta.border_color = color;
            let c = Color32F::new(color[0], color[1], color[2], color[3]);
            for buf in &mut meta.border_bufs {
                buf.set_color(c);
            }
        }
        meta.border_bufs.clone()
    }

    pub fn update_dim_buffer(
        window: &Window,
        win_size: Size<i32, Logical>,
        color: [f32; 4],
    ) -> SolidColorBuffer {
        let mut meta = Self::window_meta_mut(window);
        if meta.dim_size != win_size {
            meta.dim_size = win_size;
            meta.dim_buf.resize(win_size);
        }
        if meta.dim_color != color {
            meta.dim_color = color;
            meta.dim_buf
                .set_color(Color32F::new(color[0], color[1], color[2], color[3]));
        }
        meta.dim_buf.clone()
    }

    pub fn window_title(window: &Window) -> String {
        if let Some(tl) = window.toplevel() {
            let meta = with_states(tl.wl_surface(), |states| {
                states.data_map.get::<XdgToplevelSurfaceData>().map(|d| {
                    let guard = d.lock().unwrap();
                    (guard.title.clone(), guard.app_id.clone())
                })
            });
            if let Some((title, app_id)) = meta {
                if let Some(t) = title.filter(|s| !s.is_empty()) {
                    return t;
                }
                if let Some(id) = app_id.filter(|s| !s.is_empty()) {
                    return id;
                }
            }
        }
        if let Some(x11) = window.x11_surface() {
            let title = x11.title();
            if !title.is_empty() {
                return title;
            }
            let class = x11.class();
            if !class.is_empty() {
                return class;
            }
        }
        "Window".to_string()
    }

    pub fn window_wl_surface(window: &Window) -> Option<WlSurface> {
        if let Some(tl) = window.toplevel() {
            return Some(tl.wl_surface().clone());
        }
        if let Some(x11) = window.x11_surface() {
            return x11.wl_surface();
        }
        None
    }

    pub fn window_focus_target(window: &Window) -> Option<FocusTarget> {
        if let Some(x11) = window.x11_surface() {
            return Some(FocusTarget::X11(x11.clone()));
        }
        window
            .toplevel()
            .map(|tl| FocusTarget::Wayland(tl.wl_surface().clone()))
    }

    pub fn workspace_counts(&self) -> [usize; NUM_WORKSPACES] {
        std::array::from_fn(|i| {
            self.workspaces[i]
                .elements()
                .filter(|w| !Self::window_meta(w).override_redirect)
                .count()
        })
    }

    pub fn alloc_window_seq(&mut self) -> u64 {
        let seq = self.next_window_seq.max(1);
        self.next_window_seq = seq.wrapping_add(1).max(1);
        seq
    }

    pub fn active_ws_ordered_windows(&self) -> Vec<Window> {
        let mut wins: Vec<(usize, u64, usize, Window)> = self
            .active_space()
            .elements()
            .filter(|w| !Self::window_meta(w).override_redirect)
            .enumerate()
            .map(|(idx, win)| {
                let meta = Self::window_meta(win);
                (
                    meta.order_hint.unwrap_or(usize::MAX),
                    meta.taskbar_order,
                    idx,
                    win.clone(),
                )
            })
            .collect();
        wins.sort_by_key(|(hint, seq, idx, _)| (*hint, *seq, *idx));
        wins.into_iter().map(|(_, _, _, w)| w).collect()
    }

    pub fn taskbar_windows(&self) -> Vec<TaskWindowInfo> {
        let focused = self.focused_window();
        self.active_ws_ordered_windows()
            .into_iter()
            .map(|win| {
                let meta = Self::window_meta(&win);
                let is_terminal = self.is_terminal_window(&win);
                TaskWindowInfo {
                    title: Self::window_title(&win),
                    focused: !meta.minimized && focused.as_ref() == Some(&win),
                    floating: meta.floating,
                    always_on_top: meta.always_on_top,
                    minimized: meta.minimized,
                    maximized: meta.maximized || meta.fullscreen,
                    is_terminal,
                    minimal_view: meta.minimal_view && self.config.minimal_view_enabled,
                }
            })
            .collect()
    }

    pub fn global_windows_list(&self) -> Vec<GlobalWindowInfo> {
        let focused = self.focused_window();
        let mut out = Vec::new();
        let mut add_ws = |ws_idx: usize| {
            let elems: Vec<Window> = self.workspaces[ws_idx]
                .elements()
                .filter(|w| !Self::window_meta(w).override_redirect)
                .cloned()
                .collect();
            for (idx_in_ws, win) in elems.iter().enumerate().rev() {
                let meta = Self::window_meta(win);
                let is_terminal = self.is_terminal_window(win);
                out.push(GlobalWindowInfo {
                    workspace: ws_idx,
                    index_in_ws: idx_in_ws,
                    title: Self::window_title(win),
                    focused: !meta.minimized && ws_idx == self.active_workspace && focused.as_ref() == Some(win),
                    floating: meta.floating,
                    always_on_top: meta.always_on_top,
                    minimized: meta.minimized,
                    maximized: meta.maximized || meta.fullscreen,
                    is_terminal,
                    minimal_view: meta.minimal_view && self.config.minimal_view_enabled,
                });
            }
        };
        add_ws(self.active_workspace);
        for ws in 0..NUM_WORKSPACES {
            if ws != self.active_workspace {
                add_ws(ws);
            }
        }
        out
    }

    pub fn window_min_size(win: &Window) -> Size<i32, Logical> {
        if let Some(tl) = win.toplevel() {
            with_states(tl.wl_surface(), |states| {
                states
                    .cached_state
                    .get::<SurfaceCachedState>()
                    .current()
                    .min_size
            })
        } else if let Some(x11) = win.x11_surface() {
            x11.min_size().unwrap_or_default()
        } else {
            Size::default()
        }
    }

    pub fn should_use_minimal_view(size: Size<i32, Logical>, min_size: Size<i32, Logical>) -> bool {
        if size.w <= 0 || size.h <= 0 {
            return false;
        }
        size.w < 340
            || size.h < 220
            || (min_size.w > 0 && size.w + 4 < min_size.w)
            || (min_size.h > 0 && size.h + 4 < min_size.h)
    }

    pub fn window_effective_rect_in_ws(
        &self,
        ws_idx: usize,
        win: &Window,
    ) -> Option<Rectangle<i32, Logical>> {
        let loc = self.workspaces.get(ws_idx)?.element_location(win)?;
        let meta = Self::window_meta(win);
        let eff_size = if !meta.fullscreen
            && !meta.override_redirect
            && meta.minimal_view
            && self.config.minimal_view_enabled
            && meta.minimal_slot_size.w >= 48
            && meta.minimal_slot_size.h >= 36
        {
            meta.minimal_slot_size
        } else {
            win.geometry().size
        };
        Some(Rectangle::new(loc, eff_size))
    }

    pub fn window_under_point(&self, pos: Point<f64, Logical>) -> Option<(Window, Option<usize>)> {
        let ordered = self.active_ws_ordered_windows();
        let pt = pos.to_i32_round();
        for win in self.active_space().elements().rev() {
            let meta = Self::window_meta(win);
            if meta.minimized || meta.show_desktop_hidden {
                continue;
            }
            let is_minimal = !meta.fullscreen
                && !meta.override_redirect
                && meta.minimal_view
                && self.config.minimal_view_enabled;
            drop(meta);
            let Some(rect) = self.window_effective_rect_in_ws(self.active_workspace, win) else {
                continue;
            };
            let hit = if is_minimal {
                rect.contains(pt)
            } else {
                let win_loc = rect.loc;
                let render_loc = win_loc - win.geometry().loc;
                let mut bbox = win.bbox_with_popups();
                bbox.loc += render_loc;
                bbox.to_f64().contains(pos)
            };
            if hit {
                let idx = ordered.iter().position(|w| w == win);
                return Some((win.clone(), idx));
            }
        }
        None
    }

    pub fn register_minimal_click_should_expand(&self, win: &Window) -> bool {
        let now = Instant::now();
        let mut meta = Self::window_meta_mut(win);
        if let Some(prev) = meta.last_minimal_click_at {
            if now.saturating_duration_since(prev) <= Duration::from_millis(420) {
                meta.last_minimal_click_at = None;
                return true;
            }
        }
        meta.last_minimal_click_at = Some(now);
        false
    }

    pub fn expand_from_minimal_view(&mut self, win: &Window) {
        let usable = self.usable_rect_for_window(win);
        let cur_loc = self
            .active_space()
            .element_location(win)
            .unwrap_or(usable.loc);
        {
            let mut meta = Self::window_meta_mut(win);
            meta.minimal_view = false;
            meta.minimal_slot_size = (0, 0).into();
            meta.minimal_card_sig.clear();
            meta.last_minimal_click_at = None;
            meta.last_configured_rect = None;
            // If tiled slots are too cramped to leave Minimal View, float this window cleanly so
            // double-clicking or choosing Expand always reveals a full interactive window.
            let has_cramped_tiling = !meta.floating
                && self.config.auto_tile
                && self.config.layout_mode != LayoutMode::Floating;
            if has_cramped_tiling {
                meta.floating = true;
            }
        }
        let target_w = 560.min(usable.size.w.max(360));
        let target_h = 400.min(usable.size.h.max(260));
        let expanded = clamp_rect_to_bounds(
            Rectangle::new(cur_loc, (target_w, target_h).into()),
            usable,
        );
        self.apply_window_bounds_force(
            win,
            (expanded.loc.x, expanded.loc.y),
            (expanded.size.w, expanded.size.h),
        );
        self.relayout_active_workspace();
        self.focus_window(Some(win.clone()));
        self.wake_window_view(win);
        self.needs_redraw = true;
    }

    pub fn ensure_minimal_card(
        &mut self,
        win: &Window,
        size: Size<i32, Logical>,
        focused: bool,
    ) -> Option<MemoryRenderBuffer> {
        if size.w < 48 || size.h < 36 {
            return None;
        }
        let title = Self::window_title(win);
        let is_term = self.is_terminal_window(win);
        let app_id = Self::window_app_id(win);
        let sig = format!(
            "{}x{}:{}:{}:{}:{}:{}",
            size.w,
            size.h,
            focused,
            is_term,
            self.config.accent_theme,
            title,
            app_id
        );
        let unchanged = {
            let meta = Self::window_meta(win);
            meta.minimal_card_sig == sig && meta.minimal_card_buf.is_some()
        };
        if unchanged {
            return Self::window_meta(win).minimal_card_buf.clone();
        }
        let pixels = self
            .ui
            .render_minimal_card_pixels(size.w, size.h, &title, is_term, focused, &self.config);
        let mut meta = Self::window_meta_mut(win);
        let sz = (size.w, size.h);
        if meta.minimal_card_size != sz || meta.minimal_card_buf.is_none() {
            meta.minimal_card_buf = Some(MemoryRenderBuffer::new(
                smithay::backend::allocator::Fourcc::Abgr8888,
                sz,
                1,
                smithay::utils::Transform::Normal,
                None,
            ));
            meta.minimal_card_size = sz;
        }
        meta.minimal_card_sig = sig;
        if let Some(ref mut buf) = meta.minimal_card_buf {
            let mut ctx = buf.render();
            let _ = ctx.draw(|dst| {
                let len = dst.len().min(pixels.len());
                dst[..len].copy_from_slice(&pixels[..len]);
                Ok::<_, ()>(vec![Rectangle::from_size(sz.into())])
            });
        }
        meta.minimal_card_buf.clone()
    }

    pub fn commit_alt_tab(&mut self) {
        let list = self.global_windows_list();
        let idx = self.ui.alt_tab_index;
        self.ui.close_popup();
        if let Some(item) = list.get(idx) {
            self.switch_workspace(item.workspace);
            let win = self.workspaces[item.workspace]
                .elements()
                .filter(|w| !Self::window_meta(w).override_redirect)
                .nth(item.index_in_ws)
                .cloned();
            if let Some(w) = win {
                if Self::window_meta(&w).minimized {
                    self.unminimize_window_for(&w);
                } else {
                    self.focus_window(Some(w.clone()));
                }
                // Move pointer onto the switched monitor/window when crossing outputs so cursor
                // does not stay stranded inside a fullscreen game on the previous monitor.
                let ptr_out = self.output_at_point(self.pointer_location);
                let win_out = self.output_for_window(&w);
                if ptr_out.is_some() && win_out.is_some() && ptr_out != win_out {
                    if let Some(rect) = self
                        .active_space()
                        .element_geometry(&w)
                        .filter(|r| r.size.w > 0 && r.size.h > 0)
                        .or_else(|| {
                            win_out
                                .as_ref()
                                .and_then(|o| self.active_space().output_geometry(o))
                        })
                    {
                        self.pointer_location = (
                            (rect.loc.x + rect.size.w / 2) as f64,
                            (rect.loc.y + rect.size.h / 2) as f64,
                        )
                            .into();
                        self.cursor_dirty = true;
                    }
                }
            }
        }
        self.needs_redraw = true;
    }

    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(FocusTarget, Point<f64, Logical>)> {
        if let Some(output) = self.outputs.first() {
            let map = layer_map_for_output(output);
            for kind in [
                smithay::wayland::shell::wlr_layer::Layer::Overlay,
                smithay::wayland::shell::wlr_layer::Layer::Top,
            ] {
                if let Some(layer) = map.layer_under(kind, pos) {
                    let loc = map.layer_geometry(layer).map(|g| g.loc).unwrap_or_default();
                    if let Some((s, p)) =
                        layer.surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL)
                    {
                        return Some((FocusTarget::Wayland(s), (p + loc).to_f64()));
                    }
                }
            }
        }

        // Hit-test front-to-back skipping minimized windows, honoring popups via bbox_with_popups().
        let pt = pos.to_i32_round();
        for window in self.active_space().elements().rev() {
            let meta = Self::window_meta(window);
            if meta.minimized {
                continue;
            }
            if !meta.fullscreen
                && !meta.override_redirect
                && meta.minimal_view
                && self.config.minimal_view_enabled
            {
                let slot = meta.minimal_slot_size;
                drop(meta);
                if let Some(loc) = self.active_space().element_location(window) {
                    let card_rect = Rectangle::new(loc, slot);
                    if card_rect.contains(pt) {
                        return None;
                    }
                }
                continue;
            }
            drop(meta);
            let Some(win_loc) = self.active_space().element_location(window) else {
                continue;
            };
            let render_loc = win_loc - window.geometry().loc;
            let mut bbox = window.bbox_with_popups();
            bbox.loc += render_loc;
            if !bbox.to_f64().contains(pos) {
                continue;
            }
            if let Some((s, p)) =
                window.surface_under(pos - render_loc.to_f64(), WindowSurfaceType::ALL)
            {
                let target = if let Some(x11) = window.x11_surface() {
                    FocusTarget::X11(x11.clone())
                } else {
                    FocusTarget::Wayland(s)
                };
                return Some((target, (p + render_loc).to_f64()));
            }
        }
        None
    }

    pub fn focused_window(&self) -> Option<Window> {
        let focused = self.keyboard.current_focus()?;
        self.active_space()
            .elements()
            .find(|w| {
                let m = Self::window_meta(w);
                !m.override_redirect
                    && !m.minimized
                    && Self::window_focus_target(w).as_ref() == Some(&focused)
            })
            .cloned()
    }

    pub fn raise_window_layered(&mut self, win: &Window, activate_space: bool) {
        let (win_is_fullscreen, win_is_pinned) = {
            let m = Self::window_meta(win);
            (m.fullscreen, m.always_on_top)
        };
        let mut raised_x11: Vec<X11Surface> = Vec::new();
        self.active_space_mut().raise_element(win, activate_space);
        if let Some(x11) = win.x11_surface() {
            raised_x11.push(x11.clone());
        }
        if !win_is_fullscreen {
            let pinned_below: Vec<Window> = self
                .active_space()
                .elements()
                .filter(|w| {
                    let m = Self::window_meta(w);
                    !m.override_redirect
                        && !m.minimized
                        && !m.fullscreen
                        && m.always_on_top
                        && (!win_is_pinned || *w != win)
                })
                .cloned()
                .collect();
            for pw in pinned_below {
                self.active_space_mut().raise_element(&pw, false);
                if let Some(x11) = pw.x11_surface() {
                    raised_x11.push(x11.clone());
                }
            }
            if win_is_pinned {
                self.active_space_mut().raise_element(win, activate_space);
                if let Some(x11) = win.x11_surface() {
                    raised_x11.push(x11.clone());
                }
            }
        }
        // Keep only override-redirect popups/tooltips above normal windows; do not force unfocused
        // fullscreen windows back on top so Alt+Tab and taskbar switching uncover background apps.
        let top_popups: Vec<Window> = self
            .active_space()
            .elements()
            .filter(|w| {
                let m = Self::window_meta(w);
                !m.minimized && m.override_redirect
            })
            .cloned()
            .collect();
        for pw in top_popups {
            self.active_space_mut().raise_element(&pw, false);
            if let Some(x11) = pw.x11_surface() {
                raised_x11.push(x11.clone());
            }
        }
        if let Some(ref mut xwm) = self.xwm {
            for x11 in raised_x11 {
                let _ = xwm.raise_window(&x11);
            }
        }
    }

    pub fn focus_window(&mut self, window: Option<Window>) {
        self.focus_window_with_raise(window, true);
    }

    pub fn focus_window_with_raise(&mut self, window: Option<Window>, raise: bool) {
        if let Some(ref w) = window {
            if Self::window_meta(w).override_redirect {
                return;
            }
            if Self::window_meta(w).minimized {
                let mut meta = Self::window_meta_mut(w);
                meta.minimized = false;
                meta.show_desktop_hidden = false;
            }
        }

        let prev = self.focused_window();
        if prev.as_ref() == window.as_ref() {
            if let Some(ref win) = window {
                if raise {
                    self.raise_window_layered(win, true);
                    self.wake_window_view(win);
                    self.needs_redraw = true;
                }
                self.sync_focused_pointer_constraint();
                return;
            }
        }

        if let Some(ref old) = prev {
            if let Some(old_surf) = Self::window_wl_surface(old) {
                with_pointer_constraint(&old_surf, &self.pointer, |constraint| {
                    if let Some(c) = constraint {
                        if c.is_active() {
                            c.deactivate();
                        }
                    }
                });
            }
            if old.set_activated(false) {
                if let Some(tl) = old.toplevel() {
                    tl.send_pending_configure();
                }
            }
        }

        let serial = SERIAL_COUNTER.next_serial();
        let keyboard = self.keyboard.clone();
        if let Some(win) = window {
            if raise {
                self.raise_window_layered(&win, true);
                self.wake_window_view(&win);
            }
            if win.set_activated(true) {
                if let Some(tl) = win.toplevel() {
                    tl.send_pending_configure();
                }
            }
            if let Some(target) = Self::window_focus_target(&win) {
                keyboard.set_focus(self, Some(target.clone()), serial);
                if !self.ui.is_popup_open() {
                    if let Some(surf) = target.wl_surface() {
                        with_pointer_constraint(&surf, &self.pointer, |constraint| {
                            if let Some(c) = constraint {
                                if !c.is_active() {
                                    c.activate();
                                }
                            }
                        });
                    }
                }
            }
        } else {
            keyboard.set_focus(self, None, serial);
        }
        self.needs_redraw = true;
    }

    pub fn toggle_always_on_top_for(&mut self, win: &Window) {
        let now_pinned = {
            let mut m = Self::window_meta_mut(win);
            m.always_on_top = !m.always_on_top;
            m.always_on_top
        };
        if now_pinned {
            self.focus_window_with_raise(Some(win.clone()), true);
        } else if let Some(focused) = self.focused_window() {
            self.raise_window_layered(&focused, true);
        }
        self.needs_redraw = true;
    }

    pub fn close_window(&self, window: &Window) {
        if let Some(tl) = window.toplevel() {
            tl.send_close();
        } else if let Some(x11) = window.x11_surface() {
            let _ = x11.close();
        }
    }

    pub fn switch_workspace(&mut self, idx: usize) {
        if idx >= NUM_WORKSPACES || idx == self.active_workspace {
            return;
        }
        self.active_workspace = idx;
        let top = self
            .active_space()
            .elements()
            .filter(|w| {
                let m = Self::window_meta(w);
                !m.override_redirect && !m.minimized
            })
            .last()
            .cloned();
        self.focus_window(top);
        self.relayout_active_workspace();
        self.wake_workspace_views();
        self.needs_redraw = true;
    }

    pub fn move_focused_to_workspace(&mut self, idx: usize) {
        if idx >= NUM_WORKSPACES || idx == self.active_workspace {
            return;
        }
        let Some(win) = self.focused_window() else {
            return;
        };
        Self::window_meta_mut(&win).last_configured_rect = None;
        self.active_space_mut().unmap_elem(&win);
        self.relayout_active_workspace();

        self.workspaces[idx].map_element(win.clone(), (32, 48), true);
        self.active_workspace = idx;
        self.relayout_active_workspace();
        self.focus_window(Some(win));
        self.needs_redraw = true;
    }

    pub fn cycle_focus(&mut self) {
        let windows: Vec<Window> = self
            .active_space()
            .elements()
            .filter(|w| {
                let m = Self::window_meta(w);
                !m.override_redirect && !m.minimized
            })
            .cloned()
            .collect();
        if windows.is_empty() {
            return;
        }
        let focused = self.focused_window();
        let next = match focused.and_then(|f| windows.iter().position(|w| w == &f)) {
            Some(pos) => windows[(pos + 1) % windows.len()].clone(),
            None => windows[0].clone(),
        };
        self.focus_window(Some(next));
    }

    pub fn focus_direction(&mut self, dir: Direction) {
        let Some(current) = self.focused_window() else {
            self.cycle_focus();
            return;
        };
        let Some(cur_geo) = self.active_space().element_geometry(&current) else {
            return;
        };
        let cur_cx = cur_geo.loc.x + cur_geo.size.w / 2;
        let cur_cy = cur_geo.loc.y + cur_geo.size.h / 2;

        let mut best: Option<(i64, Window)> = None;
        for win in self.active_space().elements() {
            let m = Self::window_meta(win);
            if win == &current || m.override_redirect || m.minimized {
                continue;
            }
            let Some(geo) = self.active_space().element_geometry(win) else {
                continue;
            };
            let cx = geo.loc.x + geo.size.w / 2;
            let cy = geo.loc.y + geo.size.h / 2;
            let dx = (cx - cur_cx) as i64;
            let dy = (cy - cur_cy) as i64;

            let matches = match dir {
                Direction::Left => dx < 0,
                Direction::Right => dx > 0,
                Direction::Up => dy < 0,
                Direction::Down => dy > 0,
            };
            if !matches {
                continue;
            }
            let dist = dx * dx + dy * dy;
            if best.as_ref().is_none_or(|(b, _)| dist < *b) {
                best = Some((dist, win.clone()));
            }
        }

        if let Some((_, target)) = best {
            self.focus_window(Some(target));
        }
    }

    pub fn swap_window_direction(&mut self, dir: Direction) {
        let Some(current) = self.focused_window() else {
            return;
        };
        let Some(cur_geo) = self.active_space().element_geometry(&current) else {
            return;
        };
        let cur_cx = cur_geo.loc.x + cur_geo.size.w / 2;
        let cur_cy = cur_geo.loc.y + cur_geo.size.h / 2;

        let mut best: Option<(i64, Window)> = None;
        for win in self.active_space().elements() {
            let m = Self::window_meta(win);
            if win == &current || m.override_redirect || m.minimized {
                continue;
            }
            let Some(geo) = self.active_space().element_geometry(win) else {
                continue;
            };
            let cx = geo.loc.x + geo.size.w / 2;
            let cy = geo.loc.y + geo.size.h / 2;
            let dx = (cx - cur_cx) as i64;
            let dy = (cy - cur_cy) as i64;
            let matches = match dir {
                Direction::Left => dx < 0,
                Direction::Right => dx > 0,
                Direction::Up => dy < 0,
                Direction::Down => dy > 0,
            };
            if matches {
                let dist = dx * dx + dy * dy;
                if best.as_ref().is_none_or(|(b, _)| dist < *b) {
                    best = Some((dist, win.clone()));
                }
            }
        }

        if let Some((_, other)) = best {
            let loc_a = self.active_space().element_location(&current).unwrap_or_default();
            let loc_b = self.active_space().element_location(&other).unwrap_or_default();
            Self::window_meta_mut(&current).last_configured_rect = None;
            Self::window_meta_mut(&other).last_configured_rect = None;
            self.active_space_mut()
                .map_element(current.clone(), loc_b, false);
            self.active_space_mut().map_element(other, loc_a, false);
            self.focus_window(Some(current));
            self.needs_redraw = true;
        }
    }

    fn fallback_restore_rect_for(&self, win: &Window) -> Rectangle<i32, Logical> {
        fallback_rect_for_usable(self.usable_rect_for_window(win))
    }

    fn capture_restore_rect_if_needed(&self, win: &Window) {
        let already_expanded_or_saved = {
            let meta = Self::window_meta(win);
            meta.restore_rect.is_some()
                && (meta.maximized || meta.fullscreen || meta.snapped.is_some())
        };
        if already_expanded_or_saved {
            return;
        }
        let usable = self.usable_rect_for_window(win);
        let out_geo = self
            .output_for_window(win)
            .and_then(|o| self.active_space().output_geometry(&o))
            .unwrap_or(usable);
        let loc = self
            .active_space()
            .element_location(win)
            .unwrap_or((usable.loc.x + 32, usable.loc.y + 32).into());
        let size = win.geometry().size;
        if size.w >= 120 && size.h >= 100 {
            // Preserve pre-fullscreen geometry unless the window is already full-output fullscreen.
            if size.w < out_geo.size.w || size.h < out_geo.size.h {
                let clamped = clamp_rect_to_bounds(Rectangle::new(loc, size), usable);
                Self::window_meta_mut(win).restore_rect = Some(clamped);
            }
        }
    }

    pub fn maximize_window_for(&mut self, win: &Window) {
        self.capture_restore_rect_if_needed(win);
        {
            let mut meta = Self::window_meta_mut(win);
            meta.minimized = false;
            meta.show_desktop_hidden = false;
            meta.fullscreen = false;
            meta.maximized = true;
            meta.snapped = Some(SnapZone::Maximize);
            meta.last_configured_rect = None;
        }
        if let Some(tl) = win.toplevel() {
            tl.with_pending_state(|s| {
                s.states.set(xdg_toplevel::State::Maximized);
                s.states.unset(xdg_toplevel::State::Fullscreen);
            });
        }
        if let Some(x11) = win.x11_surface() {
            let _ = x11.set_fullscreen(false);
            let _ = x11.set_maximized(true);
        }
        let usable = self.usable_rect_for_window(win);
        let gap = if self.config.layout_mode == LayoutMode::Floating || Self::window_meta(win).floating {
            0
        } else {
            self.config.gap_px.clamp(0, 32)
        };
        let rect = rect_for_snap_zone(SnapZone::Maximize, usable, gap);
        self.apply_window_bounds_force(win, (rect.loc.x, rect.loc.y), (rect.size.w, rect.size.h));
        self.focus_window(Some(win.clone()));
    }

    pub fn unmaximize_window_for(&mut self, win: &Window) {
        let is_tiled = {
            let mut meta = Self::window_meta_mut(win);
            meta.maximized = false;
            meta.fullscreen = false;
            meta.snapped = None;
            meta.last_configured_rect = None;
            meta.restore_clamp_commits = 4;
            !meta.floating && self.config.auto_tile && self.config.layout_mode != LayoutMode::Floating
        };
        if let Some(tl) = win.toplevel() {
            tl.with_pending_state(|s| {
                s.states.unset(xdg_toplevel::State::Maximized);
                s.states.unset(xdg_toplevel::State::Fullscreen);
            });
        }
        if let Some(x11) = win.x11_surface() {
            let _ = x11.set_maximized(false);
            let _ = x11.set_fullscreen(false);
        }

        if is_tiled {
            self.relayout_active_workspace();
            if Self::window_meta(win).last_configured_rect.is_none() {
                if let Some(tl) = win.toplevel() {
                    tl.send_pending_configure();
                }
            }
        } else {
            let usable = self.usable_rect_for_window(win);
            let rect = Self::window_meta_mut(win)
                .restore_rect
                .take()
                .map(|r| clamp_rect_to_bounds(r, usable))
                .unwrap_or_else(|| self.fallback_restore_rect_for(win));
            self.apply_window_bounds_force(
                win,
                (rect.loc.x, rect.loc.y),
                (rect.size.w, rect.size.h),
            );
        }
        self.focus_window(Some(win.clone()));
        self.needs_redraw = true;
    }

    pub fn toggle_maximize_for(&mut self, win: &Window) {
        let m = Self::window_meta(win);
        let is_expanded = m.maximized || m.fullscreen;
        drop(m);
        if is_expanded {
            self.unmaximize_window_for(win);
        } else {
            self.maximize_window_for(win);
        }
    }

    pub fn minimize_window_for(&mut self, win: &Window) {
        {
            let mut meta = Self::window_meta_mut(win);
            meta.minimized = true;
            meta.show_desktop_hidden = false;
        }
        let next = self
            .active_space()
            .elements()
            .filter(|w| {
                let m = Self::window_meta(w);
                !m.override_redirect && !m.minimized && *w != win
            })
            .last()
            .cloned();
        self.focus_window(next);
        self.relayout_active_workspace();
        self.wake_workspace_views();
        self.needs_redraw = true;
    }

    pub fn unminimize_window_for(&mut self, win: &Window) {
        {
            let mut meta = Self::window_meta_mut(win);
            meta.minimized = false;
            meta.show_desktop_hidden = false;
            meta.last_configured_rect = None;
        }
        self.relayout_active_workspace();
        self.focus_window(Some(win.clone()));
        self.wake_window_view(win);
        self.needs_redraw = true;
    }

    pub fn shrink_or_minimize_for(&mut self, win: &Window) {
        let (fs, max, snapped) = {
            let m = Self::window_meta(win);
            (m.fullscreen, m.maximized, m.snapped)
        };
        if fs {
            self.set_fullscreen_for(win, false);
        } else if max || snapped.is_some() {
            self.unmaximize_window_for(win);
        } else {
            self.minimize_window_for(win);
        }
    }

    pub fn expand_or_maximize_for(&mut self, win: &Window) {
        let fs = Self::window_meta(win).fullscreen;
        if fs {
            return;
        }
        self.maximize_window_for(win);
    }

    pub fn apply_snap_zone_for(
        &mut self,
        win: &Window,
        zone: SnapZone,
        snap_rect: Rectangle<i32, Logical>,
    ) {
        if zone == SnapZone::Maximize {
            self.maximize_window_for(win);
            return;
        }
        self.capture_restore_rect_if_needed(win);
        {
            let mut meta = Self::window_meta_mut(win);
            meta.floating = true;
            meta.fullscreen = false;
            meta.maximized = false;
            meta.minimized = false;
            meta.snapped = Some(zone);
            meta.last_configured_rect = None;
        }
        if let Some(tl) = win.toplevel() {
            tl.with_pending_state(|s| {
                s.states.unset(xdg_toplevel::State::Maximized);
                s.states.unset(xdg_toplevel::State::Fullscreen);
            });
        }
        self.apply_window_bounds_force(
            win,
            (snap_rect.loc.x, snap_rect.loc.y),
            (snap_rect.size.w, snap_rect.size.h),
        );
        self.focus_window(Some(win.clone()));
    }

    pub fn snap_direction_for(&mut self, win: &Window, zone: SnapZone) {
        let cur_snap = Self::window_meta(win).snapped;
        // Windows Snap Assist behavior: pressing Left while Right-snapped shrinks back to center!
        if (zone == SnapZone::LeftHalf && cur_snap == Some(SnapZone::RightHalf))
            || (zone == SnapZone::RightHalf && cur_snap == Some(SnapZone::LeftHalf))
        {
            self.unmaximize_window_for(win);
            return;
        }
        let rect = rect_for_snap_zone(
            zone,
            self.usable_rect_for_window(win),
            self.config.gap_px.clamp(0, 32),
        );
        self.apply_snap_zone_for(win, zone, rect);
    }

    pub fn move_focused_to_next_monitor(&mut self) {
        if self.outputs.len() < 2 {
            return;
        }
        let Some(win) = self.focused_window() else {
            return;
        };
        let cur_out = self.output_for_window(&win);
        let next_pos = cur_out
            .and_then(|co| self.outputs.iter().position(|o| o == &co))
            .map(|i| (i + 1) % self.outputs.len())
            .unwrap_or(0);
        let target_out = self.outputs[next_pos].clone();
        let target_usable = self.usable_rect_for_output(&target_out);
        let cur_size = win.geometry().size;
        let w = cur_size.w.min(target_usable.size.w - 32).max(200);
        let h = cur_size.h.min(target_usable.size.h - 32).max(160);
        let x = target_usable.loc.x + (target_usable.size.w - w) / 2;
        let y = target_usable.loc.y + (target_usable.size.h - h) / 2;
        Self::window_meta_mut(&win).last_configured_rect = None;
        self.apply_window_bounds_force(&win, (x, y), (w, h));
        self.relayout_active_workspace();
        self.focus_window(Some(win));
    }

    /// Prepares a window for interactive mouse drag: if currently maximized/fullscreen, shrinks to
    /// restore size centered under the pointer cursor for natural drag-to-unmaximize behavior.
    pub fn prepare_move_grab(&mut self, win: &Window, pointer_pos: Point<f64, Logical>) {
        let (was_max, was_fs) = {
            let meta = Self::window_meta(win);
            (meta.maximized || meta.snapped == Some(SnapZone::Maximize), meta.fullscreen)
        };
        if was_max || was_fs {
            let usable = self.usable_rect_for_window(win);
            let restored = Self::window_meta_mut(win)
                .restore_rect
                .take()
                .map(|r| clamp_rect_to_bounds(r, usable))
                .unwrap_or_else(|| self.fallback_restore_rect_for(win));
            let target_rect = clamp_rect_to_bounds(
                Rectangle::new(
                    (
                        pointer_pos.x as i32 - restored.size.w / 2,
                        pointer_pos.y as i32 - 16,
                    )
                        .into(),
                    restored.size,
                ),
                usable,
            );
            {
                let mut meta = Self::window_meta_mut(win);
                meta.floating = true;
                meta.maximized = false;
                meta.fullscreen = false;
                meta.snapped = None;
                meta.last_configured_rect = None;
                meta.restore_clamp_commits = 4;
            }
            if let Some(tl) = win.toplevel() {
                tl.with_pending_state(|s| {
                    s.states.unset(xdg_toplevel::State::Maximized);
                    s.states.unset(xdg_toplevel::State::Fullscreen);
                });
            }
            if let Some(x11) = win.x11_surface() {
                let _ = x11.set_maximized(false);
                let _ = x11.set_fullscreen(false);
            }
            self.apply_window_bounds_force(
                win,
                (target_rect.loc.x, target_rect.loc.y),
                (target_rect.size.w, target_rect.size.h),
            );
        } else {
            let mut meta = Self::window_meta_mut(win);
            meta.floating = true;
            meta.maximized = false;
            meta.snapped = None;
        }
    }

    pub fn toggle_show_desktop(&mut self) {
        let any_visible = self.active_space().elements().any(|w| {
            let m = Self::window_meta(w);
            !m.override_redirect && !m.minimized
        });
        if any_visible {
            let wins: Vec<Window> = self
                .active_space()
                .elements()
                .filter(|w| !Self::window_meta(w).override_redirect && !Self::window_meta(w).minimized)
                .cloned()
                .collect();
            for win in wins {
                let mut meta = Self::window_meta_mut(&win);
                meta.minimized = true;
                meta.show_desktop_hidden = true;
            }
            self.focus_window(None);
        } else {
            let wins: Vec<Window> = self
                .active_space()
                .elements()
                .filter(|w| !Self::window_meta(w).override_redirect && Self::window_meta(w).show_desktop_hidden)
                .cloned()
                .collect();
            for win in &wins {
                let mut meta = Self::window_meta_mut(win);
                meta.minimized = false;
                meta.show_desktop_hidden = false;
                meta.last_configured_rect = None;
            }
            self.relayout_active_workspace();
            self.wake_workspace_views();
            if let Some(last) = wins.last().cloned() {
                self.focus_window(Some(last));
            }
        }
        self.needs_redraw = true;
    }

    pub fn toggle_floating_for(&mut self, win: &Window) {
        let (now_floating, was_expanded) = {
            let mut meta = Self::window_meta_mut(win);
            let was_expanded = meta.fullscreen || meta.maximized || meta.snapped.is_some();
            meta.floating = !meta.floating;
            meta.fullscreen = false;
            meta.maximized = false;
            meta.snapped = None;
            meta.last_configured_rect = None;
            meta.restore_clamp_commits = 4;
            (meta.floating, was_expanded)
        };
        if let Some(tl) = win.toplevel() {
            tl.with_pending_state(|s| {
                s.states.unset(xdg_toplevel::State::Maximized);
                s.states.unset(xdg_toplevel::State::Fullscreen);
            });
        }
        if let Some(x11) = win.x11_surface() {
            let _ = x11.set_maximized(false);
            let _ = x11.set_fullscreen(false);
        }
        if now_floating && was_expanded {
            let usable = self.usable_rect_for_window(win);
            let rect = Self::window_meta_mut(win)
                .restore_rect
                .take()
                .map(|r| clamp_rect_to_bounds(r, usable))
                .unwrap_or_else(|| self.fallback_restore_rect_for(win));
            self.apply_window_bounds_force(
                win,
                (rect.loc.x, rect.loc.y),
                (rect.size.w, rect.size.h),
            );
        }
        self.relayout_active_workspace();
        if Self::window_meta(win).last_configured_rect.is_none() {
            if let Some(tl) = win.toplevel() {
                tl.send_pending_configure();
            }
        }
        self.wake_workspace_views();
        self.needs_redraw = true;
    }

    pub fn toggle_floating(&mut self) {
        if let Some(win) = self.focused_window() {
            self.toggle_floating_for(&win);
        }
    }

    pub fn set_fullscreen_for(&mut self, win: &Window, now_fs: bool) {
        let (was_fs, was_max, was_snapped, is_floating_mode) = {
            let m = Self::window_meta(win);
            (
                m.fullscreen,
                m.maximized,
                m.snapped,
                m.floating || self.config.layout_mode == LayoutMode::Floating || !self.config.auto_tile,
            )
        };
        if was_fs == now_fs {
            return;
        }
        if now_fs {
            self.capture_restore_rect_if_needed(win);
            let mut meta = Self::window_meta_mut(win);
            meta.minimized = false;
            meta.show_desktop_hidden = false;
            meta.fullscreen = true;
            meta.minimal_view = false;
            meta.minimal_slot_size = (0, 0).into();
            meta.last_configured_rect = None;
            meta.restore_clamp_commits = 0;
            meta.unfullscreen_hold_until = None;
        } else {
            let mut meta = Self::window_meta_mut(win);
            meta.fullscreen = false;
            meta.last_configured_rect = None;
            meta.restore_clamp_commits = 6;
            meta.unfullscreen_hold_until = Some(Instant::now() + Duration::from_millis(160));
        }
        if let Some(tl) = win.toplevel() {
            tl.with_pending_state(|s| {
                if now_fs {
                    s.states.set(xdg_toplevel::State::Fullscreen);
                    s.states.unset(xdg_toplevel::State::Maximized);
                } else {
                    s.states.unset(xdg_toplevel::State::Fullscreen);
                    if was_max {
                        s.states.set(xdg_toplevel::State::Maximized);
                    } else {
                        s.states.unset(xdg_toplevel::State::Maximized);
                    }
                }
            });
        }
        if let Some(x11) = win.x11_surface() {
            let _ = x11.set_fullscreen(now_fs);
            if !now_fs && was_max {
                let _ = x11.set_maximized(true);
            }
        }
        if !now_fs {
            if was_max {
                // relayout_active_workspace restores maximized usable bounds while keeping restore_rect intact.
            } else if let Some(zone) = was_snapped {
                let usable = self.usable_rect_for_window(win);
                let gap = self.config.gap_px.clamp(0, 32);
                let rect = clamp_rect_to_bounds(rect_for_snap_zone(zone, usable, gap), usable);
                self.apply_window_bounds_force(
                    win,
                    (rect.loc.x, rect.loc.y),
                    (rect.size.w, rect.size.h),
                );
            } else if is_floating_mode {
                let usable = self.usable_rect_for_window(win);
                let rect = Self::window_meta_mut(win)
                    .restore_rect
                    .take()
                    .map(|r| clamp_rect_to_bounds(r, usable))
                    .unwrap_or_else(|| self.fallback_restore_rect_for(win));
                self.apply_window_bounds_force(
                    win,
                    (rect.loc.x, rect.loc.y),
                    (rect.size.w, rect.size.h),
                );
            }
        }
        self.relayout_active_workspace();
        if Self::window_meta(win).last_configured_rect.is_none() {
            if let Some(tl) = win.toplevel() {
                tl.send_pending_configure();
            }
        }
        if !now_fs {
            self.wake_workspace_views();
        }
        self.needs_redraw = true;
    }

    pub fn toggle_fullscreen_for(&mut self, win: &Window) {
        let cur = Self::window_meta(win).fullscreen;
        self.set_fullscreen_for(win, !cur);
    }

    pub fn toggle_fullscreen(&mut self) {
        if let Some(win) = self.focused_window() {
            self.toggle_fullscreen_for(&win);
        }
    }

    pub fn apply_window_bounds_force(&mut self, window: &Window, loc: (i32, i32), size: (i32, i32)) {
        Self::window_meta_mut(window).last_configured_rect = None;
        self.apply_window_bounds(window, loc, size);
    }

    fn apply_window_bounds_in_ws(
        &mut self,
        ws_idx: usize,
        window: &Window,
        loc: (i32, i32),
        size: (i32, i32),
    ) {
        let target_rect = Rectangle::new(loc.into(), size.into());
        {
            let mut meta = Self::window_meta_mut(window);
            if meta.last_configured_rect == Some(target_rect) {
                return;
            }
            meta.last_configured_rect = Some(target_rect);
        }

        if let Some(tl) = window.toplevel() {
            tl.with_pending_state(|s| {
                s.size = Some(size.into());
            });
            tl.send_pending_configure();
        }
        if let Some(x11) = window.x11_surface() {
            let _ = x11.configure(target_rect);
        }
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.map_element(window.clone(), loc, false);
        }
        self.needs_redraw = true;
    }

    fn apply_tiled_slot_in_ws(
        &mut self,
        ws_idx: usize,
        win: &Window,
        loc: (i32, i32),
        size: (i32, i32),
    ) {
        let min_size = Self::window_min_size(win);
        if self.config.minimal_view_enabled
            && Self::should_use_minimal_view(size.into(), min_size)
        {
            {
                let mut meta = Self::window_meta_mut(win);
                meta.minimal_view = true;
                meta.minimal_slot_size = size.into();
            }
            if let Some(ws) = self.workspaces.get_mut(ws_idx) {
                ws.map_element(win.clone(), loc, false);
            }
            self.needs_redraw = true;
            return;
        }
        {
            let mut meta = Self::window_meta_mut(win);
            if meta.minimal_view && meta.minimal_slot_size.w > 0 && meta.minimal_slot_size.h > 0 {
                // Keep manual toggle only while slot itself was not expanded, otherwise clear.
                meta.minimal_view = false;
                meta.minimal_slot_size = (0, 0).into();
                meta.minimal_card_sig.clear();
                meta.last_configured_rect = None;
            }
        }
        self.apply_window_bounds_in_ws(ws_idx, win, loc, size);
    }

    fn apply_window_bounds(&mut self, window: &Window, loc: (i32, i32), size: (i32, i32)) {
        self.apply_window_bounds_in_ws(self.active_workspace, window, loc, size);
    }

    pub fn relayout_active_workspace(&mut self) {
        self.relayout_workspace(self.active_workspace);
    }

    pub fn relayout_workspace(&mut self, ws_idx: usize) {
        let outputs = self.outputs.clone();
        if outputs.is_empty() || ws_idx >= NUM_WORKSPACES {
            return;
        }

        let gap = self.config.gap_px.clamp(0, 48);
        let layout_mode = self.config.layout_mode;
        let master_ratio = self.config.master_ratio.clamp(0.35, 0.80);

        // Sort deterministically by creation/explicit layout order so raising windows or
        // toggling fullscreen never shuffles Master/Stack slots into random corners.
        let mut indexed: Vec<(usize, u64, usize, Window)> = self.workspaces[ws_idx]
            .elements()
            .filter(|w| {
                let m = Self::window_meta(w);
                !m.override_redirect && !m.minimized
            })
            .enumerate()
            .map(|(idx, w)| {
                let m = Self::window_meta(w);
                (
                    m.order_hint.unwrap_or(usize::MAX),
                    m.taskbar_order,
                    idx,
                    w.clone(),
                )
            })
            .collect();
        indexed.sort_by_key(|(hint, seq, idx, _)| (*hint, *seq, *idx));
        let all_windows: Vec<Window> = indexed.into_iter().map(|(_, _, _, w)| w).collect();

        for output in &outputs {
            let Some(out_geo) = self.workspaces[ws_idx].output_geometry(output) else {
                continue;
            };
            let usable = self.usable_rect_for_output(output);
            let windows: Vec<Window> = all_windows
                .iter()
                .filter(|w| self.output_for_window_in_ws(ws_idx, w).as_ref() == Some(output))
                .cloned()
                .collect();

            for win in &windows {
                let m = Self::window_meta(win);
                if m.fullscreen {
                    drop(m);
                    self.apply_window_bounds_in_ws(
                        ws_idx,
                        win,
                        (out_geo.loc.x, out_geo.loc.y),
                        (out_geo.size.w, out_geo.size.h),
                    );
                } else if m.maximized {
                    drop(m);
                    self.apply_window_bounds_in_ws(
                        ws_idx,
                        win,
                        (usable.loc.x, usable.loc.y),
                        (usable.size.w, usable.size.h),
                    );
                } else if m.floating && !m.minimal_view {
                    // Ensure restored floating windows remain within output usable bounds.
                    let loc = self.workspaces[ws_idx].element_location(win);
                    let geom = win.geometry();
                    if let (Some(loc), true) = (loc, geom.size.w > 0 && geom.size.h > 0) {
                        let rect = Rectangle::new(loc, geom.size);
                        let clamped = clamp_rect_to_bounds(rect, usable);
                        if clamped.loc != loc && m.unfullscreen_hold_until.is_none() {
                            drop(m);
                            if let Some(ws) = self.workspaces.get_mut(ws_idx) {
                                ws.map_element(win.clone(), clamped.loc, false);
                            }
                        }
                    }
                }
            }

            if layout_mode == LayoutMode::Floating || !self.config.auto_tile {
                continue;
            }

            let tiled: Vec<Window> = windows
                .into_iter()
                .filter(|w| {
                    let m = Self::window_meta(w);
                    !m.floating && !m.fullscreen && !m.maximized && m.snapped.is_none()
                })
                .collect();

            let n = tiled.len() as i32;
            if n == 0 {
                continue;
            }

            let area_x = usable.loc.x + gap;
            let area_y = usable.loc.y + gap;
            let area_w = (usable.size.w - gap * 2).max(128);
            let area_h = (usable.size.h - gap * 2).max(128);

            if n == 1 {
                self.apply_tiled_slot_in_ws(ws_idx, &tiled[0], (area_x, area_y), (area_w, area_h));
                continue;
            }

            if layout_mode == LayoutMode::Columns {
                let total_gaps = gap * (n - 1);
                let each_w = ((area_w - total_gaps) / n).max(80);
                for (i, win) in tiled.iter().enumerate() {
                    let i = i as i32;
                    let x = area_x + i * (each_w + gap);
                    let w = if i == n - 1 {
                        (area_x + area_w - x).max(80)
                    } else {
                        each_w
                    };
                    self.apply_tiled_slot_in_ws(ws_idx, win, (x, area_y), (w, area_h));
                }
                continue;
            }

            // Master-Stack
            let master_w = ((area_w - gap) as f32 * master_ratio).round() as i32;
            let stack_w = (area_w - gap - master_w).max(64);
            self.apply_tiled_slot_in_ws(ws_idx, &tiled[0], (area_x, area_y), (master_w, area_h));

            let stack_count = n - 1;
            let total_stack_gaps = gap * (stack_count - 1);
            let each_h = ((area_h - total_stack_gaps) / stack_count).max(64);
            let stack_x = area_x + master_w + gap;

            for (i, win) in tiled.iter().skip(1).enumerate() {
                let i = i as i32;
                let y = area_y + i * (each_h + gap);
                let h = if i == stack_count - 1 {
                    (area_y + area_h - y).max(64)
                } else {
                    each_h
                };
                self.apply_tiled_slot_in_ws(ws_idx, win, (stack_x, y), (stack_w, h));
            }
        }
    }

    pub fn remove_window_everywhere(&mut self, surface: &WlSurface) {
        let mut removed_from_active = false;
        for (idx, ws) in self.workspaces.iter_mut().enumerate() {
            let found = ws
                .elements()
                .find(|w| Self::window_wl_surface(w).as_ref() == Some(surface))
                .cloned();
            if let Some(win) = found {
                ws.unmap_elem(&win);
                if idx == self.active_workspace {
                    removed_from_active = true;
                }
            }
        }
        if removed_from_active {
            let next = self
                .active_space()
                .elements()
                .filter(|w| {
                    let m = Self::window_meta(w);
                    !m.override_redirect && !m.minimized
                })
                .last()
                .cloned();
            self.focus_window(next);
            self.relayout_active_workspace();
            self.needs_redraw = true;
        }
        trim_process_heaps();
    }

    pub fn remove_window_elem(&mut self, win: &Window) {
        let mut removed_from_active = false;
        for (idx, ws) in self.workspaces.iter_mut().enumerate() {
            if ws.elements().any(|w| w == win) {
                ws.unmap_elem(win);
                if idx == self.active_workspace {
                    removed_from_active = true;
                }
            }
        }
        if removed_from_active {
            let next = self
                .active_space()
                .elements()
                .filter(|w| {
                    let m = Self::window_meta(w);
                    !m.override_redirect && !m.minimized
                })
                .last()
                .cloned();
            self.focus_window(next);
            self.relayout_active_workspace();
            self.needs_redraw = true;
        }
        trim_process_heaps();
    }

    pub fn root_wl_surface(surface: &WlSurface) -> WlSurface {
        let mut cur = surface.clone();
        while let Some(parent) = get_parent(&cur) {
            cur = parent;
        }
        cur
    }

    pub fn find_window_location(&self, surface: &WlSurface) -> Option<(usize, Window)> {
        let root = Self::root_wl_surface(surface);
        for (idx, ws) in self.workspaces.iter().enumerate() {
            if let Some(w) = ws
                .elements()
                .find(|w| Self::window_wl_surface(w).as_ref() == Some(&root))
            {
                return Some((idx, w.clone()));
            }
        }
        None
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.find_window_location(surface).map(|(_, w)| w)
    }

    pub fn wake_window_view(&mut self, win: &Window) {
        if let Some(past) = Instant::now().checked_sub(Duration::from_secs(2)) {
            Self::window_meta_mut(win).last_bg_frame_at = past;
        }
        self.last_visible_commit_at = Instant::now();
        self.needs_redraw = true;
    }

    pub fn wake_workspace_views(&mut self) {
        if let Some(past) = Instant::now().checked_sub(Duration::from_secs(2)) {
            for w in self.active_space().elements() {
                Self::window_meta_mut(w).last_bg_frame_at = past;
            }
        }
        self.last_visible_commit_at = Instant::now();
        self.needs_redraw = true;
    }

    /// Returns true if `window` actually needs view updates on `output`:
    /// false when minimized, Show-Desktop hidden, off-monitor, or behind a fullscreen window.
    pub fn is_window_visible_on_output(&self, output: &Output, window: &Window) -> bool {
        let meta = Self::window_meta(window);
        if meta.minimized || meta.show_desktop_hidden {
            return false;
        }
        let is_or = meta.override_redirect;
        drop(meta);

        let Some(out_geo) = self.active_space().output_geometry(output) else {
            return true;
        };
        let win_rect = self
            .active_space()
            .element_geometry(window)
            .unwrap_or_else(|| Rectangle::new((0, 0).into(), (0, 0).into()));
        if win_rect.size.w <= 0 || win_rect.size.h <= 0 {
            return true;
        }
        let is_moving = self.pointer.is_grabbed() || self.animator.any_active();
        if !is_moving && !win_rect.overlaps(out_geo) {
            return false;
        }

        if !is_or {
            for upper in self.active_space().elements().rev() {
                if upper == window {
                    break;
                }
                let um = Self::window_meta(upper);
                if um.minimized || um.show_desktop_hidden || um.override_redirect || !um.fullscreen
                {
                    continue;
                }
                drop(um);
                if let Some(u_rect) = self.active_space().element_geometry(upper) {
                    if u_rect.overlaps(out_geo)
                        && u_rect.loc.x <= win_rect.loc.x
                        && u_rect.loc.y <= win_rect.loc.y
                        && u_rect.loc.x + u_rect.size.w >= win_rect.loc.x + win_rect.size.w
                        && u_rect.loc.y + u_rect.size.h >= win_rect.loc.y + win_rect.size.h
                    {
                        return false;
                    }
                }
            }
        }
        true
    }

    pub fn is_window_visible_anywhere(&self, ws_idx: usize, window: &Window) -> bool {
        if ws_idx != self.active_workspace {
            return false;
        }
        if !self.active_space().elements().any(|w| w == window) {
            return false;
        }
        if self.outputs.is_empty() {
            let meta = Self::window_meta(window);
            return !meta.minimized && !meta.show_desktop_hidden;
        }
        self.outputs
            .iter()
            .any(|out| self.is_window_visible_on_output(out, window))
    }

    /// Keeps background/minimized/off-workspace/occluded apps alive (~1Hz keepalive callback)
    /// while their visual view remains frozen at 0 redraw cost.
    pub fn send_hidden_window_keepalives(&mut self, elapsed: Duration) {
        if !self.config.freeze_hidden_windows {
            return;
        }
        let Some(out) = self.outputs.first().cloned() else {
            return;
        };
        let mut to_notify = Vec::new();
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            for w in ws.elements() {
                if !self.is_window_visible_anywhere(ws_idx, w)
                    && Self::window_meta(w).last_bg_frame_at.elapsed() >= Duration::from_secs(1)
                {
                    to_notify.push(w.clone());
                }
            }
        }
        let now = Instant::now();
        for w in to_notify {
            Self::window_meta_mut(&w).last_bg_frame_at = now;
            w.send_frame(&out, elapsed, Some(Duration::from_millis(1000)), |_, _| {
                Some(out.clone())
            });
        }
    }

    pub fn launch_app_by_index(&mut self, idx: usize) {
        if let Some(app) = self.apps.get(idx) {
            app.launch(
                &self.config.terminal_cmd,
                self.wayland_display.as_deref(),
                self.x11_display.as_deref(),
            );
            self.ui.close_popup();
            self.needs_redraw = true;
        }
    }

    fn save_config_with_feedback(&mut self) {
        self.notif_svc.apply_config(&self.config);
        self.notif_snap = self.notif_svc.snapshot();
        if self.config.save() {
            self.ignore_reload_mtime = Config::config_modified_nanos();
            self.ui.notify_saved();
        }
    }

    pub fn apply_slider_value(&mut self, target: &SliderTarget, val: i32) {
        match target {
            SliderTarget::MasterSink { name } => {
                let pct = val.clamp(0, 150) as u8;
                if name == "@DEFAULT_SINK@" || *name == self.sys_snap.default_sink {
                    self.config.saved_volume_pct = Some(pct);
                    self.hardware_dirty = true;
                }
                self.sys.send(SysCommand::SetSinkVolume {
                    name: name.clone(),
                    pct,
                });
                self.refresh_system_snapshot();
            }
            SliderTarget::MasterSource { name } => {
                let pct = val.clamp(0, 150) as u8;
                if name == "@DEFAULT_SOURCE@" || *name == self.sys_snap.default_source {
                    self.config.saved_mic_volume_pct = Some(pct);
                    self.hardware_dirty = true;
                }
                self.sys.send(SysCommand::SetSourceVolume {
                    name: name.clone(),
                    pct,
                });
                self.refresh_system_snapshot();
            }
            SliderTarget::Stream { is_input, index } => {
                self.sys.send(SysCommand::SetStreamVolume {
                    is_input: *is_input,
                    index: *index,
                    pct: val.clamp(0, 150) as u8,
                });
                self.refresh_system_snapshot();
            }
            SliderTarget::Brightness => {
                let pct = val.clamp(5, 100) as u8;
                self.config.saved_brightness_pct = Some(pct);
                self.hardware_dirty = true;
                self.sys.send(SysCommand::SetBrightness { pct });
                self.refresh_system_snapshot();
            }
            SliderTarget::PanelOpacity => {
                self.config.panel_opacity = val.clamp(120, 250) as u8;
                self.save_config_with_feedback();
            }
            SliderTarget::CornerRadius => {
                self.config.corner_radius = val.clamp(0, 18);
                self.save_config_with_feedback();
            }
            SliderTarget::GapPx => {
                self.config.gap_px = val.clamp(0, 32);
                self.save_config_with_feedback();
                self.relayout_active_workspace();
            }
            SliderTarget::MasterRatio => {
                self.config.master_ratio = (val.clamp(40, 75) as f32) / 100.0;
                self.save_config_with_feedback();
                self.relayout_active_workspace();
            }
        }
        self.needs_redraw = true;
    }

    fn active_ws_window_at(&self, idx: usize) -> Option<Window> {
        self.active_ws_ordered_windows().into_iter().nth(idx)
    }

    fn global_ws_window_at(&self, ws: usize, idx_in_ws: usize) -> Option<Window> {
        self.workspaces
            .get(ws)?
            .elements()
            .filter(|w| !Self::window_meta(w).override_redirect)
            .nth(idx_in_ws)
            .cloned()
    }

    pub fn launch_system_settings(&mut self) {
        self.ui.close_popup();
        let cmd = self.config.settings_cmd.clone();
        if !cmd.is_empty() && executable_exists(&cmd) {
            spawn_detached(&cmd);
        } else if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let sibling = dir.join("velox-settings");
                if sibling.exists() {
                    spawn_detached(&sibling.to_string_lossy());
                    self.needs_redraw = true;
                    return;
                }
            }
            self.sys.send(SysCommand::RefreshNow);
            self.ui.toggle_settings();
        } else {
            self.sys.send(SysCommand::RefreshNow);
            self.ui.toggle_settings();
        }
        self.needs_redraw = true;
    }

    fn apply_global_and_primary_refresh(&mut self, hz: u32) {
        self.config.output_refresh_hz = hz;
        if let Some(primary_name) = self
            .display_telemetry
            .outputs
            .iter()
            .find(|o| o.primary)
            .or_else(|| self.display_telemetry.outputs.first())
            .map(|o| o.name.clone())
        {
            if let Some(mon) = self.config.monitors.iter_mut().find(|m| m.name == primary_name) {
                mon.refresh_hz = hz;
            }
        }
        self.display_config_dirty = true;
        self.save_config_with_feedback();
    }

    pub fn execute_ui_action(&mut self, action: UiAction) {
        match action {
            UiAction::ToggleStartMenu => self.ui.toggle_start_menu(),
            UiAction::ToggleAudioMixer => {
                self.sys.send(SysCommand::RefreshNow);
                self.ui.toggle_audio_mixer();
            }
            UiAction::ToggleBrightnessPopup => {
                self.sys.send(SysCommand::RefreshNow);
                self.ui.toggle_brightness_popup();
            }
            UiAction::ToggleNetworkPopup => {
                self.sys.send(SysCommand::RefreshNow);
                self.ui.toggle_network_popup();
            }
            UiAction::ToggleQuickSettings => {
                self.sys.send(SysCommand::RefreshNow);
                self.ui.toggle_quick_settings();
            }
            UiAction::ToggleCalendar => self.ui.toggle_calendar(),
            UiAction::CalendarStepMonth(delta) => {
                self.ui.calendar_month_offset = (self.ui.calendar_month_offset + delta).clamp(-120, 120);
            }
            UiAction::CalendarResetToday => {
                self.ui.calendar_month_offset = 0;
            }
            UiAction::FocusTimerToggle => {
                self.ui.toggle_focus_timer();
            }
            UiAction::FocusTimerReset(mins) => {
                self.ui.reset_focus_timer(mins);
            }
            UiAction::ToggleTaskView => {
                self.ui.toggle_task_view();
            }
            UiAction::LaunchSystemSettings => {
                self.launch_system_settings();
            }
            UiAction::ToggleActiveApps => {
                self.sys.send(SysCommand::RefreshNow);
                self.ui.toggle_active_apps();
            }
            UiAction::ToggleSettings => {
                self.sys.send(SysCommand::RefreshNow);
                self.ui.toggle_settings();
            }
            UiAction::OpenSettingsTab(tab) => {
                self.sys.send(SysCommand::RefreshNow);
                self.ui.settings_tab = tab;
                if self.ui.active_popup != ActivePopup::ControlCenter {
                    self.ui.open_popup(ActivePopup::ControlCenter);
                }
            }
            UiAction::TogglePowerMenu => self.ui.toggle_power_menu(),
            UiAction::OpenContextMenu { target, pos } => {
                self.ui.open_context_menu(target, pos);
            }
            UiAction::ToggleContextSubmenu(sub) => {
                self.ui.toggle_context_submenu(sub);
            }
            UiAction::ClosePopup => {
                self.ui.close_popup();
                self.sync_focused_pointer_constraint();
            }
            UiAction::SwitchWorkspace(idx) => {
                self.ui.close_popup();
                self.switch_workspace(idx);
            }
            UiAction::FocusWindow(idx) => {
                self.ui.close_popup();
                if let Some(win) = self.active_ws_window_at(idx) {
                    if Self::window_meta(&win).minimized {
                        self.unminimize_window_for(&win);
                    } else {
                        self.focus_window(Some(win));
                    }
                }
            }
            UiAction::MinimizeWindow(idx) => {
                self.ui.close_popup();
                if let Some(win) = self.active_ws_window_at(idx) {
                    if Self::window_meta(&win).minimized {
                        self.unminimize_window_for(&win);
                    } else {
                        self.minimize_window_for(&win);
                    }
                }
            }
            UiAction::ToggleMaximizeWindow(idx) => {
                self.ui.close_popup();
                if let Some(win) = self.active_ws_window_at(idx) {
                    self.toggle_maximize_for(&win);
                }
            }
            UiAction::ShowDesktop => {
                self.ui.close_popup();
                self.toggle_show_desktop();
            }
            UiAction::CloseWindow(idx) => {
                self.ui.close_popup();
                if let Some(win) = self.active_ws_window_at(idx) {
                    self.close_window(&win);
                }
            }
            UiAction::OpenNewInstanceWindow(idx) => {
                self.ui.close_popup();
                if let Some(win) = self.active_ws_window_at(idx) {
                    let _ = self.open_new_instance_for_window(&win);
                }
                self.sync_focused_pointer_constraint();
            }
            UiAction::TerminalCopyPrimary => {
                self.ui.close_popup();
                self.last_terminal_right_click = None;
                self.terminal_copy_primary();
                self.sync_focused_pointer_constraint();
            }
            UiAction::TerminalPasteClipboard => {
                self.ui.close_popup();
                self.terminal_paste_clipboard();
                self.sync_focused_pointer_constraint();
            }
            UiAction::TerminalPasteSelection => {
                self.ui.close_popup();
                self.terminal_paste_selection();
                self.sync_focused_pointer_constraint();
            }
            UiAction::OpenNewTerminalForFocused => {
                self.ui.close_popup();
                if let Some((win, _)) = self.last_terminal_right_click.take() {
                    let _ = self.open_new_instance_for_window(&win);
                } else {
                    spawn_detached(&self.config.terminal_cmd);
                }
                self.sync_focused_pointer_constraint();
            }
            UiAction::CloseFocusedTerminal => {
                self.ui.close_popup();
                if let Some((win, _)) = self.last_terminal_right_click.take() {
                    self.close_window(&win);
                } else if let Some(win) = self.focused_window() {
                    self.close_window(&win);
                }
            }
            UiAction::SnapWindow(idx, zone) => {
                self.ui.close_popup();
                if let Some(win) = self.active_ws_window_at(idx) {
                    let rect = rect_for_snap_zone(
                        zone,
                        self.usable_workspace_rect(),
                        self.config.gap_px.clamp(0, 32),
                    );
                    self.apply_snap_zone_for(&win, zone, rect);
                }
            }
            UiAction::ToggleFloatWindow(idx) => {
                self.ui.close_popup();
                if let Some(win) = self.active_ws_window_at(idx) {
                    self.toggle_floating_for(&win);
                }
            }
            UiAction::ToggleAlwaysOnTopWindow(idx) => {
                self.ui.close_popup();
                if let Some(win) = self.active_ws_window_at(idx) {
                    self.toggle_always_on_top_for(&win);
                }
            }
            UiAction::ToggleFullscreenWindow(idx) => {
                self.ui.close_popup();
                if let Some(win) = self.active_ws_window_at(idx) {
                    self.toggle_fullscreen_for(&win);
                }
            }
            UiAction::MoveWindowNextWs(idx) => {
                self.ui.close_popup();
                if let Some(win) = self.active_ws_window_at(idx) {
                    let next_ws = (self.active_workspace + 1) % NUM_WORKSPACES;
                    Self::window_meta_mut(&win).last_configured_rect = None;
                    Self::window_meta_mut(&win).minimized = false;
                    self.active_space_mut().unmap_elem(&win);
                    self.workspaces[next_ws].map_element(win.clone(), (32, 48), true);
                    self.switch_workspace(next_ws);
                    self.focus_window(Some(win));
                }
            }
            UiAction::ToggleMaximizeGlobal { ws, idx_in_ws } => {
                if let Some(win) = self.global_ws_window_at(ws, idx_in_ws) {
                    self.switch_workspace(ws);
                    self.toggle_maximize_for(&win);
                }
            }
            UiAction::SnapGlobal { ws, idx_in_ws, zone } => {
                if let Some(win) = self.global_ws_window_at(ws, idx_in_ws) {
                    self.ui.close_popup();
                    self.switch_workspace(ws);
                    let rect = rect_for_snap_zone(
                        zone,
                        self.usable_workspace_rect(),
                        self.config.gap_px.clamp(0, 32),
                    );
                    self.apply_snap_zone_for(&win, zone, rect);
                }
            }
            UiAction::MinimizeGlobal { ws, idx_in_ws } => {
                if let Some(win) = self.global_ws_window_at(ws, idx_in_ws) {
                    if Self::window_meta(&win).minimized {
                        self.switch_workspace(ws);
                        self.unminimize_window_for(&win);
                    } else {
                        self.switch_workspace(ws);
                        self.minimize_window_for(&win);
                    }
                }
            }
            UiAction::CloseGlobal { ws, idx_in_ws } => {
                if let Some(win) = self.global_ws_window_at(ws, idx_in_ws) {
                    self.close_window(&win);
                }
            }
            UiAction::MoveNextWsGlobal { ws, idx_in_ws } => {
                if let Some(win) = self.global_ws_window_at(ws, idx_in_ws) {
                    let next_ws = (ws + 1) % NUM_WORKSPACES;
                    Self::window_meta_mut(&win).last_configured_rect = None;
                    Self::window_meta_mut(&win).minimized = false;
                    self.workspaces[ws].unmap_elem(&win);
                    self.workspaces[next_ws].map_element(win, (32, 48), true);
                    if ws == self.active_workspace {
                        self.relayout_active_workspace();
                    }
                }
            }
            UiAction::SelectAltTab(idx) => {
                self.ui.alt_tab_index = idx;
                self.commit_alt_tab();
            }
            UiAction::CycleLayoutMode => {
                self.config.layout_mode = self.config.layout_mode.cycle();
                self.config.auto_tile = self.config.layout_mode != LayoutMode::Floating;
                self.save_config_with_feedback();
                for win in self.active_space().elements() {
                    Self::window_meta_mut(win).last_configured_rect = None;
                }
                self.relayout_active_workspace();
            }
            UiAction::SelectCategory(cat) => {
                self.ui.selected_category = cat;
                self.ui.selected_custom_category = None;
                self.ui.auto_switched_from_category = None;
                self.ui.app_scroll = 0;
                self.ui.selected_app_row = 0;
            }
            UiAction::SelectCustomCategory(cat) => {
                self.ui.selected_custom_category = Some(cat);
                self.ui.selected_category = AppCategory::All;
                self.ui.auto_switched_from_category = None;
                self.ui.app_scroll = 0;
                self.ui.selected_app_row = 0;
            }
            UiAction::SearchInAllCategories => {
                if self.ui.selected_category != AppCategory::All
                    || self.ui.selected_custom_category.is_some()
                {
                    let prev_custom = self.ui.selected_custom_category.take();
                    self.ui.auto_switched_from_category =
                        Some((self.ui.selected_category, prev_custom));
                    self.ui.selected_category = AppCategory::All;
                    self.ui.app_scroll = 0;
                    self.ui.selected_app_row = 0;
                }
            }
            UiAction::RestorePreviousCategory => {
                if let Some((prev_cat, prev_custom)) = self.ui.auto_switched_from_category.take() {
                    self.ui.selected_category = prev_cat;
                    self.ui.selected_custom_category = prev_custom;
                    self.ui.app_scroll = 0;
                    self.ui.selected_app_row = 0;
                }
            }
            UiAction::ClearSearch => {
                self.ui.search_query.clear();
                self.ui.app_scroll = 0;
                self.ui.selected_app_row = 0;
            }
            UiAction::LaunchApp(idx) => self.launch_app_by_index(idx),
            UiAction::LaunchQuickTerminal => {
                self.ui.close_popup();
                spawn_detached(&self.config.terminal_cmd);
            }
            UiAction::LaunchQuickFiles => {
                self.ui.close_popup();
                let preferred = self.config.files_cmd.clone();
                if executable_exists(&preferred) {
                    spawn_detached(&preferred);
                } else {
                    for candidate in ["thunar", "dolphin", "cosmic-files", "nautilus", "pcmanfm"] {
                        if executable_exists(candidate) {
                            spawn_detached(candidate);
                            break;
                        }
                    }
                }
            }
            UiAction::LaunchQuickBrowser => {
                self.ui.close_popup();
                let preferred = self.config.browser_cmd.clone();
                if executable_exists(&preferred) {
                    spawn_detached(&preferred);
                } else {
                    for candidate in [
                        "zen-browser",
                        "firefox",
                        "chromium",
                        "google-chrome-stable",
                        "brave",
                    ] {
                        if executable_exists(candidate) {
                            spawn_detached(candidate);
                            break;
                        }
                    }
                }
            }
            UiAction::RescanApps => {
                self.apps = scan_desktop_apps();
                self.ui.app_scroll = 0;
            }
            UiAction::SetAccent(name) => {
                self.config.accent_theme = name;
                self.save_config_with_feedback();
            }
            UiAction::SetWallpaperPreset(preset) => {
                self.config.wallpaper_preset = preset;
                self.config.use_custom_wallpaper = false;
                self.wallpaper_dirty = true;
                self.save_config_with_feedback();
            }
            UiAction::CycleWallpaperPreset => {
                self.ui.close_popup();
                let pos = WALLPAPER_PRESETS
                    .iter()
                    .position(|p| p.eq_ignore_ascii_case(&self.config.wallpaper_preset))
                    .map(|i| (i + 1) % WALLPAPER_PRESETS.len())
                    .unwrap_or(0);
                self.config.wallpaper_preset = WALLPAPER_PRESETS[pos].to_string();
                self.config.use_custom_wallpaper = false;
                self.wallpaper_dirty = true;
                self.save_config_with_feedback();
            }
            UiAction::ToggleCustomWallpaper => {
                self.config.use_custom_wallpaper = !self.config.use_custom_wallpaper;
                self.wallpaper_dirty = true;
                self.save_config_with_feedback();
            }
            UiAction::TogglePanelPosition => {
                self.config.panel_position = self.config.panel_position.toggle();
                self.save_config_with_feedback();
                self.relayout_active_workspace();
            }
            UiAction::ToggleShelfStyle => {
                self.config.shelf_style = self.config.shelf_style.toggle();
                self.save_config_with_feedback();
                self.relayout_active_workspace();
            }
            UiAction::AdjustPanelHeight(delta) => {
                self.config.panel_height = (self.config.panel_height + delta).clamp(34, 56);
                self.save_config_with_feedback();
                self.relayout_active_workspace();
            }
            UiAction::AdjustBorderWidth(delta) => {
                self.config.border_width = (self.config.border_width + delta).clamp(0, 4);
                self.save_config_with_feedback();
            }
            UiAction::ToggleEdgeSnap => {
                self.config.edge_snap = !self.config.edge_snap;
                self.save_config_with_feedback();
            }
            UiAction::ToggleFocusFollowsMouse => {
                self.config.focus_follows_mouse = !self.config.focus_follows_mouse;
                self.save_config_with_feedback();
            }
            UiAction::ToggleAnimations => {
                self.config.animations_enabled = !self.config.animations_enabled;
                self.save_config_with_feedback();
            }
            UiAction::CycleAnimationSpeed => {
                self.config.animation_speed = if self.config.animation_speed >= 0.34 {
                    0.16
                } else if self.config.animation_speed >= 0.22 {
                    0.36
                } else {
                    0.28
                };
                self.save_config_with_feedback();
            }
            UiAction::ToggleAutoRouteAudioRoles => {
                self.config.auto_route_audio_roles = !self.config.auto_route_audio_roles;
                self.sys.send(SysCommand::SetAutoRouteRoles {
                    enabled: self.config.auto_route_audio_roles,
                });
                self.save_config_with_feedback();
                self.refresh_system_snapshot();
            }
            UiAction::ToggleAutoLaunchTerminal => {
                self.config.auto_launch_terminal_on_login =
                    !self.config.auto_launch_terminal_on_login;
                self.save_config_with_feedback();
            }
            UiAction::ToggleWindowsShortcuts => {
                self.config.windows_shortcuts = !self.config.windows_shortcuts;
                self.save_config_with_feedback();
            }
            UiAction::ToggleShowSystemProcesses => {
                self.config.show_system_processes = !self.config.show_system_processes;
                self.sys.send(SysCommand::SetShowSystemProcesses {
                    enabled: self.config.show_system_processes,
                });
                self.save_config_with_feedback();
                self.refresh_system_snapshot();
            }
            UiAction::CycleRefreshHz => {
                let primary_out = self
                    .display_telemetry
                    .outputs
                    .iter()
                    .find(|o| o.primary)
                    .or_else(|| self.display_telemetry.outputs.first());
                let cur_hz = primary_out
                    .and_then(|o| self.config.monitor_cfg(&o.name))
                    .filter(|m| m.refresh_hz > 0)
                    .map(|m| m.refresh_hz)
                    .unwrap_or(self.config.output_refresh_hz);
                let mut opts: Vec<u32> = vec![0];
                if let Some(out) = primary_out {
                    for &hz in &out.available_hz {
                        if hz > 0 && !opts.contains(&hz) {
                            opts.push(hz);
                        }
                    }
                }
                if opts.len() == 1 {
                    opts.extend([60, 120, 144]);
                }
                let pos = opts
                    .iter()
                    .position(|&h| h == cur_hz)
                    .map(|i| (i + 1) % opts.len())
                    .unwrap_or(1 % opts.len());
                if self.config.confirm_before_display_apply {
                    self.ui.prompt_display_confirm(opts[pos], cur_hz, self.config.vrr_mode);
                } else {
                    self.apply_global_and_primary_refresh(opts[pos]);
                }
            }
            UiAction::ConfirmDisplayChange => {
                if let Some(target_hz) = self.ui.confirm_and_apply_display_immediately() {
                    self.apply_global_and_primary_refresh(target_hz);
                }
            }
            UiAction::KeepDisplayChange => {
                self.ui.keep_display_confirm();
                self.save_config_with_feedback();
            }
            UiAction::CancelRevertDisplayChange => {
                if let Some((prev_hz, prev_vrr)) = self.ui.revert_display_confirm() {
                    self.config.vrr_mode = prev_vrr;
                    self.apply_global_and_primary_refresh(prev_hz);
                }
            }
            UiAction::CycleVrrMode => {
                self.config.vrr_mode = self.config.vrr_mode.cycle();
                self.display_config_dirty = true;
                self.save_config_with_feedback();
            }
            UiAction::ToggleVideoBatterySaver => {
                self.config.video_battery_saver = !self.config.video_battery_saver;
                self.display_config_dirty = true;
                self.save_config_with_feedback();
            }
            UiAction::CycleIdleRefreshHz => {
                self.config.idle_refresh_hz = match self.config.idle_refresh_hz {
                    24 => 10,
                    10 => 0,
                    _ => 24,
                };
                self.display_config_dirty = true;
                self.save_config_with_feedback();
            }
            UiAction::ToggleBatterySaverModeset => {
                self.config.battery_saver_modeset = !self.config.battery_saver_modeset;
                self.display_config_dirty = true;
                self.save_config_with_feedback();
            }
            UiAction::ToggleDimUnfocused => {
                self.config.dim_unfocused_windows = !self.config.dim_unfocused_windows;
                self.save_config_with_feedback();
            }
            UiAction::ToggleFreezeHidden => {
                self.config.freeze_hidden_windows = !self.config.freeze_hidden_windows;
                self.wake_workspace_views();
                self.save_config_with_feedback();
            }
            UiAction::CleanRamCacheNow { deep_kernel } => {
                self.ui.trim_caches();
                self.sys.send(SysCommand::CleanRamCache { deep_kernel });
                self.refresh_system_snapshot();
            }
            UiAction::MoveWindowNextMonitor => {
                self.move_focused_to_next_monitor();
            }
            UiAction::CycleTerminalApp => {
                let choices: Vec<&str> = ["alacritty", "xfce4-terminal", "konsole", "kitty", "foot"]
                    .into_iter()
                    .filter(|c| executable_exists(c))
                    .collect();
                if !choices.is_empty() {
                    let pos = choices
                        .iter()
                        .position(|c| *c == self.config.terminal_cmd)
                        .map(|i| (i + 1) % choices.len())
                        .unwrap_or(0);
                    self.config.terminal_cmd = choices[pos].to_string();
                    self.save_config_with_feedback();
                }
            }
            UiAction::CycleBrowserApp => {
                let choices: Vec<&str> = [
                    "zen-browser",
                    "firefox",
                    "chromium",
                    "google-chrome-stable",
                    "brave",
                ]
                .into_iter()
                .filter(|c| executable_exists(c))
                .collect();
                if !choices.is_empty() {
                    let pos = choices
                        .iter()
                        .position(|c| *c == self.config.browser_cmd)
                        .map(|i| (i + 1) % choices.len())
                        .unwrap_or(0);
                    self.config.browser_cmd = choices[pos].to_string();
                    self.save_config_with_feedback();
                }
            }
            UiAction::CycleFilesApp => {
                let choices: Vec<&str> = ["thunar", "dolphin", "cosmic-files", "nautilus", "pcmanfm"]
                    .into_iter()
                    .filter(|c| executable_exists(c))
                    .collect();
                if !choices.is_empty() {
                    let pos = choices
                        .iter()
                        .position(|c| *c == self.config.files_cmd)
                        .map(|i| (i + 1) % choices.len())
                        .unwrap_or(0);
                    self.config.files_cmd = choices[pos].to_string();
                    self.save_config_with_feedback();
                }
            }
            UiAction::SaveConfigNow => {
                self.save_config_with_feedback();
            }
            UiAction::ResetConfig => {
                self.config = Config::default();
                self.save_config_with_feedback();
                self.wallpaper_dirty = true;
                for win in self.active_space().elements() {
                    Self::window_meta_mut(win).last_configured_rect = None;
                }
                self.relayout_active_workspace();
            }
            UiAction::ReloadConfig => {
                self.config = Config::load_or_create();
                self.apps = scan_desktop_apps();
                self.wallpaper_dirty = true;
                self.ui.close_popup();
                for win in self.active_space().elements() {
                    Self::window_meta_mut(win).last_configured_rect = None;
                }
                self.relayout_active_workspace();
            }
            UiAction::QuitSession => {
                self.notif_svc.shutdown();
                self.update_watcher.shutdown();
                self.should_quit = true;
            }
            UiAction::LockScreen => {
                self.lock_screen();
            }
            UiAction::ToggleNotifications => {
                self.ui.toggle_notifications();
                if self.ui.active_popup == ActivePopup::Notifications {
                    self.notif_svc.mark_all_read();
                    self.refresh_notifications_and_ui_flags();
                }
            }
            UiAction::ToggleDnd => {
                self.config.notifications_dnd = !self.config.notifications_dnd;
                self.save_config_with_feedback();
            }
            UiAction::SelectNotifTab(tab) => {
                self.ui.notif_tab = tab;
                self.ui.notif_scroll = 0;
                self.ui.notif_reply_target = None;
                self.ui.notif_reply_draft.clear();
                self.needs_redraw = true;
            }
            UiAction::ToggleOrganizerExpanded => {
                self.ui.notif_organizer_expanded = !self.ui.notif_organizer_expanded;
                self.needs_redraw = true;
            }
            UiAction::ToggleOrganizerSilenceCategory(cat) => {
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
                self.save_config_with_feedback();
            }
            UiAction::ClearNotificationHistory => {
                self.notif_svc.clear_history();
                self.refresh_notifications_and_ui_flags();
            }
            UiAction::DismissNotification(id) => {
                if self.ui.notif_reply_target == Some(id) {
                    self.ui.notif_reply_target = None;
                    self.ui.notif_reply_draft.clear();
                }
                self.notif_svc.dismiss(id);
                self.refresh_notifications_and_ui_flags();
            }
            UiAction::SnoozeNotification { id, secs } => {
                self.notif_svc.snooze(id, secs);
                self.refresh_notifications_and_ui_flags();
            }
            UiAction::UnsnoozeNotification(id) => {
                self.notif_svc.unsnooze_now(id);
                self.refresh_notifications_and_ui_flags();
            }
            UiAction::TogglePriorityConversation(key) => {
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
                self.save_config_with_feedback();
            }
            UiAction::OpenInlineReply(id) => {
                if self.ui.notif_reply_target == Some(id) {
                    self.ui.notif_reply_target = None;
                    self.ui.notif_reply_draft.clear();
                } else {
                    self.ui.notif_reply_target = Some(id);
                    self.ui.notif_reply_draft.clear();
                }
                self.needs_redraw = true;
            }
            UiAction::SendNotificationReply { id, text } => {
                self.ui.notif_reply_target = None;
                self.ui.notif_reply_draft.clear();
                self.notif_svc.send_reply(id, &text);
                self.refresh_notifications_and_ui_flags();
            }
            UiAction::RunSuggestedAction { id, kind, payload } => {
                match kind {
                    SuggestedActionKind::QuickReply => {
                        self.notif_svc.send_reply(id, &payload);
                    }
                    SuggestedActionKind::CopyCode => {
                        copy_text_to_clipboard(&payload);
                        self.notif_svc.mark_read(id);
                    }
                    SuggestedActionKind::OpenUrl => {
                        let url = payload;
                        let _ = std::process::Command::new("xdg-open")
                            .arg(&url)
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn();
                        self.notif_svc.mark_read(id);
                    }
                    SuggestedActionKind::MarkRead => {
                        self.notif_svc.mark_read(id);
                    }
                }
                self.refresh_notifications_and_ui_flags();
            }
            UiAction::ToggleAppNotifDisabled(app_name) => {
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
                self.save_config_with_feedback();
            }
            UiAction::ToggleAppNotifSilent(app_name) => {
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
                self.save_config_with_feedback();
            }
            UiAction::ClearNotifications => {
                self.notif_svc.clear_all();
                self.refresh_notifications_and_ui_flags();
            }
            UiAction::InvokeNotificationAction { id, key } => {
                if key == "velox-reload" {
                    self.deactivate_focused_pointer_constraint();
                    self.ui.prompt_reload_confirm("Update installed — confirm live reload");
                } else {
                    self.notif_svc.invoke_action(id, &key);
                    self.notif_svc.dismiss(id);
                    self.refresh_notifications_and_ui_flags();
                }
            }
            UiAction::CheckUpdatesNow => {
                self.update_watcher.trigger_check();
                self.refresh_notifications_and_ui_flags();
            }
            UiAction::PromptReloadConfirm(reason) => {
                self.deactivate_focused_pointer_constraint();
                self.ui.prompt_reload_confirm(reason);
            }
            UiAction::CancelReloadConfirm => {
                self.ui.close_popup();
                self.sync_focused_pointer_constraint();
            }
            UiAction::SeamlessReloadWm => {
                self.trigger_seamless_reload();
            }
            UiAction::ToggleGamePointerLock => {
                self.config.game_pointer_lock = !self.config.game_pointer_lock;
                self.save_config_with_feedback();
            }
            UiAction::ToggleFullscreenTaskbarLock => {
                self.config.lock_fullscreen_taskbar = !self.config.lock_fullscreen_taskbar;
                self.save_config_with_feedback();
            }
            UiAction::SystemSuspend => {
                self.ui.close_popup();
                spawn_detached("systemctl suspend");
            }
            UiAction::SystemReboot => {
                self.ui.close_popup();
                spawn_detached("systemctl reboot");
            }
            UiAction::SystemPoweroff => {
                self.ui.close_popup();
                spawn_detached("systemctl poweroff");
            }
            UiAction::ToggleSinkMute(name) => {
                let (_, was_muted) = self.sys_snap.default_output_vol();
                if name == "@DEFAULT_SINK@" || name == self.sys_snap.default_sink {
                    self.config.saved_muted = Some(!was_muted);
                    self.save_config_with_feedback();
                }
                self.sys.send(SysCommand::ToggleSinkMute { name });
                self.refresh_system_snapshot();
            }
            UiAction::SetDefaultSink(name) => {
                self.sys.send(SysCommand::SetDefaultSink { name });
                self.refresh_system_snapshot();
            }
            UiAction::ToggleSourceMute(name) => {
                let (_, was_muted) = self.sys_snap.default_input_vol();
                if name == "@DEFAULT_SOURCE@" || name == self.sys_snap.default_source {
                    self.config.saved_mic_muted = Some(!was_muted);
                    self.save_config_with_feedback();
                }
                self.sys.send(SysCommand::ToggleSourceMute { name });
                self.refresh_system_snapshot();
            }
            UiAction::SetDefaultSource(name) => {
                self.sys.send(SysCommand::SetDefaultSource { name });
                self.refresh_system_snapshot();
            }
            UiAction::ToggleStreamMute { is_input, index } => {
                self.sys.send(SysCommand::ToggleStreamMute { is_input, index });
                self.refresh_system_snapshot();
            }
            UiAction::CycleStreamDevice { is_input, index } => {
                if is_input {
                    let cur_dev = self
                        .sys_snap
                        .source_outputs
                        .iter()
                        .find(|s| s.index == index)
                        .map(|s| s.device_index);
                    let enabled: Vec<_> = self
                        .sys_snap
                        .sources
                        .iter()
                        .filter(|s| !s.disabled)
                        .collect();
                    if !enabled.is_empty() {
                        let next_pos = cur_dev
                            .and_then(|d| enabled.iter().position(|s| s.index == d))
                            .map(|p| (p + 1) % enabled.len())
                            .unwrap_or(0);
                        let target = enabled[next_pos].name.clone();
                        self.sys.send(SysCommand::MoveStream {
                            is_input: true,
                            index,
                            target,
                        });
                    }
                } else {
                    let cur_dev = self
                        .sys_snap
                        .sink_inputs
                        .iter()
                        .find(|s| s.index == index)
                        .map(|s| s.device_index);
                    let enabled: Vec<_> = self
                        .sys_snap
                        .sinks
                        .iter()
                        .filter(|s| !s.disabled)
                        .collect();
                    if !enabled.is_empty() {
                        let next_pos = cur_dev
                            .and_then(|d| enabled.iter().position(|s| s.index == d))
                            .map(|p| (p + 1) % enabled.len())
                            .unwrap_or(0);
                        let target = enabled[next_pos].name.clone();
                        self.sys.send(SysCommand::MoveStream {
                            is_input: false,
                            index,
                            target,
                        });
                    }
                }
            }
            UiAction::CycleRoleRouting { is_comms } => {
                let enabled: Vec<_> = self
                    .sys_snap
                    .sinks
                    .iter()
                    .filter(|s| !s.disabled)
                    .collect();
                if !enabled.is_empty() {
                    let cur = if is_comms {
                        self.sys_snap.comms_sink.as_deref()
                    } else {
                        self.sys_snap.media_sink.as_deref()
                    };
                    let next_pos = cur
                        .and_then(|name| enabled.iter().position(|s| s.name == name))
                        .map(|p| (p + 1) % enabled.len())
                        .unwrap_or(0);
                    let target_sink = enabled[next_pos].name.clone();
                    if is_comms {
                        self.config.comms_output_device = Some(target_sink.clone());
                    } else {
                        self.config.media_output_device = Some(target_sink.clone());
                    }
                    self.save_config_with_feedback();
                    self.sys.send(SysCommand::RouteRole {
                        is_comms,
                        target_sink: Some(target_sink),
                    });
                    self.refresh_system_snapshot();
                }
            }
            UiAction::AdjustBrightness(delta) => {
                let next = (self.sys_snap.brightness_pct as i32 + delta).clamp(5, 100) as u8;
                self.config.saved_brightness_pct = Some(next);
                self.save_config_with_feedback();
                self.sys.send(SysCommand::SetBrightness { pct: next });
                self.refresh_system_snapshot();
            }
            UiAction::AdjustDefaultVol(delta) => {
                let (cur, _) = self.sys_snap.default_output_vol();
                let next = (cur as i32 + delta).clamp(0, 150) as u8;
                self.config.saved_volume_pct = Some(next);
                self.save_config_with_feedback();
                self.sys.send(SysCommand::SetSinkVolume {
                    name: "@DEFAULT_SINK@".to_string(),
                    pct: next,
                });
                self.refresh_system_snapshot();
            }
            UiAction::SetPowerProfile(profile) => {
                self.config.saved_power_profile = Some(profile.clone());
                self.save_config_with_feedback();
                self.sys.send(SysCommand::SetPowerProfile { profile });
            }
            UiAction::ToggleWifi => {
                self.sys.send(SysCommand::ToggleWifi);
            }
            UiAction::ToggleBluetooth => {
                self.sys.send(SysCommand::ToggleBluetooth);
            }
            UiAction::OpenBluetoothManager => {
                self.ui.close_popup();
                if executable_exists("blueman-manager") {
                    spawn_detached("blueman-manager");
                }
            }
            UiAction::OpenNetworkEditor => {
                self.ui.close_popup();
                if executable_exists("nm-connection-editor") {
                    spawn_detached("nm-connection-editor");
                }
            }
            UiAction::KillBgProcess { pids, force } => {
                let mut tree = collect_process_tree(&pids, false);
                tree.extend(pids.iter().copied());
                let mut to_close = Vec::new();
                for ws in &self.workspaces {
                    for win in ws.elements() {
                        let matched_wl = win
                            .toplevel()
                            .and_then(|tl| self.display_handle.get_client(tl.wl_surface().id()).ok())
                            .and_then(|c| c.get_credentials(&self.display_handle).ok())
                            .map(|cred| tree.contains(&(cred.pid as u32)))
                            .unwrap_or(false);
                        let matched_x11 = win
                            .x11_surface()
                            .and_then(|x| x.pid())
                            .map(|p| tree.contains(&p))
                            .unwrap_or(false);
                        if matched_wl || matched_x11 {
                            to_close.push(win.clone());
                        }
                    }
                }
                for win in to_close {
                    self.close_window(&win);
                }
                self.sys.send(SysCommand::KillProcess { pids, force });
                self.refresh_system_snapshot();
            }
            UiAction::RefreshSystem => {
                self.sys.send(SysCommand::RefreshNow);
                self.refresh_system_snapshot();
            }
            UiAction::EditLibraryAppInSettings(app_idx) => {
                self.open_settings_for_app(app_idx);
            }
            UiAction::SetLibraryAppCategory(app_idx, cat) => {
                self.apply_library_app_override(app_idx, Some(cat), None, None, None);
            }
            UiAction::ToggleLibraryAppTerminal(app_idx) => {
                let next_term = self
                    .apps
                    .get(app_idx)
                    .map(|a| !a.terminal);
                self.apply_library_app_override(app_idx, None, next_term, None, None);
            }
            UiAction::CycleLibraryAppGameOverride(app_idx) => {
                let next_game = self.apps.get(app_idx).map(|a| match a.is_game_override {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                });
                self.apply_library_app_override(app_idx, None, None, next_game, None);
            }
            UiAction::ToggleLibraryAppHidden(app_idx) => {
                let next_hidden = self.apps.get(app_idx).map(|a| !a.hidden);
                self.apply_library_app_override(app_idx, None, None, None, next_hidden);
            }
            UiAction::ToggleQuickTileCustomize => {
                self.ui.customize_quick_tiles = !self.ui.customize_quick_tiles;
            }
            UiAction::OpenQuickTileCustomizer => {
                self.ui.open_popup(ActivePopup::QuickSettings);
                self.ui.customize_quick_tiles = true;
            }
            UiAction::MoveTaskbarQuickTile(tile, delta) => {
                if self.config.move_taskbar_tile(&tile, delta) {
                    self.save_config_with_feedback();
                }
            }
            UiAction::ToggleTaskbarQuickTileVisible(tile) => {
                if self.config.toggle_taskbar_tile_visible(&tile) {
                    self.save_config_with_feedback();
                }
            }
            UiAction::MoveQuickPanelTile(tile, delta) => {
                if self.config.move_quick_panel_tile(&tile, delta) {
                    self.save_config_with_feedback();
                }
            }
            UiAction::ResetTaskbarQuickTiles => {
                self.config.reset_taskbar_tiles();
                self.save_config_with_feedback();
            }
            UiAction::ToggleMinimalViewWindow(idx) => {
                self.ui.close_popup();
                if let Some(win) = self.active_ws_window_at(idx) {
                    self.toggle_minimal_view_for(&win);
                }
            }
            UiAction::StartSliderDrag {
                target,
                rect,
                min,
                max,
            } => {
                self.ui.begin_slider_drag(target.clone(), rect, min, max);
                if let Some((t, val)) = self.ui.update_slider_drag(self.pointer_location) {
                    self.apply_slider_value(&t, val);
                }
            }
        }
        self.needs_redraw = true;
    }

    fn apply_library_app_override(
        &mut self,
        app_idx: usize,
        category: Option<AppCategory>,
        terminal: Option<bool>,
        game_override: Option<Option<bool>>,
        hidden: Option<bool>,
    ) {
        let Some(app) = self.apps.get(app_idx).cloned() else {
            return;
        };
        let mut store = AppLibraryStore::load_or_default();
        if app.is_custom {
            if let Some(custom) = store.custom_apps.iter_mut().find(|c| c.id == app.id) {
                if let Some(cat) = category {
                    custom.category = cat;
                    custom.custom_category = None;
                }
                if let Some(term) = terminal {
                    custom.terminal = term;
                }
                if let Some(g) = game_override {
                    custom.is_game_override = g;
                }
            }
        } else {
            let mut entry = store
                .overrides
                .iter()
                .find(|o| o.id == app.id)
                .cloned()
                .unwrap_or_else(|| AppOverrideEntry {
                    id: app.id.clone(),
                    ..Default::default()
                });
            if let Some(cat) = category {
                entry.category = Some(cat);
                entry.custom_category = Some(String::new());
            }
            if let Some(term) = terminal {
                entry.terminal = Some(term);
            }
            if let Some(g) = game_override {
                entry.is_game_override = g;
            }
            if let Some(h) = hidden {
                entry.hidden = h;
            }
            store.upsert_override(entry);
        }
        if store.save() {
            self.last_app_library_mtime = AppLibraryStore::modified_nanos();
            self.apps = scan_desktop_apps();
            if game_override.is_some() {
                for ws in &self.workspaces {
                    for w in ws.elements() {
                        Self::window_meta_mut(w).is_game = None;
                    }
                }
            }
            self.needs_redraw = true;
        }
    }

    fn open_settings_for_app(&mut self, app_idx: usize) {
        let Some(app) = self.apps.get(app_idx).cloned() else {
            return;
        };
        self.ui.close_popup();
        send_settings_cmd(&format!("edit-app:{}", app.id));
        let found_settings = (0..NUM_WORKSPACES).find_map(|ws_idx| {
            self.workspaces[ws_idx]
                .elements()
                .find(|w| Self::window_app_id(w).eq_ignore_ascii_case("velox-settings"))
                .cloned()
                .map(|win| (ws_idx, win))
        });
        if let Some((ws_idx, win)) = found_settings {
            self.switch_workspace(ws_idx);
            self.focus_window(Some(win));
            return;
        }
        let bin = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("velox-settings")))
            .filter(|p| p.exists())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                if executable_exists("velox-settings") {
                    "velox-settings".to_string()
                } else {
                    self.config.settings_cmd.clone()
                }
            });
        if !bin.is_empty() {
            self.spawn_command_with_env(&[bin, "--app".to_string(), app.id]);
        }
    }

    pub fn toggle_minimal_view_for(&mut self, win: &Window) {
        let currently_minimal = Self::window_meta(win).minimal_view;
        if currently_minimal {
            self.expand_from_minimal_view(win);
            return;
        }
        let usable = self.usable_rect_for_window(win);
        let cur = self
            .active_space()
            .element_geometry(win)
            .unwrap_or_else(|| Rectangle::new((usable.loc.x + 24, usable.loc.y + 24).into(), (280, 152).into()));
        let card_w = cur.size.w.clamp(220, 310);
        let card_h = cur.size.h.clamp(120, 180);
        let clamped_loc = clamp_rect_to_bounds(
            Rectangle::new(cur.loc, (card_w, card_h).into()),
            usable,
        )
        .loc;
        {
            let mut meta = Self::window_meta_mut(win);
            meta.fullscreen = false;
            meta.maximized = false;
            meta.snapped = None;
            meta.minimal_view = true;
            meta.minimal_slot_size = (card_w, card_h).into();
            meta.minimal_card_sig.clear();
            meta.last_configured_rect = None;
        }
        self.active_space_mut()
            .map_element(win.clone(), clamped_loc, false);
        self.focus_window(Some(win.clone()));
        self.needs_redraw = true;
    }

    pub fn fractional_scale_for_window(&self, win: &Window) -> f64 {
        self.output_for_window(win)
            .or_else(|| self.outputs.first().cloned())
            .map(|o| o.current_scale().fractional_scale())
            .unwrap_or(1.0)
            .clamp(0.75, 4.0)
    }

    pub fn sync_fractional_scale_for_surface(&self, surface: &WlSurface, win: &Window) {
        let scale = self.fractional_scale_for_window(win);
        with_states(surface, |states| {
            with_fractional_scale(states, |fs| {
                fs.set_preferred_scale(scale);
            });
        });
    }

    pub fn sync_fractional_scales(&self) {
        for ws in &self.workspaces {
            for win in ws.elements() {
                if let Some(surf) = Self::window_wl_surface(win) {
                    let scale = self.fractional_scale_for_window(win);
                    with_states(&surf, |states| {
                        with_fractional_scale(states, |fs| {
                            fs.set_preferred_scale(scale);
                        });
                    });
                }
            }
        }
        for output in &self.outputs {
            let scale = output.current_scale().fractional_scale().clamp(0.75, 4.0);
            let map = layer_map_for_output(output);
            for layer in map.layers() {
                with_states(layer.wl_surface(), |states| {
                    with_fractional_scale(states, |fs| {
                        fs.set_preferred_scale(scale);
                    });
                });
            }
        }
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl VeloxState {
    pub fn init_dmabuf(
        &mut self,
        dh: &DisplayHandle,
        feedback: DmabufFeedback,
        formats: Vec<Format>,
    ) {
        if self.dmabuf_global.is_some() {
            return;
        }
        let global = self
            .dmabuf_state
            .create_global_with_default_feedback::<Self>(dh, &feedback);
        self.dmabuf_global = Some(global);
        self.dmabuf_formats = formats;
    }
}

impl DmabufHandler for VeloxState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        let fmt = dmabuf.format();
        let valid_dims = dmabuf.width() > 0 && dmabuf.height() > 0 && dmabuf.num_planes() > 0;
        let supported = valid_dims
            && (self.dmabuf_formats.is_empty()
                || self.dmabuf_formats.contains(&fmt)
                || (fmt.modifier == Modifier::Invalid
                    && self.dmabuf_formats.iter().any(|f| f.code == fmt.code)));
        if supported {
            let _ = notifier.successful::<Self>();
        } else {
            tracing::debug!(?fmt, "Rejected unsupported DMABUF format");
            notifier.failed();
        }
    }
}

impl DrmSyncobjHandler for VeloxState {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.syncobj_state.as_mut()
    }
}

impl PointerConstraintsHandler for VeloxState {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        if self.ui.is_popup_open() {
            return;
        }
        let focused = self.focused_window();
        let same_win = focused.as_ref().is_some_and(|w| {
            Self::window_wl_surface(w)
                .is_some_and(|root| root == Self::root_wl_surface(surface))
        });
        if same_win {
            with_pointer_constraint(surface, pointer, |constraint| {
                if let Some(c) = constraint {
                    c.activate();
                }
            });
        }
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        if self.ui.is_popup_open() {
            return;
        }
        let active = with_pointer_constraint(surface, pointer, |constraint| {
            constraint.as_ref().is_some_and(|c| c.is_active())
        });
        if active {
            if let Some((ws_idx, win)) = self.find_window_location(surface) {
                if ws_idx == self.active_workspace {
                    if let Some(loc) = self.active_space().element_location(&win) {
                        self.pointer_location =
                            (loc.x as f64 + location.x, loc.y as f64 + location.y).into();
                        self.cursor_dirty = true;
                    }
                }
            }
        }
    }
}

impl FractionalScaleHandler for VeloxState {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self
            .window_for_surface(&surface)
            .map(|w| self.fractional_scale_for_window(&w))
            .or_else(|| {
                self.outputs
                    .first()
                    .map(|o| o.current_scale().fractional_scale())
            })
            .unwrap_or(1.0)
            .clamp(0.75, 4.0);
        with_states(&surface, |states| {
            with_fractional_scale(states, |fs| {
                fs.set_preferred_scale(scale);
            });
        });
    }
}

delegate_dmabuf!(VeloxState);
delegate_drm_syncobj!(VeloxState);
delegate_viewporter!(VeloxState);
delegate_fractional_scale!(VeloxState);
delegate_pointer_constraints!(VeloxState);
delegate_relative_pointer!(VeloxState);

impl ClientData for ClientState {
    fn initialized(&self, client_id: ClientId) {
        tracing::debug!(?client_id, "Wayland client initialized");
    }

    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        tracing::debug!(?client_id, ?reason, "Wayland client disconnected");
    }
}

fn spawn_pipe_reader_into_slot(read_fd: OwnedFd, slot: Arc<ServerSelectionSlot>, id: u64) {
    std::thread::spawn(move || {
        use std::io::Read;
        let mut file = std::fs::File::from(read_fd);
        let mut out = Vec::new();
        let mut chunk = [0u8; 8192];
        while out.len() < 4 * 1024 * 1024 {
            match file.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        slot.store(id, out);
    });
}

pub fn new_client_data() -> Arc<ClientState> {
    Arc::new(ClientState::default())
}

fn verify_user_password_unix_chkpwd(password: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let user = std::env::var("USER").unwrap_or_default();
    if user.is_empty() {
        return true;
    }
    let checker = if std::path::Path::new("/sbin/unix_chkpwd").exists() {
        "/sbin/unix_chkpwd"
    } else if std::path::Path::new("/usr/bin/unix_chkpwd").exists() {
        "/usr/bin/unix_chkpwd"
    } else {
        return true;
    };
    let Ok(mut child) = Command::new(checker)
        .args([&user, "nullok"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(password.as_bytes());
        let _ = stdin.write_all(&[0u8]);
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

/// Clamps `rect` so its size fits within `bounds` and none of its edges extend past `bounds`.
pub fn clamp_rect_to_bounds(
    rect: Rectangle<i32, Logical>,
    bounds: Rectangle<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let bw = bounds.size.w.max(160);
    let bh = bounds.size.h.max(120);
    let min_w = 200.min(bw);
    let min_h = 140.min(bh);
    let w = rect.size.w.clamp(min_w, bw);
    let h = rect.size.h.clamp(min_h, bh);
    let max_x = bounds.loc.x + (bw - w).max(0);
    let max_y = bounds.loc.y + (bh - h).max(0);
    let x = rect.loc.x.clamp(bounds.loc.x, max_x);
    let y = rect.loc.y.clamp(bounds.loc.y, max_y);
    Rectangle::new((x, y).into(), (w, h).into())
}

/// Computes a balanced centered fallback window rectangle strictly within `usable`.
pub fn fallback_rect_for_usable(usable: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let margin_w = if usable.size.w > 240 { 32 } else { 0 };
    let margin_h = if usable.size.h > 200 { 32 } else { 0 };
    let avail_w = (usable.size.w - margin_w).max(160);
    let avail_h = (usable.size.h - margin_h).max(120);
    let w = (usable.size.w * 64 / 100).clamp(460.min(avail_w), avail_w);
    let h = (usable.size.h * 70 / 100).clamp(320.min(avail_h), avail_h);
    let x = usable.loc.x + ((usable.size.w - w) / 2).max(0);
    let y = usable.loc.y + ((usable.size.h - h) / 2).max(0);
    clamp_rect_to_bounds(Rectangle::new((x, y).into(), (w, h).into()), usable)
}

#[cfg(test)]
mod tests {
    use smithay::utils::{Logical, Rectangle};

    use super::{clamp_rect_to_bounds, fallback_rect_for_usable, WindowMeta};

    #[test]
    fn window_meta_defaults_non_pinned_and_unordered() {
        let meta = WindowMeta::default();
        assert!(!meta.always_on_top);
        assert_eq!(meta.taskbar_order, 0);
        assert!(!meta.minimized);
        assert_eq!(meta.restore_clamp_commits, 0);
    }

    #[test]
    fn clamp_rect_to_bounds_pulls_offscreen_and_oversized_windows_inside_usable() {
        let primary = Rectangle::<i32, Logical>::new((0, 48).into(), (1920, 1032).into());

        // Right & bottom overflow (e.g. window extends beyond display border on unfullscreen)
        let overflow_rb = clamp_rect_to_bounds(
            Rectangle::new((1500, 800).into(), (800, 600).into()),
            primary,
        );
        assert_eq!(overflow_rb, Rectangle::new((1120, 480).into(), (800, 600).into()));

        // Negative / behind top panel
        let overflow_tl = clamp_rect_to_bounds(
            Rectangle::new((-180, 10).into(), (900, 700).into()),
            primary,
        );
        assert_eq!(overflow_tl, Rectangle::new((0, 48).into(), (900, 700).into()));

        // Fullscreen size exiting fullscreen clamps cleanly to usable
        let fs_exit = clamp_rect_to_bounds(
            Rectangle::new((0, 0).into(), (1920, 1080).into()),
            primary,
        );
        assert_eq!(fs_exit, primary);

        // Secondary monitor with extended stale coordinates
        let secondary = Rectangle::<i32, Logical>::new((1920, 0).into(), (2560, 1440).into());
        let far_offscreen = clamp_rect_to_bounds(
            Rectangle::new((4200, 1200).into(), (1000, 800).into()),
            secondary,
        );
        assert_eq!(
            far_offscreen,
            Rectangle::new((3480, 640).into(), (1000, 800).into())
        );
    }

    #[test]
    fn fallback_rect_for_usable_stays_centered_within_monitor() {
        for usable in [
            Rectangle::<i32, Logical>::new((0, 48).into(), (1920, 1032).into()),
            Rectangle::<i32, Logical>::new((1920, 0).into(), (1920, 1080).into()),
            Rectangle::<i32, Logical>::new((0, 48).into(), (320, 240).into()),
        ] {
            let r = fallback_rect_for_usable(usable);
            assert!(r.loc.x >= usable.loc.x);
            assert!(r.loc.y >= usable.loc.y);
            assert!(r.loc.x + r.size.w <= usable.loc.x + usable.size.w);
            assert!(r.loc.y + r.size.h <= usable.loc.y + usable.size.h);
        }
    }

    #[test]
    fn cadence_tracker_handles_144hz_quantized_60fps_and_secondary_surfaces() {
        use super::CommitCadenceTracker;
        use std::time::{Duration, Instant};

        let mut tracker = CommitCadenceTracker::default();
        let mut t = Instant::now();
        // Alternating 13.89ms and 20.83ms commits (real 60fps video quantized onto 144Hz scanouts)
        // interleaved with periodic UI overlay commits on surface 202 must cleanly lock to 60 fps.
        for i in 0..24 {
            let step_us = if i % 2 == 0 { 13_889 } else { 20_833 };
            t += Duration::from_micros(step_us);
            tracker.record_commit_at(101, 1280 * 720, false, false, t);
            if i % 4 == 0 {
                tracker.record_commit_at(202, 500 * 220, false, false, t + Duration::from_millis(8));
            }
        }
        assert_eq!(tracker.detected_video_fps, 60);

        // Game surfaces must clear game cadence instead of dropping refresh rate.
        let mut game_tracker = CommitCadenceTracker::default();
        let mut gt = Instant::now();
        for _ in 0..20 {
            gt += Duration::from_micros(16_667);
            game_tracker.record_commit_at(303, 1920 * 1080, false, true, gt);
        }
        assert_eq!(game_tracker.detected_video_fps, 0);
    }

    #[test]
    fn minimal_view_triggers_when_slot_too_small_or_below_client_min_size() {
        use super::VeloxState;
        use smithay::utils::Size;

        assert!(!VeloxState::should_use_minimal_view(
            Size::from((640, 420)),
            Size::from((300, 200)),
        ));
        assert!(VeloxState::should_use_minimal_view(
            Size::from((310, 420)),
            Size::default(),
        ));
        assert!(VeloxState::should_use_minimal_view(
            Size::from((420, 200)),
            Size::default(),
        ));
        // Client declaring larger min_size than tile slot activates Minimal View instead of clipping
        assert!(VeloxState::should_use_minimal_view(
            Size::from((500, 400)),
            Size::from((600, 450)),
        ));
    }
}
