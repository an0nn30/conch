//! Terminal viewport geometry: cell dimensions, grid size, and padding.

/// Terminal size information derived from the viewport pixel dimensions
/// and the monospace font cell size.
#[derive(Debug, Clone, Copy)]
pub struct SizeInfo {
    pub width: f32,
    pub height: f32,
    pub cell_width: f32,
    pub cell_height: f32,
    /// Horizontal padding to center the grid within the viewport.
    pub padding_x: f32,
    /// Vertical padding to center the grid within the viewport.
    pub padding_y: f32,
}

impl SizeInfo {
    /// Compute size info for a viewport of `width x height` pixels.
    ///
    /// Padding is the leftover space after fitting whole cells, split evenly on both sides.
    pub fn new(width: f32, height: f32, cell_width: f32, cell_height: f32) -> Self {
        let padding_x = ((width % cell_width) / 2.0).floor();
        let padding_y = ((height % cell_height) / 2.0).floor();
        Self {
            width,
            height,
            cell_width,
            cell_height,
            padding_x,
            padding_y,
        }
    }

    /// Number of columns that fit in the viewport.
    pub fn columns(&self) -> usize {
        ((self.width - 2.0 * self.padding_x) / self.cell_width) as usize
    }

    /// Number of rows that fit in the viewport.
    pub fn rows(&self) -> usize {
        ((self.height - 2.0 * self.padding_y) / self.cell_height) as usize
    }

    /// Pixel offset of the top-left corner of cell `(col, row)` within the viewport.
    pub fn cell_position(&self, col: usize, row: usize) -> (f32, f32) {
        let x = self.padding_x + col as f32 * self.cell_width;
        let y = self.padding_y + row as f32 * self.cell_height;
        (x, y)
    }
}
