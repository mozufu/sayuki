//! Column tiling: first-class tiling that lives alongside the floating canvas
//! model.
//!
//! Tiled windows are arranged into vertical **columns** laid left-to-right
//! inside a tiling *area* (an output's work region on the canvas). Each column
//! is a top-to-bottom stack; columns share the area width equally and a
//! column's windows share its height equally, separated by a uniform gap. This
//! is the niri-style *column* policy shape adapted to Sayuki's fit-to-area
//! sizing — variable column widths and horizontal scrolling are a documented
//! future enhancement.
//!
//! Floating remains the default: a canvas is tiled only when opted in (project
//! `layout = "tiling"`) or per window rule, and individual windows can still be
//! floated on a tiled canvas. The column structure and navigation are generic
//! over the window identity `T` so the column algebra is unit-testable without a
//! real Smithay `Window`.

use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::swap::Direction;

/// Tiling sizing parameters, configured under `[tiling]`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TilingConfig {
    /// Uniform gap in logical pixels between tiles and around the area edges.
    /// `0` packs tiles flush.
    pub gap: i32,
}

impl Default for TilingConfig {
    fn default() -> Self {
        Self { gap: 8 }
    }
}

/// A canvas's layout mode. Floating is the default; tiling is opt-in per
/// workspace (project `layout = "tiling"`) or per window rule.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LayoutMode {
    #[default]
    Floating,
    Tiling,
}

impl LayoutMode {
    /// The other mode — used by the `ToggleTiling` action.
    pub fn toggled(self) -> Self {
        match self {
            LayoutMode::Floating => LayoutMode::Tiling,
            LayoutMode::Tiling => LayoutMode::Floating,
        }
    }
}

/// The column structure of a canvas's tiled windows. Generic over the window
/// identity `T` for testability; a canvas instantiates it with the Smithay
/// `Window`.
#[derive(Clone, Debug, PartialEq)]
pub struct TilingLayout<T> {
    /// Columns left-to-right; each column holds its windows top-to-bottom. A
    /// column is never empty (emptied columns are dropped).
    columns: Vec<Vec<T>>,
    /// The active `(column, row)`; meaningless (and `(0, 0)`) when empty.
    active: (usize, usize),
}

impl<T> Default for TilingLayout<T> {
    fn default() -> Self {
        Self {
            columns: Vec::new(),
            active: (0, 0),
        }
    }
}

