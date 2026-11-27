use std::{
    cell::RefCell,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use smithay::{
    backend::renderer::{
        sync::Fence,
        utils::{on_commit_buffer_handler, with_renderer_surface_state},
    },
    delegate_compositor, delegate_output, delegate_shm,
    desktop::PopupKind,
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_surface::WlSurface},
        Client, Resource,
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            add_blocker, add_pre_commit_hook, with_states, CompositorClientState, CompositorHandler,
            CompositorState,
        },
        drm_syncobj::DrmSyncobjCachedState,
        output::OutputHandler,
        seat::WaylandFocus,
        shell::xdg::{XdgPopupSurfaceData, XdgToplevelSurfaceData},
        shm::{ShmHandler, ShmState},
    },
};

use crate::{
    grabs::{handle_resize_commit, ResizeData, ResizeState},
    handlers::layer_shell::handle_layer_commit,
    state::{ClientState, VeloxState},
};

impl BufferHandler for VeloxState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl CompositorHandler for VeloxState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        static FALLBACK: std::sync::OnceLock<CompositorClientState> = std::sync::OnceLock::new();
        if let Some(state) = client.get_data::<ClientState>() {
            &state.compositor_state
        } else if let Some(state) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            &state.compositor_state
        } else {
            tracing::warn!("Unknown Wayland client data type; using fallback compositor state");
            FALLBACK.get_or_init(CompositorClientState::default)
        }
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, move |state, _dh, surface| {
            let maybe_sync = with_states(surface, |surface_data| {
                surface_data
                    .cached_state
                    .get::<DrmSyncobjCachedState>()
                    .pending()
                    .acquire_point
                    .clone()
            });
            if let Some(sync_point) = maybe_sync {
                if !sync_point.is_signaled() {
                    if let Ok((blocker, source)) = sync_point.generate_blocker() {
                        if let Some(client) = surface.client() {
                            add_blocker(surface, blocker);
                            state.pending_sync_sources.push((client, source));
                        }
                    }
                }
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        let win_loc = self.find_window_location(surface);
        let commit_area = with_renderer_surface_state(surface, |rss| {
            rss.buffer_size()
                .map(|s| s.w.saturating_mul(s.h))
                .unwrap_or(0)
        })
        .unwrap_or(0);

        if let Some((ws_idx, window)) = win_loc {
            let geom_empty = window.geometry().size.w <= 0 || window.geometry().size.h <= 0;
            let visible = !self.config.freeze_hidden_windows
                || geom_empty
                || self.is_window_visible_anywhere(ws_idx, &window);

            if visible {
                self.needs_redraw = true;
                self.last_visible_commit_at = std::time::Instant::now();
                if commit_area > 0 {
                    let mut hasher = DefaultHasher::new();
                    VeloxState::root_wl_surface(surface).id().hash(&mut hasher);
                    let sid = hasher.finish();
                    let animating = self.animator.any_active() || self.ui.is_animating();
                    let is_game = self.is_game_window(&window);
                    self.cadence.record_commit(sid, commit_area, animating, is_game);
                }
            }

            window.on_commit();
            self.sync_fractional_scale_for_surface(surface, &window);
            let check_count = {
                let mut meta = VeloxState::window_meta_mut(&window);
                if meta.restore_fallback_checks < 4 {
                    meta.restore_fallback_checks += 1;
                    if meta.restore_fallback_checks <= 2 && meta.is_game == Some(false) {
                        meta.is_game = None;
                    }
                    meta.restore_fallback_checks
                } else {
                    4
                }
            };
            if !self.pending_restores.is_empty()
                && check_count <= 4
                && VeloxState::window_meta(&window).order_hint.is_none()
            {
                self.try_apply_window_restore(&window, true);
            } else if ws_idx == self.active_workspace {
                let was_resizing = window
                    .wl_surface()
                    .map(|s| {
                        with_states(&s, |states| {
                            states
                                .data_map
                                .get::<RefCell<ResizeData>>()
                                .map(|cell| !matches!(cell.borrow().state, ResizeState::Idle))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);
                handle_resize_commit(self.active_space_mut(), &window);
                if was_resizing {
                    if let Some(loc) = self.active_space().element_location(&window) {
                        let rect = smithay::utils::Rectangle::new(loc, window.geometry().size);
                        let win_id = (window.user_data() as *const _) as usize as u64;
                        self.animator.snap(win_id, rect);
                    }
                } else if !self.pointer.is_grabbed() {
                    self.clamp_committed_window_after_restore(&window);
                }
            }

            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .map(|data| data.lock().unwrap().initial_configure_sent)
                    .unwrap_or(true)
            });

            if !initial_configure_sent {
                if let Some(toplevel) = window.toplevel() {
                    toplevel.send_configure();
                }
            }
        } else {
            self.needs_redraw = true;
            self.last_visible_commit_at = std::time::Instant::now();
        }

        handle_layer_commit(self, surface);
        self.popups.commit(surface);

        // Configure newly committed Wayland xdg_popups so menus/submenus inside terminals & apps appear reliably.
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface) {
            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<XdgPopupSurfaceData>()
                    .map(|data| data.lock().unwrap().initial_configure_sent)
                    .unwrap_or(true)
            });
            if !initial_configure_sent {
                let _ = popup.send_configure();
            }
        }
    }
}

impl ShmHandler for VeloxState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl OutputHandler for VeloxState {}

delegate_compositor!(VeloxState);
delegate_shm!(VeloxState);
delegate_output!(VeloxState);
