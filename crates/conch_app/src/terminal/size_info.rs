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

#[cfg(test)]
mod tests {
    use super::*;

    // Standard 8x16 monospace cell, 800x600 viewport.
    fn standard() -> SizeInfo {
        SizeInfo::new(800.0, 600.0, 8.0, 16.0)
    }

    #[test]
    fn new_computes_padding() {
        let s = SizeInfo::new(805.0, 610.0, 8.0, 16.0);
        // 805 % 8 = 5, padding_x = floor(5/2) = 2.
        assert_eq!(s.padding_x, 2.0);
        // 610 % 16 = 2, padding_y = floor(2/2) = 1.
        assert_eq!(s.padding_y, 1.0);
    }

    #[test]
    fn new_zero_padding_when_exact_fit() {
        let s = SizeInfo::new(800.0, 640.0, 8.0, 16.0);
        // 800 % 8 = 0, 640 % 16 = 0.
        assert_eq!(s.padding_x, 0.0);
        assert_eq!(s.padding_y, 0.0);
    }

    #[test]
    fn columns_exact() {
        let s = standard();
        // 800 / 8 = 100 columns (no padding when exact).
        assert_eq!(s.columns(), 100);
    }

    #[test]
    fn rows_exact() {
        let s = SizeInfo::new(800.0, 640.0, 8.0, 16.0);
        assert_eq!(s.rows(), 40);
    }

    #[test]
    fn columns_with_padding() {
        let s = SizeInfo::new(805.0, 600.0, 8.0, 16.0);
        // padding_x = 2, usable = 805 - 4 = 801, cols = floor(801/8) = 100.
        assert_eq!(s.columns(), 100);
    }

    #[test]
    fn cell_position_origin() {
        let s = SizeInfo::new(800.0, 640.0, 8.0, 16.0);
        // No padding, so (0,0) is at origin.
        assert_eq!(s.cell_position(0, 0), (0.0, 0.0));
    }

    #[test]
    fn cell_position_with_padding() {
        let s = SizeInfo::new(805.0, 610.0, 8.0, 16.0);
        // padding = (2.0, 1.0), cell (0,0) starts at padding.
        assert_eq!(s.cell_position(0, 0), (2.0, 1.0));
    }

    #[test]
    fn cell_position_offset() {
        let s = SizeInfo::new(800.0, 640.0, 8.0, 16.0);
        // Cell (5, 3): x = 0 + 5*8 = 40, y = 0 + 3*16 = 48.
        assert_eq!(s.cell_position(5, 3), (40.0, 48.0));
    }

    #[test]
    fn cell_position_last_cell() {
        let s = SizeInfo::new(800.0, 640.0, 8.0, 16.0);
        let cols = s.columns();
        let rows = s.rows();
        let (x, y) = s.cell_position(cols - 1, rows - 1);
        // Should be within viewport.
        assert!(x + s.cell_width <= s.width);
        assert!(y + s.cell_height <= s.height);
    }
}
