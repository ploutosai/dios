use crate::context::{ImageId, TextMetrics};
use crate::renderer::TextureType;
use crate::{Align, Bounds, Extent, ImageFlags, NonaError, Renderer};
use slab::Slab;
use std::{
    collections::HashMap,
    fmt::{Debug},
};

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

const TEX_WIDTH: usize = 1024;
const TEX_HEIGHT: usize = 1024;

pub type FontId = usize;

/// A positioned glyph specified by glyph ID (not character).
pub struct GlyphPosition {
    /// Glyph index in the font
    pub glyph_id: u16,
    /// X position in pixels
    pub x: f32,
    /// Y position in pixels
    pub y: f32,
}

/// Result of laying out glyphs by ID - ready for rendering.
pub struct LayoutGlyph {
    pub uv: Bounds,
    pub bounds: Bounds,
}

#[derive(Debug)]
pub struct LayoutChar {
    pub uv: Bounds,
    pub bounds: Bounds,
}

/// Key for caching a rasterized glyph in the atlas.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
struct GlyphCacheKey {
    font_id: FontId,
    glyph_id: u16,
    /// Font size in 1/64th of a pixel for sub-pixel precision
    size_fixed: u32,
    /// Sub-pixel X offset in 1/4 pixel increments (0..3)
    sub_x: u8,
    /// Sub-pixel Y offset in 1/4 pixel increments (0..3)
    sub_y: u8,
}

/// A cached glyph in the atlas texture.
#[derive(Debug, Clone, Copy)]
struct CachedGlyph {
    /// Position in atlas texture
    atlas_x: u32,
    atlas_y: u32,
    /// Size of the rasterized glyph
    width: u32,
    height: u32,
    /// Offset from glyph origin to top-left of bitmap
    offset_x: i32,
    offset_y: i32,
}

/// Simple shelf-based atlas packer.
struct ShelfPacker {
    width: u32,
    height: u32,
    /// Current X position in the current shelf
    cursor_x: u32,
    /// Y position of the current shelf
    shelf_y: u32,
    /// Height of the current shelf
    shelf_height: u32,
}

impl ShelfPacker {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            cursor_x: 0,
            shelf_y: 0,
            shelf_height: 0,
        }
    }

    /// Try to allocate a rectangle. Returns (x, y) if successful.
    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w == 0 || h == 0 {
            return Some((0, 0));
        }
        // Add 1px padding to avoid bleeding
        let pw = w + 1;
        let ph = h + 1;

        if self.cursor_x + pw <= self.width
            && self.shelf_y + ph.max(self.shelf_height) <= self.height
        {
            // Fits in current shelf
            let x = self.cursor_x;
            let y = self.shelf_y;
            self.cursor_x += pw;
            if ph > self.shelf_height {
                self.shelf_height = ph;
            }
            Some((x, y))
        } else if pw <= self.width && self.shelf_y + self.shelf_height + ph <= self.height {
            // Start a new shelf
            self.shelf_y += self.shelf_height;
            self.shelf_height = ph;
            self.cursor_x = pw;
            Some((0, self.shelf_y))
        } else {
            None // Atlas full
        }
    }

    fn reset(&mut self) {
        self.cursor_x = 0;
        self.shelf_y = 0;
        self.shelf_height = 0;
    }
}

struct FontData {
    data: Vec<u8>,
    fallback_fonts: Vec<FontId>,
}

impl Debug for FontData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontData({} bytes)", self.data.len())
    }
}

pub struct Fonts {
    fonts: Slab<FontData>,
    fonts_by_name: HashMap<String, FontId>,
    cache: HashMap<GlyphCacheKey, CachedGlyph>,
    packer: ShelfPacker,
    pub(crate) img: ImageId,
    /// Dirty flag - when true, we need to re-upload parts of the texture
    texture_data: Vec<u8>,
}

impl Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Fonts")
    }
}

