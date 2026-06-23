//! Per-output render-element assembly for the canvas/viewport model.
//!
//! One primitive drives zoom, overview and minimap: render the active canvas's
//! elements and optionally rescale. **Zoom 1.0 keeps the proven 1:1
//! `Space::render_elements_for_region` path** (zero overhead in the common
//! case); only under zoom do we render per element so pinned HUD windows can be
//! excluded from the rescale and drawn 1:1 on top. The minimap is the same
//! canvas rescaled into a corner with a viewport indicator.
//!
//! This module owns the only render logic that cannot be unit-tested (it needs a
//! live renderer); the geometry it relies on lives in [`crate::wm::viewport`]
//! and is tested there.

pub(crate) mod help;
use smithay::{
    backend::renderer::{
        ImportAll, Renderer, Texture,
        element::{
            AsRenderElements, Kind,
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::{CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement},
        },
    },
    desktop::{LayerSurface, Space, Window, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    render_elements,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
    wayland::{session_lock::LockSurface, shell::wlr_layer::Layer as WlrLayer},
};

use crate::wm::{
    Canvas,
    viewport::{self, Viewport},
};

/// Minimap size as a fraction of the output's shorter dimension, its margin
/// from the corner, and the indicator/backdrop colors.
const MINIMAP_FRACTION: f64 = 0.2;
const MINIMAP_MARGIN: i32 = 24;
const MINIMAP_BACKDROP: [f32; 4] = [0.0, 0.0, 0.0, 0.4];
const MINIMAP_INDICATOR: [f32; 4] = [0.45, 0.62, 0.95, 0.35];
/// Overview fit margin: leave a 10% border so windows are not flush to the edge.
pub(crate) const OVERVIEW_MARGIN: f64 = 0.9;

render_elements! {
    /// Every render element Sayuki emits for an output.
    pub SayukiRenderElement<R> where R: ImportAll;
    /// A surface drawn 1:1 (windows at zoom 1, the cursor, pinned HUD overlays).
    Surface = WaylandSurfaceRenderElement<R>,
    /// A surface rescaled for continuous zoom / overview.
    Rescaled = RescaleRenderElement<WaylandSurfaceRenderElement<R>>,
    /// Minimap content: the canvas rescaled, relocated and clipped into the minimap rect.
    Minimap = CropRenderElement<RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<R>>>>,
    /// A solid color (minimap backdrop and viewport indicator).
    Solid = SolidColorRenderElement,
}

/// The client cursor surface to draw, if any, plus where the pointer is.
#[derive(Clone, Copy)]
pub(crate) struct CursorRender<'a> {
    pub(crate) surface: &'a WlSurface,
    pub(crate) hotspot: Point<i32, Logical>,
    pub(crate) location: Point<f64, Logical>,
}

