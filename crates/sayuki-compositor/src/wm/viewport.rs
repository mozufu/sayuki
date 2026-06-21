//! Viewport geometry: the camera an output points at the shared canvas.
//!
//! A [`Viewport`] is the canvas coordinate shown at an output's top-left plus a
//! zoom factor. All maths here is pure so the coordinate transforms, clamps,
//! reveal-on-focus pan and placement can be unit-tested.

use smithay::utils::{Logical, Point, Rectangle, Size};

/// Pan is clamped to this many logical pixels from the origin on each axis so
/// snap/centering arithmetic stays well clear of `i32` overflow.
pub(crate) const PAN_LIMIT: i32 = 1_000_000;

/// Zoom bounds. `1.0` is native; below zooms out (more canvas visible), above
/// zooms in.
pub(crate) const MIN_ZOOM: f64 = 0.1;
pub(crate) const MAX_ZOOM: f64 = 10.0;

/// Logical-pixel stagger applied to freshly placed windows, cycling through
/// [`WINDOW_STAGGER_STEPS`] positions.
pub(crate) const WINDOW_STAGGER: i32 = 32;
pub(crate) const WINDOW_STAGGER_STEPS: i32 = 10;

/// An output's camera onto the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Viewport {
    /// Canvas coordinate rendered at the output's top-left corner at zoom 1.
    pub(crate) loc: Point<i32, Logical>,
    /// `1.0` = native; `<1` zoom out, `>1` zoom in.
    pub(crate) zoom: f64,
}

impl Viewport {
    /// A native (zoom 1) viewport anchored at `loc`.
    pub(crate) const fn new(loc: Point<i32, Logical>) -> Self {
        Self { loc, zoom: 1.0 }
    }
}

/// Clamp a pan location to [`PAN_LIMIT`] on each axis.
pub(crate) fn clamp_pan(loc: Point<i32, Logical>) -> Point<i32, Logical> {
    Point::from((
        loc.x.clamp(-PAN_LIMIT, PAN_LIMIT),
        loc.y.clamp(-PAN_LIMIT, PAN_LIMIT),
    ))
}

/// Clamp a zoom factor to [`MIN_ZOOM`, `MAX_ZOOM`].
pub(crate) fn clamp_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    }
}

/// The canvas region (in logical pixels) visible through `output_size` at
/// `zoom`. Zooming in shrinks the region; zooming out enlarges it.
pub(crate) fn visible_size(output_size: Size<i32, Logical>, zoom: f64) -> Size<i32, Logical> {
    let zoom = clamp_zoom(zoom);
    let w = (f64::from(output_size.w.max(1)) / zoom).round() as i32;
    let h = (f64::from(output_size.h.max(1)) / zoom).round() as i32;
    Size::from((w.max(1), h.max(1)))
}

/// The canvas rectangle visible through `viewport` for an output of
/// `output_size`. This is exactly the region to feed
/// `Space::render_elements_for_region`.
pub(crate) fn visible_region(
    viewport: &Viewport,
    output_size: Size<i32, Logical>,
) -> Rectangle<i32, Logical> {
    Rectangle::new(viewport.loc, visible_size(output_size, viewport.zoom))
}

/// Map an output-local pointer position to canvas coordinates, inverting the
/// zoom: `canvas = loc + output_local / zoom`.
pub(crate) fn to_canvas(
    viewport: &Viewport,
    output_local: Point<f64, Logical>,
) -> Point<f64, Logical> {
    let zoom = clamp_zoom(viewport.zoom);
    viewport.loc.to_f64() + Point::from((output_local.x / zoom, output_local.y / zoom))
}

/// Zoom by `factor` about the output center, keeping the canvas point under the
/// center fixed. The result is pan- and zoom-clamped.
pub(crate) fn zoom_about_center(
    viewport: &Viewport,
    output_size: Size<i32, Logical>,
    factor: f64,
) -> Viewport {
    let new_zoom = clamp_zoom(viewport.zoom * factor);
    let before = visible_size(output_size, viewport.zoom);
    let center = viewport.loc + Point::from((before.w / 2, before.h / 2));
    let after = visible_size(output_size, new_zoom);
    let loc = clamp_pan(center - Point::from((after.w / 2, after.h / 2)));
    Viewport {
        loc,
        zoom: new_zoom,
    }
}

