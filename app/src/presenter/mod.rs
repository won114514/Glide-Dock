//! Main Dock presentation. A renderer failure switches this HWND to the CPU
//! for the remainder of the run. The tray menu continues to use its own DIB.

mod d2d;
mod gpu;
mod mask;

use dock_platform_windows::{LayeredSurface, PlatformError};
use dock_render::{DockFrame, Renderer};
use windows::Win32::Foundation::HWND;

use gpu::{GpuSurface, TestFault};
use mask::InputMask;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Submission {
    Presented,
    Busy,
    Failed,
}

pub(crate) struct DockPresenter {
    hwnd: HWND,
    surface: LayeredSurface,
    gpu: Option<GpuSurface>,
    // A renderer override or GPU fault keeps hardware disabled for this run.
    gpu_blocked: bool,
    input: InputMask,
}

impl DockPresenter {
    pub(crate) fn new(hwnd: HWND) -> Result<Self, PlatformError> {
        let forced_cpu =
            std::env::var("GLIDE_DOCK_RENDERER").is_ok_and(|mode| mode.eq_ignore_ascii_case("cpu"));
        if forced_cpu {
            tracing::info!("Dock renderer forced to tiny-skia");
        }
        Ok(Self {
            hwnd,
            surface: LayeredSurface::new(hwnd)?,
            gpu: None,
            gpu_blocked: forced_cpu,
            input: InputMask::default(),
        })
    }

    pub(crate) fn present(
        &mut self,
        renderer: &mut Renderer,
        frame: &DockFrame,
        gpu_requested: bool,
    ) -> Submission {
        if gpu_requested && !self.gpu_blocked && self.gpu.is_none() {
            match GpuSurface::new(self.hwnd, frame.width, frame.height, test_fault()) {
                Ok(gpu) => self.gpu = Some(gpu),
                Err(error) => {
                    self.gpu_blocked = true;
                    tracing::warn!(%error, "GPU initialization failed; using tiny-skia");
                }
            }
        }
        if gpu_requested && let Some(gpu) = &mut self.gpu {
            let submitted = gpu
                .present(frame)
                .map_err(|error| error.to_string())
                .and_then(|submitted| {
                    if submitted {
                        self.input
                            .present(&mut self.surface, frame)
                            .map_err(|error| error.to_string())?;
                        gpu.attach().map_err(|error| error.to_string())?;
                    }
                    Ok(submitted)
                });
            match submitted {
                Ok(true) => return Submission::Presented,
                Ok(false) => return Submission::Busy,
                Err(error) => {
                    self.gpu_blocked = true;
                    self.detach();
                    tracing::warn!(%error, "GPU frame failed; repainting this HWND with tiny-skia");
                }
            }
        }
        let Some(pixmap) = renderer.render_frame(frame) else {
            tracing::error!("could not allocate CPU Dock frame");
            return Submission::Failed;
        };
        match self
            .surface
            .present(pixmap.width(), pixmap.height(), pixmap.data())
        {
            Ok(()) => {
                // Keep the visual attached until its CPU replacement is ready.
                if self.gpu.is_some() {
                    self.detach();
                    tracing::info!("GPU acceleration disabled; using tiny-skia");
                }
                Submission::Presented
            }
            Err(error) => {
                tracing::error!(%error, "CPU frame submission failed");
                Submission::Failed
            }
        }
    }

    /// DirectComposition requests a paint after device loss, even when no
    /// animation is running. Return true if the window needs a CPU repaint.
    pub(crate) fn recover_on_paint(&mut self) -> bool {
        if let Some(gpu) = &self.gpu
            && let Err(error) = gpu.check_device()
        {
            self.gpu_blocked = true;
            self.detach();
            tracing::warn!(%error, "GPU device lost; repainting this HWND with tiny-skia");
            return true;
        }
        false
    }

    pub(crate) fn detach(&mut self) {
        self.gpu = None;
    }

    pub(crate) fn gpu_active(&self) -> bool {
        self.gpu.is_some()
    }
}

fn test_fault() -> Option<TestFault> {
    if !cfg!(debug_assertions) {
        return None;
    }
    match std::env::var("GLIDE_DOCK_GPU_TEST_FAULT").ok().as_deref() {
        Some("init") => Some(TestFault::Init),
        Some("present") => Some(TestFault::Present),
        Some("busy") => Some(TestFault::Busy),
        _ => None,
    }
}