/// Build the render elements for `output` from the active `canvas`.
///
/// Returned front-to-back (index 0 is topmost): cursor, then the minimap
/// overlay, then pinned HUD windows, then the canvas windows. The output's own
/// clipping discards anything outside its rectangle, so no explicit crop is
/// needed for the main pass.
pub(crate) fn output_elements<R>(
    renderer: &mut R,
    canvas: &Canvas,
    output: &Output,
    cursor: Option<CursorRender<'_>>,
    help_menu: Option<&help::HelpMenu>,
    lock_surfaces: &[(LockSurface, Output)],
    locked: bool,
) -> Vec<SayukiRenderElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Texture + Clone + 'static,
{
    let Some(output_geometry) = canvas.space().output_geometry(output) else {
        return Vec::new();
    };
    let viewport = canvas.viewport(&output.name());
    let output_size = output_geometry.size;
    let region = viewport::visible_region(&viewport, output_size);

    let mut elements: Vec<SayukiRenderElement<R>> = Vec::new();

    // Cursor first so it stays above the compositor help overlay and every
    // client-provided layer/window surface. Its position is scaled by the zoom
    // so it tracks the interaction point; the surface itself is drawn native
    // size (a HUD), so the hotspot offset is not scaled.
    if let Some(cursor) = cursor
        && region.to_f64().contains(cursor.location)
    {
        let pointer = (cursor.location - output_geometry.loc.to_f64())
            .to_physical_precise_round(viewport.zoom);
        let position = pointer - cursor.hotspot.to_physical_precise_round(1.0);
        elements.extend(
            render_elements_from_surface_tree::<R, WaylandSurfaceRenderElement<R>>(
                renderer,
                cursor.surface,
                position,
                1.0,
                1.0,
                Kind::Unspecified,
            )
            .into_iter()
            .map(SayukiRenderElement::Surface),
        );
    }
    let mut drew_lock_surface = false;
    for (lock_surface, lock_output) in lock_surfaces {
        if lock_output == output {
            drew_lock_surface = true;
            elements.extend(
                render_elements_from_surface_tree::<R, WaylandSurfaceRenderElement<R>>(
                    renderer,
                    lock_surface.wl_surface(),
                    (0, 0),
                    1.0,
                    1.0,
                    Kind::Unspecified,
                )
                .into_iter()
                .map(SayukiRenderElement::Surface),
            );
        }
    }
    if locked && !drew_lock_surface {
        elements.push(SayukiRenderElement::Solid(solid(
            Rectangle::from_size(output_size),
            [0.0, 0.0, 0.0, 1.0],
        )));
    }
    if let Some(help_menu) = help_menu {
        help_elements(help_menu, output_size, &mut elements);
    }
    layer_elements(renderer, output, WlrLayer::Overlay, &mut elements);
    layer_elements(renderer, output, WlrLayer::Top, &mut elements);

    // Minimap overlay above the windows.
    if canvas.minimap_enabled(&output.name()) {
        minimap_elements(renderer, canvas, &viewport, output_size, &mut elements);
    }

    if (viewport.zoom - 1.0).abs() < f64::EPSILON {
        // Common path: the proven Space renderer at 1:1. Pinned windows are
        // ordinary space elements at their HUD coordinates and render correctly.
        elements.extend(
            canvas
                .space()
                .render_elements_for_region(renderer, &region, 1.0, 1.0)
                .into_iter()
                .map(SayukiRenderElement::Surface),
        );
    } else {
        // Zoomed: draw pinned HUD windows 1:1 on top, then rescale the rest.
        let space = canvas.space();
        let scale = Scale::from(viewport.zoom);
        let origin = Point::<i32, Physical>::from((0, 0));

        for window in space
            .elements()
            .rev()
            .filter(|window| canvas.is_pinned(window))
        {
            elements.extend(
                window_surface_elements(renderer, space, window, region)
                    .into_iter()
                    .map(SayukiRenderElement::Surface),
            );
        }
        for window in space.elements().rev() {
            if canvas.is_pinned(window) {
                continue;
            }
            if space
                .element_bbox(window)
                .map(|bbox| !region.overlaps(bbox))
                .unwrap_or(true)
            {
                continue;
            }
            elements.extend(
                window_surface_elements(renderer, space, window, region)
                    .into_iter()
                    .map(|element| {
                        SayukiRenderElement::Rescaled(RescaleRenderElement::from_element(
                            element, origin, scale,
                        ))
                    }),
            );
        }
    }

    layer_elements(renderer, output, WlrLayer::Bottom, &mut elements);
    layer_elements(renderer, output, WlrLayer::Background, &mut elements);

    elements
}

fn layer_elements<R>(
    renderer: &mut R,
    output: &Output,
    layer: WlrLayer,
    elements: &mut Vec<SayukiRenderElement<R>>,
) where
    R: Renderer + ImportAll,
    R::TextureId: Texture + Clone + 'static,
{
    let layer_map = layer_map_for_output(output);
    for surface in layer_map.layers_on(layer).rev() {
        elements.extend(layer_surface_elements(renderer, &layer_map, surface));
    }
}

fn layer_surface_elements<R>(
    renderer: &mut R,
    layer_map: &smithay::desktop::LayerMap,
    surface: &LayerSurface,
) -> Vec<SayukiRenderElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Texture + Clone + 'static,
{
    let Some(geometry) = layer_map.layer_geometry(surface) else {
        return Vec::new();
    };

    render_elements_from_surface_tree::<R, WaylandSurfaceRenderElement<R>>(
        renderer,
        surface.wl_surface(),
        geometry.loc.to_physical_precise_round(1.0),
        1.0,
        1.0,
        Kind::Unspecified,
    )
    .into_iter()
    .map(SayukiRenderElement::Surface)
    .collect()
}

/// Render a single window's surfaces at 1:1, positioned output-local relative to
/// `region` exactly as `Space::render_elements_for_region` would.
fn window_surface_elements<R>(
    renderer: &mut R,
    space: &Space<Window>,
    window: &Window,
    region: Rectangle<i32, Logical>,
) -> Vec<WaylandSurfaceRenderElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Texture + Clone + 'static,
{
    let Some(location) = space.element_location(window) else {
        return Vec::new();
    };
    let render_location = location - window.geometry().loc;
    let physical = (render_location - region.loc).to_physical_precise_round(1.0);
    window.render_elements::<WaylandSurfaceRenderElement<R>>(
        renderer,
        physical,
        Scale::from(1.0),
        1.0,
    )
}