impl<T: Clone + PartialEq> TilingLayout<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// Total tiled window count.
    pub fn len(&self) -> usize {
        self.columns.iter().map(Vec::len).sum()
    }

    pub fn contains(&self, window: &T) -> bool {
        self.columns.iter().flatten().any(|w| w == window)
    }

    /// Every tiled window in column-major order (left-to-right, top-to-bottom).
    pub fn windows(&self) -> impl Iterator<Item = &T> {
        self.columns.iter().flatten()
    }

    /// The columns, for inspection (overview/minimap) and tests.
    pub fn columns(&self) -> &[Vec<T>] {
        &self.columns
    }

    /// The active tile, if any.
    pub fn active(&self) -> Option<&T> {
        let (column, row) = self.active;
        self.columns.get(column).and_then(|col| col.get(row))
    }

    /// Locate a window's `(column, row)`.
    fn position(&self, window: &T) -> Option<(usize, usize)> {
        self.columns.iter().enumerate().find_map(|(column, col)| {
            col.iter()
                .position(|w| w == window)
                .map(|row| (column, row))
        })
    }

    /// Insert `window` as a new column immediately to the right of the active
    /// column (niri opens a new window in its own column next to the focus) and
    /// make it active. A no-op if already tiled.
    pub fn insert(&mut self, window: T) {
        if self.contains(&window) {
            return;
        }
        let index = if self.columns.is_empty() {
            0
        } else {
            (self.active.0 + 1).min(self.columns.len())
        };
        self.columns.insert(index, vec![window]);
        self.active = (index, 0);
    }

    /// Remove `window`; drop its column if it empties, then keep the active
    /// position valid. Returns whether the window was present.
    pub fn remove(&mut self, window: &T) -> bool {
        let Some((column, row)) = self.position(window) else {
            return false;
        };
        self.columns[column].remove(row);
        if self.columns[column].is_empty() {
            self.columns.remove(column);
        }
        self.clamp_active();
        true
    }

    /// Make `window` the active tile if it is tiled. Returns whether it was
    /// found (so callers can ignore non-tiled windows).
    pub fn focus(&mut self, window: &T) -> bool {
        match self.position(window) {
            Some(position) => {
                self.active = position;
                true
            }
            None => false,
        }
    }

    fn clamp_active(&mut self) {
        if self.columns.is_empty() {
            self.active = (0, 0);
            return;
        }
        let column = self.active.0.min(self.columns.len() - 1);
        let row = self.active.1.min(self.columns[column].len() - 1);
        self.active = (column, row);
    }

    /// Move focus in `direction`, returning the newly active window. Left/Right
    /// move between columns (clamping the row into the destination column);
    /// Up/Down move within the active column. Movement does not wrap and is a
    /// no-op at an edge.
    pub fn focus_direction(&mut self, direction: Direction) -> Option<&T> {
        if self.columns.is_empty() {
            return None;
        }
        let (column, row) = self.active;
        match direction {
            Direction::Left => {
                if column > 0 {
                    let row = row.min(self.columns[column - 1].len() - 1);
                    self.active = (column - 1, row);
                }
            }
            Direction::Right => {
                if column + 1 < self.columns.len() {
                    let row = row.min(self.columns[column + 1].len() - 1);
                    self.active = (column + 1, row);
                }
            }
            Direction::Up => {
                if row > 0 {
                    self.active = (column, row - 1);
                }
            }
            Direction::Down => {
                if row + 1 < self.columns[column].len() {
                    self.active = (column, row + 1);
                }
            }
        }
        self.active()
    }

    /// Move the active window in `direction`, returning whether anything moved.
    /// Up/Down reorder it within its column; Left/Right relocate it to the
    /// adjacent column, splitting off a new edge column when the active window
    /// is at the outer column but not alone there. A lone window already at the
    /// outer column has nowhere to go (no-op).
    pub fn move_direction(&mut self, direction: Direction) -> bool {
        if self.columns.is_empty() {
            return false;
        }
        let (column, row) = self.active;
        match direction {
            Direction::Up => {
                if row == 0 {
                    return false;
                }
                self.columns[column].swap(row, row - 1);
                self.active = (column, row - 1);
                true
            }
            Direction::Down => {
                if row + 1 >= self.columns[column].len() {
                    return false;
                }
                self.columns[column].swap(row, row + 1);
                self.active = (column, row + 1);
                true
            }
            Direction::Left => self.move_horizontal(true),
            Direction::Right => self.move_horizontal(false),
        }
    }

    /// Relocate the active window one column toward `left` (or right when
    /// `false`). See [`Self::move_direction`] for the edge semantics.
    fn move_horizontal(&mut self, left: bool) -> bool {
        let (column, row) = self.active;
        let columns = self.columns.len();
        let alone = self.columns[column].len() == 1;
        let at_left_edge = column == 0;
        let at_right_edge = column + 1 == columns;
        // A lone window already at the outer column has no neighbour to join and
        // splitting would just recreate the same column.
        if alone && ((left && at_left_edge) || (!left && at_right_edge)) {
            return false;
        }

        let window = self.columns[column].remove(row);
        let source_emptied = self.columns[column].is_empty();
        if source_emptied {
            self.columns.remove(column);
        }

        if left {
            if at_left_edge {
                // Not alone (guarded above): split off a new leftmost column.
                self.columns.insert(0, vec![window]);
                self.active = (0, 0);
            } else {
                // Columns at indices < `column` are unaffected by the removal.
                let target = column - 1;
                self.columns[target].push(window);
                self.active = (target, self.columns[target].len() - 1);
            }
        } else if at_right_edge {
            // Not alone (guarded above): split off a new rightmost column.
            self.columns.push(vec![window]);
            self.active = (self.columns.len() - 1, 0);
        } else {
            // The neighbour was original index `column + 1`; removing an emptied
            // source at `column` shifts it left to `column`.
            let target = if source_emptied { column } else { column + 1 };
            self.columns[target].push(window);
            self.active = (target, self.columns[target].len() - 1);
        }
        true
    }

    /// Compute each tiled window's rectangle inside `area`: columns share the
    /// width equally and each column's windows share its height equally, with a
    /// uniform `gap` between tiles and around the edges. Integer-division
    /// leftovers are spread one pixel at a time across the leading tiles so the
    /// tiles plus gaps fill `area` exactly. Returns column-major order; an empty
    /// layout or a degenerate area yields no rectangles.
    pub fn geometry(
        &self,
        area: Rectangle<i32, Logical>,
        gap: i32,
    ) -> Vec<(T, Rectangle<i32, Logical>)> {
        if self.columns.is_empty() || area.size.w <= 0 || area.size.h <= 0 {
            return Vec::new();
        }
        let gap = gap.max(0);
        let column_spans = split(area.size.w, self.columns.len(), gap);
        let mut tiles = Vec::with_capacity(self.len());
        for (column, windows) in self.columns.iter().enumerate() {
            let (x, width) = column_spans[column];
            let row_spans = split(area.size.h, windows.len(), gap);
            for (row, window) in windows.iter().enumerate() {
                let (y, height) = row_spans[row];
                let location = area.loc + Point::from((x, y));
                let size = Size::from((width.max(1), height.max(1)));
                tiles.push((window.clone(), Rectangle::new(location, size)));
            }
        }
        tiles
    }
}

