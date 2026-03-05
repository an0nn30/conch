use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent,
};

/// A rasterized glyph image.
#[derive(Debug, Clone)]
pub struct RasterizedGlyph {
    pub width: u32,
    pub height: u32,
    /// Offset from the cell origin to place the glyph.
    pub left: i32,
    pub top: i32,
    /// RGBA pixel data.
    pub data: Vec<u8>,
    /// True if this is a color (emoji) glyph.
    pub is_color: bool,
}

/// Font system wrapper for terminal text rasterization.
pub struct FontContext {
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub metrics: Metrics,
}

impl FontContext {
    pub fn new(_font_name: &str, font_size: f32) -> Self {
        let font_system = FontSystem::new();
        let metrics = Metrics::new(font_size, font_size * 1.2);

        Self {
            font_system,
            swash_cache: SwashCache::new(),
            metrics,
        }
    }

    /// Measure the cell dimensions for the current font.
    pub fn cell_size(&mut self) -> (f32, f32) {
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        buffer.set_size(&mut self.font_system, Some(1000.0), Some(self.metrics.line_height));
        let attrs = Attrs::new().family(Family::Monospace);
        buffer.set_text(
            &mut self.font_system,
            "M",
            &attrs,
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        let cell_width = if let Some(line) = buffer.layout_runs().next() {
            line.glyphs.first().map(|g| g.w).unwrap_or(font_size_to_cell_width(self.metrics.font_size))
        } else {
            font_size_to_cell_width(self.metrics.font_size)
        };

        let cell_height = self.metrics.line_height;
        (cell_width, cell_height)
    }

    /// Rasterize a single character at the given font settings.
    pub fn rasterize_char(&mut self, c: char, bold: bool, italic: bool) -> Option<RasterizedGlyph> {
        let mut attrs = Attrs::new().family(Family::Monospace);
        if bold {
            attrs = attrs.weight(cosmic_text::Weight::BOLD);
        }
        if italic {
            attrs = attrs.style(cosmic_text::Style::Italic);
        }

        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        buffer.set_size(
            &mut self.font_system,
            Some(self.metrics.font_size * 2.0),
            Some(self.metrics.line_height),
        );
        buffer.set_text(
            &mut self.font_system,
            &c.to_string(),
            &attrs,
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        let run = buffer.layout_runs().next()?;
        let glyph = run.glyphs.first()?;

        let physical = glyph.physical((0.0, 0.0), 1.0);

        let image_ref = self.swash_cache.get_image(&mut self.font_system, physical.cache_key);
        let image = image_ref.as_ref()?;

        let is_color = matches!(image.content, SwashContent::Color);

        let data = match image.content {
            SwashContent::Mask => {
                let mut rgba = Vec::with_capacity(image.data.len() * 4);
                for &a in &image.data {
                    rgba.extend_from_slice(&[255, 255, 255, a]);
                }
                rgba
            }
            SwashContent::Color => {
                image.data.clone()
            }
            SwashContent::SubpixelMask => {
                let mut rgba = Vec::with_capacity(image.data.len() / 3 * 4);
                for chunk in image.data.chunks(3) {
                    let a = ((chunk[0] as u16 + chunk[1] as u16 + chunk[2] as u16) / 3) as u8;
                    rgba.extend_from_slice(&[255, 255, 255, a]);
                }
                rgba
            }
        };

        Some(RasterizedGlyph {
            width: image.placement.width,
            height: image.placement.height,
            left: image.placement.left,
            top: image.placement.top,
            data,
            is_color,
        })
    }
}

fn font_size_to_cell_width(size: f32) -> f32 {
    size * 0.6
}
