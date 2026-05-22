use std::{error::Error, time::Instant};

use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, Axis, Device, Event as InputBackendEvent, InputEvent,
            KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        },
        renderer::{Color32F, Frame, Renderer, gles::GlesRenderer},
        winit::{WinitEvent, WinitGraphicsBackend, WinitInput},
    },
    input::{
        Seat, SeatState,
        keyboard::{FilterResult, KeyboardHandle},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerHandle},
    },
    output::Output,
    reexports::wayland_server::Display,
    utils::{Physical, Rectangle, SERIAL_COUNTER, Size, Transform},
    wayland::{compositor::CompositorState, shm::ShmState},
};
use tracing::{debug, info};

use crate::output::{configure_output, create_output};

const BACKGROUND: Color32F = Color32F::new(0.07, 0.08, 0.11, 1.0);

pub(crate) struct SayukiState {
    pub(crate) compositor_state: CompositorState,
    pub(crate) shm_state: ShmState,
    pub(crate) seat_state: SeatState<Self>,

    backend: WinitGraphicsBackend<GlesRenderer>,
    pub(crate) output: Output,
    _seat: Seat<Self>,
    keyboard: KeyboardHandle<Self>,
    pointer: PointerHandle<Self>,

    pointer_location: smithay::utils::Point<f64, smithay::utils::Logical>,
    start_time: Instant,
    pub(crate) running: bool,
}

impl SayukiState {
    pub(crate) fn new(
        display: &Display<Self>,
        backend: WinitGraphicsBackend<GlesRenderer>,
    ) -> Result<Self, Box<dyn Error>> {
        let display_handle = display.handle();

        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, Vec::new());

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "seat0");
        let keyboard = seat.add_keyboard(Default::default(), 500, 25)?;
        let pointer = seat.add_pointer();

        let output = create_output(&display_handle, backend.window_size());

        Ok(Self {
            compositor_state,
            shm_state,
            seat_state,
            backend,
            output,
            _seat: seat,
            keyboard,
            pointer,
            pointer_location: (0.0, 0.0).into(),
            start_time: Instant::now(),
            running: true,
        })
    }

    pub(crate) fn handle_winit_event(&mut self, event: WinitEvent) {
        match event {
            WinitEvent::CloseRequested => {
                info!("close requested");
                self.running = false;
            }
            WinitEvent::Focus(focused) => {
                debug!(focused, "nested window focus changed");
            }
            WinitEvent::Resized { size, scale_factor } => {
                debug!(?size, scale_factor, "nested window resized");
                self.configure_output(size);
            }
            WinitEvent::Input(event) => self.handle_input(event),
            WinitEvent::Redraw => {}
        }
    }

    fn handle_input(&mut self, event: InputEvent<WinitInput>) {
        match event {
            InputEvent::Keyboard { event } => self.forward_keyboard_event(event),
            InputEvent::PointerMotionAbsolute { event } => self.forward_pointer_motion(event),
            InputEvent::PointerButton { event } => self.forward_pointer_button(event),
            InputEvent::PointerAxis { event } => self.forward_pointer_axis(event),
            InputEvent::DeviceAdded { device } => {
                debug!(name = device.name(), "input device added")
            }
            InputEvent::DeviceRemoved { device } => {
                debug!(name = device.name(), "input device removed");
            }
            _ => {}
        }
    }

    fn forward_keyboard_event(
        &mut self,
        event: <WinitInput as smithay::backend::input::InputBackend>::KeyboardKeyEvent,
    ) {
        let keyboard = self.keyboard.clone();
        let _binding = keyboard.input::<(), _>(
            self,
            event.key_code(),
            event.state(),
            SERIAL_COUNTER.next_serial(),
            event.time_msec(),
            |_, _, _| FilterResult::Forward,
        );
    }

    fn forward_pointer_motion(
        &mut self,
        event: <WinitInput as smithay::backend::input::InputBackend>::PointerMotionAbsoluteEvent,
    ) {
        let logical_size = self.logical_output_size();
        let location = event.position_transformed(logical_size);
        self.pointer_location = location;

        let pointer = self.pointer.clone();
        pointer.motion(
            self,
            None,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
            },
        );
        pointer.frame(self);
    }

    fn forward_pointer_button(
        &mut self,
        event: <WinitInput as smithay::backend::input::InputBackend>::PointerButtonEvent,
    ) {
        let pointer = self.pointer.clone();
        pointer.button(
            self,
            &ButtonEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                button: event.button_code(),
                state: event.state(),
            },
        );
        pointer.frame(self);
    }

    fn forward_pointer_axis(
        &mut self,
        event: <WinitInput as smithay::backend::input::InputBackend>::PointerAxisEvent,
    ) {
        let mut frame = AxisFrame::new(event.time_msec()).source(event.source());

        for axis in [Axis::Horizontal, Axis::Vertical] {
            if let Some(amount) = event.amount(axis) {
                frame = frame.value(axis, amount);
            }
            if let Some(amount_v120) = event.amount_v120(axis) {
                frame = frame.v120(axis, amount_v120 as i32);
            }
            frame = frame.relative_direction(axis, event.relative_direction(axis));
        }

        let pointer = self.pointer.clone();
        pointer.axis(self, frame);
        pointer.frame(self);
    }

    fn configure_output(&self, size: Size<i32, Physical>) {
        configure_output(&self.output, size);
    }

    fn logical_output_size(&self) -> Size<i32, smithay::utils::Logical> {
        self.output
            .current_mode()
            .map(|mode| mode.size.to_logical(1))
            .unwrap_or_else(|| self.backend.window_size().to_logical(1))
    }

    pub(crate) fn render(&mut self) -> Result<(), Box<dyn Error>> {
        let size = self.backend.window_size();
        if size.w == 0 || size.h == 0 {
            return Ok(());
        }

        let damage = Rectangle::from_size(size);
        {
            let (renderer, mut framebuffer) = self.backend.bind()?;
            let mut frame = renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
            frame.clear(BACKGROUND, &[damage])?;
            let _sync_point = frame.finish()?;
        }
        self.backend.submit(Some(&[damage]))?;

        Ok(())
    }

    pub(crate) fn frame_time(&self) -> u32 {
        self.start_time.elapsed().as_millis() as u32
    }
}
