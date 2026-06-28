//! Server-side bindings for Sayuki's first-party Wayland protocols.
//!
//! Generated from `protocols/*.xml` by the `wayland-scanner` proc-macros at
//! compile time. wayland-scanner 0.31 exposes only proc-macros (no build-script
//! API), so there is deliberately no `build.rs`.
//!
//! This crate is bindings-only: the `GlobalDispatch`/`Dispatch` impls live in
//! the compositor, mirroring the hand-rolled `screencopy.rs` precedent. Each
//! protocol is exposed under its own module with a `server` submodule, so
//! consumers write e.g.
//! `sayuki_protocols::project::server::zsayuki_project_manager_v1::ZsayukiProjectManagerV1`
//! — matching how the compositor already imports `wayland-protocols-wlr` server
//! bindings.
//!
//! The generated modules carry broad `#[allow(..)]` (including `unsafe_code`,
//! which the workspace otherwise denies): scanner output is machine-generated
//! and intentionally exempt from the project's hand-written-code lints.

/// `zsayuki_project_v1`: per-surface project affinity. A client declares which
/// project canvas a toplevel belongs to, plus its persistent canvas coordinates
/// and rule hints, before the surface maps.
pub mod project {
    #[allow(
        unsafe_code,
        non_camel_case_types,
        non_upper_case_globals,
        non_snake_case,
        unused_imports,
        unused_unsafe,
        dead_code,
        missing_docs,
        clippy::all
    )]
    pub mod server {
        use wayland_server;
        use wayland_server::protocol::*;

        pub mod __interfaces {
            use wayland_server::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/sayuki-project-v1.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_server_code!("protocols/sayuki-project-v1.xml");
    }
}

/// `zsayuki_canvas_v1`: the unbounded-canvas viewport, scoped per `wl_output`.
/// Observer events feed minimap/overview clients; controller requests let the
/// shell pan, zoom, and focus the camera.
pub mod canvas {
    #[allow(
        unsafe_code,
        non_camel_case_types,
        non_upper_case_globals,
        non_snake_case,
        unused_imports,
        unused_unsafe,
        dead_code,
        missing_docs,
        clippy::all
    )]
    pub mod server {
        use wayland_server;
        use wayland_server::protocol::*;

        pub mod __interfaces {
            use wayland_server::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/sayuki-canvas-v1.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_server_code!("protocols/sayuki-canvas-v1.xml");
    }
}

#[cfg(test)]
mod tests {
    use wayland_server::Resource;

    use crate::canvas::server::{
        zsayuki_canvas_manager_v1::ZsayukiCanvasManagerV1, zsayuki_canvas_v1::ZsayukiCanvasV1,
    };
    use crate::project::server::{
        zsayuki_project_manager_v1::ZsayukiProjectManagerV1,
        zsayuki_project_surface_v1::ZsayukiProjectSurfaceV1,
    };

    // Proves the XML generated all four interfaces with their expected wire
    // names — the milestone-9 "generated-binding smoke" acceptance check.
    #[test]
    fn interfaces_are_generated_with_expected_names() {
        assert_eq!(
            ZsayukiProjectManagerV1::interface().name,
            "zsayuki_project_manager_v1"
        );
        assert_eq!(
            ZsayukiProjectSurfaceV1::interface().name,
            "zsayuki_project_surface_v1"
        );
        assert_eq!(
            ZsayukiCanvasManagerV1::interface().name,
            "zsayuki_canvas_manager_v1"
        );
        assert_eq!(ZsayukiCanvasV1::interface().name, "zsayuki_canvas_v1");
    }
}
