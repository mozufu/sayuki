//! Pinning: a window sticks to an output's viewport (a HUD) rather than the
//! canvas, staying in the same screen position while the canvas pans under it.
//!
//! A pinned window remains a `Space` element; each time its output's viewport
//! changes we recompute its canvas location so it renders at a fixed on-screen
//! anchor. The anchor maths is pure and unit-tested here.

use smithay::{
    desktop::Window,
    utils::{Logical, Point, Rectangle, Size},
};

/// Which output corner a pinned window anchors to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// An anchor: a corner plus an inset, in output-local logical pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ViewportAnchor {
    pub(crate) corner: Corner,
    pub(crate) offset: Point<i32, Logical>,
}

/// A window pinned to a specific output's viewport.
pub(crate) struct Pinned {
    pub(crate) window: Window,
    pub(crate) output: String,
    pub(crate) anchor: ViewportAnchor,
}

/// The canvas-space location for a pinned window so it renders at its anchor.
/// `output_geometry` is the output's rectangle in the canvas (`[viewport.loc,
/// output_size]`), so the result is independent of zoom (pinned windows draw in
/// a 1:1 overlay).
pub(crate) fn pinned_location(
    output_geometry: Rectangle<i32, Logical>,
    window_size: Size<i32, Logical>,
    anchor: &ViewportAnchor,
) -> Point<i32, Logical> {
    let left = output_geometry.loc.x;
    let top = output_geometry.loc.y;
    let right = left + output_geometry.size.w;
    let bottom = top + output_geometry.size.h;

    let x = match anchor.corner {
        Corner::TopLeft | Corner::BottomLeft => left + anchor.offset.x,
        Corner::TopRight | Corner::BottomRight => right - window_size.w - anchor.offset.x,
    };
    let y = match anchor.corner {
        Corner::TopLeft | Corner::TopRight => top + anchor.offset.y,
        Corner::BottomLeft | Corner::BottomRight => bottom - window_size.h - anchor.offset.y,
    };
    Point::from((x, y))
}

/// Capture an anchor from a window's current on-screen position: the nearest
/// corner of its output, and the inset to that corner.
pub(crate) fn capture_anchor(
    output_geometry: Rectangle<i32, Logical>,
    window: Rectangle<i32, Logical>,
) -> ViewportAnchor {
    let inset_left = window.loc.x - output_geometry.loc.x;
    let inset_top = window.loc.y - output_geometry.loc.y;
    let inset_right =
        (output_geometry.loc.x + output_geometry.size.w) - (window.loc.x + window.size.w);
    let inset_bottom =
        (output_geometry.loc.y + output_geometry.size.h) - (window.loc.y + window.size.h);

    let anchor_left = inset_left <= inset_right;
    let anchor_top = inset_top <= inset_bottom;
    let corner = match (anchor_left, anchor_top) {
        (true, true) => Corner::TopLeft,
        (false, true) => Corner::TopRight,
        (true, false) => Corner::BottomLeft,
        (false, false) => Corner::BottomRight,
    };
    let offset = match corner {
        Corner::TopLeft => Point::from((inset_left, inset_top)),
        Corner::TopRight => Point::from((inset_right, inset_top)),
        Corner::BottomLeft => Point::from((inset_left, inset_bottom)),
        Corner::BottomRight => Point::from((inset_right, inset_bottom)),
    };
    ViewportAnchor { corner, offset }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    fn point(x: i32, y: i32) -> Point<i32, Logical> {
        Point::from((x, y))
    }

    #[test]
    fn pinned_top_right_tracks_the_viewport() {
        let anchor = ViewportAnchor {
            corner: Corner::TopRight,
            offset: point(16, 16),
        };
        let window = Size::from((300, 200));

        // Output at canvas origin.
        let at_origin = pinned_location(rect(0, 0, 1920, 1080), window, &anchor);
        assert_eq!(at_origin, point(1920 - 300 - 16, 16));

        // After the canvas pans (output rect moves), the pinned location moves
        // with it, keeping the same on-screen position.
        let panned = pinned_location(rect(500, 300, 1920, 1080), window, &anchor);
        assert_eq!(panned, point(500 + 1920 - 300 - 16, 300 + 16));
        // The on-screen inset from the output's top-right is unchanged.
        assert_eq!(panned - point(500, 300), at_origin);
    }

    #[test]
    fn pinned_corners_map_to_expected_edges() {
        let output = rect(0, 0, 1000, 800);
        let window = Size::from((100, 100));
        let offset = point(10, 20);

        let cases = [
            (Corner::TopLeft, point(10, 20)),
            (Corner::TopRight, point(1000 - 100 - 10, 20)),
            (Corner::BottomLeft, point(10, 800 - 100 - 20)),
            (Corner::BottomRight, point(1000 - 100 - 10, 800 - 100 - 20)),
        ];
        for (corner, expected) in cases {
            let anchor = ViewportAnchor { corner, offset };
            assert_eq!(pinned_location(output, window, &anchor), expected);
        }
    }

    #[test]
    fn capture_anchor_picks_nearest_corner() {
        let output = rect(0, 0, 1000, 800);

        // Near the bottom-right.
        let anchor = capture_anchor(output, rect(880, 690, 100, 100));
        assert_eq!(anchor.corner, Corner::BottomRight);
        assert_eq!(anchor.offset, point(20, 10));

        // Near the top-left.
        let anchor = capture_anchor(output, rect(15, 25, 100, 100));
        assert_eq!(anchor.corner, Corner::TopLeft);
        assert_eq!(anchor.offset, point(15, 25));
    }

    #[test]
    fn capture_then_locate_round_trips() {
        let output = rect(200, 100, 1000, 800);
        let window = rect(1050, 650, 120, 90);
        let anchor = capture_anchor(output, window);
        // Recomputing the location from the captured anchor reproduces the
        // window's original position.
        let located = pinned_location(output, window.size, &anchor);
        assert_eq!(located, window.loc);
    }
}
