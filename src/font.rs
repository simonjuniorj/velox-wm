use std::{collections::HashMap, fs};

use fontdue::{Font, FontSettings, Metrics};

const FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/TTF/FiraSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];

const FALLBACK_FONT_CANDIDATES: &[(&str, u32)] = &[
    ("/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf", 0),
    ("/usr/share/fonts/noto/NotoSansSymbols-Regular.ttf", 0),
    ("/usr/share/fonts/noto/NotoSansMath-Regular.ttf", 0),
    ("/usr/share/fonts/TTF/MesloLGLNerdFont-Regular.ttf", 0),
    ("/usr/share/fonts/TTF/MesloLGMNerdFont-Regular.ttf", 0),
    ("/usr/share/fonts/OTF/PowerlineSymbols.otf", 0),
    ("/usr/share/fonts/awesome-terminal-fonts/fontawesome-regular.ttf", 0),
    ("/usr/share/fonts/awesome-terminal-fonts/octicons-regular.ttf", 0),
    ("/usr/share/fonts/awesome-terminal-fonts/devicons-regular.ttf", 0),
    ("/usr/share/fonts/awesome-terminal-fonts/pomicons-regular.ttf", 0),
    ("/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf", 0),
    ("/usr/share/fonts/TTF/DejaVuSansMono.ttf", 0),
    ("/usr/share/fonts/TTF/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/noto/NotoSans-Regular.ttf", 0),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
];

const EMOJI_FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/noto/NotoColorEmoji.ttf",
    "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
    "/usr/share/fonts/TTF/NotoColorEmoji.ttf",
];

#[derive(Clone)]
struct ColorGlyph {
    width: usize,
    height: usize,
    advance: f32,
    xmin: f32,
    top_from_baseline: f32,
    rgba: Vec<u8>,
}

struct CmapRange {
    start: u32,
    end: u32,
    start_gid: u32,
}

#[derive(Clone, Copy)]
struct StrikeGlyphMetrics {
    bearing_x: i8,
    bearing_y: i8,
    advance: u8,
}

#[derive(Clone)]
enum IndexSubtable {
    Fmt1 {
        first_glyph: u16,
        last_glyph: u16,
        image_format: u16,
        image_data_offset: u32,
        offsets: Vec<u32>,
    },
    Fmt2 {
        first_glyph: u16,
        last_glyph: u16,
        image_format: u16,
        image_data_offset: u32,
        image_size: u32,
        metrics: StrikeGlyphMetrics,
    },
    Fmt3 {
        first_glyph: u16,
        last_glyph: u16,
        image_format: u16,
        image_data_offset: u32,
        offsets: Vec<u16>,
    },
}

struct ColorEmojiFont {
    data: Vec<u8>,
    cmap_ranges: Vec<CmapRange>,
    cbdt_offset: usize,
    ppem: u8,
    subtables: Vec<IndexSubtable>,
}

impl ColorEmojiFont {
    fn load(path: &str) -> Option<Self> {
        let data = fs::read(path).ok()?;
        Self::from_bytes(data)
    }

    fn from_bytes(data: Vec<u8>) -> Option<Self> {
        let num_tables = read_u16(&data, 4)? as usize;
        let mut cmap_table = None;
        let mut cblc_table = None;
        let mut cbdt_table = None;
        for i in 0..num_tables {
            let rec = 12 + i * 16;
            let tag = data.get(rec..rec + 4)?;
            let offset = read_u32(&data, rec + 8)? as usize;
            let length = read_u32(&data, rec + 12)? as usize;
            match tag {
                b"cmap" => cmap_table = Some((offset, length)),
                b"CBLC" => cblc_table = Some((offset, length)),
                b"CBDT" => cbdt_table = Some((offset, length)),
                _ => {}
            }
        }
        let (cmap_off, _) = cmap_table?;
        let (cblc_off, _) = cblc_table?;
        let (cbdt_off, _) = cbdt_table?;

        let cmap_ranges = parse_cmap_fmt12(&data, cmap_off)?;
        let (ppem, subtables) = parse_cblc_strike0(&data, cblc_off)?;
        Some(Self {
            data,
            cmap_ranges,
            cbdt_offset: cbdt_off,
            ppem: ppem.max(1),
            subtables,
        })
    }

