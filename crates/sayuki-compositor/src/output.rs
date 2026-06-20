use smithay::{
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_server::DisplayHandle,
    utils::{Physical, Size, Transform},
};

use crate::state::SayukiState;

pub(crate) const OUTPUT_REFRESH_MHZ: i32 = 60_000;

pub(crate) fn create_nested_output(
    display_handle: &DisplayHandle,
    size: Size<i32, Physical>,
) -> Output {
    let output = Output::new(
        "sayuki-nested-0".into(),
        PhysicalProperties {
            size: (340, 210).into(),
            subpixel: Subpixel::Unknown,
            make: "Sayuki".into(),
            model: "Nested winit".into(),
        },
    );

    output.create_global::<SayukiState>(display_handle);
    configure_output(&output, size);

    output
}

pub(crate) fn configure_output(output: &Output, size: Size<i32, Physical>) {
    let mode = Mode {
        size,
        refresh: OUTPUT_REFRESH_MHZ,
    };

    output.set_preferred(mode);
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
}