/// A viewport that fits `bbox` into `output_size` with `margin` (e.g. `0.9`),
/// centered. Used by the overview. Falls back to native when `bbox` is empty.
pub(crate) fn fit_viewport(
    output_size: Size<i32, Logical>,
    bbox: Rectangle<i32, Logical>,
    margin: f64,
) -> Viewport {
    if bbox.size.w <= 0 || bbox.size.h <= 0 {
        return Viewport::new(clamp_pan(bbox.loc));
    }
    let zoom_w = f64::from(output_size.w.max(1)) / f64::from(bbox.size.w);
    let zoom_h = f64::from(output_size.h.max(1)) / f64::from(bbox.size.h);
    let zoom = clamp_zoom(zoom_w.min(zoom_h) * margin);
    let visible = visible_size(output_size, zoom);
    let center = bbox.loc + Point::from((bbox.size.w / 2, bbox.size.h / 2));
    let loc = clamp_pan(center - Point::from((visible.w / 2, visible.h / 2)));
    Viewport { loc, zoom }
}

/// The minimal pan that brings `window` fully into the viewport's visible
/// region. Windows larger than the region are aligned to their top-left so at
/// least their origin is visible.
pub(crate) fn reveal_pan(
    viewport: &Viewport,
    output_size: Size<i32, Logical>,
    window: Rectangle<i32, Logical>,
) -> Point<i32, Logical> {
    let visible = visible_size(output_size, viewport.zoom);
    let x = reveal_axis(viewport.loc.x, visible.w, window.loc.x, window.size.w);
    let y = reveal_axis(viewport.loc.y, visible.h, window.loc.y, window.size.h);
    clamp_pan(Point::from((x, y)))
}

fn reveal_axis(loc: i32, visible: i32, window_start: i32, window_len: i32) -> i32 {
    let window_end = window_start + window_len;
    let view_end = loc + visible;
    if window_start < loc {
        window_start
    } else if window_end > view_end {
        if window_len >= visible {
            window_start
        } else {
            window_end - visible
        }
    } else {
        loc
    }
}

/// Staggered offset for the `index`-th placed window.
pub(crate) fn stagger_offset(index: i32) -> i32 {
    WINDOW_STAGGER * index.rem_euclid(WINDOW_STAGGER_STEPS)
}

/// Placement location for a new window inside `region` (the viewport region the
/// pointer is over). No clamping: windows may sit anywhere on the canvas.
pub(crate) fn placement_location(
    region: Rectangle<i32, Logical>,
    index: i32,
) -> Point<i32, Logical> {
    let offset = stagger_offset(index);
    region.loc + Point::from((offset, offset))
}