    fn lookup_gid(&self, cp: u32) -> Option<u16> {
        let idx = self.cmap_ranges.binary_search_by(|r| {
            if cp < r.start {
                std::cmp::Ordering::Greater
            } else if cp > r.end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        });
        let r = self.cmap_ranges.get(idx.ok()?)?;
        let gid = r.start_gid + (cp - r.start);
        u16::try_from(gid).ok().filter(|&g| g > 0)
    }

    fn has_char(&self, ch: char) -> bool {
        self.lookup_gid(ch as u32).is_some()
    }

    fn glyph_png_and_metrics(&self, ch: char) -> Option<(StrikeGlyphMetrics, &[u8])> {
        let gid = self.lookup_gid(ch as u32)?;
        for sub in &self.subtables {
            match sub {
                IndexSubtable::Fmt1 {
                    first_glyph,
                    last_glyph,
                    image_format,
                    image_data_offset,
                    offsets,
                } => {
                    if gid < *first_glyph || gid > *last_glyph {
                        continue;
                    }
                    let idx = (gid - *first_glyph) as usize;
                    let rel0 = *offsets.get(idx)? as usize;
                    let rel1 = *offsets.get(idx + 1)? as usize;
                    if rel1 <= rel0 {
                        return None;
                    }
                    let start = self.cbdt_offset + *image_data_offset as usize + rel0;
                    let end = self.cbdt_offset + *image_data_offset as usize + rel1;
                    return extract_cbdt_glyph(self.data.get(start..end)?, *image_format, None);
                }
                IndexSubtable::Fmt2 {
                    first_glyph,
                    last_glyph,
                    image_format,
                    image_data_offset,
                    image_size,
                    metrics,
                } => {
                    if gid < *first_glyph || gid > *last_glyph || *image_size == 0 {
                        continue;
                    }
                    let idx = (gid - *first_glyph) as usize;
                    let start = self.cbdt_offset
                        + *image_data_offset as usize
                        + idx * (*image_size as usize);
                    let end = start + *image_size as usize;
                    return extract_cbdt_glyph(
                        self.data.get(start..end)?,
                        *image_format,
                        Some(*metrics),
                    );
                }
                IndexSubtable::Fmt3 {
                    first_glyph,
                    last_glyph,
                    image_format,
                    image_data_offset,
                    offsets,
                } => {
                    if gid < *first_glyph || gid > *last_glyph {
                        continue;
                    }
                    let idx = (gid - *first_glyph) as usize;
                    let rel0 = *offsets.get(idx)? as usize;
                    let rel1 = *offsets.get(idx + 1)? as usize;
                    if rel1 <= rel0 {
                        return None;
                    }
                    let start = self.cbdt_offset + *image_data_offset as usize + rel0;
                    let end = self.cbdt_offset + *image_data_offset as usize + rel1;
                    return extract_cbdt_glyph(self.data.get(start..end)?, *image_format, None);
                }
            }
        }
        None
    }

    fn rasterize(&self, ch: char, px: f32) -> Option<ColorGlyph> {
        let (metrics, png_bytes) = self.glyph_png_and_metrics(ch)?;
        let decoded =
            image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png).ok()?;
        let rgba_img = decoded.into_rgba8();
        let src_w = rgba_img.width() as usize;
        let src_h = rgba_img.height() as usize;
        if src_w == 0 || src_h == 0 {
            return None;
        }
        let scale = (px * 0.98 / self.ppem as f32).max(0.04);
        let dst_w = ((src_w as f32 * scale).round() as usize).clamp(4, 160);
        let dst_h = ((src_h as f32 * scale).round() as usize).clamp(4, 160);
        let scaled = downsample_rgba_area(rgba_img.as_raw(), src_w, src_h, dst_w, dst_h);
        let adv_base = if metrics.advance > 0 {
            metrics.advance as f32
        } else {
            self.ppem as f32
        };
        let top_base = if metrics.bearing_y != 0 {
            metrics.bearing_y as f32 * 0.90
        } else {
            self.ppem as f32 * 0.84
        };
        Some(ColorGlyph {
            width: dst_w,
            height: dst_h,
            advance: adv_base * scale + 1.0,
            xmin: metrics.bearing_x as f32 * scale,
            top_from_baseline: top_base * scale,
            rgba: scaled,
        })
    }
}

fn read_u16(data: &[u8], off: usize) -> Option<u16> {
    let b: [u8; 2] = data.get(off..off + 2)?.try_into().ok()?;
    Some(u16::from_be_bytes(b))
}

