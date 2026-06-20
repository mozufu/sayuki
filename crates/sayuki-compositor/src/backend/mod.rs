pub(crate) mod nested;
pub(crate) mod udev;

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
}
