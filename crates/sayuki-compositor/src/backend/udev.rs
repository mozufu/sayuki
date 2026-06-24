use std::{
    collections::HashMap,
    error::Error,
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

use calloop::{LoopHandle, RegistrationToken};
use smithay::{
    backend::{
        allocator::{
            Fourcc,
            dmabuf::Dmabuf,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata as EventMetadata,
            compositor::{FrameError, FrameFlags, PrimaryPlaneElement},
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            Bind, Color32F, element::surface::WaylandSurfaceRenderElement, gles::GlesRenderer,
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent},
    },
    desktop::{layer_map_for_output, utils::OutputPresentationFeedback},
    output::{Output, PhysicalProperties, Scale},
    reexports::{
        drm::control::{ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_server::{DisplayHandle, backend::GlobalId},
    },
    utils::{DeviceFd, Logical, Monotonic, Point, Transform},
    wayland::{presentation::Refresh, session_lock::LockSurface},
};
use smithay_drm_extras::{
    display_info,
    drm_scanner::{DrmScanEvent, DrmScanner},
};
use tracing::{debug, error, info, warn};

use crate::{
    output::OUTPUT_REFRESH_MHZ,
    render::{self, CursorRender},
    state::SayukiState,
    wm::{Canvas, WindowManager},
};

const OUTPUT_GLOBAL_REMOVAL_DELAY: Duration = Duration::from_secs(5);
const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];

type NativeAllocator = GbmAllocator<DrmDeviceFd>;
type NativeExporter = GbmFramebufferExporter<DrmDeviceFd>;
type NativeOutputManager = DrmOutputManager<NativeAllocator, NativeExporter, (), DrmDeviceFd>;
type NativeDrmOutput = DrmOutput<NativeAllocator, NativeExporter, (), DrmDeviceFd>;

pub(crate) struct NativeBackend {
    session: LibSeatSession,
    active: bool,
    devices: HashMap<u64, NativeDevice>,
    output_order: Vec<(u64, crtc::Handle)>,
}

struct NativeDevice {
    path: PathBuf,
    notifier_token: RegistrationToken,
    scanner: DrmScanner,
    manager: NativeOutputManager,
    renderer: GlesRenderer,
    outputs: HashMap<crtc::Handle, NativeOutput>,
}

struct NativeOutput {
    output: Output,
    global: GlobalId,
    drm_output: NativeDrmOutput,
    connector: connector::Handle,
    pending_frame: bool,
    pending_feedback: Option<OutputPresentationFeedback>,
}

#[derive(Default)]
struct NativeOutputUpdates {
    added: Vec<Output>,
    removed: Vec<(Output, GlobalId)>,
}

fn device_id_to_u64(device_id: impl Into<u64>) -> u64 {
    device_id.into()
}

impl NativeBackend {
    pub(crate) fn init(
        display_handle: &DisplayHandle,
        loop_handle: &LoopHandle<SayukiState>,
    ) -> Result<Self, Box<dyn Error>> {
        let (session, session_notifier) = LibSeatSession::new()?;
        let active = session.is_active();
        let udev_backend = UdevBackend::new(session.seat())?;
        let device_list = udev_backend
            .device_list()
            .map(|(device_id, path)| (device_id_to_u64(device_id), path.to_path_buf()))
            .collect::<Vec<_>>();

        let mut libinput = Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
        libinput.udev_assign_seat(&session.seat()).map_err(|()| {
            io::Error::new(io::ErrorKind::NotFound, "failed to assign libinput seat")
        })?;
        let libinput_backend = LibinputInputBackend::new(libinput);
        loop_handle.insert_source(libinput_backend, |event, _, state| {
            state.handle_libinput_event(event);
        })?;

        loop_handle.insert_source(session_notifier, |event, _, state| {
            if let Err(error) = state.handle_session_event(event) {
                error!(?error, "failed to handle session event");
            }
        })?;

        let display_handle_clone = display_handle.clone();
        let loop_handle_clone = loop_handle.clone();
        loop_handle.insert_source(udev_backend, move |event, _, state| {
            if let Err(error) =
                state.handle_udev_event(event, &display_handle_clone, &loop_handle_clone)
            {
                error!(?error, "failed to handle udev event");
            }
        })?;

        let mut backend = Self {
            session,
            active,
            devices: HashMap::new(),
            output_order: Vec::new(),
        };

        for (device_id, path) in device_list {
            let updates = backend.add_device(device_id, path, display_handle, loop_handle)?;
            debug!(
                added_outputs = updates.added.len(),
                removed_outputs = updates.removed.len(),
                "initial DRM scan complete"
            );
        }

        Ok(backend)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn primary_output(&self) -> Option<&Output> {
        self.output_order.iter().find_map(|(device_id, crtc)| {
            self.devices
                .get(device_id)
                .and_then(|device| device.outputs.get(crtc))
                .map(|native_output| &native_output.output)
        })
    }

    pub(crate) fn for_each_output(&self, mut f: impl FnMut(&Output)) {
        for (device_id, crtc) in &self.output_order {
            if let Some(output) = self
                .devices
                .get(device_id)
                .and_then(|device| device.outputs.get(crtc))
            {
                f(&output.output);
            }
        }
    }

    /// The `GlesRenderer` whose device drives `output`, matched by name.
    pub(crate) fn renderer_for_output(&mut self, output: &Output) -> Option<&mut GlesRenderer> {
        let name = output.name();
        for device in self.devices.values_mut() {
            if device
                .outputs
                .values()
                .any(|native| native.output.name() == name)
            {
                return Some(&mut device.renderer);
            }
        }
        None
    }

    pub(crate) fn handle_udev_event(
        &mut self,
        event: UdevEvent,
        display_handle: &DisplayHandle,
        loop_handle: &LoopHandle<SayukiState>,
        wm: &mut WindowManager,
        pending_removals: &mut Vec<(GlobalId, Instant)>,
    ) -> Result<(), Box<dyn Error>> {
        match event {
            UdevEvent::Added { device_id, path } => {
                let device_id = device_id_to_u64(device_id);
                let updates = if self.devices.contains_key(&device_id) {
                    self.scan_connectors(device_id, display_handle)?
                } else {
                    self.add_device(device_id, path, display_handle, loop_handle)?
                };
                apply_output_updates(display_handle, wm, pending_removals, updates);
            }
            UdevEvent::Changed { device_id } => {
                let device_id = device_id_to_u64(device_id);
                if self.devices.contains_key(&device_id) {
                    let updates = self.scan_connectors(device_id, display_handle)?;
                    apply_output_updates(display_handle, wm, pending_removals, updates);
                } else {
                    warn!(device_id, "udev change ignored for unknown DRM device");
                }
            }
            UdevEvent::Removed { device_id } => {
                self.remove_device(
                    device_id_to_u64(device_id),
                    display_handle,
                    loop_handle,
                    wm,
                    pending_removals,
                );
            }
        }

        Ok(())
    }

    pub(crate) fn handle_session_event(
        &mut self,
        event: SessionEvent,
        display_handle: &DisplayHandle,
        wm: &mut WindowManager,
        pending_removals: &mut Vec<(GlobalId, Instant)>,
    ) -> Result<(), Box<dyn Error>> {
        match event {
            SessionEvent::PauseSession => {
                self.active = false;
                for device in self.devices.values_mut() {
                    device.manager.pause();
                    for output in device.outputs.values_mut() {
                        output.pending_frame = false;
                    }
                }
            }
            SessionEvent::ActivateSession => {
                self.active = true;
                for device in self.devices.values_mut() {
                    device.manager.activate(true)?;
                    for output in device.outputs.values_mut() {
                        output.drm_output.reset_buffers();
                        output.pending_frame = false;
                    }
                }

                let device_ids = self.devices.keys().copied().collect::<Vec<_>>();
                for device_id in device_ids {
                    let updates = self.scan_connectors(device_id, display_handle)?;
                    apply_output_updates(display_handle, wm, pending_removals, updates);
                }
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn handle_drm_event(
        &mut self,
        device_id: u64,
        event: DrmEvent,
        metadata: Option<EventMetadata>,
        display_handle: &DisplayHandle,
        loop_handle: &LoopHandle<SayukiState>,
        wm: &mut WindowManager,
        pending_removals: &mut Vec<(GlobalId, Instant)>,
    ) -> Result<Option<Output>, Box<dyn Error>> {
        match event {
            DrmEvent::VBlank(crtc) => {
                debug!(device_id, ?crtc, ?metadata, "DRM vblank");
                let Some(device) = self.devices.get_mut(&device_id) else {
                    warn!(device_id, ?crtc, "vblank for unknown DRM device");
                    return Ok(None);
                };
                let Some(output) = device.outputs.get_mut(&crtc) else {
                    warn!(device_id, ?crtc, "vblank for unknown DRM output");
                    return Ok(None);
                };

                match output.drm_output.frame_submitted() {
                    Ok(_) => {
                        output.pending_frame = false;
                        if let Some(mut feedback) = output.pending_feedback.take() {
                            let refresh = Refresh::fixed(Duration::from_nanos(
                                (1_000_000_000u64 * 1000) / OUTPUT_REFRESH_MHZ as u64,
                            ));
                            if let Some(metadata) = metadata {
                                let time = match metadata.time {
                                    smithay::backend::drm::DrmEventTime::Monotonic(duration) => {
                                        duration
                                    }
                                    smithay::backend::drm::DrmEventTime::Realtime(system_time) => {
                                        system_time
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                    }
                                };
                                feedback.presented::<_, Monotonic>(
                                    time,
                                    refresh,
                                    u64::from(metadata.sequence),
                                    smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::Vsync
                                        | smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::HwCompletion,
                                );
                            } else {
                                feedback.discarded();
                            }
                        }
                        Ok(Some(output.output.clone()))
                    }
                    Err(error) => {
                        warn!(?error, output = %output.output.name(), "failed to mark DRM frame submitted");
                        output.drm_output.reset_buffers();
                        output.pending_frame = false;
                        Ok(None)
                    }
                }
            }
            DrmEvent::Error(error) => {
                warn!(?error, device_id, "DRM device event error");
                self.remove_device(device_id, display_handle, loop_handle, wm, pending_removals);
                Ok(None)
            }
        }
    }

    pub(crate) fn render(
        &mut self,
        canvas: &Canvas,
        cursor: Option<CursorRender<'_>>,
        background: Color32F,
        help_menu: Option<&render::help::HelpMenu>,
        lock_surfaces: &[(LockSurface, Output)],
        locked: bool,
    ) -> Result<(), Box<dyn Error>> {
        if !self.active {
            return Ok(());
        }

        let output_order = self.output_order.clone();
        for (device_id, crtc) in output_order {
            let Some(device) = self.devices.get_mut(&device_id) else {
                continue;
            };
            let NativeDevice {
                renderer, outputs, ..
            } = device;
            let Some(native_output) = outputs.get_mut(&crtc) else {
                continue;
            };
            if native_output.pending_frame {
                continue;
            }
            let elements = render::output_elements(
                renderer,
                canvas,
                &native_output.output,
                cursor,
                help_menu,
                lock_surfaces,
                locked,
            );

            let render_result = native_output.drm_output.render_frame(
                renderer,
                &elements,
                background,
                FrameFlags::empty(),
            );
            let frame_result = match render_result {
                Ok(frame_result) => frame_result,
                Err(error) => {
                    warn!(?error, output = %native_output.output.name(), "failed to render DRM output");
                    native_output.drm_output.reset_buffers();
                    native_output.pending_frame = false;
                    continue;
                }
            };

            if frame_result.is_empty {
                native_output.pending_frame = false;
                continue;
            }

            if frame_result.needs_sync()
                && let PrimaryPlaneElement::Swapchain(primary) = &frame_result.primary_element
            {
                let _ = primary.sync.wait();
            }

            let mut feedback = OutputPresentationFeedback::new(&native_output.output);
            let canvas_space = canvas.space();
            for window in canvas_space.elements() {
                window.take_presentation_feedback(
                    &mut feedback,
                    |_, _| Some(native_output.output.clone()),
                    |_, _| smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::empty(),
                );
            }
            let layer_map = layer_map_for_output(&native_output.output);
            for layer in layer_map.layers() {
                layer.take_presentation_feedback(
                    &mut feedback,
                    |_, _| Some(native_output.output.clone()),
                    |_, _| smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::empty(),
                );
            }
            native_output.pending_feedback = Some(feedback);

            match native_output.drm_output.queue_frame(()) {
                Ok(()) => native_output.pending_frame = true,
                Err(FrameError::EmptyFrame) => native_output.pending_frame = false,
                Err(error) => {
                    warn!(?error, output = %native_output.output.name(), "failed to queue DRM frame");
                    native_output.drm_output.reset_buffers();
                    native_output.pending_frame = false;
                }
            }
        }

        Ok(())
    }

    fn add_device(
        &mut self,
        device_id: u64,
        path: PathBuf,
        display_handle: &DisplayHandle,
        loop_handle: &LoopHandle<SayukiState>,
    ) -> Result<NativeOutputUpdates, Box<dyn Error>> {
        let mut session = self.session.clone();
        let fd = session.open(&path, OFlags::RDWR | OFlags::CLOEXEC)?;
        let drm_fd = DrmDeviceFd::new(DeviceFd::from(fd));
        let (drm_device, notifier) = DrmDevice::new(drm_fd.clone(), true)?;
        let gbm = GbmDevice::new(drm_fd.clone())?;
        let renderer = create_gles_renderer(gbm.clone())?;
        let renderer_formats =
            <GlesRenderer as Bind<Dmabuf>>::supported_formats(&renderer).unwrap_or_default();
        let allocator = GbmAllocator::new(
            gbm.clone(),
            GbmBufferFlags::SCANOUT | GbmBufferFlags::RENDERING,
        );
        let exporter = GbmFramebufferExporter::new(gbm.clone(), None);
        let manager = DrmOutputManager::new(
            drm_device,
            allocator,
            exporter,
            Some(gbm.clone()),
            SUPPORTED_FORMATS.iter().copied(),
            renderer_formats,
        );

        let loop_handle_clone = loop_handle.clone();
        let notifier_token =
            loop_handle.insert_source(notifier, move |event, metadata, state| {
                if let Err(error) =
                    state.handle_drm_event(device_id, event, metadata.take(), &loop_handle_clone)
                {
                    error!(?error, "failed to handle DRM event");
                }
            })?;

        self.devices.insert(
            device_id,
            NativeDevice {
                path: path.clone(),
                notifier_token,
                scanner: DrmScanner::new(),
                manager,
                renderer,
                outputs: HashMap::new(),
            },
        );
        info!(device_id, path = ?path, "added DRM device");

        self.scan_connectors(device_id, display_handle)
    }

    fn scan_connectors(
        &mut self,
        device_id: u64,
        display_handle: &DisplayHandle,
    ) -> Result<NativeOutputUpdates, Box<dyn Error>> {
        let scan_result = {
            let Some(device) = self.devices.get_mut(&device_id) else {
                warn!(device_id, "connector scan ignored for unknown DRM device");
                return Ok(NativeOutputUpdates::default());
            };
            device.scanner.scan_connectors(device.manager.device())?
        };

        let mut updates = NativeOutputUpdates::default();
        for event in scan_result.iter() {
            match event {
                DrmScanEvent::Connected {
                    connector,
                    crtc: Some(crtc),
                } => {
                    if self
                        .devices
                        .get(&device_id)
                        .and_then(|device| device.outputs.get(&crtc))
                        .is_some()
                    {
                        continue;
                    }
                    if let Some(output) =
                        self.create_native_output(device_id, connector, crtc, display_handle)?
                    {
                        updates.added.push(output.output.clone());
                        self.output_order.push((device_id, crtc));
                        if let Some(device) = self.devices.get_mut(&device_id) {
                            device.outputs.insert(crtc, output);
                        }
                    }
                }
                DrmScanEvent::Connected {
                    connector,
                    crtc: None,
                } => {
                    warn!(connector = %connector, "connected DRM connector has no available CRTC");
                }
                DrmScanEvent::Disconnected { connector, crtc } => {
                    if let Some(removed) = self.remove_output(device_id, connector.handle(), crtc) {
                        updates.removed.push(removed);
                    }
                }
            }
        }

        Ok(updates)
    }

    fn create_native_output(
        &mut self,
        device_id: u64,
        connector: connector::Info,
        crtc: crtc::Handle,
        display_handle: &DisplayHandle,
    ) -> Result<Option<NativeOutput>, Box<dyn Error>> {
        let Some(selected_mode) = connector
            .modes()
            .iter()
            .copied()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| connector.modes().first().copied())
        else {
            warn!(connector = %connector, "connected DRM connector has no modes");
            return Ok(None);
        };

        let location = self.next_output_location();
        let Some(device) = self.devices.get_mut(&device_id) else {
            warn!(device_id, connector = %connector, "output creation ignored for unknown DRM device");
            return Ok(None);
        };

        let output_name = connector.to_string();
        let (width_mm, height_mm) = connector.size().unwrap_or((0, 0));
        let display_info = display_info::for_connector(device.manager.device(), connector.handle());
        let make = display_info
            .as_ref()
            .and_then(|info| info.make())
            .unwrap_or_else(|| "Sayuki".into());
        let model = display_info
            .as_ref()
            .and_then(|info| info.model())
            .unwrap_or_else(|| output_name.clone());
        let output = Output::new(
            output_name.clone(),
            PhysicalProperties {
                size: (width_mm as i32, height_mm as i32).into(),
                subpixel: connector.subpixel().into(),
                make,
                model,
            },
        );
        for mode in connector.modes() {
            output.add_mode((*mode).into());
        }
        let output_mode = selected_mode.into();
        output.set_preferred(output_mode);
        output.change_current_state(
            Some(output_mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some(location),
        );

        let render_elements: DrmOutputRenderElements<
            GlesRenderer,
            WaylandSurfaceRenderElement<GlesRenderer>,
        > = DrmOutputRenderElements::new();
        let drm_output = device.manager.initialize_output(
            crtc,
            selected_mode,
            &[connector.handle()],
            &output,
            None,
            &mut device.renderer,
            &render_elements,
        )?;
        let global = output.create_global::<SayukiState>(display_handle);

        info!(device_id, connector = %connector, crtc = ?crtc, output = %output_name, "created DRM output");
        Ok(Some(NativeOutput {
            output,
            global,
            drm_output,
            connector: connector.handle(),
            pending_frame: false,
            pending_feedback: None,
        }))
    }

    fn remove_output(
        &mut self,
        device_id: u64,
        connector: connector::Handle,
        crtc: Option<crtc::Handle>,
    ) -> Option<(Output, GlobalId)> {
        let device = self.devices.get_mut(&device_id)?;
        let crtc = crtc.or_else(|| {
            device.outputs.iter().find_map(|(candidate_crtc, output)| {
                (output.connector == connector).then_some(*candidate_crtc)
            })
        })?;
        let native_output = device.outputs.remove(&crtc)?;
        self.output_order
            .retain(|(ordered_device_id, ordered_crtc)| {
                *ordered_device_id != device_id || *ordered_crtc != crtc
            });
        info!(device_id, ?crtc, output = %native_output.output.name(), "removed DRM output");
        Some((native_output.output, native_output.global))
    }

    fn remove_device(
        &mut self,
        device_id: u64,
        display_handle: &DisplayHandle,
        loop_handle: &LoopHandle<SayukiState>,
        wm: &mut WindowManager,
        pending_removals: &mut Vec<(GlobalId, Instant)>,
    ) {
        let Some(device) = self.devices.remove(&device_id) else {
            warn!(device_id, "DRM device removal ignored for unknown device");
            return;
        };

        loop_handle.remove(device.notifier_token);
        self.output_order
            .retain(|(ordered_device_id, _)| *ordered_device_id != device_id);
        for (_, native_output) in device.outputs {
            wm.unmap_output_all(&native_output.output);
            queue_global_removal(display_handle, pending_removals, native_output.global);
        }
        info!(device_id, path = ?device.path, "removed DRM device");
    }

    fn next_output_location(&self) -> Point<i32, Logical> {
        let right_edge = self
            .output_order
            .iter()
            .filter_map(|(device_id, crtc)| {
                self.devices
                    .get(device_id)
                    .and_then(|device| device.outputs.get(crtc))
                    .and_then(|output| {
                        output.output.current_mode().map(|mode| {
                            output.output.current_location().x + mode.size.to_logical(1).w
                        })
                    })
            })
            .max()
            .unwrap_or(0);

        (right_edge, 0).into()
    }
}

fn apply_output_updates(
    display_handle: &DisplayHandle,
    wm: &mut WindowManager,
    pending_removals: &mut Vec<(GlobalId, Instant)>,
    updates: NativeOutputUpdates,
) {
    for (output, global) in updates.removed {
        wm.unmap_output_all(&output);
        queue_global_removal(display_handle, pending_removals, global);
    }

    for output in updates.added {
        wm.map_output_active(&output);
    }
}

fn queue_global_removal(
    display_handle: &DisplayHandle,
    pending_removals: &mut Vec<(GlobalId, Instant)>,
    global: GlobalId,
) {
    display_handle.disable_global::<SayukiState>(global.clone());
    pending_removals.push((global, Instant::now() + OUTPUT_GLOBAL_REMOVAL_DELAY));
}

#[allow(unsafe_code)]
fn create_gles_renderer(gbm: GbmDevice<DrmDeviceFd>) -> Result<GlesRenderer, Box<dyn Error>> {
    // SAFETY: `gbm` is a live GBM device created from an open DRM fd owned by this backend,
    // and the resulting EGL display/context/renderer are kept on the event-loop thread.
    let egl_display = unsafe { EGLDisplay::new(gbm)? };
    let egl_context = EGLContext::new(&egl_display)?;
    // SAFETY: the EGL context was just created by Smithay for this thread and is not current elsewhere.
    unsafe { GlesRenderer::new(egl_context) }.map_err(Into::into)
}
