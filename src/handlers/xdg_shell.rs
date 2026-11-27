use smithay::{
    delegate_xdg_shell,
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, PopupKeyboardGrab, PopupKind,
        PopupPointerGrab, Window,
    },
    input::pointer::{Focus, GrabStartData},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::{wl_output, wl_seat},
    },
    utils::{Rectangle, Serial},
    wayland::shell::xdg::{
        PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    },
};

use crate::{
    grabs::{nearest_resize_edges, MoveSurfaceGrab, ResizeEdge, ResizeSurfaceGrab},
    state::VeloxState,
};

impl XdgShellHandler for VeloxState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface.clone());
        VeloxState::window_meta_mut(&window).taskbar_order = self.alloc_window_seq();

        let count = self.active_space().elements().count() as i32;
        let offset = 32 + (count % 8) * 28;
        let usable = self.usable_workspace_rect();
        self.active_space_mut()
            .map_element(window.clone(), (usable.loc.x + offset, usable.loc.y + offset), true);

        if !self.try_apply_window_restore(&window, false) {
            self.relayout_active_workspace();
            if !self.has_pending_focused_restore_missing() {
                self.focus_window(Some(window));
            }
        }
        surface.send_configure();
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        unconstrain_popup(self, &surface, positioner);
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface.clone())) {
            tracing::warn!(?err, "Failed to track xdg popup");
        }
        let _ = surface.send_configure();
        self.needs_redraw = true;
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        let pointer = self.pointer.clone();
        if !pointer.has_grab(serial) {
            return;
        }
        let Some(start_data) = pointer.grab_start_data() else {
            return;
        };
        let Some(window) = self
            .active_space()
            .elements()
            .find(|w| w.toplevel().is_some_and(|tl| tl == &surface))
            .cloned()
        else {
            return;
        };
        self.prepare_move_grab(&window, start_data.location);
        let initial_window_location = self
            .active_space()
            .element_location(&window)
            .unwrap_or_default();
        let usable = self.usable_rect_for_window(&window);
        let grab = MoveSurfaceGrab::start(start_data, window, initial_window_location, usable);
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let pointer = self.pointer.clone();
        if !pointer.has_grab(serial) {
            return;
        }
        let Some(start_data) = pointer.grab_start_data() else {
            return;
        };
        let Some(window) = self
            .active_space()
            .elements()
            .find(|w| w.toplevel().is_some_and(|tl| tl == &surface))
            .cloned()
        else {
            return;
        };
        {
            let mut meta = VeloxState::window_meta_mut(&window);
            meta.floating = true;
            meta.maximized = false;
            meta.snapped = None;
        }
        let _ = surface.with_pending_state(|s| s.states.unset(xdg_toplevel::State::Maximized));
        let initial_loc = self
            .active_space()
            .element_location(&window)
            .unwrap_or_default();
        let initial_size = window.geometry().size;
        let initial_rect = Rectangle::new(initial_loc, initial_size);

        let grab = ResizeSurfaceGrab::start(
            start_data,
            window,
            ResizeEdge::from(edges),
            initial_rect,
        );
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    fn grab(&mut self, surface: PopupSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        let popup = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&popup) else {
            return;
        };
        let seat = self.seat.clone();
        if let Ok(grab) = self.popups.grab_popup(root.into(), popup, &seat, serial) {
            let kbd = self.keyboard.clone();
            let ptr = self.pointer.clone();
            ptr.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
            if let Some(focus) = grab.current_grab() {
                kbd.set_focus(self, Some(focus), serial);
                kbd.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
            }
        }
        self.needs_redraw = true;
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(win) = self.window_for_surface(surface.wl_surface()) {
            self.maximize_window_for(&win);
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(win) = self.window_for_surface(surface.wl_surface()) {
            self.unmaximize_window_for(&win);
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<wl_output::WlOutput>) {
        if let Some(win) = self.window_for_surface(surface.wl_surface()) {
            self.set_fullscreen_for(&win, true);
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(win) = self.window_for_surface(surface.wl_surface()) {
            self.set_fullscreen_for(&win, false);
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(win) = self.window_for_surface(surface.wl_surface()) {
            self.minimize_window_for(&win);
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        unconstrain_popup(self, &surface, positioner);
        surface.send_repositioned(token);
        self.needs_redraw = true;
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_window_everywhere(surface.wl_surface());
    }
}

fn unconstrain_popup(state: &VeloxState, popup: &PopupSurface, positioner: PositionerState) {
    let popup_kind = PopupKind::Xdg(popup.clone());
    let Ok(root) = find_popup_root_surface(&popup_kind) else {
        return;
    };
    let Some(window) = state.window_for_surface(&root) else {
        return;
    };
    let Some(output_geo) = state
        .output_for_window(&window)
        .or_else(|| state.outputs.first().cloned())
        .and_then(|o| state.active_space().output_geometry(&o))
    else {
        return;
    };
    let win_loc = state
        .active_space()
        .element_location(&window)
        .unwrap_or_default();
    let mut target = output_geo;
    target.loc -= win_loc;
    target.loc -= get_popup_toplevel_coords(&popup_kind);

    popup.with_pending_state(|s| {
        s.geometry = positioner.get_unconstrained_geometry(target);
        s.positioner = positioner;
    });
}

/// Starts a compositor-initiated move grab (e.g. Super/Alt + Left Click).
pub fn start_move_grab(state: &mut VeloxState, window: Window, serial: Serial, button: u32) {
    state.prepare_move_grab(&window, state.pointer_location);
    let pointer = state.pointer.clone();
    let initial_window_location = state
        .active_space()
        .element_location(&window)
        .unwrap_or_default();
    let start_data = GrabStartData {
        focus: None,
        button,
        location: state.pointer_location,
    };
    let usable = state.usable_rect_for_window(&window);
    let grab = MoveSurfaceGrab::start(start_data, window, initial_window_location, usable);
    pointer.set_grab(state, grab, serial, Focus::Clear);
}

/// Starts a compositor-initiated resize grab (e.g. Super/Alt + Right Click).
pub fn start_resize_grab(state: &mut VeloxState, window: Window, serial: Serial, button: u32) {
    {
        let mut meta = VeloxState::window_meta_mut(&window);
        meta.floating = true;
        meta.maximized = false;
        meta.snapped = None;
    }
    if let Some(tl) = window.toplevel() {
        tl.with_pending_state(|s| {
            s.states.unset(xdg_toplevel::State::Maximized);
        });
    }
    let pointer = state.pointer.clone();
    let initial_loc = state
        .active_space()
        .element_location(&window)
        .unwrap_or_default();
    let initial_size = window.geometry().size;
    let initial_rect = Rectangle::new(initial_loc, initial_size);
    let start_data = GrabStartData {
        focus: None,
        button,
        location: state.pointer_location,
    };
    let edges = nearest_resize_edges(state.pointer_location, initial_rect);
    let grab = ResizeSurfaceGrab::start(start_data, window, edges, initial_rect);
    pointer.set_grab(state, grab, serial, Focus::Clear);
}

delegate_xdg_shell!(VeloxState);
