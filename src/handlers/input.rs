use std::os::unix::io::OwnedFd;

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    delegate_data_control, delegate_data_device, delegate_ext_data_control,
    delegate_primary_selection, delegate_seat,
    input::{
        keyboard::{keysyms, FilterResult},
        pointer::{AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent, RelativeMotionEvent},
        Seat, SeatHandler, SeatState,
    },
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
    wayland::{
        seat::WaylandFocus,
        selection::{
            data_device::{
                set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler,
            },
            ext_data_control::{
                DataControlHandler as ExtDataControlHandler, DataControlState as ExtDataControlState,
            },
            primary_selection::{
                set_primary_focus, PrimarySelectionHandler, PrimarySelectionState,
            },
            wlr_data_control::{
                DataControlHandler as WlrDataControlHandler, DataControlState as WlrDataControlState,
            },
            SelectionHandler, SelectionSource, SelectionTarget,
        },
    },
};

use crate::{
    apps::executable_exists,
    grabs::SnapZone,
    handlers::xdg_shell::{start_move_grab, start_resize_grab},
    shell_ui::{ActivePopup, ContextTarget, UiAction},
    state::{Direction, FocusTarget, PointerLockMode, VeloxSelectionData, VeloxState, NUM_WORKSPACES},
    system_ctrl::SysCommand,
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

impl SeatHandler for VeloxState {
    type KeyboardFocus = FocusTarget;
    type PointerFocus = FocusTarget;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&FocusTarget>) {
        let dh = &self.display_handle;
        let client = focused
            .and_then(|f| f.wl_surface())
            .and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
        self.cursor_dirty = true;
    }
}

impl SelectionHandler for VeloxState {
    type SelectionUserData = VeloxSelectionData;

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.new_selection(ty, source.map(|s| s.mime_types())) {
                tracing::debug!(?err, ?ty, "Failed forwarding Wayland selection to X11Wm");
            }
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        user_data: &Self::SelectionUserData,
    ) {
        match user_data {
            VeloxSelectionData::X11 => {
                self.pending_x11_selections.push((ty, mime_type, fd));
            }
            VeloxSelectionData::ServerBuffer(id) => match ty {
                SelectionTarget::Clipboard => self.server_clipboard.write_to_fd_async(*id, fd),
                SelectionTarget::Primary => self.server_primary.write_to_fd_async(*id, fd),
            },
        }
    }
}