fn read_u32(data: &[u8], off: usize) -> Option<u32> {
    let b: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(u32::from_be_bytes(b))
}

fn parse_cmap_fmt12(data: &[u8], cmap_off: usize) -> Option<Vec<CmapRange>> {
    let num_sub = read_u16(data, cmap_off + 2)? as usize;
    for i in 0..num_sub {
        let rec = cmap_off + 4 + i * 8;
        let sub_off = cmap_off + read_u32(data, rec + 4)? as usize;
        if read_u16(data, sub_off)? == 12 {
            let num_groups = read_u32(data, sub_off + 12)? as usize;
            let mut ranges = Vec::with_capacity(num_groups);
            for g in 0..num_groups {
                let goff = sub_off + 16 + g * 12;
                let start = read_u32(data, goff)?;
                let end = read_u32(data, goff + 4)?;
                let start_gid = read_u32(data, goff + 8)?;
                ranges.push(CmapRange {
                    start,
                    end,
                    start_gid,
                });
            }
            return Some(ranges);
        }
    }
    None
}

fn parse_cblc_strike0(data: &[u8], cblc_off: usize) -> Option<(u8, Vec<IndexSubtable>)> {
    let num_sizes = read_u32(data, cblc_off + 4)?;
    if num_sizes == 0 {
        return None;
    }
    let rec = cblc_off + 8;
    let index_subtable_array_off = read_u32(data, rec)? as usize;
    let num_subtables = read_u32(data, rec + 8)? as usize;
    let ppem_y = *data.get(rec + 45)?;
    let array_abs = cblc_off + index_subtable_array_off;
    let mut subtables = Vec::with_capacity(num_subtables);
    for i in 0..num_subtables {
        let item = array_abs + i * 8;
        let first_glyph = read_u16(data, item)?;
        let last_glyph = read_u16(data, item + 2)?;
        if last_glyph < first_glyph {
            continue;
        }
        let add_off = read_u32(data, item + 4)? as usize;
        let sub_abs = array_abs + add_off;
        let index_format = read_u16(data, sub_abs)?;
        let image_format = read_u16(data, sub_abs + 2)?;
        let image_data_offset = read_u32(data, sub_abs + 4)?;
        let count = (last_glyph - first_glyph) as usize + 1;
        match index_format {
            1 => {
                let mut offsets = Vec::with_capacity(count + 1);
                for k in 0..=count {
                    offsets.push(read_u32(data, sub_abs + 8 + k * 4)?);
                }
                subtables.push(IndexSubtable::Fmt1 {
                    first_glyph,
                    last_glyph,
                    image_format,
                    image_data_offset,
                    offsets,
                });
            }
            2 => {
                let image_size = read_u32(data, sub_abs + 8)?;
                let metrics = parse_small_metrics_from_big(data.get(sub_abs + 12..sub_abs + 20)?)?;
                subtables.push(IndexSubtable::Fmt2 {
                    first_glyph,
                    last_glyph,
                    image_format,
                    image_data_offset,
                    image_size,
                    metrics,
                });
            }
            3 => {
                let mut offsets = Vec::with_capacity(count + 1);
                for k in 0..=count {
                    offsets.push(read_u16(data, sub_abs + 8 + k * 2)?);
                }
                subtables.push(IndexSubtable::Fmt3 {
                    first_glyph,
                    last_glyph,
                    image_format,
                    image_data_offset,
                    offsets,
                });
            }
            _ => {}
        }
    }
    Some((ppem_y, subtables))
}

fn parse_small_metrics_from_big(slice: &[u8]) -> Option<StrikeGlyphMetrics> {
    if slice.len() < 8 {
        return None;
    }
    Some(StrikeGlyphMetrics {
        bearing_x: slice[2] as i8,
        bearing_y: slice[3] as i8,
        advance: slice[4],
    })
}

