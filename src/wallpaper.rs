use std::path::Path;

use drm_fourcc::DrmFourcc;
use image::imageops::FilterType;
use smithay::{
    backend::renderer::{
        element::{
            texture::{TextureBuffer, TextureRenderElement},
            Kind,
        },
        gles::{GlesRenderer, GlesTexture},
        ImportMem,
    },
    utils::{Physical, Point, Rectangle, Size, Transform},
};

/// Holds the native-resolution GPU-uploaded wallpaper texture so rendering requires zero
/// per-frame GPU upscaling shader work.
pub struct Wallpaper {
    buffer: TextureBuffer<GlesTexture>,
    src_size: Size<i32, Physical>,
}

impl Wallpaper {
    /// Loads a custom wallpaper (pre-cropped/scaled once on CPU to `target_size`) or generates
    /// one of the built-in procedural themes at exact `target_size` for 1:1 blitting.
    pub fn load_or_generate(
        renderer: &mut GlesRenderer,
        preset: &str,
        custom_path: Option<&Path>,
        target_size: Size<i32, Physical>,
    ) -> Option<Self> {
        let tw = target_size.w.max(320) as u32;
        let th = target_size.h.max(240) as u32;

        let (rgba, width, height) = if let Some(p) = custom_path {
            match load_image_rgba_cover(p, tw, th) {
                Ok(data) => {
                    tracing::info!(?p, tw, th, "Loaded custom wallpaper at native resolution");
                    data
                }
                Err(err) => {
                    tracing::warn!(?err, ?p, "Failed to load wallpaper image, using preset");
                    generate_preset_rgba(preset, tw, th)
                }
            }
        } else {
            generate_preset_rgba(preset, tw, th)
        };

        let size: Size<i32, smithay::utils::Buffer> = (width as i32, height as i32).into();
        let texture = match renderer.import_memory(&rgba, DrmFourcc::Abgr8888, size, false) {
            Ok(tex) => tex,
            Err(err) => {
                tracing::warn!(?err, "Failed to upload wallpaper texture to GPU");
                return None;
            }
        };

        let buffer = TextureBuffer::from_texture(renderer, texture, 1, Transform::Normal, None);

        Some(Self {
            buffer,
            src_size: (width as i32, height as i32).into(),
        })
    }

    pub fn matches_size(&self, output_size: Size<i32, Physical>) -> bool {
        self.src_size == output_size
    }

    /// Produces a 1:1 native-resolution render element when matched to `output_size`.
    pub fn render_element(
        &self,
        output_size: Size<i32, Physical>,
    ) -> TextureRenderElement<GlesTexture> {
        let src_rect = Rectangle::from_size(self.src_size.to_f64());
        let dst = if self.src_size == output_size {
            None
        } else {
            Some((output_size.w, output_size.h).into())
        };
        TextureRenderElement::from_texture_buffer(
            Point::<f64, Physical>::from((0.0, 0.0)),
            &self.buffer,
            None,
            Some(src_rect.to_logical(1.0)),
            dst,
            Kind::Unspecified,
        )
    }
}

fn load_image_rgba_cover(
    path: &Path,
    target_w: u32,
    target_h: u32,
) -> Result<(Vec<u8>, u32, u32), Box<dyn std::error::Error>> {
    let img = image::open(path)?;
    let resized = img.resize_to_fill(target_w, target_h, FilterType::Triangle).into_rgba8();
    let (w, h) = resized.dimensions();
    Ok((resized.into_raw(), w, h))
}

/// Generates one of 6 crisp procedural GPU wallpapers at exact native output resolution.
fn generate_preset_rgba(preset: &str, width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let mut buf = vec![0u8; (width * height * 4) as usize];
    let wf = width as f32;
    let hf = height as f32;

    let (base, c1, c2) = match preset.to_ascii_lowercase().as_str() {
        "nebula" => ([10.0, 6.0, 24.0], [115.0, 48.0, 175.0], [32.0, 110.0, 195.0]),
        "ocean" => ([4.0, 14.0, 28.0], [18.0, 115.0, 165.0], [12.0, 155.0, 145.0]),
        "emerald" => ([5.0, 16.0, 14.0], [18.0, 128.0, 92.0], [28.0, 88.0, 130.0]),
        "sunset" => ([16.0, 8.0, 22.0], [195.0, 72.0, 96.0], [235.0, 142.0, 64.0]),
        "carbon" => ([8.0, 10.0, 16.0], [28.0, 34.0, 52.0], [20.0, 24.0, 38.0]),
        _ => ([6.0, 10.0, 24.0], [24.0, 62.0, 148.0], [18.0, 88.0, 132.0]),
    };

    // Precompute 1D x-terms to keep generation fast even at 2560x1440 / 4K.
    let mut x_vals = Vec::with_capacity(width as usize);
    for x in 0..width {
        let nx = x as f32 / wf;
        x_vals.push(nx);
    }

    for y in 0..height {
        let ny = y as f32 / hf;
        let dy1 = ny - 0.28;
        let dy2 = ny - 0.76;
        let dy1_term = -(dy1 * dy1 * 3.0);
        let dy2_term = -(dy2 * dy2 * 3.2);
        let row_off = (y * width * 4) as usize;

        for (x, &nx) in x_vals.iter().enumerate() {
            let dx1 = nx - 0.72;
            let g1 = (-(dx1 * dx1 * 2.2) + dy1_term).exp();

            let dx2 = nx - 0.22;
            let g2 = (-(dx2 * dx2 * 2.4) + dy2_term).exp();

            let wave = (0.5 + 0.5 * (nx * 4.2 + ny * 2.1).sin()) * 0.14;

            let idx = row_off + x * 4;
            buf[idx] = (base[0] + c1[0] * g1 + c2[0] * (g2 + wave)).clamp(0.0, 255.0) as u8;
            buf[idx + 1] = (base[1] + c1[1] * g1 + c2[1] * (g2 + wave)).clamp(0.0, 255.0) as u8;
            buf[idx + 2] = (base[2] + c1[2] * g1 + c2[2] * (g2 + wave)).clamp(0.0, 255.0) as u8;
            buf[idx + 3] = 255;
        }
    }

    (buf, width, height)
}