/// Pen that collects skrifa outline commands as zeno Commands.
struct ZenoPen {
    commands: Vec<zeno::Command>,
}

impl ZenoPen {
    fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }
}

impl OutlinePen for ZenoPen {
    fn move_to(&mut self, x: f32, y: f32) {
        // Negate Y: font outlines are Y-up, zeno rasterizes Y-down
        self.commands
            .push(zeno::Command::MoveTo(zeno::Point::new(x, -y)));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.commands
            .push(zeno::Command::LineTo(zeno::Point::new(x, -y)));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.commands.push(zeno::Command::QuadTo(
            zeno::Point::new(cx0, -cy0),
            zeno::Point::new(x, -y),
        ));
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.commands.push(zeno::Command::CurveTo(
            zeno::Point::new(cx0, -cy0),
            zeno::Point::new(cx1, -cy1),
            zeno::Point::new(x, -y),
        ));
    }

    fn close(&mut self) {
        self.commands.push(zeno::Command::Close);
    }
}

impl Fonts {
    pub fn new<R: Renderer>(renderer: &mut R) -> Result<Fonts, NonaError> {
        Ok(Fonts {
            fonts: Default::default(),
            fonts_by_name: Default::default(),
            img: renderer.create_texture(
                TextureType::Alpha,
                TEX_WIDTH,
                TEX_HEIGHT,
                ImageFlags::empty(),
                None,
            )?,
            cache: HashMap::new(),
            packer: ShelfPacker::new(TEX_WIDTH as u32, TEX_HEIGHT as u32),
            texture_data: vec![0u8; TEX_WIDTH * TEX_HEIGHT],
        })
    }

    pub fn add_font<N: Into<String>, D: Into<Vec<u8>>>(
        &mut self,
        name: N,
        data: D,
    ) -> Result<FontId, NonaError> {
        let data = data.into();
        // Validate the font data by trying to parse it
        FontRef::new(&data)
            .map_err(|_| NonaError::Font(String::from("Incorrect font data format")))?;
        let fd = FontData {
            data,
            fallback_fonts: Default::default(),
        };
        let id = self.fonts.insert(fd);
        self.fonts_by_name.insert(name.into(), id);
        Ok(id)
    }

    pub fn find<N: std::borrow::Borrow<str>>(&self, name: N) -> Option<FontId> {
        self.fonts_by_name.get(name.borrow()).map(ToOwned::to_owned)
    }

    pub fn add_fallback(&mut self, base: FontId, fallback: FontId) {
        if let Some(fd) = self.fonts.get_mut(base) {
            fd.fallback_fonts.push(fallback);
        }
    }

    /// Get the advance width for a glyph by its ID.
    pub fn advance_width(&self, font_id: FontId, glyph_id: u16, size: f32) -> Option<f32> {
        let fd = self.fonts.get(font_id)?;
        let font = FontRef::new(&fd.data).ok()?;
        let glyph_metrics = font.glyph_metrics(Size::new(size), LocationRef::default());
        glyph_metrics.advance_width(GlyphId::new(glyph_id as u32))
    }

