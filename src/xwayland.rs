use std::os::unix::io::OwnedFd;

use smithay::{
    delegate_xwayland_shell,
    desktop::Window,
    reexports::{
        calloop::LoopHandle,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle, Size, SERIAL_COUNTER},
    wayland::{
        selection::{
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_selection,
            },
            SelectionTarget,
        },
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        xwm::{Reorder, ResizeEdge as X11ResizeEdge, XwmId},
        X11Surface, X11Wm, XwmHandler,
    },
};

use crate::{
    grabs::{ResizeEdge, ResizeSurfaceGrab},
    state::{FocusTarget, VeloxSelectionData, VeloxState},
};

impl XWaylandShellHandler for VeloxState {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }

    fn surface_associated(&mut self, _xwm_id: XwmId, _surface: WlSurface, window: X11Surface) {
        tracing::debug!(title = %window.title(), "XWayland surface associated");
        // If this X11 window is active/focused (e.g., mapped before its WlSurface existed),
        // re-assign keyboard focus now so both X11 XSetInputFocus and wl_keyboard::enter fire.
        let target = FocusTarget::X11(window.clone());
        let already_focused = self.keyboard.current_focus().as_ref() == Some(&target);
        let top_unfocused = self.keyboard.current_focus().is_none()
            && self
                .active_space()
                .elements()
                .rev()
                .find(|w| {
                    let m = VeloxState::window_meta(w);
                    !m.override_redirect && !m.minimized
                })
                .is_some_and(|w| w.x11_surface() == Some(&window));
        if already_focused || top_unfocused {
            let kbd = self.keyboard.clone();
            let s1 = SERIAL_COUNTER.next_serial();
            let s2 = SERIAL_COUNTER.next_serial();
            if already_focused {
                kbd.set_focus(self, None, s1);
            }
            kbd.set_focus(self, Some(target), s2);
            self.sync_focused_pointer_constraint();
        }
        self.wake_workspace_views();
        self.needs_redraw = true;
    }
}

impl XwmHandler for VeloxState {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("X11Wm not initialized")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Err(err) = window.set_mapped(true) {
            tracing::warn!(?err, "Failed to map X11 window");
            return;
        }
        let win = Window::new_x11_window(window.clone());
        VeloxState::window_meta_mut(&win).taskbar_order = self.alloc_window_seq();
        let count = self.active_space().elements().count() as i32;
        let offset = 40 + (count % 8) * 28;
        let usable = self.usable_workspace_rect();
        self.active_space_mut()
            .map_element(win.clone(), (usable.loc.x + offset, usable.loc.y + offset), true);
        self.relayout_active_workspace();
        self.focus_window(Some(win));
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let loc = window.geometry().loc;
        let win = Window::new_x11_window(window);
        {
            let mut meta = VeloxState::window_meta_mut(&win);
            meta.floating = true;
            meta.override_redirect = true;
        }
        self.active_space_mut().map_element(win, loc, true);
        self.needs_redraw = true;
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let found = self
            .workspaces
            .iter()
            .flat_map(|ws| ws.elements())
            .find(|w| w.x11_surface() == Some(&window))
            .cloned();
        if let Some(win) = found {
            self.remove_window_elem(&win);
        }
        if !window.is_override_redirect() {
            let _ = window.set_mapped(false);
        }
        self.needs_redraw = true;
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let found = self
            .workspaces
            .iter()
            .flat_map(|ws| ws.elements())
            .find(|w| w.x11_surface() == Some(&window))
            .cloned();
        if let Some(win) = found {
            self.remove_window_elem(&win);
        }
        self.needs_redraw = true;
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let managed_win = self
            .active_space()
            .elements()
            .find(|win| win.x11_surface() == Some(&window))
            .cloned();
        // For managed tiled windows, enforce compositor geometry instead of letting apps self-reposition.
        if let Some(ref win) = managed_win {
            let (is_tiled, was_fullscreen, last_rect) = {
                let meta = VeloxState::window_meta(win);
                (
                    self.config.auto_tile && !meta.floating && !meta.override_redirect,
                    meta.fullscreen,
                    meta.last_configured_rect,
                )
            };
            if is_tiled {
                if let Some(rect) = last_rect {
                    let _ = window.configure(rect);
                    return;
                }
                if !was_fullscreen {
                    self.relayout_active_workspace();
                    if let Some(rect) = VeloxState::window_meta(win).last_configured_rect {
                        let _ = window.configure(rect);
                        return;
                    }
                }
            }
        }

