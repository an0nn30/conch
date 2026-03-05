use std::collections::HashMap;

/// Key for a cached glyph: character + style flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub c: char,
    pub bold: bool,
    pub italic: bool,
}

/// Cached glyph with its atlas coordinates.
#[derive(Debug, Clone, Copy)]
pub struct CachedGlyph {
    /// UV coordinates in the atlas texture (normalized 0..1).
    pub uv_x: f32,
    pub uv_y: f32,
    pub uv_w: f32,
    pub uv_h: f32,
    /// Pixel dimensions.
    pub width: u32,
    pub height: u32,
    /// Offset from cell origin.
    pub left: i32,
    pub top: i32,
    /// Whether this is a color glyph (emoji).
    pub is_color: bool,
}

/// Cache mapping GlyphKey → CachedGlyph (atlas coordinates).
pub struct GlyphCache {
    entries: HashMap<GlyphKey, Option<CachedGlyph>>,
}

impl GlyphCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn get(&self, key: &GlyphKey) -> Option<&Option<CachedGlyph>> {
        self.entries.get(key)
    }

    pub fn insert(&mut self, key: GlyphKey, glyph: Option<CachedGlyph>) {
        self.entries.insert(key, glyph);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
