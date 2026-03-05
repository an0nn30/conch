/// Row-based bin-packing atlas for glyph textures.
///
/// Allocates rectangular regions in a GPU texture using a simple
/// row-based (shelf) algorithm. Each new glyph is placed in the
/// current row; if it doesn't fit, a new row is started.
pub struct Atlas {
    /// Width of the atlas texture in pixels.
    pub width: u32,
    /// Height of the atlas texture in pixels.
    pub height: u32,
    /// Current X cursor in the active row.
    cursor_x: u32,
    /// Current Y cursor (top of active row).
    cursor_y: u32,
    /// Height of the tallest glyph in the active row.
    row_height: u32,
}

/// A rectangular region allocated in the atlas.
#[derive(Debug, Clone, Copy)]
pub struct AtlasRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl AtlasRegion {
    /// Normalized UV coordinates (0..1).
    pub fn uv(&self, atlas_width: u32, atlas_height: u32) -> (f32, f32, f32, f32) {
        let x = self.x as f32 / atlas_width as f32;
        let y = self.y as f32 / atlas_height as f32;
        let w = self.width as f32 / atlas_width as f32;
        let h = self.height as f32 / atlas_height as f32;
        (x, y, w, h)
    }
}

impl Atlas {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
        }
    }

    /// Try to allocate a region. Returns None if the atlas is full.
    pub fn allocate(&mut self, glyph_width: u32, glyph_height: u32) -> Option<AtlasRegion> {
        if glyph_width == 0 || glyph_height == 0 {
            return Some(AtlasRegion {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            });
        }

        // Check if glyph fits in current row
        if self.cursor_x + glyph_width > self.width {
            // Move to next row
            self.cursor_y += self.row_height;
            self.cursor_x = 0;
            self.row_height = 0;
        }

        // Check if there's vertical space
        if self.cursor_y + glyph_height > self.height {
            return None; // Atlas full
        }

        let region = AtlasRegion {
            x: self.cursor_x,
            y: self.cursor_y,
            width: glyph_width,
            height: glyph_height,
        };

        self.cursor_x += glyph_width;
        self.row_height = self.row_height.max(glyph_height);

        Some(region)
    }

    /// Reset the atlas (e.g., after recreating the texture).
    pub fn reset(&mut self) {
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.row_height = 0;
    }
}