fn extract_cbdt_glyph(
    slice: &[u8],
    image_format: u16,
    fallback_metrics: Option<StrikeGlyphMetrics>,
) -> Option<(StrikeGlyphMetrics, &[u8])> {
    match image_format {
        17 => {
            if slice.len() < 9 {
                return None;
            }
            let metrics = StrikeGlyphMetrics {
                bearing_x: slice[2] as i8,
                bearing_y: slice[3] as i8,
                advance: slice[4],
            };
            let len = read_u32(slice, 5)? as usize;
            let png = slice.get(9..9 + len)?;
            Some((metrics, png))
        }
        18 => {
            if slice.len() < 12 {
                return None;
            }
            let metrics = parse_small_metrics_from_big(&slice[..8])?;
            let len = read_u32(slice, 8)? as usize;
            let png = slice.get(12..12 + len)?;
            Some((metrics, png))
        }
        19 => {
            let len = read_u32(slice, 0)? as usize;
            let png = slice.get(4..4 + len)?;
            let metrics = fallback_metrics.unwrap_or(StrikeGlyphMetrics {
                bearing_x: 0,
                bearing_y: 0,
                advance: 0,
            });
            Some((metrics, png))
        }
        _ => None,
    }
}

/// Premultiplied area-box downsampler so downscaled PNG emoji glyphs have clean, dark-fringe-free edges.
fn downsample_rgba_area(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; dst_w * dst_h * 4];
    for dy in 0..dst_h {
        let sy0 = (dy as f32 * src_h as f32) / dst_h as f32;
        let sy1 = ((dy + 1) as f32 * src_h as f32) / dst_h as f32;
        let y0 = (sy0.floor() as usize).min(src_h - 1);
        let y1 = (sy1.ceil() as usize).max(y0 + 1).min(src_h);
        for dx in 0..dst_w {
            let sx0 = (dx as f32 * src_w as f32) / dst_w as f32;
            let sx1 = ((dx + 1) as f32 * src_w as f32) / dst_w as f32;
            let x0 = (sx0.floor() as usize).min(src_w - 1);
            let x1 = (sx1.ceil() as usize).max(x0 + 1).min(src_w);

            let mut acc_r = 0.0f32;
            let mut acc_g = 0.0f32;
            let mut acc_b = 0.0f32;
            let mut acc_a = 0.0f32;
            let mut total_w = 0.0f32;

            for sy in y0..y1 {
                let wy = (sy1.min((sy + 1) as f32) - sy0.max(sy as f32)).max(0.0);
                if wy <= 0.0 {
                    continue;
                }
                for sx in x0..x1 {
                    let wx = (sx1.min((sx + 1) as f32) - sx0.max(sx as f32)).max(0.0);
                    let w = wx * wy;
                    if w <= 0.0 {
                        continue;
                    }
                    let idx = (sy * src_w + sx) * 4;
                    let a = src[idx + 3] as f32 / 255.0;
                    acc_r += src[idx] as f32 * a * w;
                    acc_g += src[idx + 1] as f32 * a * w;
                    acc_b += src[idx + 2] as f32 * a * w;
                    acc_a += src[idx + 3] as f32 * w;
                    total_w += w;
                }
            }
            if total_w > 0.0 {
                let out_a = (acc_a / total_w).round().clamp(0.0, 255.0) as u8;
                let dst_idx = (dy * dst_w + dx) * 4;
                if out_a > 0 {
                    let a_norm = acc_a / 255.0;
                    out[dst_idx] = (acc_r / a_norm).round().clamp(0.0, 255.0) as u8;
                    out[dst_idx + 1] = (acc_g / a_norm).round().clamp(0.0, 255.0) as u8;
                    out[dst_idx + 2] = (acc_b / a_norm).round().clamp(0.0, 255.0) as u8;
                    out[dst_idx + 3] = out_a;
                }
            }
        }
    }
    out
}

#[inline]
fn is_ignored_codepoint(ch: char) -> bool {
    matches!(
        ch,
        '\u{200B}'..='\u{200D}' | '\u{FEFF}' | '\u{FE00}'..='\u{FE0F}' | '\u{20E3}'
    )
}

#[inline]
fn is_emoji_preferred(ch: char, next_ch: Option<char>) -> bool {
    if next_ch == Some('\u{FE0F}') {
        return true;
    }
    let cp = ch as u32;
    cp >= 0x1F000
        || matches!(
            ch,
            '\u{2600}'..='\u{26FF}'
                | '\u{2702}'..='\u{27B0}'
                | '\u{231A}'..='\u{231B}'
                | '\u{23E9}'..='\u{23FA}'
                | '\u{25AA}'..='\u{25FE}'
                | '\u{2B05}'..='\u{2B55}'
        )
}

