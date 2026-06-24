//! `wlr-screencopy-unstable-v1`: copy output contents into client SHM buffers.
//!
//! Smithay 0.7 ships no screencopy helper, so this module hand-writes the
//! `GlobalDispatch`/`Dispatch` glue for the manager and frame objects. Both
//! backends (nested winit and udev) render through a [`GlesRenderer`], so a
//! single [`ExportMem`]-based path serves both: render the output into an
//! offscreen texture, read it back with `copy_framebuffer`/`map_texture`, then
//! write the pixels — flipped to a top-down image — into the client's
//! shared-memory buffer.
//!
//! The capture is deferred: a `copy`/`copy_with_damage` request records a
//! pending [`Screencopy`] on [`SayukiState`], drained by
//! `SayukiState::fulfill_screencopy` at the end of the next render — matching
//! wlr-screencopy's "next frame" semantics. Only `wl_shm` buffers are
//! advertised (the format `grim` uses); linux-dmabuf import is intentionally
//! not offered.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, ExportMem, Frame, Offscreen, Renderer,
            gles::{GlesRenderer, GlesTexture},
            utils::draw_render_elements,
        },
    },
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, Flags, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::{ClientId, GlobalId},
            protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_shm},
        },
    },
    utils::{
        Buffer as BufferCoord, Clock, Logical, Monotonic, Physical, Point, Rectangle, Size,
        Transform,
    },
    wayland::shm,
};

use crate::render::SayukiRenderElement;
use crate::state::SayukiState;

/// wlr-screencopy manager version we advertise. v3 adds `buffer_done` (sent)
/// and `linux_dmabuf` (never advertised — SHM only).
const MANAGER_VERSION: u32 = 3;

/// Owns the `zwlr_screencopy_manager_v1` global for its lifetime.
pub(crate) struct ScreencopyManagerState {
    _global: GlobalId,
}

impl ScreencopyManagerState {
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let global =
            display.create_global::<SayukiState, ZwlrScreencopyManagerV1, ()>(MANAGER_VERSION, ());
        Self { _global: global }
    }
}

/// Per-frame user data attached to every `zwlr_screencopy_frame_v1`.
#[derive(Debug)]
pub(crate) struct ScreencopyFrameData {
    /// `None` marks a frame that already `failed()` at creation; a stray `copy`
    /// on it is rejected without touching the renderer.
    output: Option<Output>,
    /// Capture rectangle in output-local physical pixels, top-left origin.
    region: Rectangle<i32, Physical>,
    /// Full output size in physical pixels (the offscreen render dimensions).
    full_size: Size<i32, Physical>,
    overlay_cursor: bool,
    /// `copy`/`copy_with_damage` may be issued at most once per frame.
    used: AtomicBool,
}

impl ScreencopyFrameData {
    fn pending(
        output: Output,
        region: Rectangle<i32, Physical>,
        full_size: Size<i32, Physical>,
        overlay_cursor: bool,
    ) -> Self {
        Self {
            output: Some(output),
            region,
            full_size,
            overlay_cursor,
            used: AtomicBool::new(false),
        }
    }

    fn failed() -> Self {
        Self {
            output: None,
            region: Rectangle::default(),
            full_size: Size::default(),
            overlay_cursor: false,
            used: AtomicBool::new(true),
        }
    }
}

/// A capture awaiting fulfilment at the end of the next render pass.
pub(crate) struct Screencopy {
    pub(crate) frame: ZwlrScreencopyFrameV1,
    pub(crate) buffer: WlBuffer,
    pub(crate) output: Output,
    pub(crate) region: Rectangle<i32, Physical>,
    pub(crate) full_size: Size<i32, Physical>,
    pub(crate) overlay_cursor: bool,
    pub(crate) with_damage: bool,
}