/// Append the minimap overlay (indicator, clipped canvas, backdrop) to
/// `elements` in front-to-back order.
fn minimap_elements<R>(
    renderer: &mut R,
    canvas: &Canvas,
    viewport: &Viewport,
    output_size: Size<i32, Logical>,
    elements: &mut Vec<SayukiRenderElement<R>>,
) where
    R: Renderer + ImportAll,
    R::TextureId: Texture + Clone + 'static,
{
    let Some(bbox) = canvas_bounds(canvas) else {
        return;
    };

    let minimap = minimap_rect(output_size);
    let fit = fit_scale(bbox.size, minimap.size);
    let scaled = Size::<i32, Logical>::from((
        (f64::from(bbox.size.w) * fit).round() as i32,
        (f64::from(bbox.size.h) * fit).round() as i32,
    ));
    // Top-left of the scaled canvas inside the (centered) minimap rect.
    let place = minimap.loc
        + Point::from((
            (minimap.size.w - scaled.w) / 2,
            (minimap.size.h - scaled.h) / 2,
        ));

    // Indicator: the visible region mapped into minimap space, clipped to it.
    let visible = viewport::visible_region(viewport, output_size);
    let indicator = Rectangle::<i32, Logical>::new(
        place
            + Point::from((
                (f64::from(visible.loc.x - bbox.loc.x) * fit).round() as i32,
                (f64::from(visible.loc.y - bbox.loc.y) * fit).round() as i32,
            )),
        Size::from((
            (f64::from(visible.size.w) * fit).round() as i32,
            (f64::from(visible.size.h) * fit).round() as i32,
        )),
    );
    if let Some(clipped) = indicator.intersection(minimap) {
        elements.push(SayukiRenderElement::Solid(solid(
            clipped,
            MINIMAP_INDICATOR,
        )));
    }

    // Canvas content: scale about the origin, relocate into the minimap, clip.
    let place_physical = Point::<i32, Physical>::from((place.x, place.y));
    let crop = physical_rect(minimap);
    let canvas_elements = canvas
        .space()
        .render_elements_for_region(renderer, &bbox, 1.0, 1.0);
    for element in canvas_elements {
        let scaled = RescaleRenderElement::from_element(
            element,
            Point::<i32, Physical>::from((0, 0)),
            Scale::from(fit),
        );
        let relocated =
            RelocateRenderElement::from_element(scaled, place_physical, Relocate::Relative);
        if let Some(cropped) = CropRenderElement::from_element(relocated, Scale::from(1.0), crop) {
            elements.push(SayukiRenderElement::Minimap(cropped));
        }
    }

    // Backdrop below the content for legibility.
    elements.push(SayukiRenderElement::Solid(solid(minimap, MINIMAP_BACKDROP)));
}

fn help_elements<R>(
    help_menu: &help::HelpMenu,
    output_size: Size<i32, Logical>,
    elements: &mut Vec<SayukiRenderElement<R>>,
) where
    R: Renderer + ImportAll,
    R::TextureId: Texture + Clone + 'static,
{
    let Some(layout) = help_menu.layout(output_size) else {
        return;
    };
    elements.push(SayukiRenderElement::Solid(solid(
        layout.panel,
        [0.02, 0.03, 0.05, 0.88],
    )));
    for row in layout.rows {
        for rect in help::text_rects(&row.keys, row.baseline) {
            elements.push(SayukiRenderElement::Solid(solid(
                rect,
                [0.85, 0.90, 1.0, 1.0],
            )));
        }
        let action_origin = Point::from((row.action_x, row.baseline.y));
        for rect in help::text_rects(&row.action, action_origin) {
            elements.push(SayukiRenderElement::Solid(solid(
                rect,
                [0.68, 0.74, 0.84, 1.0],
            )));
        }
    }
}

/// The bounding rectangle of every window on the canvas, in canvas coordinates.
fn canvas_bounds(canvas: &Canvas) -> Option<Rectangle<i32, Logical>> {
    let space = canvas.space();
    viewport::bounding_rect(
        space
            .elements()
            .filter_map(|element| space.element_geometry(element)),
    )
}

fn minimap_rect(output_size: Size<i32, Logical>) -> Rectangle<i32, Logical> {
    let shorter = output_size.w.min(output_size.h);
    let side = ((f64::from(shorter) * MINIMAP_FRACTION).round() as i32).max(1);
    let loc = Point::from((
        output_size.w - side - MINIMAP_MARGIN,
        output_size.h - side - MINIMAP_MARGIN,
    ));
    Rectangle::new(loc, Size::from((side, side)))
}

fn fit_scale(content: Size<i32, Logical>, into: Size<i32, Logical>) -> f64 {
    let scale_w = f64::from(into.w) / f64::from(content.w.max(1));
    let scale_h = f64::from(into.h) / f64::from(content.h.max(1));
    scale_w.min(scale_h).min(1.0)
}

fn solid(rect: Rectangle<i32, Logical>, color: [f32; 4]) -> SolidColorRenderElement {
    let buffer = SolidColorBuffer::new(rect.size, color);
    SolidColorRenderElement::from_buffer(
        &buffer,
        physical_point(rect.loc),
        1.0,
        1.0,
        Kind::Unspecified,
    )
}

fn physical_point(point: Point<i32, Logical>) -> Point<i32, Physical> {
    Point::from((point.x, point.y))
}

fn physical_size(size: Size<i32, Logical>) -> Size<i32, Physical> {
    Size::from((size.w, size.h))
}

fn physical_rect(rect: Rectangle<i32, Logical>) -> Rectangle<i32, Physical> {
    Rectangle::new(physical_point(rect.loc), physical_size(rect.size))
}
