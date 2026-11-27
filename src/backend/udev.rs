use std::{
    collections::HashMap,
    os::fd::AsFd,
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

use calloop::{
    generic::Generic,
    timer::{TimeoutAction, Timer},
    EventLoop, Interest, Mode as TriggerMode, PostAction,
};
use drm_fourcc::DrmFourcc;
use smithay::{
    backend::{
        allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        drm::{
            compositor::{FrameFlags, PrimaryPlaneElement},
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, NodeType, VrrSupport,
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{gles::GlesRenderer, Color32F, ImportDma, Renderer},
        session::{
            libseat::{LibSeatSession, LibSeatSessionNotifier},
            Event as SessionEvent, Session,
        },
        udev::{primary_gpu, UdevBackend, UdevEvent},
    },
    desktop::layer_map_for_output,
    output::{Mode as WlMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        drm::control::{connector, crtc, Device as ControlDevice, Mode as DrmMode, ModeTypeFlags},
        input::Libinput,
        wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1,
        wayland_server::{backend::GlobalId, Display, ListeningSocket},
    },
    utils::{DeviceFd, Physical, Size, Transform},
    wayland::{
        compositor::CompositorHandler,
        dmabuf::DmabufFeedbackBuilder,
        drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState},
    },
    xwayland::{X11Wm, XWayland, XWaylandEvent},
};

use crate::{
    backend::{compose_output_elements, VeloxRenderElement},
    config::{
        compute_effective_refresh_plan, pick_nearest_mode_hz, Config, DisplaySleepMode,
        OutputTelemetry, ResolutionModes, VrrMode,
    },
    cursor::CursorManager,
    handlers::input::{process_input_event, spawn_detached},
    state::{new_client_data, VeloxState},
    wallpaper::Wallpaper,
};

type GbmDrmOutputManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    (),
    DrmDeviceFd,
>;
type GbmDrmOutput = DrmOutput<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    (),
    DrmDeviceFd,
>;

struct OutputSurface {
    name: String,
    connector: connector::Handle,
    output: Output,
    global: Option<GlobalId>,
    drm_output: GbmDrmOutput,
    all_modes: Vec<DrmMode>,
    available_modes: Vec<DrmMode>,
    available_hz: Vec<u32>,
    resolution_modes: Vec<ResolutionModes>,
    active_mode: DrmMode,
    preferred_hz: u32,
    user_cap_hz: u32,
    vrr_support: VrrSupport,
    vrr_active: bool,
    pending_vblank: bool,
    pending_client_frame: bool,
    needs_redraw: bool,
    cursor_dirty: bool,
    had_cursor: bool,
    wallpaper_dirty: bool,
    wallpaper: Option<Wallpaper>,
    last_render: Instant,
    next_frame_due: Instant,
    next_client_frame_due: Instant,
    pacing_hz: u32,
    saver_label: String,
    dpms_sleeping: bool,
}

struct GpuState {
    #[allow(dead_code)]
    node: DrmNode,
    output_manager: GbmDrmOutputManager,
    renderer: GlesRenderer,
    surfaces: HashMap<crtc::Handle, OutputSurface>,
}

struct UdevApp {
    state: VeloxState,
    display: Display<VeloxState>,
    session: LibSeatSession,
    gpus: HashMap<DrmNode, GpuState>,
    cursor_mgr: CursorManager,
    start_time: Instant,
    clients: Vec<smithay::reexports::wayland_server::Client>,
    hotplug_recovery_until: Option<Instant>,
}

crate::delegate_app_xwayland!(UdevApp);

pub fn run_udev() -> Result<bool, Box<dyn std::error::Error>> {
    let mut event_loop: EventLoop<'static, UdevApp> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    let display: Display<VeloxState> = Display::new()?;
    let dh = display.handle();
    let config = Config::load_or_create();
    let mut state = VeloxState::new(dh.clone(), config);

    let (session, notifier): (LibSeatSession, LibSeatSessionNotifier) = LibSeatSession::new()?;
    let seat_name = session.seat();

    let listener = ListeningSocket::bind_auto("wayland", 1..33)?;
    let socket_name = listener
        .socket_name()
        .and_then(|s| s.to_str())
        .unwrap_or("wayland-1")
        .to_string();
    state.wayland_display = Some(socket_name.clone());

    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &socket_name);
        std::env::set_var("XDG_CURRENT_DESKTOP", "velox");
        std::env::set_var("XDG_SESSION_TYPE", "wayland");
    }
    let _ = std::process::Command::new("dbus-update-activation-environment")
        .args(["--systemd", "WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE"])
        .spawn();
    tracing::info!(%socket_name, %seat_name, "velox-wm (DRM/udev) listening on Wayland socket");

    loop_handle.insert_source(
        Generic::new(listener, Interest::READ, TriggerMode::Level),
        |_, listener, app| {
            while let Ok(Some(stream)) = listener.accept() {
                tracing::info!("New Wayland client connected");
                if let Ok(client) = app.display.handle().insert_client(stream, new_client_data()) {
                    app.clients.push(client);
                }
            }
            Ok(PostAction::Continue)
        },
    )?;

    let mut libinput_context =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput_context
        .udev_assign_seat(&seat_name)
        .map_err(|_| "Failed to assign seat to libinput")?;
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    loop_handle.insert_source(libinput_backend, |event, _, app| {
        process_input_event(&mut app.state, event);
    })?;

    let mut libinput_for_session = libinput_context;
    loop_handle.insert_source(notifier, move |event, _, app| match event {
        SessionEvent::PauseSession => {
            libinput_for_session.suspend();
            for gpu in app.gpus.values_mut() {
                gpu.output_manager.pause();
            }
        }
        SessionEvent::ActivateSession => {
            let _ = libinput_for_session.resume();
            for gpu in app.gpus.values_mut() {
                let _ = gpu.output_manager.device_mut().activate(false);
                for surf in gpu.surfaces.values_mut() {
                    surf.pending_vblank = false;
                    surf.needs_redraw = true;
                }
            }
            app.state.needs_redraw = true;
        }
    })?;

    let mut app = UdevApp {
        state,
        display,
        session,
        gpus: HashMap::new(),
        cursor_mgr: CursorManager::new(),
        start_time: Instant::now(),
        clients: Vec::new(),
        hotplug_recovery_until: None,
    };

    let udev_backend = UdevBackend::new(&seat_name)?;
    let primary = primary_gpu(&seat_name)?.and_then(|p| DrmNode::from_path(p).ok());

    for (device_id, path) in udev_backend.device_list() {
        if let Ok(node) = DrmNode::from_dev_id(device_id) {
            if primary.is_some() && Some(node) != primary && node.ty() != NodeType::Primary {
                continue;
            }
            if let Err(err) = add_gpu(&mut app, &loop_handle, node, path) {
                tracing::warn!(?err, ?path, "Skipping DRM device");
            }
        }
    }

    // Spawn XWayland AFTER GPU/DMABUF/explicit-sync initialization so GLAMOR/Vulkan/DRI3 see hardware acceleration cleanly.
    let mut xw_envs: Vec<(&str, &str)> = Vec::new();
    if Path::new("/sys/bus/pci/drivers/nvidia").exists()
        && Path::new("/usr/share/glvnd/egl_vendor.d/10_nvidia.json").exists()
    {
        xw_envs.push((
            "__EGL_VENDOR_LIBRARY_FILENAMES",
            "/usr/share/glvnd/egl_vendor.d/10_nvidia.json",
        ));
    }
    if let Ok((xwayland, xw_client)) = XWayland::spawn(
        &dh,
        None,
        xw_envs,
        true,
        Stdio::null(),
        Stdio::inherit(),
        |_| (),
    ) {
        let loop_for_xw = loop_handle.clone();
        let _ = loop_handle.insert_source(xwayland, move |event, _, app| match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => {
                let disp = format!(":{display_number}");
                tracing::info!(display = %disp, "XWayland ready (GPU DMABUF/GLAMOR enabled)");
                app.state.x11_display = Some(disp.clone());
                unsafe {
                    std::env::set_var("DISPLAY", &disp);
                }
                let _ = std::process::Command::new("dbus-update-activation-environment")
                    .args(["--systemd", "DISPLAY"])
                    .spawn();
                match X11Wm::start_wm(loop_for_xw.clone(), x11_socket, xw_client.clone()) {
                    Ok(wm) => app.state.xwm = Some(wm),
                    Err(err) => tracing::warn!(?err, "Failed to start X11Wm"),
                }
            }
            XWaylandEvent::Error => tracing::warn!("XWayland failed to start"),
        });
    }

    let loop_handle_udev = loop_handle.clone();
    loop_handle.insert_source(udev_backend, move |event, _, app| match event {
        UdevEvent::Added { device_id, path } => {
            if let Ok(node) = DrmNode::from_dev_id(device_id) {
                if !app.gpus.contains_key(&node) {
                    if let Err(err) = add_gpu(app, &loop_handle_udev, node, &path) {
                        tracing::warn!(?err, ?path, "Failed to add hotplugged GPU");
                    }
                }
            }
        }
        UdevEvent::Changed { device_id } => {
            if let Ok(node) = DrmNode::from_dev_id(device_id) {
                tracing::info!(?node, "DRM hotplug/change detected; reconciling monitors");
                reconcile_drm_connectors(app, node);
            }
        }
        UdevEvent::Removed { device_id } => {
            if let Ok(node) = DrmNode::from_dev_id(device_id) {
                if let Some(mut gpu) = app.gpus.remove(&node) {
                    for (_, mut surf) in gpu.surfaces.drain() {
                        app.state.remove_output(&surf.output);
                        if let Some(gid) = surf.global.take() {
                            app.display.handle().remove_global::<VeloxState>(gid);
                        }
                    }
                }
            }
        }
    })?;

    // Wake epoll immediately whenever Wayland clients commit buffers or send requests.
    if let Ok(owned_wl_fd) = app.display.backend().poll_fd().as_fd().try_clone_to_owned() {
        let _ = loop_handle.insert_source(
            Generic::new(owned_wl_fd, Interest::READ, TriggerMode::Level),
            |_, _, _| Ok(PostAction::Continue),
        );
    }

    // Periodic housekeeping timer: config hot-reload check, confirm countdown, & HPD/FreeSync monitor recovery.
    let mut last_cfg_mtime = Config::config_modified_nanos();
    let timer = Timer::from_duration(Duration::from_millis(400));
    loop_handle.insert_source(timer, move |_, _, app| {
        app.state.hot_reload_config_if_needed(&mut last_cfg_mtime);
        app.state.sync_display_confirm_timeout();
        app.state.refresh_system_snapshot();
        app.state
            .send_hidden_window_keepalives(app.start_time.elapsed());
        if let Some(until) = app.hotplug_recovery_until {
            if Instant::now() <= until {
                let gpu_nodes: Vec<DrmNode> = app.gpus.keys().copied().collect();
                for node in gpu_nodes {
                    reconcile_drm_connectors(app, node);
                }
            } else {
                app.hotplug_recovery_until = None;
            }
        }
        TimeoutAction::ToDuration(Duration::from_millis(400))
    })?;

    for cmd in app.state.config.autostart.clone() {
        spawn_detached(&cmd);
    }
    if app.state.config.auto_launch_terminal_on_login {
        let term = app.state.config.terminal_cmd.clone();
        if !term.is_empty() && !app.state.config.autostart.iter().any(|c| c == &term) {
            spawn_detached(&term);
        }
    }
    app.state.spawn_restored_windows();

    reconcile_display_policies(&mut app);
    // Kick off initial render immediately so outputs show up on frame 1.
    render_ready_outputs(&mut app);
    let _ = app.display.flush_clients();
    tracing::info!("velox-wm DRM VBlank-paced VRR & Battery-Saver session ready");

    while !app.state.should_quit {
        let now = Instant::now();
        let animating = app.state.animator.any_active() || app.state.ui.is_animating();
        let global_dirty = app.state.needs_redraw
            || app.state.cursor_dirty
            || app.state.wallpaper_dirty
            || animating;

        let mut min_due: Option<Duration> = None;
        for gpu in app.gpus.values() {
            for surf in gpu.surfaces.values() {
                if !surf.pending_vblank
                    && !surf.dpms_sleeping
                    && (surf.needs_redraw || surf.cursor_dirty || surf.wallpaper_dirty || global_dirty)
                {
                    let rem = surf.next_frame_due.saturating_duration_since(now);
                    min_due = Some(min_due.map_or(rem, |cur| cur.min(rem)));
                } else if surf.pending_client_frame && !surf.pending_vblank && !surf.dpms_sleeping {
                    let rem = surf.next_client_frame_due.saturating_duration_since(now);
                    min_due = Some(min_due.map_or(rem, |cur| cur.min(rem)));
                }
            }
        }

        let timeout = match min_due {
            Some(d) if d.is_zero() => Some(Duration::ZERO),
            Some(d) => Some(d.min(Duration::from_millis(250))),
            None if animating => Some(Duration::from_millis(7)),
            None => Some(Duration::from_millis(250)),
        };

        event_loop.dispatch(timeout, &mut app)?;
        app.display.dispatch_clients(&mut app.state)?;
        for (client, source) in app.state.pending_sync_sources.drain(..) {
            let _ = loop_handle.insert_source(source, move |_, _, app| {
                app.state
                    .client_compositor_state(&client)
                    .blocker_cleared(&mut app.state, &app.display.handle());
                app.state.needs_redraw = true;
                Ok(())
            });
        }
        crate::xwayland::flush_pending_x11_selections(&mut app.state, &loop_handle);
        app.state.refresh_system_snapshot();
        if app.state.cadence.expire_if_idle() {
            app.state.display_config_dirty = true;
        }
        reconcile_display_policies(&mut app);
        flush_due_client_frames(&mut app);
        let _ = app.display.flush_clients();
        render_ready_outputs(&mut app);
        app.display.flush_clients()?;
    }

    let reload_requested = app.state.reload_requested;
    app.state.notif_svc.shutdown();
    app.state.update_watcher.shutdown();
    drop(app);
    tracing::info!(reload_requested, "velox-wm exiting cleanly");
    Ok(reload_requested)
}

