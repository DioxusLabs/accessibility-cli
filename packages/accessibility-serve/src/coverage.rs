//! Tracking which parts of the screen the accessibility tree explains.
//!
//! Two jobs. It measures how much of the display is accounted for by elements
//! the tree walk found, which is a direct health signal — a screen full of
//! content reporting 5% coverage means most of the UI is invisible to every
//! tree-based tool. And it tells the point-grid sweep where not to bother
//! probing, so discovery only pays for the unexplained regions.

use crate::ax::NormalizedRect;

/// Cells along each axis.
///
/// 32x64 over a normalized display is about 12x13 points per cell on a phone —
/// fine enough that a table row does not mark a neighbouring row as covered,
/// coarse enough to stay a cheap bitmap.
const COLUMNS: usize = 32;
const ROWS: usize = 64;

pub struct CoverageGrid {
    filled: [bool; COLUMNS * ROWS],
}

impl Default for CoverageGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl CoverageGrid {
    pub fn new() -> Self {
        Self {
            filled: [false; COLUMNS * ROWS],
        }
    }

    /// Mark every cell a normalized rect touches.
    pub fn mark(&mut self, rect: &NormalizedRect) {
        // Backdrops cover everything and would leave nothing to discover.
        if rect.is_backdrop() {
            return;
        }
        let (min_col, max_col) = span(rect.x, rect.width, COLUMNS);
        let (min_row, max_row) = span(rect.y, rect.height, ROWS);
        for row in min_row..=max_row {
            for column in min_col..=max_col {
                self.filled[row * COLUMNS + column] = true;
            }
        }
    }

    pub fn is_filled(&self, x: f64, y: f64) -> bool {
        let column = index(x, COLUMNS);
        let row = index(y, ROWS);
        self.filled[row * COLUMNS + column]
    }

    /// Fraction of the display explained so far, 0.0 to 1.0.
    pub fn ratio(&self) -> f64 {
        let filled = self.filled.iter().filter(|cell| **cell).count();
        filled as f64 / self.filled.len() as f64
    }
}

fn index(value: f64, cells: usize) -> usize {
    ((value.clamp(0.0, 1.0) * cells as f64) as usize).min(cells - 1)
}

/// Inclusive cell range covered by an interval, clamped to the display.
fn span(start: f64, length: f64, cells: usize) -> (usize, usize) {
    let low = index(start, cells);
    let high = index(start + length.max(0.0), cells);
    (low.min(high), high.max(low))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NormalizedRect {
        NormalizedRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn empty_grid_explains_nothing() {
        assert_eq!(CoverageGrid::new().ratio(), 0.0);
        assert!(!CoverageGrid::new().is_filled(0.5, 0.5));
    }

    #[test]
    fn marking_covers_the_rect_and_not_its_surroundings() {
        let mut grid = CoverageGrid::new();
        grid.mark(&rect(0.4, 0.4, 0.2, 0.2));
        assert!(grid.is_filled(0.5, 0.5));
        assert!(!grid.is_filled(0.1, 0.1));
        assert!(!grid.is_filled(0.9, 0.9));
    }

    #[test]
    fn backdrops_are_ignored() {
        // Otherwise the Application node alone would report full coverage and
        // the sweep would never probe anywhere.
        let mut grid = CoverageGrid::new();
        grid.mark(&rect(0.0, 0.0, 1.0, 1.0));
        assert_eq!(grid.ratio(), 0.0);
    }

    #[test]
    fn ratio_grows_with_marked_area() {
        let mut grid = CoverageGrid::new();
        let before = grid.ratio();
        grid.mark(&rect(0.0, 0.0, 0.5, 0.5));
        let after = grid.ratio();
        assert!(after > before);
        // A quarter of the display, within one cell of rounding on each axis.
        assert!((after - 0.25).abs() < 0.05, "ratio was {after}");
    }

    #[test]
    fn out_of_range_rects_do_not_panic() {
        let mut grid = CoverageGrid::new();
        // Rows can extend past the bottom of the screen; seen in real trees.
        grid.mark(&rect(0.9, 0.95, 0.5, 0.5));
        grid.mark(&rect(-0.2, -0.2, 0.1, 0.1));
        assert!(grid.is_filled(0.99, 0.99));
    }

    #[test]
    fn thin_rects_still_mark_a_cell() {
        let mut grid = CoverageGrid::new();
        grid.mark(&rect(0.5, 0.5, 0.0, 0.0));
        assert!(grid.is_filled(0.5, 0.5));
    }
}