/// The bounding rectangle of `rects`, or `None` when empty. Used to clamp the
/// pointer to the union of the active canvas's output rectangles.
pub(crate) fn bounding_rect(
    rects: impl IntoIterator<Item = Rectangle<i32, Logical>>,
) -> Option<Rectangle<i32, Logical>> {
    let mut iter = rects.into_iter();
    let first = iter.next()?;
    let mut min_x = first.loc.x;
    let mut min_y = first.loc.y;
    let mut max_x = first.loc.x + first.size.w;
    let mut max_y = first.loc.y + first.size.h;
    for rect in iter {
        min_x = min_x.min(rect.loc.x);
        min_y = min_y.min(rect.loc.y);
        max_x = max_x.max(rect.loc.x + rect.size.w);
        max_y = max_y.max(rect.loc.y + rect.size.h);
    }
    Some(Rectangle::new(
        Point::from((min_x, min_y)),
        Size::from((max_x - min_x, max_y - min_y)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(w: i32, h: i32) -> Size<i32, Logical> {
        Size::from((w, h))
    }

    fn point(x: i32, y: i32) -> Point<i32, Logical> {
        Point::from((x, y))
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(point(x, y), size(w, h))
    }

    #[test]
    fn pan_and_zoom_clamp_to_bounds() {
        assert_eq!(
            clamp_pan(point(2_000_000, -2_000_000)),
            point(PAN_LIMIT, -PAN_LIMIT)
        );
        assert_eq!(clamp_pan(point(5, -7)), point(5, -7));
        assert_eq!(clamp_zoom(100.0), MAX_ZOOM);
        assert_eq!(clamp_zoom(0.0), MIN_ZOOM);
        assert_eq!(clamp_zoom(f64::NAN), 1.0);
    }

    #[test]
    fn visible_size_inverts_zoom() {
        assert_eq!(visible_size(size(800, 600), 1.0), size(800, 600));
        assert_eq!(visible_size(size(800, 600), 2.0), size(400, 300));
        assert_eq!(visible_size(size(800, 600), 0.5), size(1600, 1200));
    }

    #[test]
    fn to_canvas_inverts_zoom_and_offsets_by_loc() {
        let viewport = Viewport {
            loc: point(100, 50),
            zoom: 2.0,
        };
        // An output-local (40, 20) at zoom 2 is 20,10 of canvas past loc.
        let canvas = to_canvas(&viewport, Point::from((40.0, 20.0)));
        assert_eq!(canvas, Point::<f64, Logical>::from((120.0, 60.0)));
    }

    #[test]
    fn zoom_about_center_keeps_center_fixed() {
        let viewport = Viewport::new(point(0, 0));
        let output = size(800, 600);
        let center_before = viewport.loc + point(400, 300);
        let zoomed = zoom_about_center(&viewport, output, 2.0);
        assert_eq!(zoomed.zoom, 2.0);
        let visible = visible_size(output, zoomed.zoom);
        let center_after = zoomed.loc + point(visible.w / 2, visible.h / 2);
        assert_eq!(center_before, center_after);
    }

    #[test]
    fn fit_viewport_fits_bbox_with_margin_and_centers() {
        let output = size(800, 600);
        // A canvas bbox twice the output, centered far away.
        let bbox = rect(1000, 1000, 1600, 1200);
        let viewport = fit_viewport(output, bbox, 1.0);
        // 800/1600 = 0.5 on width, 600/1200 = 0.5 on height -> 0.5.
        assert_eq!(viewport.zoom, 0.5);
        let visible = visible_size(output, viewport.zoom);
        let center = viewport.loc + point(visible.w / 2, visible.h / 2);
        assert_eq!(center, point(1000 + 800, 1000 + 600));
    }

    #[test]
    fn fit_viewport_empty_bbox_is_native() {
        let viewport = fit_viewport(size(800, 600), rect(10, 20, 0, 0), 0.9);
        assert_eq!(viewport.zoom, 1.0);
        assert_eq!(viewport.loc, point(10, 20));
    }

    #[test]
    fn reveal_pan_brings_offscreen_window_into_view_minimally() {
        let viewport = Viewport::new(point(0, 0));
        let output = size(800, 600);

        // Fully visible: no pan.
        assert_eq!(
            reveal_pan(&viewport, output, rect(10, 10, 100, 100)),
            point(0, 0)
        );

        // Off to the right: pan so the right edge aligns to the view edge.
        let target = rect(900, 100, 200, 100);
        assert_eq!(
            reveal_pan(&viewport, output, target),
            point(900 + 200 - 800, 0)
        );

        // Off to the top-left: pan to the window's top-left.
        assert_eq!(
            reveal_pan(&viewport, output, rect(-50, -30, 100, 100)),
            point(-50, -30)
        );
    }

    #[test]
    fn reveal_pan_aligns_oversized_window_to_origin() {
        let viewport = Viewport::new(point(0, 0));
        let output = size(400, 300);
        // Window wider/taller than the visible region -> align to its top-left.
        let target = rect(500, 500, 800, 600);
        assert_eq!(reveal_pan(&viewport, output, target), point(500, 500));
    }

    #[test]
    fn placement_staggers_within_region_without_clamping() {
        let region = rect(2000, -500, 800, 600);
        assert_eq!(placement_location(region, 0), point(2000, -500));
        assert_eq!(placement_location(region, 1), point(2032, -468));
        // Wraps after WINDOW_STAGGER_STEPS.
        assert_eq!(
            placement_location(region, WINDOW_STAGGER_STEPS),
            placement_location(region, 0)
        );
    }

    #[test]
    fn bounding_rect_unions_all_rects() {
        assert_eq!(bounding_rect(std::iter::empty()), None);
        let rects = [rect(0, 0, 800, 600), rect(800, 0, 1280, 720)];
        assert_eq!(bounding_rect(rects), Some(rect(0, 0, 2080, 720)));
        // Outputs with a gap between them are spanned by the union.
        let gapped = [rect(0, 0, 800, 600), rect(1000, 0, 800, 600)];
        assert_eq!(bounding_rect(gapped), Some(rect(0, 0, 1800, 600)));
    }
}