enum ResolvedGlyph<'a> {
    Mono(&'a (Metrics, Vec<u8>)),
    Color(&'a ColorGlyph),
}

/// Fast, cached TrueType + Color Emoji font rasterizer and 2D RGBA UI primitives.
pub struct UiPainter {
    font: Option<Font>,
    fallback_fonts: Vec<Font>,
    emoji_font: Option<ColorEmojiFont>,
    glyphs: HashMap<(char, u16), (Metrics, Vec<u8>)>,
    emoji_glyphs: HashMap<(char, u16), Option<ColorGlyph>>,
}

impl UiPainter {
    pub fn new() -> Self {
        let mut loaded_primary = None;
        let mut primary_path = None;
        for path in FONT_CANDIDATES {
            let Ok(bytes) = fs::read(path) else {
                continue;
            };
            match Font::from_bytes(bytes, FontSettings::default()) {
                Ok(f) => {
                    tracing::info!(font = path, "Loaded UI system font");
                    loaded_primary = Some(f);
                    primary_path = Some(*path);
                    break;
                }
                Err(err) => {
                    tracing::warn!(?err, font = path, "Failed to parse font");
                }
            }
        }

        let mut fallback_fonts = Vec::new();
        for (path, collection_index) in FALLBACK_FONT_CANDIDATES {
            if primary_path == Some(*path) && *collection_index == 0 {
                continue;
            }
            let Ok(bytes) = fs::read(path) else {
                continue;
            };
            let settings = FontSettings {
                collection_index: *collection_index,
                ..FontSettings::default()
            };
            if let Ok(f) = Font::from_bytes(bytes, settings) {
                fallback_fonts.push(f);
            }
        }

        let emoji_font = EMOJI_FONT_CANDIDATES.iter().find_map(|path| {
            let font = ColorEmojiFont::load(path);
            if font.is_some() {
                tracing::info!(font = path, "Loaded color emoji font");
            }
            font
        });

        Self {
            font: loaded_primary,
            fallback_fonts,
            emoji_font,
            glyphs: HashMap::new(),
            emoji_glyphs: HashMap::new(),
        }
    }

    fn mono_glyph(&mut self, ch: char, px: f32) -> Option<&(Metrics, Vec<u8>)> {
        let key = (ch, (px * 10.0).round() as u16);
        if !self.glyphs.contains_key(&key) {
            if self.glyphs.len() >= 2048 {
                self.glyphs.clear();
                self.glyphs.shrink_to(256);
            }
            let selected = self
                .font
                .as_ref()
                .filter(|f| f.lookup_glyph_index(ch) != 0)
                .or_else(|| {
                    self.fallback_fonts
                        .iter()
                        .find(|f| f.lookup_glyph_index(ch) != 0)
                })
                .or(self.font.as_ref())?;
            let entry = selected.rasterize(ch, px);
            self.glyphs.insert(key, entry);
        }
        self.glyphs.get(&key)
    }

    fn emoji_glyph(&mut self, ch: char, px: f32) -> Option<&ColorGlyph> {
        let emoji_font = self.emoji_font.as_ref()?;
        if !emoji_font.has_char(ch) {
            return None;
        }
        let key = (ch, (px * 10.0).round() as u16);
        if !self.emoji_glyphs.contains_key(&key) {
            if self.emoji_glyphs.len() >= 1024 {
                self.emoji_glyphs.clear();
                self.emoji_glyphs.shrink_to(128);
            }
            let entry = emoji_font.rasterize(ch, px);
            self.emoji_glyphs.insert(key, entry);
        }
        self.emoji_glyphs.get(&key).and_then(|o| o.as_ref())
    }

    fn resolve_char(&mut self, ch: char, next_ch: Option<char>, px: f32) -> Option<ResolvedGlyph<'_>> {
        let prefer_emoji = is_emoji_preferred(ch, next_ch);
        let primary_has = self
            .font
            .as_ref()
            .map(|f| f.lookup_glyph_index(ch) != 0)
            .unwrap_or(false);
        let emoji_has = self
            .emoji_font
            .as_ref()
            .map(|e| e.has_char(ch))
            .unwrap_or(false);
        let fallback_has = !primary_has
            && self
                .fallback_fonts
                .iter()
                .any(|f| f.lookup_glyph_index(ch) != 0);

        let use_emoji = emoji_has && (prefer_emoji || (!primary_has && !fallback_has));
        if use_emoji && self.emoji_glyph(ch, px).is_some() {
            let key = (ch, (px * 10.0).round() as u16);
            return self
                .emoji_glyphs
                .get(&key)
                .and_then(|o| o.as_ref())
                .map(ResolvedGlyph::Color);
        }
        self.mono_glyph(ch, px).map(ResolvedGlyph::Mono)
    }

    pub fn trim_glyph_cache(&mut self, max_entries: usize) {
        if self.glyphs.len() > max_entries {
            self.glyphs.clear();
            self.glyphs.shrink_to(256);
        }
        if self.emoji_glyphs.len() > max_entries / 2 {
            self.emoji_glyphs.clear();
            self.emoji_glyphs.shrink_to(128);
        }
    }

    pub fn measure_text(&mut self, text: &str, px: f32) -> i32 {
        let mut width = 0.0f32;
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if is_ignored_codepoint(ch) {
                continue;
            }
            let next_ch = chars.peek().copied();
            match self.resolve_char(ch, next_ch, px) {
                Some(ResolvedGlyph::Mono((m, _))) => width += m.advance_width,
                Some(ResolvedGlyph::Color(g)) => width += g.advance,
                None => width += px * 0.55,
            }
        }
        width.ceil() as i32
    }

    /// Draws anti-aliased text and color emojis with `top_y` as the visual line top.
    pub fn draw_text(
        &mut self,
        buf: &mut [u8],
        buf_w: i32,
        buf_h: i32,
        x: i32,
        top_y: i32,
        max_width: i32,
        text: &str,
        px: f32,
        color: [u8; 4],
    ) {
        if max_width <= 0 || (self.font.is_none() && self.emoji_font.is_none()) {
            return;
        }
        let baseline = top_y as f32 + px * 0.82;
        let mut cursor_x = x as f32;
        let limit_x = (x + max_width) as f32;
        let clip_right = (x + max_width).min(buf_w);

        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if is_ignored_codepoint(ch) {
                continue;
            }
            let first_glyph = cursor_x == x as f32;
            let next_ch = chars.peek().copied();
            match self.resolve_char(ch, next_ch, px) {
                Some(ResolvedGlyph::Color(g)) => {
                    if !first_glyph && cursor_x + g.advance > limit_x {
                        break;
                    }
                    let gx0 = (cursor_x + g.xmin).round() as i32;
                    let gy0 = (baseline - g.top_from_baseline).round() as i32;
                    for row in 0..g.height {
                        let py = gy0 + row as i32;
                        if py < 0 || py >= buf_h {
                            continue;
                        }
                        for col in 0..g.width {
                            let px_x = gx0 + col as i32;
                            if px_x < 0 || px_x >= clip_right {
                                continue;
                            }
                            let idx = (row * g.width + col) * 4;
                            let a = ((g.rgba[idx + 3] as u32 * color[3] as u32) / 255) as u8;
                            if a == 0 {
                                continue;
                            }
                            blend_pixel(
                                buf,
                                buf_w,
                                px_x,
                                py,
                                [g.rgba[idx], g.rgba[idx + 1], g.rgba[idx + 2], a],
                            );
                        }
                    }
                    cursor_x += g.advance;
                }
                Some(ResolvedGlyph::Mono((m, bitmap))) => {
                    if !first_glyph && cursor_x + m.advance_width > limit_x {
                        break;
                    }
                    let gx0 = (cursor_x + m.xmin as f32).round() as i32;
                    let gy0 = (baseline - m.ymin as f32 - m.height as f32).round() as i32;

                    for row in 0..m.height {
                        let py = gy0 + row as i32;
                        if py < 0 || py >= buf_h {
                            continue;
                        }
                        for col in 0..m.width {
                            let px_x = gx0 + col as i32;
                            if px_x < 0 || px_x >= clip_right {
                                continue;
                            }
                            let cov = bitmap[row * m.width + col];
                            if cov == 0 {
                                continue;
                            }
                            let alpha = ((cov as u32 * color[3] as u32) / 255) as u8;
                            blend_pixel(buf, buf_w, px_x, py, [color[0], color[1], color[2], alpha]);
                        }
                    }
                    cursor_x += m.advance_width;
                }
                None => {
                    cursor_x += px * 0.55;
                }
            }
        }
    }
}

