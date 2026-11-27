use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent,
        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData,
        MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle, Size},
    wayland::{compositor::with_states, shell::xdg::SurfaceCachedState},
};

use crate::state::VeloxState;
pub use crate::updates::SnapZone;

/// Computes which snap zone the pointer is currently hovering near screen edges.
pub fn snap_zone_for_point(
    pos: Point<f64, Logical>,
    usable: Rectangle<i32, Logical>,
) -> Option<SnapZone> {
    let margin = 16.0;
    let corner = 120.0;
    let x = pos.x;
    let y = pos.y;
    let left = usable.loc.x as f64;
    let top = usable.loc.y as f64;
    let right = (usable.loc.x + usable.size.w) as f64;
    let bottom = (usable.loc.y + usable.size.h) as f64;

    let near_left = x <= left + margin;
    let near_right = x >= right - margin;
    let near_top = y <= top + margin;
    let near_bottom = y >= bottom - margin;

    if (near_left && y <= top + corner) || (near_top && x <= left + corner) {
        Some(SnapZone::TopLeft)
    } else if (near_right && y <= top + corner) || (near_top && x >= right - corner) {
        Some(SnapZone::TopRight)
    } else if (near_left && y >= bottom - corner) || (near_bottom && x <= left + corner) {
        Some(SnapZone::BottomLeft)
    } else if (near_right && y >= bottom - corner) || (near_bottom && x >= right - corner) {
        Some(SnapZone::BottomRight)
    } else if near_top {
        Some(SnapZone::Maximize)
    } else if near_left {
        Some(SnapZone::LeftHalf)
    } else if near_right {
        Some(SnapZone::RightHalf)
    } else {
        None
    }
}

pub fn rect_for_snap_zone(
    zone: SnapZone,
    usable: Rectangle<i32, Logical>,
    gap: i32,
) -> Rectangle<i32, Logical> {
    let x = usable.loc.x + gap;
    let y = usable.loc.y + gap;
    let w = (usable.size.w - gap * 2).max(160);
    let h = (usable.size.h - gap * 2).max(160);
    let half_w = ((w - gap) / 2).max(100);
    let half_h = ((h - gap) / 2).max(100);

    match zone {
        SnapZone::Maximize => Rectangle::new((x, y).into(), (w, h).into()),
        SnapZone::LeftHalf => Rectangle::new((x, y).into(), (half_w, h).into()),
        SnapZone::RightHalf => Rectangle::new((x + half_w + gap, y).into(), (w - half_w - gap, h).into()),
        SnapZone::TopLeft => Rectangle::new((x, y).into(), (half_w, half_h).into()),
        SnapZone::TopRight => {
            Rectangle::new((x + half_w + gap, y).into(), (w - half_w - gap, half_h).into())
        }
        SnapZone::BottomLeft => {
            Rectangle::new((x, y + half_h + gap).into(), (half_w, h - half_h - gap).into())
        }
        SnapZone::BottomRight => Rectangle::new(
            (x + half_w + gap, y + half_h + gap).into(),
            (w - half_w - gap, h - half_h - gap).into(),
        ),
    }
}

/// Interactive pointer grab for zero-latency window moving with edge snap preview.
pub struct MoveSurfaceGrab {
    pub start_data: GrabStartData<VeloxState>,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
    pub last_location: Point<i32, Logical>,
    pub usable_rect: Rectangle<i32, Logical>,
}

impl MoveSurfaceGrab {
    pub fn start(
        start_data: GrabStartData<VeloxState>,
        window: Window,
        initial_window_location: Point<i32, Logical>,
        usable_rect: Rectangle<i32, Logical>,
    ) -> Self {
        Self {
            start_data,
            window,
            initial_window_location,
            last_location: initial_window_location,
            usable_rect,
        }
    }
}

