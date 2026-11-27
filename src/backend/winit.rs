use std::{
    process::Stdio,
    time::{Duration, Instant},
};

use calloop::{
    generic::Generic,
    timer::{TimeoutAction, Timer},
    EventLoop, Interest, Mode as TriggerMode, PostAction,
};
use smithay::{
    backend::{
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer, ImportDma},
        winit::{self, WinitEvent, WinitEventLoop, WinitGraphicsBackend},
    },
    desktop::layer_map_for_output,
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        wayland_server::{Display, ListeningSocket},
        winit::{dpi::LogicalSize, platform::pump_events::PumpStatus, window::Window as WinitWindow},
    },
    utils::{Rectangle, Transform},
    wayland::{
        compositor::CompositorHandler,
        dmabuf::DmabufFeedbackBuilder,
    },
    xwayland::{X11Wm, XWayland, XWaylandEvent},
};

use crate::{
    backend::{compose_output_elements, render_with_damage_tracker},
    config::Config,
    handlers::input::{process_input_event, spawn_detached},
    state::{new_client_data, VeloxState},
    wallpaper::Wallpaper,
};

struct WinitApp {
    state: VeloxState,
    display: Display<VeloxState>,
    backend: WinitGraphicsBackend<GlesRenderer>,
    winit: WinitEventLoop,
    output: Output,
    damage_tracker: OutputDamageTracker,
    wallpaper: Option<Wallpaper>,
    start_time: Instant,
    last_frame: Instant,
    clients: Vec<smithay::reexports::wayland_server::Client>,
}

crate::delegate_app_xwayland!(WinitApp);

pub fn run_winit() -> Result<bool, Box<dyn std::error::Error>> {
    let mut event_loop: EventLoop<'static, WinitApp> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    let display: Display<VeloxState> = Display::new()?;
    let dh = display.handle();
    let config = Config::load_or_create();
    let mut state = VeloxState::new(dh.clone(), config);

    let listener = ListeningSocket::bind_auto("wayland", 1..33)?;
    let socket_name = listener
        .socket_name()
        .and_then(|s| s.to_str())
        .unwrap_or("wayland-1")
        .to_string();
    state.wayland_display = Some(socket_name.clone());

    // SAFETY: Single-threaded startup before spawning child processes.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &socket_name);
        std::env::set_var("XDG_CURRENT_DESKTOP", "velox");
    }
    tracing::info!(%socket_name, "velox-wm (nested) listening on Wayland socket");

    let (mut backend, winit) = winit::init_from_attributes::<GlesRenderer>(
        WinitWindow::default_attributes()
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_title("velox-wm")
            .with_visible(true),
    )?;
    tracing::info!("velox-wm window ready!");
    let winit_formats: Vec<_> = backend.renderer().dmabuf_formats().into_iter().collect();
    if !winit_formats.is_empty() {
        if let Ok(render_dev) = smithay::backend::drm::DrmNode::from_path("/dev/dri/renderD128") {
            if let Ok(fb) =
                DmabufFeedbackBuilder::new(render_dev.dev_id(), winit_formats.iter().copied())
                    .build()
            {
                state.init_dmabuf(&dh, fb, winit_formats);
            }
        }
    }

    let initial_size = backend.window_size();
    let mode = Mode {
        size: initial_size,
        refresh: 60_000,
    };
    let output = Output::new(
        "velox-winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "velox-wm".into(),
            model: "Winit".into(),
        },
    );
    let _global = output.create_global::<VeloxState>(&dh);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.add_output(output.clone(), (0, 0).into());

    let wp_path = state.config.resolve_wallpaper_path();
    let wallpaper = Wallpaper::load_or_generate(
        backend.renderer(),
        &state.config.wallpaper_preset,
        wp_path.as_deref(),
        initial_size,
    );
    let damage_tracker = OutputDamageTracker::from_output(&output);

    // Watch Wayland socket for clients
    loop_handle.insert_source(
        Generic::new(listener, Interest::READ, TriggerMode::Level),
        |_, listener, app| {
            while let Ok(Some(stream)) = listener.accept() {
                if let Ok(client) = app.display.handle().insert_client(stream, new_client_data()) {
                    app.clients.push(client);
                }
            }
            Ok(PostAction::Continue)
        },
    )?;

    // Spawn XWayland so X11 desktop apps launch cleanly
    if let Ok((xwayland, xw_client)) = XWayland::spawn(
        &dh,
        None,
        std::iter::empty::<(String, String)>(),
        true,
        Stdio::null(),
        Stdio::null(),
        |_| (),
    ) {
        let loop_for_xw = loop_handle.clone();
        let _ = loop_handle.insert_source(xwayland, move |event, _, app| match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => {
                let disp = format!(":{display_number}");
                tracing::info!(display = %disp, "XWayland ready");
                app.state.x11_display = Some(disp.clone());
                unsafe {
                    std::env::set_var("DISPLAY", &disp);
                }
                match X11Wm::start_wm(loop_for_xw.clone(), x11_socket, xw_client.clone()) {
                    Ok(wm) => app.state.xwm = Some(wm),
                    Err(err) => tracing::warn!(?err, "Failed to start X11Wm"),
                }
            }
            XWaylandEvent::Error => {
                tracing::warn!("XWayland failed to start");
            }
        });
    }

    let mut app = WinitApp {
        state,
        display,
        backend,
        winit,
        output,
        damage_tracker,
        wallpaper,
        start_time: Instant::now(),
        last_frame: Instant::now(),
        clients: Vec::new(),
    };

    // Auto-launch terminal in nested mode for immediate usability
    spawn_detached(&app.state.config.terminal_cmd);
    app.state.spawn_restored_windows();

    let timer = Timer::from_duration(Duration::from_millis(8));
    loop_handle.insert_source(timer, |_, _, app| {
        if let Err(err) = tick_winit(app) {
            tracing::error!(?err, "Winit render tick error");
            app.state.should_quit = true;
        }
        TimeoutAction::ToDuration(Duration::from_millis(8))
    })?;

    while !app.state.should_quit {
        event_loop.dispatch(Some(Duration::from_millis(16)), &mut app)?;
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
        app.display.flush_clients()?;
    }

    let reload_requested = app.state.reload_requested;
    app.state.notif_svc.shutdown();
    app.state.update_watcher.shutdown();
    drop(app);
    Ok(reload_requested)
}

