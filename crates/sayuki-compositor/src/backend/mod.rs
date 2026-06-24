pub(crate) mod nested;
pub(crate) mod udev;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::Output;

#[allow(clippy::large_enum_variant)]
pub(crate) enum BackendState {
    Nested(nested::NestedBackend),
    Udev(udev::NativeBackend),
}

impl BackendState {
    pub(crate) fn primary_output(&self) -> Option<&Output> {
        match self {
            Self::Nested(backend) => Some(backend.output()),
            Self::Udev(backend) => backend.primary_output(),
        }
    }

    pub(crate) fn for_each_output(&self, mut f: impl FnMut(&Output)) {
        match self {
            Self::Nested(backend) => f(backend.output()),
            Self::Udev(backend) => backend.for_each_output(f),
        }
    }

    /// The `GlesRenderer` that drives `output`, if this backend owns it.
    /// Used by screencopy to render an offscreen capture of that output.
    pub(crate) fn renderer_for_output(&mut self, output: &Output) -> Option<&mut GlesRenderer> {
        match self {
            Self::Nested(backend) => {
                if backend.output().name() == output.name() {
                    Some(backend.graphics_mut().renderer())
                } else {
                    None
                }
            }
            Self::Udev(backend) => backend.renderer_for_output(output),
        }
    }
}