/// The capture could not be completed (renderer or buffer failure); the frame
/// is reported to the client via `failed`.
pub(crate) struct CaptureError;

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for SayukiState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for SayukiState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_screencopy_manager_v1::Request;
        match request {
            Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => create_frame(frame, overlay_cursor != 0, &output, None, data_init),
            Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => {
                let region = Rectangle::new(
                    Point::from((x, y)),
                    Size::from((width.max(0), height.max(0))),
                );
                create_frame(frame, overlay_cursor != 0, &output, Some(region), data_init);
            }
            Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameData> for SayukiState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &ScreencopyFrameData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_screencopy_frame_v1::Request;
        let (buffer, with_damage) = match request {
            Request::Copy { buffer } => (buffer, false),
            Request::CopyWithDamage { buffer } => (buffer, true),
            Request::Destroy => {
                state
                    .pending_screencopy
                    .retain(|capture| &capture.frame != resource);
                return;
            }
            _ => return,
        };

        if data.used.swap(true, Ordering::SeqCst) {
            resource.post_error(
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "frame already used to copy a buffer",
            );
            return;
        }

        let Some(output) = data.output.clone() else {
            resource.failed();
            return;
        };

        match validate_shm_buffer(&buffer, data.region.size) {
            BufferCheck::Ok => state.pending_screencopy.push(Screencopy {
                frame: resource.clone(),
                buffer,
                output,
                region: data.region,
                full_size: data.full_size,
                overlay_cursor: data.overlay_cursor,
                with_damage,
            }),
            BufferCheck::NotShm => resource.failed(),
            BufferCheck::Invalid => resource.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "buffer does not match the advertised frame parameters",
            ),
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ZwlrScreencopyFrameV1,
        _data: &ScreencopyFrameData,
    ) {
        state
            .pending_screencopy
            .retain(|capture| &capture.frame != resource);
    }
}

/// Resolve the requested output and region, then advertise the SHM buffer
/// parameters (or `failed` if the output/region is unusable).
fn create_frame(
    frame: New<ZwlrScreencopyFrameV1>,
    overlay_cursor: bool,
    output: &WlOutput,
    region_logical: Option<Rectangle<i32, Logical>>,
    data_init: &mut DataInit<'_, SayukiState>,
) {
    let Some(output) = Output::from_resource(output) else {
        data_init
            .init(frame, ScreencopyFrameData::failed())
            .failed();
        return;
    };
    let Some(mode) = output.current_mode() else {
        data_init
            .init(frame, ScreencopyFrameData::failed())
            .failed();
        return;
    };
    let full_size = output.current_transform().transform_size(mode.size);
    let scale = output.current_scale().integer_scale().max(1);

    let region = match region_logical {
        None => Rectangle::from_size(full_size),
        Some(region_logical) => match clip_region(region_logical, full_size, scale) {
            Some(region) => region,
            None => {
                data_init
                    .init(frame, ScreencopyFrameData::failed())
                    .failed();
                return;
            }
        },
    };

    let frame = data_init.init(
        frame,
        ScreencopyFrameData::pending(output, region, full_size, overlay_cursor),
    );
    frame.buffer(
        wl_shm::Format::Xrgb8888,
        region.size.w as u32,
        region.size.h as u32,
        region.size.w as u32 * 4,
    );
    if frame.version() >= 3 {
        frame.buffer_done();
    }
}

/// Map a `capture_output_region` rectangle to an output-local physical
/// rectangle clipped to the output. Per the protocol the rectangle is in
/// output-local logical coordinates (origin at the output's top-left, extent
/// `xdg_output.logical_size`), so clip it against the logical bounds, scale to
/// physical pixels by the output's integer scale, then clamp to `full_size`.
fn clip_region(
    region_logical: Rectangle<i32, Logical>,
    full_size: Size<i32, Physical>,
    scale: i32,
) -> Option<Rectangle<i32, Physical>> {
    let logical_bounds = Rectangle::<i32, Logical>::from_size(Size::from((
        full_size.w / scale,
        full_size.h / scale,
    )));
    let clipped = region_logical.intersection(logical_bounds)?;
    let region = Rectangle::<i32, Physical>::new(
        Point::from((clipped.loc.x * scale, clipped.loc.y * scale)),
        Size::from((clipped.size.w * scale, clipped.size.h * scale)),
    )
    .intersection(Rectangle::from_size(full_size))?;
    (region.size.w > 0 && region.size.h > 0).then_some(region)
}

enum BufferCheck {
    Ok,
    NotShm,
    Invalid,
}

/// Confirm `buffer` is an SHM buffer matching the advertised frame size,
/// a supported format, and a stride/extent that fits within the pool.
fn validate_shm_buffer(buffer: &WlBuffer, size: Size<i32, Physical>) -> BufferCheck {
    let inspection = shm::with_buffer_contents(buffer, |_ptr, len, data| {
        let format_ok = matches!(data.format, wl_shm::Format::Xrgb8888);
        let dims_ok = data.width == size.w && data.height == size.h;
        let stride_ok = data.stride >= size.w.saturating_mul(4);
        let offset_ok = data.offset >= 0;
        let fits = (data.offset as i64)
            .saturating_add((data.stride as i64).saturating_mul(data.height as i64))
            <= len as i64;
        format_ok && dims_ok && stride_ok && offset_ok && fits
    });
    match inspection {
        Ok(true) => BufferCheck::Ok,
        Ok(false) => BufferCheck::Invalid,
        Err(_) => BufferCheck::NotShm,
    }
}