#[inline]
fn fill_span(buf: &mut [u8], buf_w: i32, y: i32, x0: i32, x1: i32, color: [u8; 4]) {
    if x1 <= x0 {
        return;
    }
    if color[3] == 255 {
        let start = ((y * buf_w + x0) * 4) as usize;
        let end = ((y * buf_w + x1) * 4) as usize;
        for chunk in buf[start..end].chunks_exact_mut(4) {
            chunk.copy_from_slice(&color);
        }
    } else {
        for px in x0..x1 {
            blend_pixel(buf, buf_w, px, y, color);
        }
    }
}

/// Fast-path rounded rectangle: bulk-fills interior scanlines, computes SDF only at the 4 corners.
pub fn fill_rounded_rect(
    buf: &mut [u8],
    buf_w: i32,
    buf_h: i32,
    rx: i32,
    ry: i32,
    rw: i32,
    rh: i32,
    radius: i32,
    color: [u8; 4],
) {
    if rw <= 0 || rh <= 0 || color[3] == 0 {
        return;
    }
    let r = radius.min(rw / 2).min(rh / 2).max(0);
    let x_start = rx.max(0);
    let y_start = ry.max(0);
    let x_end = (rx + rw).min(buf_w);
    let y_end = (ry + rh).min(buf_h);

    if r == 0 {
        for py in y_start..y_end {
            fill_span(buf, buf_w, py, x_start, x_end, color);
        }
        return;
    }

    let rf = r as f32;
    let r_inner2 = (rf - 0.5).max(0.0) * (rf - 0.5).max(0.0);
    let r_outer2 = (rf + 0.5) * (rf + 0.5);

    for py in y_start..y_end {
        let ly = py - ry;
        if ly >= r && ly < rh - r {
            fill_span(buf, buf_w, py, x_start, x_end, color);
            continue;
        }
        let dy = if ly < r { r - ly } else { ly - (rh - r - 1) };

        // Left corner
        let left_end = (rx + r).min(x_end);
        for px in x_start..left_end {
            let dx = r - (px - rx);
            let dist2 = (dx * dx + dy * dy) as f32;
            if dist2 <= r_inner2 {
                blend_pixel(buf, buf_w, px, py, color);
            } else if dist2 <= r_outer2 {
                let edge_alpha = (rf + 0.5 - dist2.sqrt()).clamp(0.0, 1.0);
                let a = (color[3] as f32 * edge_alpha) as u8;
                blend_pixel(buf, buf_w, px, py, [color[0], color[1], color[2], a]);
            }
        }

        // Middle straight span
        let mid_start = (rx + r).max(x_start);
        let mid_end = (rx + rw - r).min(x_end);
        fill_span(buf, buf_w, py, mid_start, mid_end, color);

        // Right corner
        let right_start = (rx + rw - r).max(x_start);
        for px in right_start..x_end {
            let dx = (px - rx) - (rw - r - 1);
            let dist2 = (dx * dx + dy * dy) as f32;
            if dist2 <= r_inner2 {
                blend_pixel(buf, buf_w, px, py, color);
            } else if dist2 <= r_outer2 {
                let edge_alpha = (rf + 0.5 - dist2.sqrt()).clamp(0.0, 1.0);
                let a = (color[3] as f32 * edge_alpha) as u8;
                blend_pixel(buf, buf_w, px, py, [color[0], color[1], color[2], a]);
            }
        }
    }
}