impl PointerGrab<VeloxState> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        _focus: Option<(
            <VeloxState as smithay::input::SeatHandler>::PointerFocus,
            Point<f64, Logical>,
        )>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        data.pointer_location = event.location;

        let delta = event.location - self.start_data.location;
        let raw_location = (self.initial_window_location.to_f64() + delta).to_i32_round();
        let geom = self.window.geometry();
        let new_location = data.clamp_move_location(geom.size, raw_location, event.location);
        if new_location != self.last_location {
            self.last_location = new_location;
            let win_id = (self.window.user_data() as *const _) as usize as u64;
            data.animator
                .snap(win_id, Rectangle::new(new_location, geom.size));
            data.active_space_mut()
                .map_element(self.window.clone(), new_location, false);
        }

        if data.config.edge_snap {
            self.usable_rect = data.usable_workspace_rect_at(event.location);
            let next_preview = snap_zone_for_point(event.location, self.usable_rect)
                .map(|z| rect_for_snap_zone(z, self.usable_rect, data.config.gap_px.clamp(0, 32)));
            if next_preview != data.snap_preview {
                data.snap_preview = next_preview;
            }
        } else {
            data.snap_preview = None;
        }
        data.needs_redraw = true;
    }

    fn relative_motion(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        focus: Option<(
            <VeloxState as smithay::input::SeatHandler>::PointerFocus,
            Point<f64, Logical>,
        )>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start_data.button) {
            if let Some(snap_rect) = data.snap_preview.take() {
                if let Some(zone) = snap_zone_for_point(data.pointer_location, self.usable_rect) {
                    data.apply_snap_zone_for(&self.window, zone, snap_rect);
                } else {
                    VeloxState::window_meta_mut(&self.window).floating = true;
                    data.apply_window_bounds_force(
                        &self.window,
                        (snap_rect.loc.x, snap_rect.loc.y),
                        (snap_rect.size.w, snap_rect.size.h),
                    );
                }
            }
            handle.unset_grab(self, data, event.serial, event.time, true);
            data.needs_redraw = true;
        }
    }

    fn axis(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut VeloxState, handle: &mut PointerInnerHandle<'_, VeloxState>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<VeloxState> {
        &self.start_data
    }

    fn unset(&mut self, data: &mut VeloxState) {
        data.snap_preview = None;
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ResizeEdge: u32 {
        const TOP          = 0b0001;
        const BOTTOM       = 0b0010;
        const LEFT         = 0b0100;
        const RIGHT        = 0b1000;
        const TOP_LEFT     = Self::TOP.bits() | Self::LEFT.bits();
        const BOTTOM_LEFT  = Self::BOTTOM.bits() | Self::LEFT.bits();
        const TOP_RIGHT    = Self::TOP.bits() | Self::RIGHT.bits();
        const BOTTOM_RIGHT = Self::BOTTOM.bits() | Self::RIGHT.bits();
    }
}

impl From<xdg_toplevel::ResizeEdge> for ResizeEdge {
    fn from(edge: xdg_toplevel::ResizeEdge) -> Self {
        match edge {
            xdg_toplevel::ResizeEdge::Top => Self::TOP,
            xdg_toplevel::ResizeEdge::Bottom => Self::BOTTOM,
            xdg_toplevel::ResizeEdge::Left => Self::LEFT,
            xdg_toplevel::ResizeEdge::Right => Self::RIGHT,
            xdg_toplevel::ResizeEdge::TopLeft => Self::TOP_LEFT,
            xdg_toplevel::ResizeEdge::BottomLeft => Self::BOTTOM_LEFT,
            xdg_toplevel::ResizeEdge::TopRight => Self::TOP_RIGHT,
            xdg_toplevel::ResizeEdge::BottomRight => Self::BOTTOM_RIGHT,
            _ => Self::BOTTOM_RIGHT,
        }
    }
}

