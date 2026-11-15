//! Background shortcut resolution.
//!
//! Resolving a `.lnk` touches COM and may hit a slow or disconnected network
//! path. The UI thread only submits the dropped paths; a dedicated STA worker
//! resolves them and posts the result back with [`WM_PATHS_RESOLVED`].

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::com::ComGuard;
use crate::shell::{ResolvedEntry, resolve};

/// Posted to the dock window when a batch of dropped paths has been resolved.
pub const WM_PATHS_RESOLVED: u32 = WM_APP + 4;

/// A batch of dropped paths waiting to be resolved.
#[derive(Debug, Clone)]
pub struct ResolveRequest {
    /// Paths exactly as received from `WM_DROPFILES`.
    pub paths: Vec<String>,
    /// Insertion index captured at drop time.
    pub at: Option<usize>,
}

/// Resolved batch produced by the worker.
#[derive(Debug, Clone)]
pub struct ResolveResult {
    /// Insertion index echoed from the request.
    pub at: Option<usize>,
    /// Resolved entries in drop order. Unresolvable paths still produce an
    /// entry whose target falls back to the original path.
    pub entries: Vec<ResolvedEntry>,
}

/// Handle to the background resolver thread.
pub struct ResolveService {
    sender: Sender<ResolveRequest>,
    receiver: Receiver<ResolveResult>,
}

impl ResolveService {
    /// Spawns the resolver worker. `hwnd` is used only to wake the UI thread.
    #[must_use]
    pub fn spawn(hwnd: HWND) -> Self {
        let (request_sender, request_receiver) = mpsc::channel::<ResolveRequest>();
        let (result_sender, result_receiver) = mpsc::channel::<ResolveResult>();
        let hwnd_value = hwnd.0 as isize;
        let spawn_result = thread::Builder::new()
            .name("glide-dock-resolver".to_string())
            .spawn(move || {
                let _com = ComGuard::sta();
                while let Ok(request) = request_receiver.recv() {
                    let entries = request
                        .paths
                        .iter()
                        .map(|path| resolve(path))
                        .filter(|entry| !entry.target_path.trim().is_empty())
                        .collect::<Vec<_>>();
                    if entries.is_empty() {
                        continue;
                    }
                    let _ = result_sender.send(ResolveResult {
                        at: request.at,
                        entries,
                    });
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(hwnd_value as *mut _)),
                            WM_PATHS_RESOLVED,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
            });
        if let Err(error) = spawn_result {
            tracing::error!(%error, "failed to spawn shortcut resolver");
        }
        Self {
            sender: request_sender,
            receiver: result_receiver,
        }
    }

    /// Queues a batch of dropped paths.
    pub fn request(&self, paths: Vec<String>, at: Option<usize>) {
        let _ = self.sender.send(ResolveRequest { paths, at });
    }

    /// Drains all batches that have finished resolving.
    #[must_use]
    pub fn drain(&self) -> Vec<ResolveResult> {
        self.receiver.try_iter().collect()
    }
}