/// Render `capture`'s output into an offscreen texture and copy the requested
/// region into its client SHM buffer. `renderer` must drive `capture.output`.
pub(crate) fn render_capture(
    renderer: &mut GlesRenderer,
    elements: &[SayukiRenderElement<GlesRenderer>],
    capture: &Screencopy,
    background: Color32F,
) -> Result<(), CaptureError> {
    let full_size = capture.full_size;
    // Render upright: elements live in the output's logical (oriented) space and
    // the offscreen is sized to that space, so no output transform is applied
    // (screencopy has no rotation flag — the image must be delivered upright).
    let transform = Transform::Normal;
    let buffer_size = Size::<i32, BufferCoord>::from((full_size.w, full_size.h));

    let mut offscreen: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, buffer_size)
        .map_err(|_| CaptureError)?;

    let mapping = {
        let mut framebuffer = renderer.bind(&mut offscreen).map_err(|_| CaptureError)?;
        let damage = Rectangle::from_size(full_size);
        {
            let mut frame = renderer
                .render(&mut framebuffer, full_size, transform)
                .map_err(|_| CaptureError)?;
            frame
                .clear(background, &[damage])
                .map_err(|_| CaptureError)?;
            draw_render_elements(&mut frame, 1.0, elements, &[damage]).map_err(|_| CaptureError)?;
            let sync = frame.finish().map_err(|_| CaptureError)?;
            let _ = sync.wait();
        }

        // glReadPixels reads bottom-up, so flip the region's y origin.
        let region = capture.region;
        let gl_region = Rectangle::<i32, BufferCoord>::new(
            Point::from((region.loc.x, full_size.h - region.loc.y - region.size.h)),
            Size::from((region.size.w, region.size.h)),
        );
        renderer
            .copy_framebuffer(&framebuffer, gl_region, Fourcc::Xrgb8888)
            .map_err(|_| CaptureError)?
    };

    let pixels = renderer.map_texture(&mapping).map_err(|_| CaptureError)?;
    write_to_shm(&capture.buffer, pixels, capture.region.size)
}

/// Send `flags`, optional `damage`, then `ready` with a monotonic timestamp.
///
/// `copy_with_damage` is fulfilled conservatively: Sayuki has no damage-tracking
/// subsystem (it repaints every frame), so the whole region is reported as
/// damaged. That is always a valid superset of the true damage — clients capture
/// correct frames, just without the incremental-update optimisation.
pub(crate) fn send_ready(capture: &Screencopy) {
    capture.frame.flags(Flags::empty());
    if capture.with_damage {
        capture.frame.damage(
            0,
            0,
            capture.region.size.w.max(0) as u32,
            capture.region.size.h.max(0) as u32,
        );
    }
    let time: Duration = Clock::<Monotonic>::new().now().into();
    let secs = time.as_secs();
    capture.frame.ready(
        (secs >> 32) as u32,
        (secs & 0xFFFF_FFFF) as u32,
        time.subsec_nanos(),
    );
}

/// Write `pixels` (4 bytes/px, bottom-up from GL readback) into `buffer`'s
/// shared memory, flipped to a top-down image and honouring stride/offset.
fn write_to_shm(
    buffer: &WlBuffer,
    pixels: &[u8],
    size: Size<i32, Physical>,
) -> Result<(), CaptureError> {
    let width = size.w.max(0) as usize;
    let height = size.h.max(0) as usize;
    let row_bytes = width * 4;
    if pixels.len() < row_bytes * height {
        return Err(CaptureError);
    }
    shm::with_buffer_contents_mut(buffer, |ptr, len, data| {
        let dst_stride = data.stride.max(0) as usize;
        let offset = data.offset.max(0) as usize;
        // SAFETY: smithay guarantees `ptr` is valid for `len` bytes for the
        // duration of this closure. The compositor is single-threaded, so no
        // other code writes the pool concurrently, and the slice never escapes
        // the closure (mirrors the shm-write precedent in `backend/udev.rs`).
        #[allow(unsafe_code)]
        let dst = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
        copy_flipped(pixels, dst, row_bytes, height, dst_stride, offset);
    })
    .map_err(|_| CaptureError)
}