/// Picks the intuitive resize edge/corner based on where the pointer sits inside `rect`.
pub fn nearest_resize_edges(pos: Point<f64, Logical>, rect: Rectangle<i32, Logical>) -> ResizeEdge {
    let rel_x = (pos.x - rect.loc.x as f64) / rect.size.w.max(1) as f64;
    let rel_y = (pos.y - rect.loc.y as f64) / rect.size.h.max(1) as f64;

    let mut edges = ResizeEdge::empty();
    if rel_x < 0.35 {
        edges |= ResizeEdge::LEFT;
    } else if rel_x > 0.65 {
        edges |= ResizeEdge::RIGHT;
    }
    if rel_y < 0.35 {
        edges |= ResizeEdge::TOP;
    } else if rel_y > 0.65 {
        edges |= ResizeEdge::BOTTOM;
    }

    if edges.is_empty() {
        ResizeEdge::BOTTOM_RIGHT
    } else {
        edges
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeState {
    Idle,
    Resizing {
        edges: ResizeEdge,
        initial_window_location: Point<i32, Logical>,
        initial_window_size: Size<i32, Logical>,
    },
    WaitingForLastCommit {
        edges: ResizeEdge,
        initial_window_location: Point<i32, Logical>,
        initial_window_size: Size<i32, Logical>,
    },
}

#[derive(Debug)]
pub struct ResizeData {
    pub state: ResizeState,
}

impl Default for ResizeData {
    fn default() -> Self {
        Self {
            state: ResizeState::Idle,
        }
    }
}

/// Interactive pointer grab for resizing a window smoothly with configure rate-limiting
/// so 1000Hz–4000Hz gaming mice never flood apps with GPU buffer reallocations.
pub struct ResizeSurfaceGrab {
    pub start_data: GrabStartData<VeloxState>,
    pub window: Window,
    pub edges: ResizeEdge,
    pub initial_rect: Rectangle<i32, Logical>,
    pub last_window_size: Size<i32, Logical>,
    last_configure_sent: Instant,
}

impl ResizeSurfaceGrab {
    pub fn start(
        start_data: GrabStartData<VeloxState>,
        window: Window,
        edges: ResizeEdge,
        initial_window_rect: Rectangle<i32, Logical>,
    ) -> Self {
        if let Some(toplevel) = window.toplevel() {
            with_states(toplevel.wl_surface(), |states| {
                let mut resize_data = states
                    .data_map
                    .get_or_insert(|| RefCell::new(ResizeData::default()))
                    .borrow_mut();
                resize_data.state = ResizeState::Resizing {
                    edges,
                    initial_window_location: initial_window_rect.loc,
                    initial_window_size: initial_window_rect.size,
                };
            });
        }

        Self {
            start_data,
            window,
            edges,
            initial_rect: initial_window_rect,
            last_window_size: initial_window_rect.size,
            last_configure_sent: Instant::now() - Duration::from_millis(20),
        }
    }

    fn client_min_size(&self) -> Size<i32, Logical> {
        if let Some(toplevel) = self.window.toplevel() {
            with_states(toplevel.wl_surface(), |states| {
                states
                    .cached_state
                    .get::<SurfaceCachedState>()
                    .current()
                    .min_size
            })
        } else if let Some(x11) = self.window.x11_surface() {
            x11.min_size().unwrap_or_default()
        } else {
            Size::default()
        }
    }

    fn emit_configure(&mut self, data: &mut VeloxState, resizing: bool) {
        let min_size = self.client_min_size();
        let minimal = data.config.minimal_view_enabled
            && VeloxState::should_use_minimal_view(self.last_window_size, min_size);

        let mut loc = data
            .active_space()
            .element_location(&self.window)
            .unwrap_or(self.initial_rect.loc);
        if self.edges.intersects(ResizeEdge::LEFT) {
            loc.x = self.initial_rect.loc.x + (self.initial_rect.size.w - self.last_window_size.w);
        }
        if self.edges.intersects(ResizeEdge::TOP) {
            loc.y = self.initial_rect.loc.y + (self.initial_rect.size.h - self.last_window_size.h);
        }

        if minimal {
            {
                let mut meta = VeloxState::window_meta_mut(&self.window);
                meta.minimal_view = true;
                meta.minimal_slot_size = self.last_window_size;
            }
            let rect = Rectangle::new(loc, self.last_window_size);
            data.active_space_mut()
                .map_element(self.window.clone(), loc, false);
            let win_id = (self.window.user_data() as *const _) as usize as u64;
            data.animator.snap(win_id, rect);
        } else {
            {
                let mut meta = VeloxState::window_meta_mut(&self.window);
                if meta.minimal_view {
                    meta.minimal_view = false;
                    meta.minimal_card_sig.clear();
                }
            }
            if let Some(toplevel) = self.window.toplevel() {
                toplevel.with_pending_state(|state| {
                    if resizing {
                        state.states.set(xdg_toplevel::State::Resizing);
                    } else {
                        state.states.unset(xdg_toplevel::State::Resizing);
                    }
                    state.size = Some(self.last_window_size);
                });
                toplevel.send_pending_configure();
            } else if let Some(x11) = self.window.x11_surface() {
                let rect = Rectangle::new(loc, self.last_window_size);
                let _ = x11.configure(rect);
                data.active_space_mut()
                    .map_element(self.window.clone(), loc, false);
                let win_id = (self.window.user_data() as *const _) as usize as u64;
                data.animator.snap(win_id, rect);
            }
        }
        self.last_configure_sent = Instant::now();
        data.needs_redraw = true;
    }
}

impl PointerGrab<VeloxState> for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        _focus: Option<(
            <VeloxState as smithay::input::SeatHandler>::PointerFocus,
            Point<f64, Logical>,
        )>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        data.pointer_location = event.location;

        let delta = (event.location - self.start_data.location).to_i32_round::<i32>();
        let mut new_w = self.initial_rect.size.w;
        let mut new_h = self.initial_rect.size.h;

        if self.edges.intersects(ResizeEdge::LEFT) {
            new_w -= delta.x;
        } else if self.edges.intersects(ResizeEdge::RIGHT) {
            new_w += delta.x;
        }

        if self.edges.intersects(ResizeEdge::TOP) {
            new_h -= delta.y;
        } else if self.edges.intersects(ResizeEdge::BOTTOM) {
            new_h += delta.y;
        }

        let (min_w, min_h) = data.config.min_window_size;
        let max_size = self
            .window
            .toplevel()
            .map(|tl| {
                with_states(tl.wl_surface(), |states| {
                    states
                        .cached_state
                        .get::<SurfaceCachedState>()
                        .current()
                        .max_size
                })
            })
            .unwrap_or_default();

        let (eff_min_w, eff_min_h) = if data.config.minimal_view_enabled {
            (min_w.max(120), min_h.max(84))
        } else {
            let min_size = self.client_min_size();
            (min_size.w.max(min_w), min_size.h.max(min_h))
        };
        new_w = new_w.max(eff_min_w);
        new_h = new_h.max(eff_min_h);
        if max_size.w > 0 {
            new_w = new_w.min(max_size.w);
        }
        if max_size.h > 0 {
            new_h = new_h.min(max_size.h);
        }

        let next_size = Size::from((new_w, new_h));
        if next_size == self.last_window_size {
            return;
        }
        self.last_window_size = next_size;

        // Rate-limit resize configure events to ~125Hz (8ms) so 4kHz mice never stall apps/GPU.
        if self.last_configure_sent.elapsed() >= Duration::from_millis(8) {
            self.emit_configure(data, true);
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        focus: Option<(
            <VeloxState as smithay::input::SeatHandler>::PointerFocus,
            Point<f64, Logical>,
        )>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start_data.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
            self.emit_configure(data, false);

            if let Some(toplevel) = self.window.toplevel() {
                with_states(toplevel.wl_surface(), |states| {
                    let mut resize_data = states
                        .data_map
                        .get_or_insert(|| RefCell::new(ResizeData::default()))
                        .borrow_mut();
                    if let ResizeState::Resizing {
                        edges,
                        initial_window_location,
                        initial_window_size,
                    } = resize_data.state
                    {
                        resize_data.state = ResizeState::WaitingForLastCommit {
                            edges,
                            initial_window_location,
                            initial_window_size,
                        };
                    }
                });
            }
        }
    }

    fn axis(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut VeloxState, handle: &mut PointerInnerHandle<'_, VeloxState>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut VeloxState,
        handle: &mut PointerInnerHandle<'_, VeloxState>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<VeloxState> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut VeloxState) {}
}

/// Adjusts window origin when resizing from TOP or LEFT edges after commit.
pub fn handle_resize_commit(space: &mut smithay::desktop::Space<Window>, window: &Window) {
    let Some(toplevel) = window.toplevel() else {
        return;
    };
    let Some(mut loc) = space.element_location(window) else {
        return;
    };
    let geom = window.geometry();

    let mut changed = false;
    with_states(toplevel.wl_surface(), |states| {
        let mut resize_data = states
            .data_map
            .get_or_insert(|| RefCell::new(ResizeData::default()))
            .borrow_mut();

        match resize_data.state {
            ResizeState::Resizing {
                edges,
                initial_window_location,
                initial_window_size,
            } => {
                if edges.intersects(ResizeEdge::LEFT) {
                    loc.x = initial_window_location.x + (initial_window_size.w - geom.size.w);
                    changed = true;
                }
                if edges.intersects(ResizeEdge::TOP) {
                    loc.y = initial_window_location.y + (initial_window_size.h - geom.size.h);
                    changed = true;
                }
            }
            ResizeState::WaitingForLastCommit {
                edges,
                initial_window_location,
                initial_window_size,
            } => {
                if edges.intersects(ResizeEdge::LEFT) {
                    loc.x = initial_window_location.x + (initial_window_size.w - geom.size.w);
                }
                if edges.intersects(ResizeEdge::TOP) {
                    loc.y = initial_window_location.y + (initial_window_size.h - geom.size.h);
                }
                resize_data.state = ResizeState::Idle;
                changed = true;
            }
            ResizeState::Idle => {}
        }
    });

    if changed {
        space.map_element(window.clone(), loc, false);
    }
}