fn tick_winit(app: &mut WinitApp) -> Result<(), Box<dyn std::error::Error>> {
    let WinitApp {
        state,
        display,
        backend,
        winit,
        output,
        damage_tracker,
        wallpaper,
        start_time,
        last_frame,
        ..
    } = app;

    let status = winit.dispatch_new_events(|event| match event {
        WinitEvent::Resized { size, .. } => {
            output.change_current_state(
                Some(Mode {
                    size,
                    refresh: 60_000,
                }),
                None,
                None,
                None,
            );
            layer_map_for_output(output).arrange();
            state.relayout_active_workspace();
        }
        WinitEvent::Input(event) => process_input_event(state, event),
        _ => {}
    });

    if matches!(status, PumpStatus::Exit(_)) {
        state.should_quit = true;
        return Ok(());
    }

    state.refresh_system_snapshot();
    let size = backend.window_size();

    if state.wallpaper_dirty || wallpaper.as_ref().is_some_and(|w| !w.matches_size(size)) {
        state.wallpaper_dirty = false;
        let wp_path = state.config.resolve_wallpaper_path();
        *wallpaper = Wallpaper::load_or_generate(
            backend.renderer(),
            &state.config.wallpaper_preset,
            wp_path.as_deref(),
            size,
        );
    }

    let now = Instant::now();
    let dt = (now - *last_frame).as_secs_f32().clamp(0.001, 0.05);
    *last_frame = now;

    let full_damage = Rectangle::from_size(size);

    {
        let age = backend.buffer_age().unwrap_or(0);
        let (renderer, mut framebuffer) = backend.bind()?;
        let elements = compose_output_elements(
            state,
            renderer,
            output,
            size,
            wallpaper.as_ref(),
            None,
            dt,
        );
        let _ = render_with_damage_tracker(
            damage_tracker,
            renderer,
            &mut framebuffer,
            age,
            &elements,
            state.config.background_color,
        )?;

        let elapsed = start_time.elapsed();
        for window in state.active_space().elements().cloned().collect::<Vec<_>>() {
            if !state.config.freeze_hidden_windows
                || state.is_window_visible_on_output(output, &window)
            {
                window.send_frame(output, elapsed, Some(Duration::ZERO), |_, _| {
                    Some(output.clone())
                });
            }
        }
        state.send_hidden_window_keepalives(elapsed);
        for layer in layer_map_for_output(output).layers() {
            layer.send_frame(output, elapsed, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
        state.active_space_mut().refresh();
        state.popups.cleanup();

        display.dispatch_clients(state)?;
        display.flush_clients()?;
    }

    backend.submit(Some(&[full_damage]))?;
    Ok(())
}
