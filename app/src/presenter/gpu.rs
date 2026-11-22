//! UI-thread swap chain and DirectComposition lifetime.

use dock_render::DockFrame;
use windows::Win32::Foundation::{E_FAIL, HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_WAS_STILL_DRAWING, DXGI_PRESENT_DO_NOT_WAIT,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIDevice,
    IDXGIDevice1, IDXGIFactory2, IDXGIOutput, IDXGISurface, IDXGISwapChain1,
};
use windows::core::{Error, IUnknown, Interface, Result};

use super::d2d::D2dPainter;

pub(super) struct GpuSurface {
    device: ID3D11Device,
    composition: IDCompositionDevice,
    target: IDCompositionTarget,
    visual: IDCompositionVisual,
    swap_chain: IDXGISwapChain1,
    painter: D2dPainter,
    size: (u32, u32),
    attached: bool,
    fault: Option<TestFault>,
    submitted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TestFault {
    Init,
    Present,
    Busy,
}

impl GpuSurface {
    pub(super) fn new(
        hwnd: HWND,
        width: u32,
        height: u32,
        fault: Option<TestFault>,
    ) -> Result<Self> {
        if fault == Some(TestFault::Init) {
            return Err(Error::new(E_FAIL, "injected GPU initialization failure"));
        }
        // The single UI thread owns the D3D/D2D objects. No software D3D
        // fallback: using tiny-skia directly is the supported CPU path.
        unsafe {
            let mut device = None;
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?;
            let device = device.ok_or_else(|| Error::from_hresult(E_FAIL))?;
            let dxgi: IDXGIDevice = device.cast()?;
            let latency: IDXGIDevice1 = dxgi.cast()?;
            latency.SetMaximumFrameLatency(2)?;
            let adapter = dxgi.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;
            let description = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 3,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap_chain = factory.CreateSwapChainForComposition(
                &device,
                &description,
                None::<&IDXGIOutput>,
            )?;
            let mut painter = D2dPainter::new(&dxgi)?;
            painter.bind(&swap_chain.GetBuffer::<IDXGISurface>(0)?)?;
            // This device does not create DComp surfaces. Keeping it separate
            // from D3D lets us detach the visual even after a D3D device loss.
            let composition: IDCompositionDevice = DCompositionCreateDevice(None::<&IDXGIDevice>)?;
            let target = composition.CreateTargetForHwnd(hwnd, true)?;
            let visual = composition.CreateVisual()?;
            visual.SetContent(&swap_chain)?;
            if let Ok(description) = adapter.GetDesc() {
                let end = description
                    .Description
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(description.Description.len());
                let adapter = String::from_utf16_lossy(&description.Description[..end]);
                tracing::info!(%adapter, "D3D11 hardware device created");
            }
            Ok(Self {
                device,
                composition,
                target,
                visual,
                swap_chain,
                painter,
                size: (width, height),
                attached: false,
                fault,
                submitted: false,
            })
        }
    }

    pub(super) fn check_device(&self) -> Result<()> {
        unsafe { self.device.GetDeviceRemovedReason() }?;
        if !unsafe { self.composition.CheckDeviceState() }?.as_bool() {
            return Err(Error::new(
                DXGI_ERROR_DEVICE_REMOVED,
                "DirectComposition device lost",
            ));
        }
        Ok(())
    }

    /// Returns `false` on submission backpressure. The caller retains the
    /// latest scene and retries on its next scheduled frame.
    pub(super) fn present(&mut self, frame: &DockFrame) -> Result<bool> {
        if self.size != (frame.width, frame.height) {
            self.check_device()?;
            self.painter.unbind();
            unsafe {
                self.swap_chain.ResizeBuffers(
                    0,
                    frame.width,
                    frame.height,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )?;
                self.painter
                    .bind(&self.swap_chain.GetBuffer::<IDXGISurface>(0)?)?;
            }
            self.size = (frame.width, frame.height);
        }
        self.painter.draw(frame)?;
        if self.submitted && self.fault == Some(TestFault::Present) {
            return Err(Error::new(
                DXGI_ERROR_DEVICE_REMOVED,
                "injected GPU submission failure",
            ));
        }
        if self.fault == Some(TestFault::Busy) {
            self.fault = None;
            return Ok(false);
        }
        // Queue the newest image for the compositor without blocking the UI
        // thread behind a full present queue or waiting for vertical blank.
        let status = unsafe { self.swap_chain.Present(0, DXGI_PRESENT_DO_NOT_WAIT) };
        if status == DXGI_ERROR_WAS_STILL_DRAWING {
            return Ok(false);
        }
        status.ok()?;
        self.submitted = true;
        Ok(true)
    }

    pub(super) fn attach(&mut self) -> Result<()> {
        if !self.attached {
            unsafe {
                self.target.SetRoot(&self.visual)?;
                self.attached = true;
                self.composition.Commit()?;
            }
        }
        Ok(())
    }
}

impl Drop for GpuSurface {
    fn drop(&mut self) {
        self.painter.unbind();
        if self.attached {
            unsafe {
                let _ = self.target.SetRoot(None::<&IDCompositionVisual>);
                let _ = self.visual.SetContent(None::<&IUnknown>);
                if let Err(error) = self.composition.Commit() {
                    tracing::debug!(%error, "could not commit GPU detach; releasing composition target");
                }
            }
        }
    }
}
