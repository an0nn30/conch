/// Terminal size information: cell dimensions, padding, viewport.
#[derive(Debug, Clone, Copy)]
pub struct SizeInfo {
    /// Total pixel width of the terminal viewport.
    pub width: f32,
    /// Total pixel height of the terminal viewport.
    pub height: f32,
    /// Width of a single cell in pixels.
    pub cell_width: f32,
    /// Height of a single cell in pixels.
    pub cell_height: f32,
    /// Horizontal padding in pixels.
    pub padding_x: f32,
    /// Vertical padding in pixels.
    pub padding_y: f32,
}

impl SizeInfo {
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

    /// Number of columns that fit.
    pub fn columns(&self) -> usize {
        ((self.width - 2.0 * self.padding_x) / self.cell_width) as usize
    }

    /// Number of rows (lines) that fit.
    pub fn rows(&self) -> usize {
        ((self.height - 2.0 * self.padding_y) / self.cell_height) as usize
    }

    /// Pixel position of a cell at (col, row).
    pub fn cell_position(&self, col: usize, row: usize) -> (f32, f32) {
        let x = self.padding_x + col as f32 * self.cell_width;
        let y = self.padding_y + row as f32 * self.cell_height;
        (x, y)
    }
}