        let mut geo = window.geometry();
        if let Some(ref win) = managed_win {
            let (clamp_commits, is_fs, last_rect) = {
                let meta = VeloxState::window_meta(win);
                (meta.restore_clamp_commits, meta.fullscreen, meta.last_configured_rect)
            };
            if !is_fs && clamp_commits > 0 {
                if let Some(lr) = last_rect {
                    if w.is_none() || h.is_none() {
                        geo = lr;
                    }
                }
            }
        }
        if let Some(nx) = x {
            geo.loc.x = nx;
        }
        if let Some(ny) = y {
            geo.loc.y = ny;
        }
        if let Some(nw) = w {
            geo.size.w = nw as i32;
        }
        if let Some(nh) = h {
            geo.size.h = nh as i32;
        }
        if !window.is_override_redirect() {
            if let Some(ref win) = managed_win {
                let fs = VeloxState::window_meta(win).fullscreen;
                geo = self.clamp_rect_to_best_output(win, geo, fs);
            }
        }
        let _ = window.configure(geo);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        let found = self
            .active_space()
            .elements()
            .find(|w| w.x11_surface() == Some(&window))
            .cloned();
        if let Some(win) = found {
            let (meta_floating, is_fullscreen, is_or, last_rect, clamp_commits, hold_until) = {
                let meta = VeloxState::window_meta(&win);
                (
                    !self.config.auto_tile || meta.floating || meta.override_redirect,
                    meta.fullscreen,
                    meta.override_redirect,
                    meta.last_configured_rect,
                    meta.restore_clamp_commits,
                    meta.unfullscreen_hold_until,
                )
            };
            if meta_floating {
                let out_size = self
                    .output_for_window(&win)
                    .and_then(|o| self.active_space().output_geometry(&o))
                    .map(|g| g.size)
                    .unwrap_or_default();
                let stale_fs_notify = !is_fullscreen
                    && (clamp_commits > 0 || hold_until.is_some())
                    && out_size.w > 0
                    && geometry.size.w >= out_size.w
                    && geometry.size.h >= out_size.h
                    && last_rect.is_some_and(|r| r.size != geometry.size);
                if stale_fs_notify {
                    return;
                }
                let win_id = (win.user_data() as *const _) as usize as u64;
                let clamped = if is_or || self.pointer.is_grabbed() {
                    geometry
                } else {
                    self.clamp_rect_to_best_output(&win, geometry, is_fullscreen)
                };
                self.active_space_mut()
                    .map_element(win.clone(), clamped.loc, false);
                if clamped != geometry && last_rect != Some(clamped) {
                    VeloxState::window_meta_mut(&win).last_configured_rect = Some(clamped);
                    let _ = window.configure(clamped);
                }
                if self.pointer.is_grabbed() {
                    self.animator.snap(win_id, clamped);
                }
            }
        }
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        _button: u32,
        resize_edge: X11ResizeEdge,
    ) {
        let Some(win) = self
            .active_space()
            .elements()
            .find(|w| w.x11_surface() == Some(&window))
            .cloned()
        else {
            return;
        };
        let Some(start_data) = self.pointer.grab_start_data() else {
            return;
        };
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        VeloxState::window_meta_mut(&win).floating = true;
        let loc = self.active_space().element_location(&win).unwrap_or_default();
        let size: Size<i32, Logical> = win.geometry().size;
        let edge = match resize_edge {
            X11ResizeEdge::Top => ResizeEdge::TOP,
            X11ResizeEdge::Bottom => ResizeEdge::BOTTOM,
            X11ResizeEdge::Left => ResizeEdge::LEFT,
            X11ResizeEdge::Right => ResizeEdge::RIGHT,
            X11ResizeEdge::TopLeft => ResizeEdge::TOP_LEFT,
            X11ResizeEdge::TopRight => ResizeEdge::TOP_RIGHT,
            X11ResizeEdge::BottomLeft => ResizeEdge::BOTTOM_LEFT,
            _ => ResizeEdge::BOTTOM_RIGHT,
        };
        let grab = ResizeSurfaceGrab::start(start_data, win, edge, Rectangle::new(loc, size));
        self.pointer
            .clone()
            .set_grab(self, grab, serial, smithay::input::pointer::Focus::Clear);
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        let Some(win) = self
            .active_space()
            .elements()
            .find(|w| w.x11_surface() == Some(&window))
            .cloned()
        else {
            return;
        };
        let Some(start_data) = self.pointer.grab_start_data() else {
            return;
        };
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        self.prepare_move_grab(&win, start_data.location);
        let loc: Point<i32, Logical> = self.active_space().element_location(&win).unwrap_or_default();
        let usable = self.usable_rect_for_window(&win);
        let grab = crate::grabs::MoveSurfaceGrab::start(start_data, win, loc, usable);
        self.pointer
            .clone()
            .set_grab(self, grab, serial, smithay::input::pointer::Focus::Clear);
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let found = self
            .active_space()
            .elements()
            .find(|w| w.x11_surface() == Some(&window))
            .cloned();
        if let Some(win) = found {
            self.maximize_window_for(&win);
        } else {
            let _ = window.set_maximized(true);
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let found = self
            .active_space()
            .elements()
            .find(|w| w.x11_surface() == Some(&window))
            .cloned();
        if let Some(win) = found {
            self.unmaximize_window_for(&win);
        } else {
            let _ = window.set_maximized(false);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let found = self
            .active_space()
            .elements()
            .find(|w| w.x11_surface() == Some(&window))
            .cloned();
        if let Some(win) = found {
            self.set_fullscreen_for(&win, true);
        } else {
            let _ = window.set_fullscreen(true);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let found = self
            .active_space()
            .elements()
            .find(|w| w.x11_surface() == Some(&window))
            .cloned();
        if let Some(win) = found {
            self.set_fullscreen_for(&win, false);
        } else {
            let _ = window.set_fullscreen(false);
        }
    }

    fn allow_selection_access(&mut self, _xwm: XwmId, _selection: SelectionTarget) -> bool {
        true
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(
                    &self.display_handle,
                    &self.seat,
                    mime_types,
                    VeloxSelectionData::X11,
                );
            }
            SelectionTarget::Primary => {
                set_primary_selection(
                    &self.display_handle,
                    &self.seat,
                    mime_types,
                    VeloxSelectionData::X11,
                );
            }
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                let is_x11 = matches!(
                    current_data_device_selection_userdata(&self.seat).as_deref(),
                    Some(VeloxSelectionData::X11)
                );
                if is_x11 {
                    clear_data_device_selection(&self.display_handle, &self.seat);
                }
            }
            SelectionTarget::Primary => {
                let is_x11 = matches!(
                    current_primary_selection_userdata(&self.seat).as_deref(),
                    Some(VeloxSelectionData::X11)
                );
                if is_x11 {
                    clear_primary_selection(&self.display_handle, &self.seat);
                }
            }
        }
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        match selection {
            SelectionTarget::Clipboard => {
                if let Err(err) = request_data_device_client_selection(&self.seat, mime_type, fd) {
                    tracing::debug!(?err, "X11 clipboard read from Wayland source failed");
                }
            }
            SelectionTarget::Primary => {
                if let Err(err) = request_primary_client_selection(&self.seat, mime_type, fd) {
                    tracing::debug!(?err, "X11 primary read from Wayland source failed");
                }
            }
        }
    }
}