pub fn fill_circle(
    buf: &mut [u8],
    buf_w: i32,
    buf_h: i32,
    cx: i32,
    cy: i32,
    radius: i32,
    color: [u8; 4],
) {
    if radius <= 0 || color[3] == 0 {
        return;
    }
    let rf = radius as f32;
    let r2_outer = (rf + 0.6) * (rf + 0.6);
    let r2_inner = (rf - 0.6) * (rf - 0.6);
    for py in (cy - radius).max(0)..=(cy + radius).min(buf_h - 1) {
        let dy = py - cy;
        for px in (cx - radius).max(0)..=(cx + radius).min(buf_w - 1) {
            let dx = px - cx;
            let d2 = (dx * dx + dy * dy) as f32;
            if d2 <= r2_inner {
                blend_pixel(buf, buf_w, px, py, color);
            } else if d2 <= r2_outer {
                let a = (color[3] as f32 * (rf + 0.6 - d2.sqrt()).clamp(0.0, 1.0)) as u8;
                blend_pixel(buf, buf_w, px, py, [color[0], color[1], color[2], a]);
            }
        }
    }
}

pub fn stroke_rounded_rect(
    buf: &mut [u8],
    buf_w: i32,
    buf_h: i32,
    rx: i32,
    ry: i32,
    rw: i32,
    rh: i32,
    radius: i32,
    thickness: i32,
    color: [u8; 4],
) {
    if rw <= 0 || rh <= 0 || thickness <= 0 || color[3] == 0 {
        return;
    }
    let r = radius.min(rw / 2).min(rh / 2).max(0);
    let inner_r = (r - thickness).max(0);
    let x_start = rx.max(0);
    let y_start = ry.max(0);
    let x_end = (rx + rw).min(buf_w);
    let y_end = (ry + rh).min(buf_h);

    for py in y_start..y_end {
        let ly = py - ry;
        if ly >= r && ly < rh - r {
            let lx_end = (rx + thickness).min(x_end);
            fill_span(buf, buf_w, py, x_start, lx_end, color);
            let rx_start = (rx + rw - thickness).max(x_start);
            fill_span(buf, buf_w, py, rx_start, x_end, color);
            continue;
        }
        for px in x_start..x_end {
            let lx = px - rx;
            let in_border_strip =
                lx < thickness || lx >= rw - thickness || ly < thickness || ly >= rh - thickness;
            let dx = if lx < r {
                r - lx
            } else if lx >= rw - r {
                lx - (rw - r - 1)
            } else {
                0
            };
            let dy = if ly < r {
                r - ly
            } else if ly >= rh - r {
                ly - (rh - r - 1)
            } else {
                0
            };
            if dx > 0 && dy > 0 {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                if d > r as f32 + 0.5 || d < inner_r as f32 - 0.5 {
                    continue;
                }
            } else if !in_border_strip {
                continue;
            }
            blend_pixel(buf, buf_w, px, py, color);
        }
    }
}