    fn font_ref(&self, id: FontId) -> Option<FontRef<'_>> {
        let fd = self.fonts.get(id)?;
        FontRef::new(&fd.data).ok()
    }

    /// Look up a character in the given font or its fallbacks.
    fn glyph_for_char(&self, id: FontId, c: char) -> Option<(FontId, GlyphId)> {
        if let Some(font) = self.font_ref(id) {
            if let Some(glyph_id) = font.charmap().map(c) {
                return Some((id, glyph_id));
            }
            // Try fallback fonts
            if let Some(fd) = self.fonts.get(id) {
                for &fallback_id in &fd.fallback_fonts {
                    if let Some(font) = self.font_ref(fallback_id) {
                        if let Some(glyph_id) = font.charmap().map(c) {
                            return Some((fallback_id, glyph_id));
                        }
                    }
                }
            }
        }
        None
    }

    /// Rasterize a glyph and cache it in the atlas. Returns the cache entry.
    fn rasterize_glyph<R: Renderer>(
        &mut self,
        renderer: &mut R,
        key: GlyphCacheKey,
    ) -> Result<Option<CachedGlyph>, NonaError> {
        if let Some(&cached) = self.cache.get(&key) {
            return Ok(Some(cached));
        }

        let fd = self
            .fonts
            .get(key.font_id)
            .ok_or_else(|| NonaError::Font(format!("Font {} not found", key.font_id)))?;

        let font = FontRef::new(&fd.data)
            .map_err(|_| NonaError::Font("Failed to parse font".to_string()))?;

        let size = key.size_fixed as f32 / 64.0;
        let outlines = font.outline_glyphs();
        let glyph_id = GlyphId::new(key.glyph_id as u32);

        let outline = match outlines.get(glyph_id) {
            Some(o) => o,
            None => return Ok(None),
        };

        // Draw the glyph outline
        let mut pen = ZenoPen::new();
        let settings = DrawSettings::unhinted(Size::new(size), LocationRef::default());
        let _ = outline.draw(settings, &mut pen);

        if pen.commands.is_empty() {
            return Ok(None);
        }

        // Rasterize with zeno
        // Apply sub-pixel offset
        let sub_x_offset = key.sub_x as f32 * 0.25;
        let sub_y_offset = key.sub_y as f32 * 0.25;

        let (alpha_data, placement) = zeno::Mask::new(&pen.commands[..])
            .offset(zeno::Vector::new(sub_x_offset, sub_y_offset))
            .render();

        if placement.width == 0 || placement.height == 0 {
            return Ok(None);
        }

        // Allocate space in atlas
        let (atlas_x, atlas_y) = match self.packer.alloc(placement.width, placement.height) {
            Some(pos) => pos,
            None => {
                // Atlas full - clear and start over
                // TODO: smarter eviction policy
                self.cache.clear();
                self.packer.reset();
                self.texture_data.fill(0);
                self.packer
                    .alloc(placement.width, placement.height)
                    .ok_or_else(|| NonaError::Font("Glyph too large for atlas".to_string()))?
            }
        };

        // Copy alpha data into texture buffer
        for row in 0..placement.height {
            for col in 0..placement.width {
                let src_idx = (row * placement.width + col) as usize;
                let dst_idx = (atlas_y + row) as usize * TEX_WIDTH + (atlas_x + col) as usize;
                if src_idx < alpha_data.len() && dst_idx < self.texture_data.len() {
                    self.texture_data[dst_idx] = alpha_data[src_idx];
                }
            }
        }

        // Upload the changed region to GPU
        renderer
            .update_texture(
                self.img.clone(),
                atlas_x as usize,
                atlas_y as usize,
                placement.width as usize,
                placement.height as usize,
                &self.sub_texture_region(atlas_x, atlas_y, placement.width, placement.height),
            )
            .map_err(|err| NonaError::Texture(format!("{:?}", err)))?;

        let cached = CachedGlyph {
            atlas_x,
            atlas_y,
            width: placement.width,
            height: placement.height,
            offset_x: placement.left,
            offset_y: placement.top,
        };

        self.cache.insert(key, cached);
        Ok(Some(cached))
    }

    /// Extract a sub-region from the texture data for uploading.
    fn sub_texture_region(&self, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
        let mut region = Vec::with_capacity((w * h) as usize);
        for row in 0..h {
            let start = (y + row) as usize * TEX_WIDTH + x as usize;
            region.extend_from_slice(&self.texture_data[start..start + w as usize]);
        }
        region
    }

    fn make_cache_key(
        &self,
        font_id: FontId,
        glyph_id: u16,
        size: f32,
        x: f32,
        y: f32,
    ) -> GlyphCacheKey {
        // Quantize sub-pixel position to 1/4 pixel
        let sub_x = ((x.fract() + 1.0).fract() * 4.0) as u8;
        let sub_y = ((y.fract() + 1.0).fract() * 4.0) as u8;
        GlyphCacheKey {
            font_id,
            glyph_id,
            size_fixed: (size * 64.0) as u32,
            sub_x,
            sub_y,
        }
    }

    pub fn text_metrics(&self, id: FontId, size: f32) -> TextMetrics {
        if let Some(font) = self.font_ref(id) {
            let metrics = font.metrics(Size::new(size), LocationRef::default());
            TextMetrics {
                ascender: metrics.ascent,
                descender: metrics.descent,
                line_gap: metrics.leading,
            }
        } else {
            TextMetrics {
                ascender: 0.0,
                descender: 0.0,
                line_gap: 0.0,
            }
        }
    }

    pub fn text_size(&self, text: &str, id: FontId, size: f32, spacing: f32) -> Extent {
        if let Some(font) = self.font_ref(id) {
            let metrics = font.metrics(Size::new(size), LocationRef::default());
            let glyph_metrics = font.glyph_metrics(Size::new(size), LocationRef::default());
            let charmap = font.charmap();

            let mut width = 0.0f32;
            let mut char_count = 0;
            let mut _last_glyph = None;

            for c in text.chars() {
                if let Some(glyph_id) = charmap.map(c) {
                    if let Some(advance) = glyph_metrics.advance_width(glyph_id) {
                        width += advance;
                    }
                    // TODO: kerning
                    _last_glyph = Some(glyph_id);
                    char_count += 1;
                }
            }

            if char_count >= 2 {
                width += spacing * (char_count - 1) as f32;
            }

            Extent::new(width, metrics.ascent - metrics.descent + metrics.leading)
        } else {
            Default::default()
        }
    }

    pub fn layout_text<R: Renderer>(
        &mut self,
        renderer: &mut R,
        text: &str,
        id: FontId,
        position: crate::Point,
        size: f32,
        align: Align,
        spacing: f32,
        cache: bool,
        result: &mut Vec<LayoutChar>,
    ) -> Result<(), NonaError> {
        result.clear();

        // Phase 1: compute glyph positions (immutable borrow of self.fonts)
        struct PendingGlyph {
            x: f32,
            y: f32,
            key: GlyphCacheKey,
        }
        let mut pending: Vec<PendingGlyph> = Vec::new();

        {
            let fd = self
                .fonts
                .get(id)
                .ok_or_else(|| NonaError::Font(format!("Font {} not found", id)))?;
            let font = FontRef::new(&fd.data)
                .map_err(|_| NonaError::Font("Failed to parse font".to_string()))?;

            let metrics = font.metrics(Size::new(size), LocationRef::default());
            let glyph_metrics = font.glyph_metrics(Size::new(size), LocationRef::default());

            let mut offset_x = 0.0f32;
            let mut offset_y = 0.0f32;

            let sz = if align.contains(Align::CENTER)
                || align.contains(Align::RIGHT)
                || align.contains(Align::MIDDLE)
            {
                self.text_size(text, id, size, spacing)
            } else {
                Extent::new(0.0, 0.0)
            };

            if align.contains(Align::CENTER) {
                offset_x -= sz.width / 2.0;
            } else if align.contains(Align::RIGHT) {
                offset_x -= sz.width;
            }

            if align.contains(Align::MIDDLE) {
                offset_y = metrics.descent + sz.height / 2.0;
            } else if align.contains(Align::BOTTOM) {
                offset_y = metrics.descent;
            } else if align.contains(Align::TOP) {
                offset_y = metrics.ascent;
            }

            let mut pos_x = position.x + offset_x;
            let pos_y = position.y + offset_y;
            let mut _last_glyph = None;

            for (_idx, c) in text.chars().enumerate() {
                if let Some((font_id, glyph_id)) = self.glyph_for_char(id, c) {
                    let gid = glyph_id.to_u32() as u16;
                    let advance = if font_id == id {
                        glyph_metrics.advance_width(glyph_id).unwrap_or(0.0)
                    } else {
                        self.advance_width(font_id, gid, size).unwrap_or(0.0)
                    };

                    let next_x = pos_x + advance;
                    let key = self.make_cache_key(font_id, gid, size, pos_x, pos_y);

                    pending.push(PendingGlyph {
                        x: pos_x,
                        y: pos_y,
                        key,
                    });

                    pos_x = next_x;
                    _last_glyph = Some(glyph_id);
                }
            }
        }

        // Phase 2: rasterize and build results (mutable borrow of self)
        if cache {
            for pg in &pending {
                if let Ok(Some(cached)) = self.rasterize_glyph(renderer, pg.key) {
                    let pixel_x = pg.x.floor() as i32 + cached.offset_x;
                    let pixel_y = pg.y.floor() as i32 + cached.offset_y;

                    let uv_min_x = cached.atlas_x as f32 / TEX_WIDTH as f32;
                    let uv_min_y = cached.atlas_y as f32 / TEX_HEIGHT as f32;
                    let uv_max_x = (cached.atlas_x + cached.width) as f32 / TEX_WIDTH as f32;
                    let uv_max_y = (cached.atlas_y + cached.height) as f32 / TEX_HEIGHT as f32;

                    result.push(LayoutChar {
                        uv: Bounds {
                            min: crate::Point {
                                x: uv_min_x,
                                y: uv_min_y,
                            },
                            max: crate::Point {
                                x: uv_max_x,
                                y: uv_max_y,
                            },
                        },
                        bounds: Bounds {
                            min: (pixel_x as f32, pixel_y as f32).into(),
                            max: (
                                (pixel_x + cached.width as i32) as f32,
                                (pixel_y + cached.height as i32) as f32,
                            )
                                .into(),
                        },
                    });
                }
            }
        } else {
            for _pg in &pending {
                result.push(LayoutChar {
                    uv: Default::default(),
                    bounds: Default::default(),
                });
            }
        }

        Ok(())
    }

    /// Layout and cache glyphs specified by glyph ID and position.
    /// This is used by external layout engines (like blitz) that already know
    /// the glyph IDs and positions.
    pub fn layout_glyphs_by_id<R: Renderer>(
        &mut self,
        renderer: &mut R,
        font_id: FontId,
        size: f32,
        glyphs: &[GlyphPosition],
        result: &mut Vec<LayoutGlyph>,
    ) -> Result<(), NonaError> {
        result.clear();

        for gp in glyphs {
            let key = self.make_cache_key(font_id, gp.glyph_id, size, gp.x, gp.y);

            if let Some(cached) = self.rasterize_glyph(renderer, key)? {
                let pixel_x = gp.x.floor() as i32 + cached.offset_x;
                let pixel_y = gp.y.floor() as i32 + cached.offset_y;

                let uv_min_x = cached.atlas_x as f32 / TEX_WIDTH as f32;
                let uv_min_y = cached.atlas_y as f32 / TEX_HEIGHT as f32;
                let uv_max_x = (cached.atlas_x + cached.width) as f32 / TEX_WIDTH as f32;
                let uv_max_y = (cached.atlas_y + cached.height) as f32 / TEX_HEIGHT as f32;

                result.push(LayoutGlyph {
                    uv: Bounds {
                        min: crate::Point {
                            x: uv_min_x,
                            y: uv_min_y,
                        },
                        max: crate::Point {
                            x: uv_max_x,
                            y: uv_max_y,
                        },
                    },
                    bounds: Bounds {
                        min: (pixel_x as f32, pixel_y as f32).into(),
                        max: (
                            (pixel_x + cached.width as i32) as f32,
                            (pixel_y + cached.height as i32) as f32,
                        )
                            .into(),
                    },
                });
            }
        }

        Ok(())
    }
}