pub fn flush_pending_x11_selections<D: XwmHandler + 'static>(
    state: &mut VeloxState,
    loop_handle: &LoopHandle<'static, D>,
) {
    if state.pending_x11_selections.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut state.pending_x11_selections);
    let Some(xwm) = state.xwm.as_mut() else {
        return;
    };
    for (ty, mime_type, fd) in pending {
        if let Err(err) = xwm.send_selection(ty, mime_type, fd, loop_handle.clone()) {
            tracing::debug!(?err, ?ty, "Failed forwarding X11 selection to Wayland requester");
        }
    }
}

delegate_xwayland_shell!(VeloxState);

#[macro_export]
macro_rules! delegate_app_xwayland {
    ($app:ty) => {
        impl smithay::wayland::xwayland_shell::XWaylandShellHandler for $app {
            fn xwayland_shell_state(
                &mut self,
            ) -> &mut smithay::wayland::xwayland_shell::XWaylandShellState {
                self.state.xwayland_shell_state()
            }

            fn surface_associated(
                &mut self,
                xwm_id: smithay::xwayland::xwm::XwmId,
                surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
                window: smithay::xwayland::X11Surface,
            ) {
                self.state.surface_associated(xwm_id, surface, window);
            }
        }

        impl smithay::xwayland::XwmHandler for $app {
            fn xwm_state(&mut self, xwm: smithay::xwayland::xwm::XwmId) -> &mut smithay::xwayland::X11Wm {
                self.state.xwm_state(xwm)
            }

            fn new_window(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
            ) {
                self.state.new_window(xwm, window);
            }

            fn new_override_redirect_window(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
            ) {
                self.state.new_override_redirect_window(xwm, window);
            }

            fn map_window_request(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
            ) {
                self.state.map_window_request(xwm, window);
            }

            fn mapped_override_redirect_window(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
            ) {
                self.state.mapped_override_redirect_window(xwm, window);
            }

            fn unmapped_window(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
            ) {
                self.state.unmapped_window(xwm, window);
            }

            fn destroyed_window(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
            ) {
                self.state.destroyed_window(xwm, window);
            }

            fn configure_request(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
                x: Option<i32>,
                y: Option<i32>,
                w: Option<u32>,
                h: Option<u32>,
                reorder: Option<smithay::xwayland::xwm::Reorder>,
            ) {
                self.state.configure_request(xwm, window, x, y, w, h, reorder);
            }

            fn configure_notify(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
                geometry: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
                above: Option<u32>,
            ) {
                self.state.configure_notify(xwm, window, geometry, above);
            }

            fn resize_request(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
                button: u32,
                resize_edge: smithay::xwayland::xwm::ResizeEdge,
            ) {
                self.state.resize_request(xwm, window, button, resize_edge);
            }

            fn move_request(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
                button: u32,
            ) {
                self.state.move_request(xwm, window, button);
            }

            fn maximize_request(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
            ) {
                self.state.maximize_request(xwm, window);
            }

            fn unmaximize_request(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
            ) {
                self.state.unmaximize_request(xwm, window);
            }

            fn fullscreen_request(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
            ) {
                self.state.fullscreen_request(xwm, window);
            }

            fn unfullscreen_request(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                window: smithay::xwayland::X11Surface,
            ) {
                self.state.unfullscreen_request(xwm, window);
            }

            fn allow_selection_access(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                selection: smithay::wayland::selection::SelectionTarget,
            ) -> bool {
                self.state.allow_selection_access(xwm, selection)
            }

            fn new_selection(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                selection: smithay::wayland::selection::SelectionTarget,
                mime_types: Vec<String>,
            ) {
                self.state.new_selection(xwm, selection, mime_types);
            }

            fn cleared_selection(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                selection: smithay::wayland::selection::SelectionTarget,
            ) {
                self.state.cleared_selection(xwm, selection);
            }

            fn send_selection(
                &mut self,
                xwm: smithay::xwayland::xwm::XwmId,
                selection: smithay::wayland::selection::SelectionTarget,
                mime_type: String,
                fd: std::os::unix::io::OwnedFd,
            ) {
                self.state.send_selection(xwm, selection, mime_type, fd);
            }
        }
    };
}