impl DataDeviceHandler for VeloxState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl PrimarySelectionHandler for VeloxState {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl WlrDataControlHandler for VeloxState {
    fn data_control_state(&self) -> &WlrDataControlState {
        &self.data_control_state
    }
}

impl ExtDataControlHandler for VeloxState {
    fn data_control_state(&self) -> &ExtDataControlState {
        &self.ext_data_control_state
    }
}

impl ClientDndGrabHandler for VeloxState {}
impl ServerDndGrabHandler for VeloxState {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

delegate_seat!(VeloxState);
delegate_data_device!(VeloxState);
delegate_primary_selection!(VeloxState);
delegate_data_control!(VeloxState);
delegate_ext_data_control!(VeloxState);

#[derive(Debug)]
enum KeyAction {
    SpawnTerminal,
    LaunchFiles,
    LaunchSettings,
    ToggleStartMenu,
    ToggleAudioMixer,
    ToggleActiveApps,
    ToggleCalendar,
    ToggleQuickSettings,
    ToggleTaskView,
    TogglePowerMenu,
    LockScreen,
    ToggleNotifications,
    SeamlessReload,
    ShowDesktop,
    CloseFocused,
    ToggleFloating,
    ToggleAlwaysOnTop,
    ToggleFullscreen,
    ExpandMaximize,
    ShrinkOrMinimize,
    SnapHalf(SnapZone),
    StepAltTab { forward: bool },
    CommitAltTab,
    FocusDir(Direction),
    SwapDir(Direction),
    SwitchWorkspace(usize),
    StepWorkspace(i32),
    MoveToWorkspace(usize),
    MoveNextMonitor,
    Quit,
    VolumeUp,
    VolumeDown,
    VolumeMute,
    PopupEscape,
    PopupBackspace,
    PopupEnter,
    PopupUp,
    PopupDown,
    PopupChar(String),
    LockBackspace,
    LockEnter,
    LockEscape,
    LockChar(String),
    LockIgnore,
}

pub fn process_input_event<B: InputBackend>(state: &mut VeloxState, event: InputEvent<B>) {
    match event {
        InputEvent::Keyboard { event } => {
            state.wake_from_user_input();
            state.needs_redraw = true;
            let serial = SERIAL_COUNTER.next_serial();
            let time = EventTime::from_micros(event.time()).millis();
            let keyboard = state.keyboard.clone();
            let keycode = event.key_code();
            let key_state = event.state();
            let screen_locked = state.screen_locked;
            let popup_open = state.ui.is_popup_open();
            let start_menu_open = state.ui.active_popup == ActivePopup::StartMenu;
            let notif_reply_open = state.ui.active_popup == ActivePopup::Notifications
                && state.ui.notif_reply_target.is_some();
            let reload_confirm_open = matches!(state.ui.active_popup, ActivePopup::ReloadConfirm { .. });
            let alt_tab_open = state.ui.is_alt_tab_open();
            let taskbar_locked = state.is_taskbar_locked();
            let win_shortcuts = state.config.windows_shortcuts;

            let action = keyboard.input::<KeyAction, _>(
                state,
                keycode,
                key_state,
                serial,
                time,
                |st, modifiers, handle| {
                    let sym = handle.modified_sym();
                    let raw = sym.raw();
                    if screen_locked {
                        if key_state == KeyState::Released {
                            return FilterResult::Intercept(KeyAction::LockIgnore);
                        }
                        return match raw {
                            keysyms::KEY_BackSpace => {
                                FilterResult::Intercept(KeyAction::LockBackspace)
                            }
                            keysyms::KEY_Return | keysyms::KEY_KP_Enter => {
                                FilterResult::Intercept(KeyAction::LockEnter)
                            }
                            keysyms::KEY_Escape => FilterResult::Intercept(KeyAction::LockEscape),
                            _ => {
                                if !modifiers.ctrl && !modifiers.alt && !modifiers.logo {
                                    if let Some(ch) = sym.key_char() {
                                        if !ch.is_control() {
                                            return FilterResult::Intercept(KeyAction::LockChar(
                                                ch.to_string(),
                                            ));
                                        }
                                    }
                                }
                                FilterResult::Intercept(KeyAction::LockIgnore)
                            }
                        };
                    }
                    let is_super_key = matches!(raw, keysyms::KEY_Super_L | keysyms::KEY_Super_R);
                    let is_alt_key = matches!(
                        raw,
                        keysyms::KEY_Alt_L
                            | keysyms::KEY_Alt_R
                            | keysyms::KEY_Meta_L
                            | keysyms::KEY_Meta_R
                            | keysyms::KEY_ISO_Level3_Shift
                    );

                    if key_state == KeyState::Released {
                        if alt_tab_open && (is_alt_key || is_super_key) {
                            st.super_only_pressed = false;
                            return FilterResult::Intercept(KeyAction::CommitAltTab);
                        }
                        if win_shortcuts && is_super_key && st.super_only_pressed {
                            st.super_only_pressed = false;
                            if !taskbar_locked {
                                return FilterResult::Intercept(KeyAction::ToggleStartMenu);
                            }
                            return FilterResult::Forward;
                        }
                        if is_super_key {
                            st.super_only_pressed = false;
                        }
                        return FilterResult::Forward;
                    }

                    // Track clean Super-key tap (opening Start Menu on release if no chorded key pressed).
                    if is_super_key {
                        if !modifiers.ctrl && !modifiers.alt && !modifiers.shift {
                            st.super_only_pressed = true;
                        } else {
                            st.super_only_pressed = false;
                        }
                        return FilterResult::Forward;
                    }
                    st.super_only_pressed = false;

                    match raw {
                        keysyms::KEY_XF86AudioRaiseVolume => {
                            return FilterResult::Intercept(KeyAction::VolumeUp)
                        }
                        keysyms::KEY_XF86AudioLowerVolume => {
                            return FilterResult::Intercept(KeyAction::VolumeDown)
                        }
                        keysyms::KEY_XF86AudioMute => {
                            return FilterResult::Intercept(KeyAction::VolumeMute)
                        }
                        _ => {}
                    }

                    // Standard Windows Ctrl+Shift+Esc (Process / Active Apps) & Alt+F4 (Close Window)
                    if win_shortcuts && modifiers.ctrl && modifiers.shift && raw == keysyms::KEY_Escape {
                        return FilterResult::Intercept(KeyAction::ToggleActiveApps);
                    }
                    if win_shortcuts && modifiers.ctrl && !modifiers.logo && !modifiers.alt && raw == keysyms::KEY_Escape {
                        return FilterResult::Intercept(KeyAction::ToggleStartMenu);
                    }
                    if win_shortcuts && modifiers.alt && !modifiers.logo && raw == keysyms::KEY_F4 {
                        return FilterResult::Intercept(KeyAction::CloseFocused);
                    }
                    if modifiers.ctrl && modifiers.alt && !modifiers.logo && matches!(raw, keysyms::KEY_l | keysyms::KEY_L) {
                        return FilterResult::Intercept(KeyAction::LockScreen);
                    }
                    // Windows Alt+Tab / Alt+Shift+Tab live switcher (always keep separate from Win+Tab TaskView)
                    let is_tab_key = matches!(raw, keysyms::KEY_Tab | keysyms::KEY_ISO_Left_Tab)
                        || handle.raw_syms().iter().any(|s| s.raw() == keysyms::KEY_Tab);
                    if modifiers.alt && !modifiers.logo && is_tab_key {
                        return FilterResult::Intercept(KeyAction::StepAltTab {
                            forward: !modifiers.shift && raw != keysyms::KEY_ISO_Left_Tab,
                        });
                    }

                    // Ctrl + Win + Left/Right -> Prev/Next Virtual Desktop; Win+Ctrl+Shift+R -> Seamless Live Reload
                    if modifiers.logo && modifiers.ctrl && modifiers.shift && matches!(raw, keysyms::KEY_r | keysyms::KEY_R) {
                        return FilterResult::Intercept(KeyAction::SeamlessReload);
                    }
                    if win_shortcuts && modifiers.logo && modifiers.ctrl && !modifiers.alt {
                        match raw {
                            keysyms::KEY_Left => return FilterResult::Intercept(KeyAction::StepWorkspace(-1)),
                            keysyms::KEY_Right => return FilterResult::Intercept(KeyAction::StepWorkspace(1)),
                            _ => {}
                        }
                    }

                    if modifiers.logo || modifiers.alt {
                        if raw == keysyms::KEY_Tab && modifiers.logo {
                            if win_shortcuts {
                                return FilterResult::Intercept(KeyAction::ToggleTaskView);
                            }
                            return FilterResult::Intercept(KeyAction::StepAltTab {
                                forward: !modifiers.shift,
                            });
                        }

                        if modifiers.shift {
                            match raw {
                                keysyms::KEY_E | keysyms::KEY_e => {
                                    return FilterResult::Intercept(KeyAction::Quit)
                                }
                                keysyms::KEY_M | keysyms::KEY_m => {
                                    return FilterResult::Intercept(KeyAction::MoveNextMonitor)
                                }
                                keysyms::KEY_Left | keysyms::KEY_h | keysyms::KEY_H => {
                                    return FilterResult::Intercept(KeyAction::SwapDir(Direction::Left))
                                }
                                keysyms::KEY_Right | keysyms::KEY_l | keysyms::KEY_L => {
                                    return FilterResult::Intercept(KeyAction::SwapDir(Direction::Right))
                                }
                                keysyms::KEY_Up | keysyms::KEY_k | keysyms::KEY_K => {
                                    return FilterResult::Intercept(KeyAction::SwapDir(Direction::Up))
                                }
                                keysyms::KEY_Down | keysyms::KEY_j | keysyms::KEY_J => {
                                    return FilterResult::Intercept(KeyAction::SwapDir(Direction::Down))
                                }
                                keysyms::KEY_exclam | keysyms::KEY_1 => {
                                    return FilterResult::Intercept(KeyAction::MoveToWorkspace(0))
                                }
                                keysyms::KEY_at | keysyms::KEY_2 => {
                                    return FilterResult::Intercept(KeyAction::MoveToWorkspace(1))
                                }
                                keysyms::KEY_numbersign | keysyms::KEY_3 => {
                                    return FilterResult::Intercept(KeyAction::MoveToWorkspace(2))
                                }
                                keysyms::KEY_dollar | keysyms::KEY_4 => {
                                    return FilterResult::Intercept(KeyAction::MoveToWorkspace(3))
                                }
                                keysyms::KEY_percent | keysyms::KEY_5 => {
                                    return FilterResult::Intercept(KeyAction::MoveToWorkspace(4))
                                }
                                keysyms::KEY_asciicircum | keysyms::KEY_6 => {
                                    return FilterResult::Intercept(KeyAction::MoveToWorkspace(5))
                                }
                                keysyms::KEY_ampersand | keysyms::KEY_7 => {
                                    return FilterResult::Intercept(KeyAction::MoveToWorkspace(6))
                                }
                                keysyms::KEY_asterisk | keysyms::KEY_8 => {
                                    return FilterResult::Intercept(KeyAction::MoveToWorkspace(7))
                                }
                                keysyms::KEY_parenleft | keysyms::KEY_9 => {
                                    return FilterResult::Intercept(KeyAction::MoveToWorkspace(8))
                                }
                                _ => {}
                            }
                        }

                        // Windows-compatible Super (Win) productivity chords
                        if win_shortcuts && modifiers.logo && !modifiers.shift {
                            match raw {
                                keysyms::KEY_Left => {
                                    return FilterResult::Intercept(KeyAction::SnapHalf(SnapZone::LeftHalf))
                                }
                                keysyms::KEY_Right => {
                                    return FilterResult::Intercept(KeyAction::SnapHalf(SnapZone::RightHalf))
                                }
                                keysyms::KEY_Up => {
                                    return FilterResult::Intercept(KeyAction::ExpandMaximize)
                                }
                                keysyms::KEY_Down => {
                                    return FilterResult::Intercept(KeyAction::ShrinkOrMinimize)
                                }
                                keysyms::KEY_d | keysyms::KEY_D => {
                                    return FilterResult::Intercept(KeyAction::ShowDesktop)
                                }
                                keysyms::KEY_e | keysyms::KEY_E => {
                                    return FilterResult::Intercept(KeyAction::LaunchFiles)
                                }
                                keysyms::KEY_i | keysyms::KEY_I => {
                                    return FilterResult::Intercept(KeyAction::LaunchSettings)
                                }
                                keysyms::KEY_s | keysyms::KEY_S => {
                                    return FilterResult::Intercept(KeyAction::ToggleStartMenu)
                                }
                                keysyms::KEY_a | keysyms::KEY_A => {
                                    return FilterResult::Intercept(KeyAction::ToggleQuickSettings)
                                }
                                keysyms::KEY_v | keysyms::KEY_V => {
                                    return FilterResult::Intercept(KeyAction::ToggleAudioMixer)
                                }
                                keysyms::KEY_n | keysyms::KEY_N => {
                                    return FilterResult::Intercept(KeyAction::ToggleCalendar)
                                }
                                keysyms::KEY_l | keysyms::KEY_L => {
                                    return FilterResult::Intercept(KeyAction::LockScreen)
                                }
                                keysyms::KEY_x | keysyms::KEY_X => {
                                    return FilterResult::Intercept(KeyAction::TogglePowerMenu)
                                }
                                keysyms::KEY_u | keysyms::KEY_U => {
                                    return FilterResult::Intercept(KeyAction::ToggleNotifications)
                                }
                                keysyms::KEY_t | keysyms::KEY_T => {
                                    return FilterResult::Intercept(KeyAction::ToggleAlwaysOnTop)
                                }
                                _ => {}
                            }
                        }

                        match raw {
                            keysyms::KEY_Return => {
                                return FilterResult::Intercept(KeyAction::SpawnTerminal)
                            }
                            keysyms::KEY_space => {
                                return FilterResult::Intercept(KeyAction::ToggleStartMenu)
                            }
                            keysyms::KEY_d | keysyms::KEY_D if !win_shortcuts => {
                                return FilterResult::Intercept(KeyAction::ToggleStartMenu)
                            }
                            keysyms::KEY_s | keysyms::KEY_S if !win_shortcuts => {
                                return FilterResult::Intercept(KeyAction::LaunchSettings)
                            }
                            keysyms::KEY_a | keysyms::KEY_A => {
                                return FilterResult::Intercept(KeyAction::ToggleAudioMixer)
                            }
                            keysyms::KEY_b | keysyms::KEY_B => {
                                return FilterResult::Intercept(KeyAction::ToggleActiveApps)
                            }
                            keysyms::KEY_q | keysyms::KEY_Q => {
                                return FilterResult::Intercept(KeyAction::CloseFocused)
                            }
                            keysyms::KEY_f | keysyms::KEY_F => {
                                return FilterResult::Intercept(KeyAction::ToggleFloating)
                            }
                            keysyms::KEY_t | keysyms::KEY_T if modifiers.logo && !modifiers.shift => {
                                return FilterResult::Intercept(KeyAction::ToggleAlwaysOnTop)
                            }
                            keysyms::KEY_m | keysyms::KEY_M => {
                                return FilterResult::Intercept(KeyAction::ToggleFullscreen)
                            }
                            keysyms::KEY_Left | keysyms::KEY_h | keysyms::KEY_H => {
                                return FilterResult::Intercept(KeyAction::FocusDir(Direction::Left))
                            }
                            keysyms::KEY_Right | keysyms::KEY_l | keysyms::KEY_L => {
                                return FilterResult::Intercept(KeyAction::FocusDir(Direction::Right))
                            }
                            keysyms::KEY_Up | keysyms::KEY_k | keysyms::KEY_K => {
                                return FilterResult::Intercept(KeyAction::FocusDir(Direction::Up))
                            }
                            keysyms::KEY_Down | keysyms::KEY_j | keysyms::KEY_J => {
                                return FilterResult::Intercept(KeyAction::FocusDir(Direction::Down))
                            }
                            keysyms::KEY_1 => return FilterResult::Intercept(KeyAction::SwitchWorkspace(0)),
                            keysyms::KEY_2 => return FilterResult::Intercept(KeyAction::SwitchWorkspace(1)),
                            keysyms::KEY_3 => return FilterResult::Intercept(KeyAction::SwitchWorkspace(2)),
                            keysyms::KEY_4 => return FilterResult::Intercept(KeyAction::SwitchWorkspace(3)),
                            keysyms::KEY_5 => return FilterResult::Intercept(KeyAction::SwitchWorkspace(4)),
                            keysyms::KEY_6 => return FilterResult::Intercept(KeyAction::SwitchWorkspace(5)),
                            keysyms::KEY_7 => return FilterResult::Intercept(KeyAction::SwitchWorkspace(6)),
                            keysyms::KEY_8 => return FilterResult::Intercept(KeyAction::SwitchWorkspace(7)),
                            keysyms::KEY_9 => return FilterResult::Intercept(KeyAction::SwitchWorkspace(8)),
                            _ => {}
                        }
                    }

                    if popup_open {
                        match raw {
                            keysyms::KEY_Escape => {
                                return FilterResult::Intercept(KeyAction::PopupEscape)
                            }
                            keysyms::KEY_Return | keysyms::KEY_KP_Enter if reload_confirm_open => {
                                return FilterResult::Intercept(KeyAction::PopupEnter)
                            }
                            _ if start_menu_open || notif_reply_open => match raw {
                                keysyms::KEY_BackSpace => {
                                    return FilterResult::Intercept(KeyAction::PopupBackspace)
                                }
                                keysyms::KEY_Return | keysyms::KEY_KP_Enter => {
                                    return FilterResult::Intercept(KeyAction::PopupEnter)
                                }
                                keysyms::KEY_Up if start_menu_open => {
                                    return FilterResult::Intercept(KeyAction::PopupUp)
                                }
                                keysyms::KEY_Down if start_menu_open => {
                                    return FilterResult::Intercept(KeyAction::PopupDown)
                                }
                                _ => {
                                    if !modifiers.ctrl && !modifiers.alt && !modifiers.logo {
                                        if let Some(ch) = sym.key_char() {
                                            if !ch.is_control() {
                                                return FilterResult::Intercept(KeyAction::PopupChar(
                                                    ch.to_string(),
                                                ));
                                            }
                                        }
                                    }
                                    return FilterResult::Intercept(KeyAction::PopupChar(String::new()));
                                }
                            },
                            _ => {}
                        }
                    }

                    FilterResult::Forward
                },
            );

            if let Some(action) = action {
                execute_action(state, action);
            }
        }
        InputEvent::PointerMotion { event } => {
            if state.screen_locked {
                let delta = event.delta();
                let pos = clamp_to_outputs(
                    state,
                    state.pointer_location + Point::<f64, Logical>::from((delta.x, delta.y)),
                );
                let dx = pos.x - state.pointer_location.x;
                let dy = pos.y - state.pointer_location.y;
                state.pointer_location = pos;
                state.cursor_dirty = true;
                if dx * dx + dy * dy >= 4.0 {
                    state.wake_from_user_input();
                }
                return;
            }
            let delta = event.delta();
            let delta_unaccel = event.delta_unaccel();
            let utime = event.time();
            let under = state.surface_under(state.pointer_location);
            state.pointer.clone().relative_motion(
                state,
                under,
                &RelativeMotionEvent {
                    delta: (delta.x, delta.y).into(),
                    delta_unaccel,
                    utime,
                },
            );
            let mut pos = state.pointer_location + Point::<f64, Logical>::from((delta.x, delta.y));
            pos = match state.pointer_lock_mode() {
                PointerLockMode::Locked => state.pointer_location,
                PointerLockMode::Confined(rect) => clamp_to_rect(pos, rect),
                PointerLockMode::Free => clamp_to_outputs(state, pos),
            };
            handle_pointer_motion(state, pos, EventTime::from_micros(utime).millis());
        }
        InputEvent::PointerMotionAbsolute { event } => {
            let output_geo = state
                .outputs
                .first()
                .and_then(|o| state.active_space().output_geometry(o));

            let mut pos = if let Some(geo) = output_geo {
                event.position_transformed(geo.size) + geo.loc.to_f64()
            } else {
                let raw = event.position();
                (raw.x, raw.y).into()
            };
            pos = match state.pointer_lock_mode() {
                PointerLockMode::Locked => state.pointer_location,
                PointerLockMode::Confined(rect) => clamp_to_rect(pos, rect),
                PointerLockMode::Free => clamp_to_outputs(state, pos),
            };

            handle_pointer_motion(state, pos, EventTime::from_micros(event.time()).millis());
        }
        InputEvent::PointerButton { event } => {
            state.wake_from_user_input();
            state.needs_redraw = true;
            if state.screen_locked {
                return;
            }
            let serial = SERIAL_COUNTER.next_serial();
            let button = event.button_code();
            let button_state = event.state();
            let time = EventTime::from_micros(event.time()).millis();
            let pos = state.pointer_location;
            state.pointer_button_down = button_state == ButtonState::Pressed;
            if button_state == ButtonState::Pressed {
                state.popups.cleanup();
            }

            if button_state == ButtonState::Released && state.ui.is_dragging_slider() {
                state.ui.end_slider_drag();
                state.persist_hardware_if_dirty();
                return;
            }

            if !state.pointer.is_grabbed()
                && (state.ui.captures_pointer(pos) || state.ui.is_popup_open())
            {
                if button_state == ButtonState::Pressed {
                    let right = button == BTN_RIGHT;
                    if let Some(action) = state.ui.click_at(pos, right) {
                        state.execute_ui_action(action);
                        return;
                    }
                }
                if state.ui.captures_pointer(pos) {
                    return;
                }
            }

            let pointer = state.pointer.clone();
            let keyboard = state.keyboard.clone();
            let mod_held = keyboard.modifier_state().logo || keyboard.modifier_state().alt;
            let shift_held = keyboard.modifier_state().shift;

            // Only route focus / WM drags when no pointer grab (like a terminal popup grab) is active!
            if !state.pointer.is_grabbed() && button_state == ButtonState::Pressed {
                if let Some((window, win_idx)) = state.window_under_point(pos) {
                    state.focus_window(Some(window.clone()));

                    if mod_held && button == BTN_LEFT {
                        start_move_grab(state, window, serial, button);
                        return;
                    }
                    if mod_held && button == BTN_RIGHT {
                        start_resize_grab(state, window, serial, button);
                        return;
                    }
                    if VeloxState::window_meta(&window).minimal_view
                        && state.config.minimal_view_enabled
                    {
                        if button == BTN_RIGHT {
                            if let Some(idx) = win_idx {
                                let p = pos.to_i32_round();
                                state.execute_ui_action(UiAction::OpenContextMenu {
                                    target: ContextTarget::TaskbarWindow(idx),
                                    pos: (p.x, p.y),
                                });
                            }
                            return;
                        }
                        if button == BTN_LEFT && state.register_minimal_click_should_expand(&window)
                        {
                            state.expand_from_minimal_view(&window);
                        }
                        return;
                    }
                    if !mod_held
                        && !shift_held
                        && button == BTN_RIGHT
                        && state.is_builtin_menu_terminal(&window)
                    {
                        state.last_terminal_right_click = Some((window, pos));
                        let p = pos.to_i32_round();
                        state.execute_ui_action(UiAction::OpenContextMenu {
                            target: ContextTarget::Terminal,
                            pos: (p.x, p.y),
                        });
                        return;
                    }
                } else {
                    state.focus_window(None);
                    if button == BTN_RIGHT {
                        let p = pos.to_i32_round();
                        state.execute_ui_action(UiAction::OpenContextMenu {
                            target: ContextTarget::Desktop,
                            pos: (p.x, p.y),
                        });
                        return;
                    }
                }
            }

            pointer.button(
                state,
                &ButtonEvent {
                    button,
                    state: button_state,
                    serial,
                    time,
                },
            );
            pointer.frame(state);
        }
        InputEvent::PointerAxis { event } => {
            state.wake_from_user_input();
            state.needs_redraw = true;
            if state.screen_locked {
                return;
            }
            let vertical = event.amount(Axis::Vertical).unwrap_or(0.0);
            let total = state.ui.filtered_app_indices(&state.apps).len();
            if let Some(action) = state.ui.scroll_at(state.pointer_location, vertical, total) {
                state.execute_ui_action(action);
                return;
            }

            let source = event.source();
            let horizontal = event.amount(Axis::Horizontal).unwrap_or(0.0);
            let horizontal_v120 = event.amount_v120(Axis::Horizontal);
            let vertical_v120 = event.amount_v120(Axis::Vertical);

            let mut frame =
                AxisFrame::new(EventTime::from_micros(event.time()).millis()).source(source);
            if horizontal != 0.0 {
                frame = frame.value(Axis::Horizontal, horizontal);
                if let Some(v) = horizontal_v120 {
                    frame = frame.v120(Axis::Horizontal, v as i32);
                }
            }
            if vertical != 0.0 {
                frame = frame.value(Axis::Vertical, vertical);
                if let Some(v) = vertical_v120 {
                    frame = frame.v120(Axis::Vertical, v as i32);
                }
            }
            if source == AxisSource::Finger {
                if event.amount(Axis::Horizontal) == Some(0.0) {
                    frame = frame.stop(Axis::Horizontal);
                }
                if event.amount(Axis::Vertical) == Some(0.0) {
                    frame = frame.stop(Axis::Vertical);
                }
            }

            let pointer = state.pointer.clone();
            pointer.axis(state, frame);
            pointer.frame(state);
        }
        _ => {}
    }
}

fn handle_pointer_motion(state: &mut VeloxState, pos: Point<f64, Logical>, time: u32) {
    let delta_sq = {
        let dx = pos.x - state.pointer_location.x;
        let dy = pos.y - state.pointer_location.y;
        dx * dx + dy * dy
    };
    state.pointer_location = pos;
    state.cursor_dirty = true;
    if delta_sq >= 4.0 {
        state.wake_from_user_input();
    }

    if let Some((target, val)) = state.ui.update_slider_drag(pos) {
        state.needs_redraw = true;
        state.apply_slider_value(&target, val);
        return;
    }

    if state.ui.update_hover(pos) {
        state.needs_redraw = true;
    }
    if state.ui.captures_pointer(pos) && !state.pointer.is_grabbed() {
        state.cursor_status = CursorImageStatus::default_named();
        return;
    }

    if state.pointer.is_grabbed() {
        state.needs_redraw = true;
    }

    let serial = SERIAL_COUNTER.next_serial();
    let under = state.surface_under(pos);

    if state.config.focus_follows_mouse
        && matches!(state.pointer_lock_mode(), PointerLockMode::Free)
        && !state.pointer.is_grabbed()
        && !state.ui.is_popup_open()
        && !state.ui.captures_pointer(pos)
        && !state.pointer_button_down
    {
        if let Some((window, _)) = state.window_under_point(pos) {
            let m = VeloxState::window_meta(&window);
            let is_fs = m.fullscreen;
            drop(m);
            if !is_fs
                && !state.is_game_window(&window)
                && state.focused_window().as_ref() != Some(&window)
            {
                // Hover focus must never auto-raise background windows on top of floating apps,
                // nor passively pull focus back into a background fullscreen/game window.
                state.focus_window_with_raise(Some(window), false);
                state.needs_redraw = true;
            }
        }
    }

    let pointer = state.pointer.clone();
    pointer.motion(
        state,
        under,
        &MotionEvent {
            location: pos,
            serial,
            time,
        },
    );
    pointer.frame(state);
}

fn clamp_to_rect(pos: Point<f64, Logical>, rect: Rectangle<i32, Logical>) -> Point<f64, Logical> {
    let min_x = rect.loc.x as f64;
    let min_y = rect.loc.y as f64;
    let max_x = (rect.loc.x + rect.size.w - 1).max(rect.loc.x) as f64;
    let max_y = (rect.loc.y + rect.size.h - 1).max(rect.loc.y) as f64;
    (pos.x.clamp(min_x, max_x), pos.y.clamp(min_y, max_y)).into()
}

fn clamp_to_outputs(state: &VeloxState, pos: Point<f64, Logical>) -> Point<f64, Logical> {
    let mut best_pt = pos;
    let mut best_dist2 = f64::MAX;
    for output in &state.outputs {
        if let Some(geo) = state.active_space().output_geometry(output) {
            let max_x = (geo.loc.x + geo.size.w - 1).max(geo.loc.x) as f64;
            let max_y = (geo.loc.y + geo.size.h - 1).max(geo.loc.y) as f64;
            let cx = pos.x.clamp(geo.loc.x as f64, max_x);
            let cy = pos.y.clamp(geo.loc.y as f64, max_y);
            let dx = pos.x - cx;
            let dy = pos.y - cy;
            let d2 = dx * dx + dy * dy;
            if d2 == 0.0 {
                return pos;
            }
            if d2 < best_dist2 {
                best_dist2 = d2;
                best_pt = (cx, cy).into();
            }
        }
    }
    best_pt
}

fn execute_action(state: &mut VeloxState, action: KeyAction) {
    match action {
        KeyAction::SpawnTerminal => {
            state.ui.close_popup();
            spawn_detached(&state.config.terminal_cmd);
        }
        KeyAction::LaunchFiles => {
            state.ui.close_popup();
            let preferred = state.config.files_cmd.clone();
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
        KeyAction::LaunchSettings => {
            if !state.is_taskbar_locked() {
                state.launch_system_settings();
            }
        }
        KeyAction::ToggleStartMenu => {
            if !state.is_taskbar_locked() {
                state.deactivate_focused_pointer_constraint();
                state.ui.toggle_start_menu();
            }
        }
        KeyAction::ToggleAudioMixer => {
            if !state.is_taskbar_locked() {
                state.deactivate_focused_pointer_constraint();
                state.sys.send(SysCommand::RefreshNow);
                state.ui.toggle_audio_mixer();
            }
        }
        KeyAction::ToggleActiveApps => {
            if !state.is_taskbar_locked() {
                state.deactivate_focused_pointer_constraint();
                state.sys.send(SysCommand::RefreshNow);
                state.ui.toggle_active_apps();
            }
        }
        KeyAction::ToggleCalendar => {
            if !state.is_taskbar_locked() {
                state.deactivate_focused_pointer_constraint();
                state.ui.toggle_calendar();
            }
        }
        KeyAction::ToggleQuickSettings => {
            if !state.is_taskbar_locked() {
                state.deactivate_focused_pointer_constraint();
                state.sys.send(SysCommand::RefreshNow);
                state.ui.toggle_quick_settings();
            }
        }
        KeyAction::ToggleTaskView => {
            if !state.is_taskbar_locked() {
                state.deactivate_focused_pointer_constraint();
                state.ui.toggle_task_view();
            }
        }
        KeyAction::TogglePowerMenu => {
            if !state.is_taskbar_locked() {
                state.deactivate_focused_pointer_constraint();
                state.ui.toggle_power_menu();
            }
        }
        KeyAction::LockScreen => {
            state.lock_screen();
        }
        KeyAction::LockBackspace => {
            state.lock_password_input.pop();
            state.lock_error_msg.clear();
        }
        KeyAction::LockEscape => {
            state.lock_password_input.clear();
            state.lock_error_msg.clear();
        }
        KeyAction::LockEnter => {
            state.submit_lock_password();
        }
        KeyAction::LockChar(txt) => {
            if state.lock_password_input.len() < 128 {
                state.lock_password_input.push_str(&txt);
                state.lock_error_msg.clear();
            }
        }
        KeyAction::LockIgnore => {}
        KeyAction::ToggleNotifications => {
            if !state.is_taskbar_locked() {
                state.deactivate_focused_pointer_constraint();
                state.ui.toggle_notifications();
                if state.ui.active_popup == ActivePopup::Notifications {
                    state.notif_svc.mark_all_read();
                    state.refresh_notifications_and_ui_flags();
                }
            }
        }
        KeyAction::SeamlessReload => {
            state.deactivate_focused_pointer_constraint();
            state
                .ui
                .prompt_reload_confirm("Manual compositor reload requested (Win+Ctrl+Shift+R)");
        }
        KeyAction::ShowDesktop => {
            state.ui.close_popup();
            state.toggle_show_desktop();
        }
        KeyAction::CloseFocused => {
            if let Some(win) = state.focused_window() {
                state.close_window(&win);
            }
        }
        KeyAction::ToggleFloating => state.toggle_floating(),
        KeyAction::ToggleAlwaysOnTop => {
            if let Some(win) = state.focused_window() {
                state.toggle_always_on_top_for(&win);
            }
        }
        KeyAction::ToggleFullscreen => state.toggle_fullscreen(),
        KeyAction::ExpandMaximize => {
            if let Some(win) = state.focused_window() {
                state.expand_or_maximize_for(&win);
            }
        }
        KeyAction::ShrinkOrMinimize => {
            if let Some(win) = state.focused_window() {
                state.shrink_or_minimize_for(&win);
            }
        }
        KeyAction::SnapHalf(zone) => {
            if let Some(win) = state.focused_window() {
                state.snap_direction_for(&win, zone);
            }
        }
        KeyAction::StepAltTab { forward } => {
            state.deactivate_focused_pointer_constraint();
            let total = state.global_windows_list().len();
            state.ui.step_alt_tab(total, forward);
        }
        KeyAction::CommitAltTab => state.commit_alt_tab(),
        KeyAction::FocusDir(dir) => state.focus_direction(dir),
        KeyAction::SwapDir(dir) => state.swap_window_direction(dir),
        KeyAction::SwitchWorkspace(idx) => {
            state.ui.close_popup();
            self::switch_ws_and_redraw(state, idx);
        }
        KeyAction::StepWorkspace(delta) => {
            state.ui.close_popup();
            let next = ((state.active_workspace as i32 + delta).rem_euclid(NUM_WORKSPACES as i32))
                as usize;
            self::switch_ws_and_redraw(state, next);
        }
        KeyAction::MoveToWorkspace(idx) => state.move_focused_to_workspace(idx),
        KeyAction::MoveNextMonitor => state.move_focused_to_next_monitor(),
        KeyAction::Quit => {
            tracing::info!("Quit requested by user");
            state.should_quit = true;
        }
        KeyAction::VolumeUp => {
            let (cur, _) = state.sys_snap.default_output_vol();
            state.sys.send(SysCommand::SetSinkVolume {
                name: "@DEFAULT_SINK@".to_string(),
                pct: (cur as i32 + 5).clamp(0, 150) as u8,
            });
            state.refresh_system_snapshot();
        }
        KeyAction::VolumeDown => {
            let (cur, _) = state.sys_snap.default_output_vol();
            state.sys.send(SysCommand::SetSinkVolume {
                name: "@DEFAULT_SINK@".to_string(),
                pct: (cur as i32 - 5).clamp(0, 150) as u8,
            });
            state.refresh_system_snapshot();
        }
        KeyAction::VolumeMute => {
            state.sys.send(SysCommand::ToggleSinkMute {
                name: "@DEFAULT_SINK@".to_string(),
            });
            state.refresh_system_snapshot();
        }
        KeyAction::PopupEscape => {
            if state.ui.active_popup == ActivePopup::Notifications
                && state.ui.notif_reply_target.is_some()
            {
                state.ui.notif_reply_target = None;
                state.ui.notif_reply_draft.clear();
            } else {
                state.ui.close_popup();
                state.sync_focused_pointer_constraint();
            }
        }
        KeyAction::PopupBackspace => {
            if state.ui.active_popup == ActivePopup::Notifications
                && state.ui.notif_reply_target.is_some()
            {
                state.ui.notif_reply_draft.pop();
            } else {
                state.ui.search_query.pop();
                state.ui.app_scroll = 0;
                state.ui.selected_app_row = 0;
                state.ui.apply_search_smart_switch(&state.apps);
            }
        }
        KeyAction::PopupUp => {
            state.ui.selected_app_row = state.ui.selected_app_row.saturating_sub(1);
            if state.ui.selected_app_row < state.ui.app_scroll {
                state.ui.app_scroll = state.ui.selected_app_row;
            }
        }
        KeyAction::PopupDown => {
            let total = state.ui.filtered_app_indices(&state.apps).len();
            if total > 0 {
                state.ui.selected_app_row = (state.ui.selected_app_row + 1).min(total - 1);
                if state.ui.selected_app_row >= state.ui.app_scroll + 8 {
                    state.ui.app_scroll = state.ui.selected_app_row - 7;
                }
            }
        }
        KeyAction::PopupEnter => {
            if matches!(state.ui.active_popup, ActivePopup::ReloadConfirm { .. }) {
                state.trigger_seamless_reload();
            } else if state.ui.active_popup == ActivePopup::Notifications {
                if let Some(id) = state.ui.notif_reply_target.take() {
                    let reply = std::mem::take(&mut state.ui.notif_reply_draft);
                    state.notif_svc.send_reply(id, &reply);
                    state.refresh_notifications_and_ui_flags();
                }
            } else {
                let filtered = state.ui.filtered_app_indices(&state.apps);
                if let Some(&app_idx) = filtered.get(state.ui.selected_app_row) {
                    state.launch_app_by_index(app_idx);
                }
            }
        }
        KeyAction::PopupChar(txt) => {
            if !txt.is_empty() {
                if state.ui.active_popup == ActivePopup::Notifications
                    && state.ui.notif_reply_target.is_some()
                {
                    state.ui.notif_reply_draft.push_str(&txt);
                } else {
                    state.ui.search_query.push_str(&txt);
                    state.ui.app_scroll = 0;
                    state.ui.selected_app_row = 0;
                    state.ui.apply_search_smart_switch(&state.apps);
                }
            }
        }
    }
    state.needs_redraw = true;
}

fn switch_ws_and_redraw(state: &mut VeloxState, idx: usize) {
    state.switch_workspace(idx);
}

pub fn spawn_detached(cmd: &str) {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if let Err(err) = command.spawn() {
        tracing::warn!(?err, cmd, "Failed to spawn command");
    }
}

struct EventTime(u64);

impl EventTime {
    fn from_micros(usec: u64) -> Self {
        Self(usec)
    }

    fn millis(&self) -> u32 {
        (self.0 / 1000) as u32
    }
}
