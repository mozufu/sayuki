use std::error::Error;

use smithay::{
    backend::{
        renderer::gles::GlesRenderer,
        winit::{self, WinitEventLoop, WinitGraphicsBackend},
    },
    output::Output,
    reexports::wayland_server::DisplayHandle,
    utils::{Physical, Size},
};

use crate::output::{configure_output, create_nested_output};

pub(crate) struct NestedBackend {
    graphics: WinitGraphicsBackend<GlesRenderer>,
    output: Output,
}

pub(crate) fn init(
    display_handle: &DisplayHandle,
) -> Result<(NestedBackend, WinitEventLoop), Box<dyn Error>> {
    let (graphics, winit_event_loop) = winit::init::<GlesRenderer>()?;
    let output = create_nested_output(display_handle, graphics.window_size());

    Ok((NestedBackend { graphics, output }, winit_event_loop))
}

impl NestedBackend {
    pub(crate) fn output(&self) -> &Output {
        &self.output
    }

    pub(crate) fn graphics_mut(&mut self) -> &mut WinitGraphicsBackend<GlesRenderer> {
        &mut self.graphics
    }

    pub(crate) fn window_size(&self) -> Size<i32, Physical> {
        self.graphics.window_size()
    }

    pub(crate) fn configure_output(&mut self, size: Size<i32, Physical>) {
        configure_output(&self.output, size);
    }
}