/// Copy `height` rows of `row_bytes` each from `src` (bottom-up) into `dst`
/// top-down, starting at `dst_offset` and advancing `dst_stride` per row. Rows
/// that fall outside either buffer are skipped rather than panicking.
fn copy_flipped(
    src: &[u8],
    dst: &mut [u8],
    row_bytes: usize,
    height: usize,
    dst_stride: usize,
    dst_offset: usize,
) {
    for y in 0..height {
        let src_start = (height - 1 - y) * row_bytes;
        let Some(src_row) = src.get(src_start..src_start + row_bytes) else {
            continue;
        };
        let dst_start = dst_offset + y * dst_stride;
        let Some(dst_row) = dst.get_mut(dst_start..dst_start + row_bytes) else {
            continue;
        };
        dst_row.copy_from_slice(src_row);
    }
}

#[cfg(test)]
mod tests {
    use super::{clip_region, copy_flipped};
    use smithay::utils::{Logical, Physical, Point, Rectangle, Size};

    #[test]
    fn copy_flipped_reverses_rows_and_respects_stride() {
        // 2x2 image, 4 bytes/px => 8 bytes/row. `src` is bottom-up: row 0 is
        // the bottom (0xBB), row 1 is the top (0x11).
        let row_bytes = 2 * 4;
        let height = 2;
        let mut src = vec![0u8; row_bytes * height];
        src[0..row_bytes].fill(0xBB);
        src[row_bytes..2 * row_bytes].fill(0x11);

        // Larger destination stride (padding) and a leading offset.
        let dst_stride = row_bytes + 4;
        let dst_offset = 2;
        let mut dst = vec![0u8; dst_offset + dst_stride * height];
        copy_flipped(&src, &mut dst, row_bytes, height, dst_stride, dst_offset);

        // Top destination row holds the top source row.
        assert!(
            dst[dst_offset..dst_offset + row_bytes]
                .iter()
                .all(|&b| b == 0x11)
        );
        // Second destination row holds the bottom source row.
        let second = dst_offset + dst_stride;
        assert!(dst[second..second + row_bytes].iter().all(|&b| b == 0xBB));
        // Inter-row padding is left untouched.
        assert!(dst[dst_offset + row_bytes..second].iter().all(|&b| b == 0));
    }

    #[test]
    fn copy_flipped_skips_rows_that_do_not_fit() {
        // Destination has room for a single row: must not panic.
        let row_bytes = 4;
        let height = 3;
        let src = vec![0xAB; row_bytes * height];
        let mut dst = vec![0u8; row_bytes];
        copy_flipped(&src, &mut dst, row_bytes, height, row_bytes, 0);
        assert!(dst.iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn clip_region_maps_output_local_logical_to_physical() {
        let full = Size::<i32, Physical>::from((1920, 1080));
        // Origin-anchored output-local region at scale 1 maps 1:1.
        let region = Rectangle::<i32, Logical>::new(Point::from((10, 10)), Size::from((100, 100)));
        assert_eq!(
            clip_region(region, full, 1),
            Some(Rectangle::new(
                Point::from((10, 10)),
                Size::from((100, 100))
            )),
        );
    }

    #[test]
    fn clip_region_clamps_to_output_bounds() {
        let full = Size::<i32, Physical>::from((1920, 1080));
        // Region overruns the bottom-right; the excess is clipped to the output.
        let region =
            Rectangle::<i32, Logical>::new(Point::from((1900, 1000)), Size::from((100, 100)));
        assert_eq!(
            clip_region(region, full, 1),
            Some(Rectangle::new(
                Point::from((1900, 1000)),
                Size::from((20, 80))
            )),
        );
    }

    #[test]
    fn clip_region_rejects_fully_outside_region() {
        let full = Size::<i32, Physical>::from((1920, 1080));
        let region =
            Rectangle::<i32, Logical>::new(Point::from((5000, 5000)), Size::from((100, 100)));
        assert_eq!(clip_region(region, full, 1), None);
    }

    #[test]
    fn clip_region_scales_logical_request_to_physical() {
        let full = Size::<i32, Physical>::from((1920, 1080));
        // At scale 2 the logical bounds are 960x540; the request scales up by 2.
        let region = Rectangle::<i32, Logical>::new(Point::from((10, 10)), Size::from((50, 50)));
        assert_eq!(
            clip_region(region, full, 2),
            Some(Rectangle::new(
                Point::from((20, 20)),
                Size::from((100, 100))
            )),
        );
    }
}