fn add_gpu(
    app: &mut UdevApp,
    loop_handle: &calloop::LoopHandle<'static, UdevApp>,
    node: DrmNode,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let fd = app.session.open(
        path,
        smithay::reexports::rustix::fs::OFlags::RDWR
            | smithay::reexports::rustix::fs::OFlags::CLOEXEC
            | smithay::reexports::rustix::fs::OFlags::NOCTTY
            | smithay::reexports::rustix::fs::OFlags::NONBLOCK,
    )?;
    let device_fd = DrmDeviceFd::new(DeviceFd::from(fd));

    let (drm, drm_notifier) = DrmDevice::new(device_fd.clone(), true)?;
    let gbm = GbmDevice::new(device_fd.clone())?;

    let egl_display = unsafe { EGLDisplay::new(gbm.clone())? };
    let egl_context = EGLContext::new(&egl_display)?;
    let renderer = unsafe { GlesRenderer::new(egl_context)? };
    let renderer_formats = renderer.dmabuf_formats().into_iter().collect::<Vec<_>>();
    if app.state.dmabuf_global.is_none() && !renderer_formats.is_empty() {
        let render_node = node
            .node_with_type(NodeType::Render)
            .and_then(Result::ok)
            .unwrap_or(node);
        match DmabufFeedbackBuilder::new(
            render_node.dev_id(),
            renderer_formats.iter().copied(),
        )
        .add_preference_tranche(
            node.dev_id(),
            Some(zwp_linux_dmabuf_feedback_v1::TrancheFlags::Scanout),
            renderer_formats.iter().copied(),
        )
        .build()
        {
            Ok(fb) => {
                app.state.init_dmabuf(
                    &app.display.handle(),
                    fb,
                    renderer_formats.clone(),
                );
                tracing::info!(
                    ?render_node,
                    formats = renderer_formats.len(),
                    "Advertising GPU zwp_linux_dmabuf_v1 + direct-scanout feedback"
                );
            }
            Err(err) => tracing::warn!(?err, "Failed to build DmabufFeedback"),
        }
    }
    if app.state.syncobj_state.is_none() && supports_syncobj_eventfd(&device_fd) {
        app.state.syncobj_state = Some(DrmSyncobjState::new::<VeloxState>(
            &app.display.handle(),
            device_fd,
        ));
        tracing::info!("Enabled wp_linux_drm_syncobj_v1 GPU explicit synchronization");
    }

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), node.into());

    let color_formats = [DrmFourcc::Argb8888, DrmFourcc::Xrgb8888];
    let output_manager = DrmOutputManager::new(
        drm,
        allocator,
        exporter,
        Some(gbm),
        color_formats,
        renderer_formats,
    );

    loop_handle.insert_source(drm_notifier, move |event, _, app| match event {
        DrmEvent::VBlank(crtc) => {
            let elapsed = app.start_time.elapsed();
            let now = Instant::now();
            let content_active = is_content_active(&app.state);
            let output_to_notify = if let Some(gpu) = app.gpus.get_mut(&node) {
                if let Some(surf) = gpu.surfaces.get_mut(&crtc) {
                    let _ = surf.drm_output.frame_submitted();
                    surf.pending_vblank = false;
                    let desired_hz = if content_active {
                        surf.user_cap_hz
                    } else {
                        surf.pacing_hz
                    };
                    // Deliver frame callbacks right on VBlank at native cap whenever unthrottled
                    // so active video players & browsers never stutter or wait on timer jitter.
                    if desired_hz >= surf.active_mode.vrefresh() {
                        surf.next_frame_due = now;
                        surf.next_client_frame_due = now;
                        surf.pending_client_frame = false;
                        Some(surf.output.clone())
                    } else if now >= surf.next_client_frame_due {
                        surf.pending_client_frame = false;
                        surf.next_client_frame_due = now + pacing_period(desired_hz);
                        Some(surf.output.clone())
                    } else {
                        surf.pending_client_frame = true;
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(out) = output_to_notify {
                send_surface_frame_callbacks(&app.state, &out, elapsed);
            }
        }
        DrmEvent::Error(err) => {
            tracing::warn!(?err, "DRM device error — recovering VBlank and connector state");
            if let Some(gpu) = app.gpus.get_mut(&node) {
                for surf in gpu.surfaces.values_mut() {
                    surf.pending_vblank = false;
                    surf.needs_redraw = true;
                }
            }
            app.hotplug_recovery_until = Some(Instant::now() + Duration::from_secs(6));
        }
    })?;

    app.gpus.insert(
        node,
        GpuState {
            node,
            output_manager,
            renderer,
            surfaces: HashMap::new(),
        },
    );

    reconcile_drm_connectors(app, node);

    Ok(())
}

fn build_resolution_modes(all_modes: &[DrmMode]) -> Vec<ResolutionModes> {
    let mut map: std::collections::BTreeMap<(i32, i32), Vec<u32>> = std::collections::BTreeMap::new();
    for m in all_modes {
        let (w, h) = (m.size().0 as i32, m.size().1 as i32);
        if w < 640 || h < 480 {
            continue;
        }
        let hz = m.vrefresh();
        if hz == 0 {
            continue;
        }
        map.entry((w, h)).or_default().push(hz);
    }
    let mut groups: Vec<ResolutionModes> = map
        .into_iter()
        .map(|((width, height), mut hz)| {
            hz.sort_unstable();
            hz.dedup();
            ResolutionModes { width, height, hz }
        })
        .collect();
    groups.sort_by(|a, b| {
        let area_a = (a.width as i64) * (a.height as i64);
        let area_b = (b.width as i64) * (b.height as i64);
        area_b
            .cmp(&area_a)
            .then_with(|| b.width.cmp(&a.width))
    });
    groups
}

fn modes_for_resolution(all_modes: &[DrmMode], w: i32, h: i32) -> (Vec<DrmMode>, Vec<u32>, u32) {
    let same_res_modes: Vec<DrmMode> = all_modes
        .iter()
        .filter(|m| m.size() == (w as u16, h as u16))
        .copied()
        .collect();
    let mut available_hz: Vec<u32> = same_res_modes.iter().map(|m| m.vrefresh()).collect();
    available_hz.sort_unstable();
    available_hz.dedup();
    let preferred_hz = *available_hz.last().unwrap_or(&60);
    (same_res_modes, available_hz, preferred_hz)
}

/// Synchronizes connected DRM connectors with active `OutputSurface`s cleanly:
/// handles multi-monitor plug/unplug AND full recovery when FreeSync/VRR monitor OSD toggles reset link/EDID.
fn reconcile_drm_connectors(app: &mut UdevApp, node: DrmNode) {
    let Ok(res_handles) = app
        .gpus
        .get(&node)
        .and_then(|g| g.output_manager.device().resource_handles().ok())
        .ok_or(())
    else {
        return;
    };

    #[derive(Clone)]
    struct ConnectedSpec {
        conn_handle: connector::Handle,
        name: String,
        phys_w: u32,
        phys_h: u32,
        all_modes: Vec<DrmMode>,
        same_res_modes: Vec<DrmMode>,
        available_hz: Vec<u32>,
        resolution_modes: Vec<ResolutionModes>,
        preferred_hz: u32,
        user_cap_hz: u32,
        scale: f32,
        chosen_mode: DrmMode,
        compatible_crtcs: Vec<crtc::Handle>,
    }

    let global_target_hz = app.state.config.output_refresh_hz;
    let seamless_overlay = app.state.config.seamless_refresh_overlay;
    let mut connected_specs = Vec::new();
    let mut connected_handles = Vec::new();

    if let Some(gpu) = app.gpus.get(&node) {
        for conn_handle in res_handles.connectors() {
            let Ok(conn) = gpu.output_manager.device().get_connector(*conn_handle, true) else {
                continue;
            };
            if conn.state() != connector::State::Connected || conn.modes().is_empty() {
                continue;
            }
            let name = format!("{:?}-{}", conn.interface(), conn.interface_id());
            let all_modes: Vec<DrmMode> = conn.modes().to_vec();
            let resolution_modes = build_resolution_modes(&all_modes);
            let mon_cfg = app.state.config.monitor_cfg(&name);

            let preferred_res = conn
                .modes()
                .iter()
                .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
                .map(|m| m.size())
                .or_else(|| conn.modes().iter().map(|m| m.size()).max());
            let target_res = mon_cfg
                .filter(|m| {
                    m.width > 0
                        && m.height > 0
                        && all_modes
                            .iter()
                            .any(|dm| dm.size() == (m.width as u16, m.height as u16))
                })
                .map(|m| (m.width as u16, m.height as u16))
                .or(preferred_res);
            let (tw, th) = target_res.unwrap_or((1920, 1080));
            let (same_res_modes, available_hz, preferred_hz) =
                modes_for_resolution(&all_modes, tw as i32, th as i32);

            let mon_hz = mon_cfg
                .filter(|m| m.refresh_hz > 0)
                .map(|m| m.refresh_hz)
                .unwrap_or(global_target_hz);
            let user_cap_hz = if mon_hz > 0 {
                pick_nearest_mode_hz(mon_hz, &available_hz).unwrap_or(mon_hz.min(preferred_hz))
            } else {
                preferred_hz
            };
            let kms_target_hz = if seamless_overlay { 0 } else { mon_hz };
            let Some(chosen_mode) = same_res_modes
                .iter()
                .max_by_key(|m| {
                    let exact_target = kms_target_hz > 0 && m.vrefresh() == kms_target_hz;
                    let prefer_flag = m.mode_type().contains(ModeTypeFlags::PREFERRED);
                    (exact_target, m.vrefresh(), prefer_flag)
                })
                .or_else(|| conn.modes().first())
                .copied()
            else {
                continue;
            };
            let scale = mon_cfg
                .filter(|m| (0.75..=3.0).contains(&m.scale))
                .map(|m| m.scale)
                .unwrap_or(1.0);

            let mut compatible_crtcs = Vec::new();
            for enc_handle in conn.encoders() {
                if let Ok(enc) = gpu.output_manager.device().get_encoder(*enc_handle) {
                    for c in res_handles.filter_crtcs(enc.possible_crtcs()) {
                        if !compatible_crtcs.contains(&c) {
                            compatible_crtcs.push(c);
                        }
                    }
                }
            }
            let (phys_w, phys_h) = conn.size().unwrap_or((0, 0));
            connected_handles.push(*conn_handle);
            connected_specs.push(ConnectedSpec {
                conn_handle: *conn_handle,
                name,
                phys_w,
                phys_h,
                all_modes,
                same_res_modes,
                available_hz,
                resolution_modes,
                preferred_hz,
                user_cap_hz,
                scale,
                chosen_mode,
                compatible_crtcs,
            });
        }
    }

    // 1. Remove any disconnected surfaces, plus re-create surfaces whose EDID mode table or VRR changed
    //    (e.g. toggling FreeSync Premium in monitor OSD) so link training resets cleanly!
    let mut to_remove: Vec<crtc::Handle> = Vec::new();
    let mut refresh_outputs: Vec<(
        crtc::Handle,
        Vec<DrmMode>,
        Vec<DrmMode>,
        Vec<u32>,
        Vec<ResolutionModes>,
        u32,
    )> = Vec::new();
    if let Some(gpu) = app.gpus.get(&node) {
        for (crtc, surf) in &gpu.surfaces {
            let Some(spec) = connected_specs
                .iter()
                .find(|s| s.conn_handle == surf.connector)
            else {
                to_remove.push(*crtc);
                continue;
            };
            let new_vrr = surf
                .drm_output
                .with_compositor(|c| c.vrr_supported(surf.connector))
                .unwrap_or(VrrSupport::NotSupported);
            if spec.all_modes != surf.all_modes || new_vrr != surf.vrr_support {
                refresh_outputs.push((
                    *crtc,
                    spec.all_modes.clone(),
                    spec.same_res_modes.clone(),
                    spec.available_hz.clone(),
                    spec.resolution_modes.clone(),
                    spec.preferred_hz,
                ));
            }
        }
    }

    for crtc in to_remove {
        if let Some(gpu) = app.gpus.get_mut(&node) {
            if let Some(mut surf) = gpu.surfaces.remove(&crtc) {
                tracing::info!(output = %surf.name, "Removing disconnected DRM output");
                app.state.remove_output(&surf.output);
                if let Some(gid) = surf.global.take() {
                    app.display.handle().remove_global::<VeloxState>(gid);
                }
            }
        }
    }

    // Re-initialize any surface whose EDID modes or VRR changed (FreeSync toggle), resetting buffers cleanly.
    for (crtc, all_modes, same_res_modes, available_hz, resolution_modes, preferred_hz) in refresh_outputs {
        if let Some(gpu) = app.gpus.get_mut(&node) {
            if let Some(surf) = gpu.surfaces.get_mut(&crtc) {
                let next_vrr = surf
                    .drm_output
                    .with_compositor(|c| c.vrr_supported(surf.connector))
                    .unwrap_or(VrrSupport::NotSupported);
                for m in &all_modes {
                    surf.output.add_mode(WlMode::from(*m));
                }
                surf.all_modes = all_modes;
                surf.available_modes = same_res_modes;
                surf.available_hz = available_hz;
                surf.resolution_modes = resolution_modes;
                surf.preferred_hz = preferred_hz;
                surf.vrr_support = next_vrr;
                surf.drm_output.reset_buffers();
                surf.pending_vblank = false;
                surf.needs_redraw = true;
                tracing::info!(
                    output = %surf.name,
                    vrr_capable = (next_vrr != VrrSupport::NotSupported),
                    "Updated output EDID/VRR state after monitor change"
                );
            }
        }
    }

    // 2. Bind newly connected monitors (second monitor plug-in OR monitor reconnecting after FreeSync OSD toggle).
    let used_crtcs: Vec<crtc::Handle> = app
        .gpus
        .get(&node)
        .map(|g| g.surfaces.keys().copied().collect())
        .unwrap_or_default();
    let existing_conns: Vec<connector::Handle> = app
        .gpus
        .get(&node)
        .map(|g| g.surfaces.values().map(|s| s.connector).collect())
        .unwrap_or_default();
    let mut newly_used_crtcs = used_crtcs;
    let wp_path = app.state.config.resolve_wallpaper_path();
    let mut added_any = false;

    for spec in connected_specs {
        if existing_conns.contains(&spec.conn_handle) {
            continue;
        }
        let crtc_handle = spec
            .compatible_crtcs
            .iter()
            .copied()
            .find(|c| !newly_used_crtcs.contains(c));
        let Some(crtc) = crtc_handle else {
            continue;
        };

        let wl_mode = WlMode::from(spec.chosen_mode);
        let next_x = app
            .state
            .outputs
            .iter()
            .filter_map(|o| app.state.active_space().output_geometry(o))
            .map(|g| g.loc.x + g.size.w)
            .max()
            .unwrap_or(0);

        let output = Output::new(
            spec.name.clone(),
            PhysicalProperties {
                size: (spec.phys_w as i32, spec.phys_h as i32).into(),
                subpixel: Subpixel::Unknown,
                make: "velox-wm".into(),
                model: spec.name.clone(),
            },
        );
        let global = output.create_global::<VeloxState>(&app.display.handle());
        for m in &spec.all_modes {
            output.add_mode(WlMode::from(*m));
        }
        output.change_current_state(
            Some(wl_mode),
            Some(Transform::Normal),
            Some(Scale::Fractional(spec.scale as f64)),
            Some((next_x, 0).into()),
        );
        output.set_preferred(wl_mode);

        let init_res = if let Some(gpu) = app.gpus.get_mut(&node) {
            gpu.output_manager
                .initialize_output::<GlesRenderer, VeloxRenderElement>(
                    crtc,
                    spec.chosen_mode,
                    &[spec.conn_handle],
                    &output,
                    None,
                    &mut gpu.renderer,
                    &Default::default(),
                )
        } else {
            continue;
        };

        let drm_output = match init_res {
            Ok(out) => out,
            Err(err) => {
                tracing::warn!(?err, output = %spec.name, "Failed to initialize hotplugged DRM output");
                app.display.handle().remove_global::<VeloxState>(global);
                continue;
            }
        };

        newly_used_crtcs.push(crtc);
        app.state.add_output(output.clone(), (next_x, 0).into());

        let vrr_support = drm_output
            .with_compositor(|c| c.vrr_supported(spec.conn_handle))
            .unwrap_or(VrrSupport::NotSupported);
        let vrr_active = if app.state.config.vrr_mode == VrrMode::AlwaysOn
            && vrr_support != VrrSupport::NotSupported
        {
            drm_output
                .with_compositor(|c| c.use_vrr(true))
                .is_ok()
        } else {
            false
        };

        let wp_size = Size::<i32, Physical>::from((wl_mode.size.w, wl_mode.size.h));
        let wallpaper = app.gpus.get_mut(&node).and_then(|gpu| {
            Wallpaper::load_or_generate(
                &mut gpu.renderer,
                &app.state.config.wallpaper_preset,
                wp_path.as_deref(),
                wp_size,
            )
        });
        let now = Instant::now();
        if let Some(gpu) = app.gpus.get_mut(&node) {
            tracing::info!(
                output = %spec.name,
                width = wl_mode.size.w,
                height = wl_mode.size.h,
                hz = spec.chosen_mode.vrefresh(),
                "Initialized DRM output"
            );
            gpu.surfaces.insert(
                crtc,
                OutputSurface {
                    name: spec.name,
                    connector: spec.conn_handle,
                    output,
                    global: Some(global),
                    drm_output,
                    all_modes: spec.all_modes,
                    available_modes: spec.same_res_modes,
                    available_hz: spec.available_hz,
                    resolution_modes: spec.resolution_modes,
                    active_mode: spec.chosen_mode,
                    preferred_hz: spec.preferred_hz,
                    user_cap_hz: spec.user_cap_hz,
                    vrr_support,
                    vrr_active,
                    pending_vblank: false,
                    pending_client_frame: false,
                    needs_redraw: true,
                    cursor_dirty: false,
                    had_cursor: true,
                    wallpaper_dirty: false,
                    wallpaper,
                    last_render: now,
                    next_frame_due: now,
                    next_client_frame_due: now,
                    pacing_hz: spec.chosen_mode.vrefresh().max(60),
                    saver_label: format!("Native {}Hz", spec.chosen_mode.vrefresh()),
                    dpms_sleeping: false,
                },
            );
            added_any = true;
        }
    }

    if app.state.outputs.is_empty() {
        // Keep recovery window active while monitor finishes FreeSync/EDID handshake!
        app.hotplug_recovery_until = Some(Instant::now() + Duration::from_secs(8));
    } else if added_any {
        app.hotplug_recovery_until = None;
    }

    app.state.relayout_outputs();
    app.state.relayout_active_workspace();
    app.state.display_config_dirty = true;
    reconcile_display_policies(app);
    app.state.needs_redraw = true;
    render_ready_outputs(app);
}

fn pacing_period(hz: u32) -> Duration {
    let clamped = hz.clamp(10, 360) as u64;
    Duration::from_micros((1_000_000 / clamped).saturating_sub(600))
}

fn send_surface_frame_callbacks(state: &VeloxState, output: &Output, elapsed: Duration) {
    for window in state.active_space().elements() {
        if !state.config.freeze_hidden_windows || state.is_window_visible_on_output(output, window)
        {
            window.send_frame(output, elapsed, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
    }
    for layer in layer_map_for_output(output).layers() {
        layer.send_frame(output, elapsed, Some(Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }
}

const ACTIVE_CONTENT_WINDOW_SECS: f32 = 2.5;

fn is_content_active(state: &VeloxState) -> bool {
    if state.animator.any_active()
        || state.ui.is_animating()
        || state.pointer.is_grabbed()
        || state.last_input_at.elapsed().as_secs_f32() < 1.2
    {
        return true;
    }
    let video_saver_throttling =
        state.config.video_battery_saver && state.cadence.detected_video_fps > 0;
    !video_saver_throttling
        && state.last_visible_commit_at.elapsed().as_secs_f32() < ACTIVE_CONTENT_WINDOW_SECS
}

fn flush_due_client_frames(app: &mut UdevApp) {
    let now = Instant::now();
    let elapsed = app.start_time.elapsed();
    let content_active = is_content_active(&app.state);
    let mut ready_outputs = Vec::new();
    for gpu in app.gpus.values_mut() {
        for surf in gpu.surfaces.values_mut() {
            if surf.pending_client_frame && !surf.pending_vblank && now >= surf.next_client_frame_due
            {
                let cb_hz = if content_active {
                    surf.user_cap_hz
                } else {
                    surf.pacing_hz
                };
                surf.pending_client_frame = false;
                surf.next_client_frame_due = now + pacing_period(cb_hz);
                ready_outputs.push(surf.output.clone());
            }
        }
    }
    for out in ready_outputs {
        send_surface_frame_callbacks(&app.state, &out, elapsed);
    }
}

fn reconcile_display_policies(app: &mut UdevApp) {
    let force_sync = std::mem::take(&mut app.state.display_config_dirty);
    if let Some(primary_name) = app
        .state
        .config
        .monitors
        .iter()
        .find(|m| m.primary && !m.name.is_empty())
        .map(|m| m.name.clone())
    {
        app.state.promote_primary_output(&primary_name);
    }

    let animating = app.state.animator.any_active() || app.state.ui.is_animating();
    let dragging = app.state.pointer.is_grabbed() || app.state.pointer_button_down;
    let idle_secs = app.state.last_input_at.elapsed().as_secs_f32();
    let video_fps = app.state.cadence.detected_video_fps;
    let visible_content_active =
        app.state.last_visible_commit_at.elapsed().as_secs_f32() < ACTIVE_CONTENT_WINDOW_SECS;
    let seamless_overlay = app.state.config.seamless_refresh_overlay;

    let mut telemetry_outputs = Vec::new();
    let mut scale_layout_dirty = false;
    let nodes: Vec<(DrmNode, Vec<crtc::Handle>)> = app
        .gpus
        .iter()
        .map(|(n, g)| (*n, g.surfaces.keys().copied().collect()))
        .collect();

    for (node, crtcs) in nodes {
        for crtc in crtcs {
            let (
                base_cap_hz,
                want_w,
                want_h,
                want_scale,
                monitor_max_hz,
                available_hz,
                vrr_mode,
                vrr_support,
                cur_vrr_active,
                cur_w,
                cur_h,
                cur_kms_hz,
                cur_user_cap_hz,
                cur_pacing_hz,
                cur_label,
                cur_scale,
                video_saver,
                idle_hz,
                saver_modeset,
            ) = {
                let Some(gpu) = app.gpus.get(&node) else {
                    continue;
                };
                let Some(surf) = gpu.surfaces.get(&crtc) else {
                    continue;
                };
                let cur_size = surf.active_mode.size();
                let cur_w = cur_size.0 as i32;
                let cur_h = cur_size.1 as i32;
                let mon_cfg = app.state.config.monitor_cfg(&surf.name);
                let (tw, th) = mon_cfg
                    .filter(|m| {
                        m.width > 0
                            && m.height > 0
                            && surf
                                .all_modes
                                .iter()
                                .any(|dm| dm.size() == (m.width as u16, m.height as u16))
                    })
                    .map(|m| (m.width, m.height))
                    .unwrap_or((cur_w, cur_h));
                let (_modes_for_res, avail_hz_for_res, pref_hz_for_res) =
                    modes_for_resolution(&surf.all_modes, tw, th);
                let base_cap_hz = mon_cfg
                    .filter(|m| m.refresh_hz > 0)
                    .map(|m| m.refresh_hz)
                    .unwrap_or(app.state.config.output_refresh_hz);
                let vrr_mode = mon_cfg
                    .and_then(|m| m.vrr_mode)
                    .unwrap_or(app.state.config.vrr_mode);
                let want_scale = mon_cfg
                    .filter(|m| (0.75..=3.0).contains(&m.scale))
                    .map(|m| m.scale as f64)
                    .unwrap_or(1.0);
                (
                    base_cap_hz,
                    tw,
                    th,
                    want_scale,
                    pref_hz_for_res,
                    avail_hz_for_res,
                    vrr_mode,
                    surf.vrr_support,
                    surf.vrr_active,
                    cur_w,
                    cur_h,
                    surf.active_mode.vrefresh(),
                    surf.user_cap_hz,
                    surf.pacing_hz,
                    surf.saver_label.clone(),
                    surf.output.current_scale().fractional_scale(),
                    app.state.config.video_battery_saver,
                    app.state.config.idle_refresh_hz,
                    app.state.config.battery_saver_modeset,
                )
            };

            if (cur_scale - want_scale).abs() > 0.01 {
                if let Some(gpu) = app.gpus.get_mut(&node) {
                    if let Some(surf) = gpu.surfaces.get_mut(&crtc) {
                        surf.output.change_current_state(
                            None,
                            None,
                            Some(Scale::Fractional(want_scale)),
                            None,
                        );
                        surf.wallpaper_dirty = true;
                        surf.needs_redraw = true;
                        scale_layout_dirty = true;
                    }
                }
            }

            // Avoid HDMI modeset flickering on VRR unless user explicitly requests AlwaysOn.
            let vrr_can_seamless = vrr_support == VrrSupport::Supported
                || (vrr_support == VrrSupport::RequiresModeset && vrr_mode == VrrMode::AlwaysOn);

            let plan = compute_effective_refresh_plan(
                base_cap_hz,
                monitor_max_hz,
                &available_hz,
                vrr_mode,
                vrr_can_seamless,
                video_saver,
                idle_hz,
                saver_modeset,
                seamless_overlay,
                idle_secs,
                animating,
                dragging,
                video_fps,
                visible_content_active,
            );

            let want_vrr = plan.use_vrr && vrr_support != VrrSupport::NotSupported;
            if want_vrr != cur_vrr_active {
                if let Some(gpu) = app.gpus.get_mut(&node) {
                    if let Some(surf) = gpu.surfaces.get_mut(&crtc) {
                        if surf
                            .drm_output
                            .with_compositor(|c| c.use_vrr(want_vrr))
                            .is_ok()
                        {
                            surf.vrr_active = want_vrr;
                        }
                    }
                }
            }

            // Switch KMS mode ONLY on resolution change, explicit physical-mode change (!seamless_overlay),
            // explicit battery-saver physical modeset, or returning up to native max after a prior downclock.
            let res_changed = want_w != cur_w || want_h != cur_h;
            let chosen_hz = pick_nearest_mode_hz(plan.target_kms_hz, &available_hz)
                .unwrap_or(plan.target_kms_hz);
            let need_mode_switch = res_changed
                || (chosen_hz != cur_kms_hz
                    && (!seamless_overlay || saver_modeset || chosen_hz > cur_kms_hz));
            if need_mode_switch {
                apply_crtc_mode_switch(app, node, crtc, want_w, want_h, chosen_hz);
            }

            let want_user_cap_hz = if base_cap_hz > 0 {
                pick_nearest_mode_hz(base_cap_hz, &available_hz)
                    .unwrap_or(base_cap_hz.min(monitor_max_hz))
            } else {
                monitor_max_hz
            };

            let first_output = app.state.outputs.first().cloned();
            if let Some(gpu) = app.gpus.get_mut(&node) {
                if let Some(surf) = gpu.surfaces.get_mut(&crtc) {
                    if cur_pacing_hz != plan.effective_pace_hz
                        || cur_user_cap_hz != want_user_cap_hz
                        || cur_label != plan.label
                    {
                        surf.user_cap_hz = want_user_cap_hz;
                        surf.pacing_hz = plan.effective_pace_hz;
                        surf.saver_label = plan.label.clone();
                        if plan.effective_pace_hz > cur_pacing_hz
                            || want_user_cap_hz > cur_user_cap_hz
                        {
                            surf.next_frame_due = Instant::now();
                            surf.next_client_frame_due = Instant::now();
                            surf.pending_client_frame = true;
                            surf.needs_redraw = true;
                        }
                    }
                    let size = surf
                        .output
                        .current_mode()
                        .map(|m| m.size)
                        .unwrap_or((want_w, want_h).into());
                    telemetry_outputs.push(OutputTelemetry {
                        name: surf.name.clone(),
                        width: size.w,
                        height: size.h,
                        current_hz: surf.active_mode.vrefresh(),
                        preferred_hz: surf.preferred_hz,
                        available_hz: surf.available_hz.clone(),
                        resolution_modes: surf.resolution_modes.clone(),
                        scale_x100: (want_scale * 100.0).round() as u32,
                        primary: first_output.as_ref() == Some(&surf.output),
                        vrr_capable: surf.vrr_support != VrrSupport::NotSupported,
                        vrr_requires_modeset: surf.vrr_support == VrrSupport::RequiresModeset,
                        vrr_enabled: surf.vrr_active,
                        battery_saver_state: surf.saver_label.clone(),
                        active_pacing_hz: surf.pacing_hz,
                    });
                }
            }
        }
    }

    if scale_layout_dirty {
        app.state.relayout_outputs();
        app.state.relayout_active_workspace();
        app.state.sync_fractional_scales();
    }

    if force_sync
        || app.state.display_telemetry.outputs != telemetry_outputs
        || app.state.display_telemetry.detected_video_fps != video_fps
    {
        app.state.display_telemetry.outputs = telemetry_outputs;
        app.state.display_telemetry.detected_video_fps = video_fps;
        app.state.display_telemetry.save();
        app.state.ui.display_telemetry = app.state.display_telemetry.clone();
        app.state.needs_redraw = true;
    }
}

fn apply_crtc_mode_switch(
    app: &mut UdevApp,
    node: DrmNode,
    crtc: crtc::Handle,
    target_w: i32,
    target_h: i32,
    target_hz: u32,
) {
    let clear = app.state.config.background_color;
    let color = Color32F::new(clear[0], clear[1], clear[2], clear[3]);

    let (output, size, wallpaper, next_mode, next_avail_modes, next_avail_hz, next_pref_hz) = {
        let Some(gpu) = app.gpus.get_mut(&node) else {
            return;
        };
        let Some(surf) = gpu.surfaces.get_mut(&crtc) else {
            return;
        };
        let (same_res_modes, avail_hz, pref_hz) =
            modes_for_resolution(&surf.all_modes, target_w, target_h);
        let pool = if same_res_modes.is_empty() {
            &surf.available_modes
        } else {
            &same_res_modes
        };
        let Some(mode) = pool
            .iter()
            .copied()
            .min_by_key(|m| {
                (
                    m.vrefresh().abs_diff(target_hz),
                    !m.mode_type().contains(ModeTypeFlags::PREFERRED),
                )
            })
        else {
            return;
        };
        if mode == surf.active_mode {
            return;
        }
        let next_wl = WlMode::from(mode);
        (
            surf.output.clone(),
            next_wl.size,
            surf.wallpaper.take(),
            mode,
            if same_res_modes.is_empty() {
                surf.available_modes.clone()
            } else {
                same_res_modes
            },
            if avail_hz.is_empty() {
                surf.available_hz.clone()
            } else {
                avail_hz
            },
            pref_hz,
        )
    };

    let res_changed = {
        let Some(gpu) = app.gpus.get_mut(&node) else {
            return;
        };
        let prev_size = gpu
            .surfaces
            .get(&crtc)
            .map(|s| s.active_mode.size())
            .unwrap_or(next_mode.size());
        prev_size != next_mode.size()
    };

    let Some(gpu) = app.gpus.get_mut(&node) else {
        return;
    };
    let elements = compose_output_elements(
        &mut app.state,
        &mut gpu.renderer,
        &output,
        size,
        wallpaper.as_ref(),
        Some(&app.cursor_mgr),
        0.008,
    );
    let mut drm_elems = DrmOutputRenderElements::new();
    drm_elems.add_output(&crtc, color, elements);

    let mut mode_switched = false;
    if let Some(surf) = gpu.surfaces.get_mut(&crtc) {
        surf.wallpaper = wallpaper;
        match surf
            .drm_output
            .use_mode(next_mode, &mut gpu.renderer, &drm_elems)
        {
            Ok(()) => {
                surf.output
                    .change_current_state(Some(WlMode::from(next_mode)), None, None, None);
                surf.active_mode = next_mode;
                surf.available_modes = next_avail_modes;
                surf.available_hz = next_avail_hz;
                surf.preferred_hz = next_pref_hz;
                surf.pending_vblank = false;
                surf.wallpaper_dirty = res_changed;
                surf.needs_redraw = true;
                surf.next_frame_due = Instant::now();
                mode_switched = true;
                tracing::info!(
                    output = %surf.name,
                    width = size.w,
                    height = size.h,
                    hz = next_mode.vrefresh(),
                    "Switched DRM monitor mode"
                );
            }
            Err(err) => {
                tracing::warn!(?err, target_w, target_h, target_hz, "Failed to switch DRM monitor mode");
            }
        }
    }
    if mode_switched && res_changed {
        app.state.relayout_outputs();
        app.state.relayout_active_workspace();
    }
}

fn render_ready_outputs(app: &mut UdevApp) {
    let animating = app.state.animator.any_active() || app.state.ui.is_animating();
    let global_redraw = std::mem::take(&mut app.state.needs_redraw) || animating;
    let global_wp = std::mem::take(&mut app.state.wallpaper_dirty);
    let global_cursor = std::mem::take(&mut app.state.cursor_dirty);
    let cursor_pos = app.state.pointer_location;
    let dpms_sleep = app.state.idle_asleep
        && app.state.config.idle_sleep_mode == DisplaySleepMode::DpmsSleep;

    let mut targets: Vec<(DrmNode, crtc::Handle)> = Vec::new();
    for (&node, gpu) in &mut app.gpus {
        for (&crtc, surf) in &mut gpu.surfaces {
            if dpms_sleep {
                if !surf.dpms_sleeping {
                    let _ = surf.drm_output.with_compositor(|c| c.clear());
                    surf.dpms_sleeping = true;
                    surf.pending_vblank = false;
                }
                continue;
            }
            if surf.dpms_sleeping {
                surf.dpms_sleeping = false;
                surf.pending_vblank = false;
                surf.needs_redraw = true;
                surf.next_frame_due = Instant::now();
                surf.drm_output.reset_buffers();
            }
            if global_redraw {
                surf.needs_redraw = true;
            }
            if global_wp {
                surf.wallpaper_dirty = true;
            }
            if global_cursor {
                let on_output = app
                    .state
                    .active_space()
                    .output_geometry(&surf.output)
                    .map(|g| g.to_f64().contains(cursor_pos))
                    .unwrap_or(true);
                if on_output || surf.had_cursor {
                    surf.cursor_dirty = true;
                }
                surf.had_cursor = on_output;
            }
            if !surf.pending_vblank
                && (surf.needs_redraw || surf.cursor_dirty || surf.wallpaper_dirty)
            {
                targets.push((node, crtc));
            }
        }
    }

    for (node, crtc) in targets {
        render_crtc(app, node, crtc);
    }
}

fn render_crtc(app: &mut UdevApp, node: DrmNode, crtc: crtc::Handle) {
    let clear = app.state.config.background_color;
    let color = Color32F::new(clear[0], clear[1], clear[2], clear[3]);
    let elapsed = app.start_time.elapsed();
    let content_active = is_content_active(&app.state);
    let wp_path = if app
        .gpus
        .get(&node)
        .and_then(|g| g.surfaces.get(&crtc))
        .is_some_and(|s| s.wallpaper_dirty || s.wallpaper.is_none())
    {
        app.state.config.resolve_wallpaper_path()
    } else {
        None
    };

    let Some(gpu) = app.gpus.get_mut(&node) else {
        return;
    };
    let Some(surf) = gpu.surfaces.get_mut(&crtc) else {
        return;
    };

    let size = surf
        .output
        .current_mode()
        .map(|m| m.size)
        .unwrap_or((1920, 1080).into());

    if surf.wallpaper_dirty || surf.wallpaper.as_ref().is_some_and(|w| !w.matches_size(size)) {
        surf.wallpaper = Wallpaper::load_or_generate(
            &mut gpu.renderer,
            &app.state.config.wallpaper_preset,
            wp_path.as_deref(),
            size,
        );
        surf.wallpaper_dirty = false;
        surf.needs_redraw = true;
    }

    if surf.pending_vblank {
        return;
    }

    let now = Instant::now();
    let cursor_only = surf.cursor_dirty && !surf.needs_redraw;
    let effective_render_hz = if content_active {
        surf.user_cap_hz
    } else {
        surf.pacing_hz
    };
    // Pace overlay caps cleanly over steady native KMS (0 black-screen modesets).
    if !cursor_only
        && effective_render_hz < surf.active_mode.vrefresh()
        && now < surf.next_frame_due
    {
        return;
    }

    let dt = (now - surf.last_render).as_secs_f32().clamp(0.001, 0.05);
    surf.last_render = now;
    surf.needs_redraw = false;
    surf.cursor_dirty = false;
    surf.next_frame_due = if effective_render_hz < surf.active_mode.vrefresh() {
        now + pacing_period(effective_render_hz)
    } else {
        now
    };
    let cb_hz = if content_active {
        surf.user_cap_hz
    } else {
        surf.pacing_hz
    };

    let elements = compose_output_elements(
        &mut app.state,
        &mut gpu.renderer,
        &surf.output,
        size,
        surf.wallpaper.as_ref(),
        Some(&app.cursor_mgr),
        dt,
    );

    let mut frame_flags =
        FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT | FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT;
    if surf.vrr_active && cursor_only {
        frame_flags |= FrameFlags::SKIP_CURSOR_ONLY_UPDATES;
    }

    let mut queued_vblank = false;
    let mut output_for_empty_frame: Option<Output> = None;
    match surf
        .drm_output
        .render_frame(&mut gpu.renderer, &elements, color, frame_flags)
    {
        Ok(res) => {
            if res.needs_sync() {
                if let PrimaryPlaneElement::Swapchain(ref elem) = res.primary_element {
                    // Schedule non-blocking GPU fence wait (EGL_KHR_wait_sync) cleanly without stalling CPU/libinput.
                    let _ = gpu.renderer.wait(&elem.sync);
                }
            }
            let is_empty = res.is_empty;
            drop(res);
            if !is_empty {
                if surf.drm_output.queue_frame(()).is_ok() {
                    surf.pending_vblank = true;
                    queued_vblank = true;
                } else {
                    surf.next_frame_due = now + Duration::from_millis(2);
                    surf.needs_redraw = true;
                    surf.pending_client_frame = true;
                }
            } else if now >= surf.next_client_frame_due {
                surf.pending_client_frame = false;
                surf.next_client_frame_due = now + pacing_period(cb_hz);
                output_for_empty_frame = Some(surf.output.clone());
            } else {
                // Keep empty-frame callbacks armed so browsers/players requesting vsync between
                // video frames receive `wl_callback::done` instead of stalling until user input.
                surf.pending_client_frame = true;
            }
        }
        Err(err) => {
            tracing::debug!(?err, "render_frame skipped");
            surf.next_frame_due = now + Duration::from_millis(2);
            surf.pending_client_frame = true;
        }
    }

    if !queued_vblank {
        if let Some(out) = output_for_empty_frame {
            send_surface_frame_callbacks(&app.state, &out, elapsed);
        }
    }

    app.state.active_space_mut().refresh();
    app.state.popups.cleanup();
}