/// Split `total` pixels into `count` segments separated and bordered by `gap`,
/// returning each segment's `(offset_from_start, length)`. The integer-division
/// remainder is spread one pixel at a time across the leading segments so the
/// segments plus gaps exactly fill `total`. `count` is always `>= 1` here.
fn split(total: i32, count: usize, gap: i32) -> Vec<(i32, i32)> {
    let count_i = count as i32;
    let inner = (total - gap * (count_i + 1)).max(0);
    let base = inner / count_i;
    let extra = inner % count_i;
    let mut spans = Vec::with_capacity(count);
    let mut offset = gap;
    for index in 0..count {
        let length = base + i32::from((index as i32) < extra);
        spans.push((offset, length));
        offset += length + gap;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(tiling: &TilingLayout<i32>) -> Vec<Vec<i32>> {
        tiling.columns().to_vec()
    }

    #[test]
    fn insert_opens_new_column_right_of_active() {
        let mut tiling = TilingLayout::new();
        tiling.insert(1);
        tiling.insert(2);
        tiling.insert(3);
        // Each new window opens its own column to the right of the focus.
        assert_eq!(columns(&tiling), vec![vec![1], vec![2], vec![3]]);
        assert_eq!(tiling.active(), Some(&3));
        assert_eq!(tiling.len(), 3);
    }

    #[test]
    fn insert_is_idempotent() {
        let mut tiling = TilingLayout::new();
        tiling.insert(1);
        tiling.insert(1);
        assert_eq!(tiling.len(), 1);
    }

    #[test]
    fn move_down_then_keeps_stack_order() {
        let mut tiling = TilingLayout::new();
        tiling.insert(1);
        tiling.insert(2);
        // Move window 2 left so it joins window 1's column (stacked below).
        assert!(tiling.move_direction(Direction::Left));
        assert_eq!(columns(&tiling), vec![vec![1, 2]]);
        assert_eq!(tiling.active(), Some(&2));
    }

    #[test]
    fn focus_navigates_columns_and_rows_without_wrapping() {
        let mut tiling = TilingLayout::new();
        tiling.insert(1);
        tiling.insert(2);
        tiling.move_direction(Direction::Left); // column 0 = [1, 2]
        tiling.insert(3); // column 1 = [3], active = 3
        assert_eq!(columns(&tiling), vec![vec![1, 2], vec![3]]);

        // From column 1 row 0, Left lands in column 0 row 0 (row clamped).
        assert_eq!(tiling.focus_direction(Direction::Left), Some(&1));
        assert_eq!(tiling.focus_direction(Direction::Down), Some(&2));
        // Down at the bottom of the column is a no-op.
        assert_eq!(tiling.focus_direction(Direction::Down), Some(&2));
        // Right from row 1 clamps into the single-row neighbour column.
        assert_eq!(tiling.focus_direction(Direction::Right), Some(&3));
        // Left edge: Left from column 0 stays put.
        tiling.focus_direction(Direction::Left);
        assert_eq!(tiling.focus_direction(Direction::Left), Some(&1));
    }

    #[test]
    fn move_reorders_within_column() {
        let mut tiling = TilingLayout::new();
        tiling.insert(1);
        tiling.insert(2);
        tiling.move_direction(Direction::Left); // [1, 2], active = 2 (row 1)
        assert!(tiling.move_direction(Direction::Up)); // swap to [2, 1]
        assert_eq!(columns(&tiling), vec![vec![2, 1]]);
        assert_eq!(tiling.active(), Some(&2));
        // Up at the top is a no-op.
        assert!(!tiling.move_direction(Direction::Up));
    }

    #[test]
    fn move_splits_and_merges_columns() {
        let mut tiling = TilingLayout::new();
        tiling.insert(1);
        tiling.insert(2);
        tiling.move_direction(Direction::Left); // [[1, 2]], active = 2
        // Right from the only (and rightmost) column splits 2 into a new column.
        assert!(tiling.move_direction(Direction::Right));
        assert_eq!(columns(&tiling), vec![vec![1], vec![2]]);
        assert_eq!(tiling.active(), Some(&2));
        // A lone window at the right edge cannot move further right.
        assert!(!tiling.move_direction(Direction::Right));
        // Moving it left merges it back into column 0.
        assert!(tiling.move_direction(Direction::Left));
        assert_eq!(columns(&tiling), vec![vec![1, 2]]);
    }

    #[test]
    fn remove_drops_empty_columns_and_clamps_active() {
        let mut tiling = TilingLayout::new();
        tiling.insert(1);
        tiling.insert(2);
        tiling.insert(3); // [[1], [2], [3]], active = (2, 0)
        assert!(tiling.remove(&3));
        // Active clamps back onto the last remaining column.
        assert_eq!(columns(&tiling), vec![vec![1], vec![2]]);
        assert_eq!(tiling.active(), Some(&2));
        assert!(!tiling.remove(&99));
    }

    #[test]
    fn geometry_fills_area_exactly_with_gaps() {
        let mut tiling = TilingLayout::new();
        tiling.insert(1);
        tiling.insert(2);
        tiling.move_direction(Direction::Left); // column 0 = [1, 2]
        tiling.insert(3); // column 1 = [3]

        let area = Rectangle::new(Point::from((100, 50)), Size::from((420, 300)));
        let gap = 10;
        let tiles = tiling.geometry(area, gap);

        // Two columns share width 420 with three 10px gaps: inner = 420 - 10*3 =
        // 390, so each column is 195 wide.
        let by_window = |id: i32| {
            tiles
                .iter()
                .find(|(w, _)| *w == id)
                .map(|(_, r)| *r)
                .expect("tiled window")
        };
        let w1 = by_window(1);
        let w2 = by_window(2);
        let w3 = by_window(3);

        // Column 0 starts at x = loc.x + gap.
        assert_eq!(w1.loc, Point::from((110, 60)));
        assert_eq!(w1.size.w, 195);
        // Window 2 is stacked below window 1 in the same column (same x/width).
        assert_eq!(w2.loc.x, 110);
        assert_eq!(w2.size.w, 195);
        assert!(w2.loc.y > w1.loc.y);
        // Column 1 sits to the right: x = loc.x + gap + width + gap.
        assert_eq!(w3.loc.x, 110 + 195 + 10);
        assert_eq!(w3.size.w, 195);
        // Right edge of the last column lands exactly one gap from the area edge.
        assert_eq!(w3.loc.x + w3.size.w + gap, area.loc.x + area.size.w);
        // Single-window column spans the full inner height.
        assert_eq!(w3.size.h, area.size.h - 2 * gap);
    }

    #[test]
    fn geometry_distributes_rounding_remainder() {
        // 3 columns, no gap, width 100 -> 34 + 33 + 33 = 100 (remainder to the
        // leading column).
        let mut tiling = TilingLayout::new();
        tiling.insert(1);
        tiling.insert(2);
        tiling.insert(3);
        let area = Rectangle::new(Point::from((0, 0)), Size::from((100, 90)));
        let tiles = tiling.geometry(area, 0);
        let widths: Vec<i32> = tiles.iter().map(|(_, r)| r.size.w).collect();
        assert_eq!(widths, vec![34, 33, 33]);
        let total: i32 = widths.iter().sum();
        assert_eq!(total, area.size.w);
    }

    #[test]
    fn geometry_is_empty_for_degenerate_area() {
        // Smithay forbids negative `Size`, so zero is the only degenerate area
        // a real viewport can produce; the guard rejects it on either axis.
        let mut tiling = TilingLayout::new();
        tiling.insert(1);
        let zero_w = Rectangle::new(Point::from((0, 0)), Size::from((0, 100)));
        let zero_h = Rectangle::new(Point::from((0, 0)), Size::from((100, 0)));
        assert!(tiling.geometry(zero_w, 8).is_empty());
        assert!(tiling.geometry(zero_h, 8).is_empty());
    }
}
