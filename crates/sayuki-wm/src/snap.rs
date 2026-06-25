//! Snap-on-drag: adjust only the drop point of an interactive move. Storage
//! stays free `(x, y)` coordinates — this just nudges the target location when
//! a dragged edge lands within [`SnapConfig::threshold`] of an attractor.

use smithay::utils::{Logical, Point, Rectangle};

/// Snap behaviour, configured under `[snap]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapConfig {
    /// Magnetization distance in logical px; `0` disables edge/window snapping.
    pub threshold: i32,
    /// Soft grid pitch in logical px; `0` disables grid snapping.
    pub grid: i32,
    /// Snap to other windows' edges.
    pub to_windows: bool,
    /// Snap to viewport/output edges.
    pub to_edges: bool,
}

impl Default for SnapConfig {
    fn default() -> Self {
        Self {
            threshold: 16,
            grid: 0,
            to_windows: true,
            to_edges: true,
        }
    }
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

/// Compute the snapped top-left for `dragged` given `window_edges` (other
/// windows) and `viewport_edges` (output rectangles). The dragged size is
/// preserved; only the location moves.
pub fn snap_location(
    dragged: Rectangle<i32, Logical>,
    window_edges: &[Rectangle<i32, Logical>],
    viewport_edges: &[Rectangle<i32, Logical>],
    config: &SnapConfig,
) -> Point<i32, Logical> {
    let mut location = dragged.loc;

    if config.threshold > 0 {
        let mut attractors: Vec<Rectangle<i32, Logical>> = Vec::new();
        if config.to_windows {
            attractors.extend_from_slice(window_edges);
        }
        if config.to_edges {
            attractors.extend_from_slice(viewport_edges);
        }

        if !attractors.is_empty() {
            location.x = snap_axis(
                location.x,
                dragged.size.w,
                &attractors,
                Axis::X,
                config.threshold,
            );
            location.y = snap_axis(
                location.y,
                dragged.size.h,
                &attractors,
                Axis::Y,
                config.threshold,
            );
        }
    }

    if config.grid > 0 {
        location.x = snap_to_grid(
            location.x,
            config.grid,
            config.threshold.max(config.grid / 2),
        );
        location.y = snap_to_grid(
            location.y,
            config.grid,
            config.threshold.max(config.grid / 2),
        );
    }

    location
}

fn snap_axis(
    start: i32,
    len: i32,
    attractors: &[Rectangle<i32, Logical>],
    axis: Axis,
    threshold: i32,
) -> i32 {
    let mut best = start;
    let mut best_distance = threshold + 1;

    for attractor in attractors {
        let (attractor_start, attractor_len) = match axis {
            Axis::X => (attractor.loc.x, attractor.size.w),
            Axis::Y => (attractor.loc.y, attractor.size.h),
        };
        let attractor_end = attractor_start + attractor_len;

        // Candidate starts that flush an edge of the dragged window to an edge
        // of the attractor: left-left, left-right, right-left, right-right.
        for candidate in [
            attractor_start,
            attractor_end,
            attractor_start - len,
            attractor_end - len,
        ] {
            let distance = (candidate - start).abs();
            if distance < best_distance {
                best_distance = distance;
                best = candidate;
            }
        }
    }

    best
}

fn snap_to_grid(value: i32, grid: i32, threshold: i32) -> i32 {
    let rounded = ((f64::from(value) / f64::from(grid)).round() as i32) * grid;
    if (rounded - value).abs() <= threshold {
        rounded
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::Size;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    fn point(x: i32, y: i32) -> Point<i32, Logical> {
        Point::from((x, y))
    }

    #[test]
    fn snaps_flush_to_a_nearby_window_edge() {
        let config = SnapConfig::default();
        let neighbor = rect(0, 0, 200, 200);
        // Dragged window's left edge is 6px past the neighbor's right edge.
        let dragged = rect(206, 0, 100, 100);
        let snapped = snap_location(dragged, &[neighbor], &[], &config);
        assert_eq!(snapped, point(200, 0));
    }

    #[test]
    fn snaps_left_edges_to_align() {
        let config = SnapConfig::default();
        let neighbor = rect(300, 0, 200, 200);
        // Dragged left edge 5px off the neighbor's left edge.
        let dragged = rect(305, 400, 100, 100);
        let snapped = snap_location(dragged, &[neighbor], &[], &config);
        assert_eq!(snapped.x, 300);
    }

    #[test]
    fn ignores_attractors_beyond_threshold() {
        let config = SnapConfig::default();
        let neighbor = rect(0, 0, 200, 200);
        // 40px gap exceeds the 16px threshold.
        let dragged = rect(240, 0, 100, 100);
        let snapped = snap_location(dragged, &[neighbor], &[], &config);
        assert_eq!(snapped, point(240, 0));
    }

    #[test]
    fn respects_disable_flags() {
        let config = SnapConfig {
            to_windows: false,
            ..SnapConfig::default()
        };
        let neighbor = rect(0, 0, 200, 200);
        let dragged = rect(206, 6, 100, 100);
        // Window snapping is off and no viewport edges supplied: unchanged.
        assert_eq!(
            snap_location(dragged, &[neighbor], &[], &config),
            point(206, 6)
        );
    }

    #[test]
    fn snaps_to_grid_when_enabled() {
        let config = SnapConfig {
            threshold: 0,
            grid: 50,
            ..SnapConfig::default()
        };
        // Grid snapping is independent of the edge threshold.
        let dragged = rect(48, 103, 100, 100);
        let snapped = snap_location(dragged, &[], &[], &config);
        assert_eq!(snapped, point(50, 100));
    }

    #[test]
    fn snap_then_one_px_nudge_is_honored() {
        // After snapping flush, a later free move is not re-snapped away: a 1px
        // nudge past the threshold survives (storage stays free coords).
        let config = SnapConfig::default();
        let neighbor = rect(0, 0, 200, 200);
        let nudged = rect(217, 0, 100, 100); // 17px from edge > 16 threshold
        assert_eq!(
            snap_location(nudged, &[neighbor], &[], &config),
            point(217, 0)
        );
    }
}
