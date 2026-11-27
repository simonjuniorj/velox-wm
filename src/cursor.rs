use drm_fourcc::DrmFourcc;
use smithay::{
    backend::renderer::{
        element::{
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            Kind,
        },
        gles::GlesRenderer,
        ImportAll, ImportMem,
    },
    input::pointer::CursorImageStatus,
    render_elements,
    utils::{Logical, Point, Transform},
};

render_elements! {
    pub CursorRenderElement<R> where R: ImportAll + ImportMem;
    Surface=WaylandSurfaceRenderElement<R>,
    Memory=MemoryRenderBufferRenderElement<R>,
}

/// Manages rendering the pointer cursor in bare-metal DRM mode.
pub struct CursorManager {
    default_buffer: MemoryRenderBuffer,
}

impl CursorManager {
    pub fn new() -> Self {
        let (rgba, w, h) = build_default_cursor_rgba();
        let default_buffer = MemoryRenderBuffer::from_slice(
            &rgba,
            DrmFourcc::Abgr8888,
            (w, h),
            1,
            Transform::Normal,
            None,
        );
        Self { default_buffer }
    }

    pub fn render_elements(
        &self,
        renderer: &mut GlesRenderer,
        status: &CursorImageStatus,
        location: Point<f64, Logical>,
        scale: f64,
    ) -> Vec<CursorRenderElement<GlesRenderer>> {
        match status {
            CursorImageStatus::Hidden => Vec::new(),
            CursorImageStatus::Surface(surface) => {
                let pos = location.to_physical_precise_round(scale);
                render_elements_from_surface_tree(
                    renderer,
                    surface,
                    pos,
                    scale,
                    1.0,
                    Kind::Cursor,
                )
                .into_iter()
                .map(CursorRenderElement::Surface)
                .collect()
            }
            CursorImageStatus::Named(_) => {
                let pos = location.to_physical_precise_round::<f64, i32>(scale);
                match MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    (pos.x as f64, pos.y as f64),
                    &self.default_buffer,
                    None,
                    None,
                    None,
                    Kind::Cursor,
                ) {
                    Ok(elem) => vec![CursorRenderElement::Memory(elem)],
                    Err(_) => Vec::new(),
                }
            }
        }
    }
}

/// Generates a crisp 20x20 white arrow cursor with a dark 1px border.
fn build_default_cursor_rgba() -> (Vec<u8>, i32, i32) {
    const W: usize = 20;
    const H: usize = 20;
    #[rustfmt::skip]
    const SHAPE: [&str; H] = [
        "B...................",
        "BB..................",
        "BWB.................",
        "BWWB................",
        "BWWWB...............",
        "BWWWWB..............",
        "BWWWWWB.............",
        "BWWWWWWB............",
        "BWWWWWWWB...........",
        "BWWWWWWWWB..........",
        "BWWWWWWWWWB.........",
        "BWWWWWWWWWWB........",
        "BWWWWWWBBBBBB.......",
        "BWWWBWWB............",
        "BWWB.BWWB...........",
        "BWB..BWWB...........",
        "BB....BWWB..........",
        "B.....BWWB..........",
        ".......BB...........",
        "....................",
    ];

    let mut rgba = Vec::with_capacity(W * H * 4);
    for row in SHAPE {
        for ch in row.bytes() {
            match ch {
                b'B' => rgba.extend_from_slice(&[12, 16, 28, 255]),
                b'W' => rgba.extend_from_slice(&[245, 248, 255, 255]),
                _ => rgba.extend_from_slice(&[0, 0, 0, 0]),
            }
        }
    }
    (rgba, W as i32, H as i32)
}