#[inline]
pub fn blend_pixel(buf: &mut [u8], buf_w: i32, x: i32, y: i32, src: [u8; 4]) {
    if src[3] == 0 {
        return;
    }
    let idx = ((y * buf_w + x) * 4) as usize;
    if src[3] == 255 {
        buf[idx] = src[0];
        buf[idx + 1] = src[1];
        buf[idx + 2] = src[2];
        buf[idx + 3] = 255;
        return;
    }
    let sa = src[3] as u32;
    let inv = 255 - sa;
    let da = buf[idx + 3] as u32;
    let out_a = sa + (da * inv) / 255;
    if out_a == 0 {
        return;
    }
    buf[idx] = ((src[0] as u32 * sa + buf[idx] as u32 * inv) / 255) as u8;
    buf[idx + 1] = ((src[1] as u32 * sa + buf[idx + 1] as u32 * inv) / 255) as u8;
    buf[idx + 2] = ((src[2] as u32 * sa + buf[idx + 2] as u32 * inv) / 255) as u8;
    buf[idx + 3] = out_a.min(255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_and_measures_emoji_when_system_font_present() {
        let mut painter = UiPainter::new();
        let w_text = painter.measure_text("Discord", 12.0);
        let w_emoji = painter.measure_text("Discord 🔥🚀❤️", 12.0);
        assert!(w_text > 0);
        assert!(w_emoji > w_text, "emoji advance should add non-zero width");

        let mut buf = vec![0u8; 160 * 32 * 4];
        painter.draw_text(&mut buf, 160, 32, 4, 4, 152, "🔥🚀❤️", 14.0, [255, 255, 255, 255]);
        let painted_pixels = buf.chunks_exact(4).filter(|px| px[3] > 0).count();
        if painter.emoji_font.is_some() {
            assert!(painted_pixels > 20, "expected color emoji pixels in buffer");
        }
    }

    #[test]
    fn draws_vertical_bar_and_terminal_symbols_when_fonts_present() {
        let mut painter = UiPainter::new();
        let sym_sample = "︱ ★ ⚡ ❯ ~/proj";
        assert!(painter.measure_text(sym_sample, 12.5) > 20);
        // Wide single symbol inside narrow taskbar badge must still paint pixels (not drop cleanly blank)
        let mut badge_buf = vec![0u8; 22 * 22 * 4];
        painter.draw_text(&mut badge_buf, 22, 22, 2, 4, 18, "︱", 11.5, [255, 255, 255, 255]);
        let badge_pixels = badge_buf.chunks_exact(4).filter(|px| px[3] > 0).count();
        assert!(badge_pixels > 0, "expected U+FE31 vertical bar glyph to render in badge");
    }
}
