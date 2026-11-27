pub mod udev;
pub mod winit;

use smithay::{
    backend::renderer::{
        damage::OutputDamageTracker,
        element::{
            memory::MemoryRenderBufferRenderElement,
            solid::SolidColorRenderElement,
            surface::WaylandSurfaceRenderElement,
            texture::TextureRenderElement,
            AsRenderElements, Kind,
        },
        gles::{GlesRenderer, GlesTexture},
        Color32F,
    },
    desktop::layer_map_for_output,
    output::Output,
    render_elements,
    utils::{Logical, Physical, Point, Rectangle, Size},
    wayland::shell::wlr_layer::Layer,
};

use crate::{
    config::DisplaySleepMode,
    cursor::{CursorManager, CursorRenderElement},
    state::VeloxState,
    wallpaper::Wallpaper,
};

render_elements! {
    pub VeloxRenderElement<=GlesRenderer>;
    Cursor=CursorRenderElement<GlesRenderer>,
    Ui=MemoryRenderBufferRenderElement<GlesRenderer>,
    Wayland=WaylandSurfaceRenderElement<GlesRenderer>,
    Border=SolidColorRenderElement,
    Wallpaper=TextureRenderElement<GlesTexture>,
}

/// Renders a single output frame: cursor, Shell UI, layer-shell, animated windows + borders, wallpaper.
#[allow(clippy::too_many_arguments)]
pub fn compose_output_elements(
    state: &mut VeloxState,
    renderer: &mut GlesRenderer,
    output: &Output,
    output_size: Size<i32, Physical>,
    wallpaper: Option<&Wallpaper>,
    cursor: Option<&CursorManager>,
    dt_secs: f32,
) -> Vec<VeloxRenderElement> {
    let scale = output.current_scale().fractional_scale();
    let logical_size: Size<i32, Logical> = (
        (output_size.w as f64 / scale).round() as i32,
        (output_size.h as f64 / scale).round() as i32,
    )
        .into();
    let mut elements: Vec<VeloxRenderElement> = Vec::new();
    let is_primary_ui = state.outputs.first() == Some(output) || state.outputs.is_empty();
    let out_geo = state
        .active_space()
        .output_geometry(output)
        .unwrap_or_else(|| Rectangle::new((0, 0).into(), logical_size));

    // Black Screen Sleep Mode: cover entire output in solid black with zero desktop leakage.
    if state.idle_asleep && state.config.idle_sleep_mode == DisplaySleepMode::BlackScreen {
        state.idle_sleep_buf.update(logical_size, [0.0, 0.0, 0.0, 1.0]);
        elements.push(VeloxRenderElement::Border(
            SolidColorRenderElement::from_buffer(
                &state.idle_sleep_buf,
                (0, 0),
                scale,
                1.0,
                Kind::Unspecified,
            ),
        ));
        return elements;
    }

    // 1. Cursor (for bare-metal DRM mode)
    if let Some(cursor_mgr) = cursor {
        let out_loc = state
            .active_space()
            .output_geometry(output)
            .map(|g| g.loc.to_f64())
            .unwrap_or_default();
        let rel_pos = state.pointer_location - out_loc;
        for elem in cursor_mgr.render_elements(renderer, &state.cursor_status, rel_pos, scale) {
            elements.push(VeloxRenderElement::Cursor(elem));
        }
    }

    // Whole-screen idle dim curtain (above everything below cursor so entire desktop dims uniformly)
    if state.idle_dimmed {
        state.idle_dim_buf.update(logical_size, [0.0, 0.0, 0.0, 0.54]);
        elements.push(VeloxRenderElement::Border(
            SolidColorRenderElement::from_buffer(
                &state.idle_dim_buf,
                (0, 0),
                scale,
                1.0,
                Kind::Unspecified,
            ),
        ));
    }

    // Lock Screen Overlay (covers all outputs cleanly and prevents any underlying window exposure)
    if state.screen_locked {
        for elem in state.ui.render_lock_screen(
            renderer,
            out_geo,
            is_primary_ui,
            scale,
            &state.config,
            &state.lock_password_input,
            &state.lock_error_msg,
            state.lock_auth_pending.is_some(),
        ) {
            elements.push(VeloxRenderElement::Ui(elem));
        }
        state.idle_sleep_buf.update(logical_size, [0.02, 0.03, 0.08, 1.0]);
        elements.push(VeloxRenderElement::Border(
            SolidColorRenderElement::from_buffer(
                &state.idle_sleep_buf,
                (0, 0),
                scale,
                1.0,
                Kind::Unspecified,
            ),
        ));
        return elements;
    }

    // 2. Built-in Panel, Start Menu, Notification Toasts, HUD & Active Popups/Context Menus (multi-monitor aware)
    let has_popup = state.ui.active_popup != crate::shell_ui::ActivePopup::None;
    if is_primary_ui || has_popup {
        let primary_geo = state
            .outputs
            .first()
            .and_then(|o| state.active_space().output_geometry(o))
            .unwrap_or(out_geo);
        let output_containing = |pt: Point<i32, Logical>| -> Rectangle<i32, Logical> {
            state
                .outputs
                .iter()
                .filter_map(|o| state.active_space().output_geometry(o))
                .find(|g| g.contains(pt))
                .unwrap_or(primary_geo)
        };
        let popup_geo = match &state.ui.active_popup {
            crate::shell_ui::ActivePopup::ContextMenu { target, pos } => {
                if matches!(target, crate::shell_ui::ContextTarget::TaskbarWindow(_)) {
                    primary_geo
                } else {
                    output_containing((*pos).into())
                }
            }
            crate::shell_ui::ActivePopup::AltTab
            | crate::shell_ui::ActivePopup::TaskView
            | crate::shell_ui::ActivePopup::DisplayConfirm
            | crate::shell_ui::ActivePopup::ReloadConfirm { .. } => {
                output_containing(state.pointer_location.to_i32_round())
            }
            _ => primary_geo,
        };

        let ws_counts = state.workspace_counts();
        let tasks = state.taskbar_windows();
        let global_windows = state.global_windows_list();
        let has_fs = state.has_fullscreen_on_output(output);
        let taskbar_locked = state.is_taskbar_locked_for_output(output);
        let show_shelf = is_primary_ui
            && !taskbar_locked
            && (!has_fs || state.ui.active_popup != crate::shell_ui::ActivePopup::None);
        let allow_toasts = is_primary_ui && !has_fs && !taskbar_locked;
        let ui_elems = state.ui.render_elements(
            renderer,
            out_geo,
            primary_geo,
            popup_geo,
            is_primary_ui,
            scale,
            dt_secs,
            &state.config,
            state.active_workspace,
            &ws_counts,
            &tasks,
            &global_windows,
            &state.apps,
            &state.sys_snap,
            &state.notif_snap,
            &state.update_status,
            show_shelf,
            allow_toasts,
        );
        for elem in ui_elems {
            elements.push(VeloxRenderElement::Ui(elem));
        }
    }

    // 3. Layer-shell Overlay & Top
    let layer_map = layer_map_for_output(output);
    for layer_kind in [Layer::Overlay, Layer::Top] {
        for layer in layer_map.layers_on(layer_kind).rev() {
            let loc = layer_map
                .layer_geometry(layer)
                .map(|g| g.loc)
                .unwrap_or_default()
                .to_physical_precise_round(scale);
            for elem in layer.render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                renderer,
                loc,
                scale.into(),
                1.0,
            ) {
                elements.push(VeloxRenderElement::Wayland(elem));
            }
        }
    }

    // 4. Windows in active workspace (front-to-back) with CSD-aligned coords & cached borders
    let out_loc = state
        .active_space()
        .output_geometry(output)
        .map(|g| g.loc)
        .unwrap_or_default();
    let focused_win = state.focused_window();
    let bw = state.config.border_width.clamp(0, 4);
    let accent = state.config.accent_rgb();
    let anim_enabled = state.config.animations_enabled;
    let anim_speed = state.config.animation_speed;

    // Translucent drag-to-snap preview above windows (below UI & cursor)
    if let Some(snap_rect) = state.snap_preview {
        if snap_rect.size.w > 0 && snap_rect.size.h > 0 {
            let rel_snap = snap_rect.loc - out_loc;
            let snap_pos: Point<i32, Physical> = rel_snap.to_physical_precise_round(scale);
            let snap_color = Color32F::from([
                accent[0] as f32 / 255.0,
                accent[1] as f32 / 255.0,
                accent[2] as f32 / 255.0,
                0.24,
            ]);
            state.snap_preview_buf.update(snap_rect.size, snap_color);
            elements.push(VeloxRenderElement::Border(
                SolidColorRenderElement::from_buffer(
                    &state.snap_preview_buf,
                    snap_pos,
                    scale,
                    1.0,
                    Kind::Unspecified,
                ),
            ));
        }
    }

    let windows: Vec<_> = state.active_space().elements().rev().cloned().collect();
    let mut live_ids = Vec::with_capacity(windows.len());

    for window in &windows {
        let win_id = (window.user_data() as *const _) as usize as u64;
        live_ids.push(win_id);

        let meta = VeloxState::window_meta(window);
        if meta.minimized {
            continue;
        }
        let is_or = meta.override_redirect;
        let is_fullscreen = meta.fullscreen;
        let is_minimal = !is_fullscreen && !is_or && meta.minimal_view && state.config.minimal_view_enabled;
        let minimal_slot = meta.minimal_slot_size;
        let unfs_hold_until = meta.unfullscreen_hold_until;
        drop(meta);

        if state.config.freeze_hidden_windows && !state.is_window_visible_on_output(output, window) {
            continue;
        }

        let target_loc = state
            .active_space()
            .element_location(window)
            .unwrap_or_default();
        let geom = window.geometry();
        let eff_size = if is_minimal && minimal_slot.w >= 48 && minimal_slot.h >= 36 {
            minimal_slot
        } else {
            geom.size
        };

        // While client hasn't committed its post-fullscreen buffer yet (1-3 frames), keep origin
        // anchored to output origin so a fullscreen-sized frame doesn't overhang screen borders.
        let holding_pre_unfs = !is_fullscreen
            && !is_minimal
            && unfs_hold_until.is_some_and(|dl| std::time::Instant::now() < dl)
            && geom.size.w >= out_geo.size.w
            && geom.size.h >= out_geo.size.h;
        if !is_fullscreen && unfs_hold_until.is_some() && !holding_pre_unfs {
            VeloxState::window_meta_mut(window).unfullscreen_hold_until = None;
        }
        let anim_target = if holding_pre_unfs {
            Rectangle::new(out_geo.loc, eff_size)
        } else {
            Rectangle::new(target_loc, eff_size)
        };

        let vis_loc = if is_or {
            target_loc
        } else if holding_pre_unfs {
            state.animator.snap(win_id, anim_target);
            state.needs_redraw = true;
            out_geo.loc
        } else {
            state
                .animator
                .step(win_id, anim_target, anim_enabled, anim_speed, dt_secs)
        };

        let is_focused = focused_win.as_ref() == Some(window);
        let allow_dim = !is_focused && !is_or && !(is_fullscreen && state.config.dim_exclude_fullscreen);
        let win_alpha = if allow_dim {
            state.config.unfocused_window_opacity.clamp(0.60, 1.0)
        } else {
            1.0
        };

        // Match Smithay Space::render_location(): subtract geom.loc so CSD shadow offsets align perfectly!
        let render_loc = vis_loc - geom.loc - out_loc;
        let phys_pos: Point<i32, Physical> = render_loc.to_physical_precise_round(scale);

        // GPU-composited dim tint placed directly in front of unfocused windows (front-to-back order).
        if allow_dim
            && state.config.dim_unfocused_windows
            && state.config.unfocused_dim_opacity > 0.01
            && eff_size.w > 0
            && eff_size.h > 0
        {
            let dim_alpha = state.config.unfocused_dim_opacity.clamp(0.0, 0.70);
            let dim_buf =
                VeloxState::update_dim_buffer(window, eff_size, [0.02, 0.03, 0.07, dim_alpha]);
            let dim_pos: Point<i32, Physical> = (vis_loc - out_loc).to_physical_precise_round(scale);
            elements.push(VeloxRenderElement::Border(
                SolidColorRenderElement::from_buffer(&dim_buf, dim_pos, scale, 1.0, Kind::Unspecified),
            ));
        }

        if is_minimal && eff_size.w >= 48 && eff_size.h >= 36 {
            if let Some(card_buf) = state.ensure_minimal_card(window, eff_size, is_focused) {
                let card_pos: Point<i32, Physical> =
                    (vis_loc - out_loc).to_physical_precise_round(scale);
                if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    (card_pos.x as f64, card_pos.y as f64),
                    &card_buf,
                    Some(win_alpha),
                    None,
                    None,
                    Kind::Unspecified,
                ) {
                    elements.push(VeloxRenderElement::Ui(elem));
                }
            }
        } else {
            for elem in window.render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                renderer,
                phys_pos,
                scale.into(),
                win_alpha,
            ) {
                elements.push(VeloxRenderElement::Wayland(elem));
            }
        }

        // Cached 4 thin opaque edge strips around the window so the interior stays zero-overdraw!
        if bw > 0 && !is_or && eff_size.w > 0 && eff_size.h > 0 && !is_fullscreen {
            let border_color = if is_focused {
                [
                    accent[0] as f32 / 255.0,
                    accent[1] as f32 / 255.0,
                    accent[2] as f32 / 255.0,
                    1.0,
                ]
            } else {
                [0.20, 0.25, 0.36, 0.85]
            };
            let rel_loc = vis_loc - out_loc;
            let strip_specs = [
                ((rel_loc.x - bw, rel_loc.y - bw), (eff_size.w + bw * 2, bw)),
                ((rel_loc.x - bw, rel_loc.y + eff_size.h), (eff_size.w + bw * 2, bw)),
                ((rel_loc.x - bw, rel_loc.y), (bw, eff_size.h)),
                ((rel_loc.x + eff_size.w, rel_loc.y), (bw, eff_size.h)),
            ];
            let bufs = VeloxState::update_border_buffers(window, eff_size, bw, border_color);
            for (i, (pos_l, _)) in strip_specs.into_iter().enumerate() {
                let p: Point<i32, Physical> =
                    Point::<i32, Logical>::from(pos_l).to_physical_precise_round(scale);
                elements.push(VeloxRenderElement::Border(
                    SolidColorRenderElement::from_buffer(&bufs[i], p, scale, 1.0, Kind::Unspecified),
                ));
            }
        }
    }
    state.animator.retain_ids(&live_ids);

    // 5. Layer-shell Bottom & Background
    for layer_kind in [Layer::Bottom, Layer::Background] {
        for layer in layer_map.layers_on(layer_kind).rev() {
            let loc = layer_map
                .layer_geometry(layer)
                .map(|g| g.loc)
                .unwrap_or_default()
                .to_physical_precise_round(scale);
            for elem in layer.render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                renderer,
                loc,
                scale.into(),
                1.0,
            ) {
                elements.push(VeloxRenderElement::Wayland(elem));
            }
        }
    }
    drop(layer_map);

    // 6. Built-in GPU wallpaper
    if let Some(wp) = wallpaper {
        elements.push(VeloxRenderElement::Wallpaper(wp.render_element(output_size)));
    }

    elements
}

pub fn render_with_damage_tracker<'a>(
    damage_tracker: &mut OutputDamageTracker,
    renderer: &mut GlesRenderer,
    framebuffer: &mut <GlesRenderer as smithay::backend::renderer::RendererSuper>::Framebuffer<'a>,
    age: usize,
    elements: &[VeloxRenderElement],
    clear_color: [f32; 4],
) -> Result<Option<Vec<Rectangle<i32, Physical>>>, Box<dyn std::error::Error>> {
    let color = Color32F::new(clear_color[0], clear_color[1], clear_color[2], clear_color[3]);
    let res = damage_tracker.render_output(renderer, framebuffer, age, elements, color)?;
    Ok(res.damage.cloned())
}
