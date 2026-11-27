use smithay::{
    delegate_layer_shell,
    desktop::{layer_map_for_output, LayerSurface, WindowSurfaceType},
    output::Output,
    reexports::wayland_server::protocol::wl_output,
    wayland::shell::wlr_layer::{
        Layer, LayerSurfaceData, WlrLayerShellHandler, WlrLayerShellState,
    },
};

use crate::state::VeloxState;

impl WlrLayerShellHandler for VeloxState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: smithay::wayland::shell::wlr_layer::LayerSurface,
        output: Option<wl_output::WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.outputs.first().cloned());

        let Some(output) = output else {
            tracing::warn!(namespace, "No output available for layer surface");
            return;
        };

        let mut map = layer_map_for_output(&output);
        if let Err(err) = map.map_layer(&LayerSurface::new(surface, namespace)) {
            tracing::warn!(?err, "Failed to map layer surface");
        }
        drop(map);

        self.relayout_active_workspace();
    }

    fn layer_destroyed(&mut self, surface: smithay::wayland::shell::wlr_layer::LayerSurface) {
        for output in &self.outputs {
            let mut map = layer_map_for_output(output);
            let found = map
                .layers()
                .find(|l| l.layer_surface() == &surface)
                .cloned();
            if let Some(layer) = found {
                map.unmap_layer(&layer);
            }
        }
        self.relayout_active_workspace();
    }
}

delegate_layer_shell!(VeloxState);

/// Handles commits on layer-shell surfaces so their initial configure and exclusive zones update.
pub fn handle_layer_commit(state: &mut VeloxState, surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface) {
    let mut changed = false;
    for output in &state.outputs {
        let mut map = layer_map_for_output(output);
        let found = map
            .layers()
            .find(|l| l.wl_surface() == surface)
            .cloned();
        if let Some(layer) = found {
            let initial_configure_sent = smithay::wayland::compositor::with_states(surface, |states| {
                states
                    .data_map
                    .get::<LayerSurfaceData>()
                    .map(|data| data.lock().unwrap().initial_configure_sent)
                    .unwrap_or(true)
            });

            if !initial_configure_sent {
                layer.layer_surface().send_configure();
            }
            map.arrange();
            changed = true;
            let _ = WindowSurfaceType::ALL;
        }
    }
    if changed {
        state.relayout_active_workspace();
    }
}
