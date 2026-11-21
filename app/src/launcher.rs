//! Background launcher service.
//!
//! The service only starts applications off the UI thread and reports the
//! result. It deliberately knows nothing about animation state: the motion
//! engine is told about launches by `DockApp`, and the window tracker is the
//! only source of truth for a running application.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::{Arc, Condvar, Mutex};

use dock_core::DockItem;
use dock_platform_windows::{ComGuard, WM_APP_LAUNCH_RESULT, launch};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// One completed background launch request.
pub type LaunchResult = (u32, Result<(), String>);

const WORKER_COUNT: usize = 2;
const MAX_QUEUED_REQUESTS: usize = 16;
const QUEUE_FULL_MESSAGE: &str = "启动队列已满，请稍后再试";

struct QueueState {
    pending: VecDeque<DockItem>,
    shutdown: bool,
}

struct LaunchQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl LaunchQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                pending: VecDeque::with_capacity(MAX_QUEUED_REQUESTS),
                shutdown: false,
            }),
            ready: Condvar::new(),
        }
    }

    fn try_push(&self, item: DockItem) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.shutdown || state.pending.len() >= MAX_QUEUED_REQUESTS {
            return false;
        }
        state.pending.push_back(item);
        self.ready.notify_one();
        true
    }

    fn pop(&self) -> Option<DockItem> {
        let mut state = self.state.lock().ok()?;
        loop {
            if let Some(item) = state.pending.pop_front() {
                return Some(item);
            }
            if state.shutdown {
                return None;
            }
            state = self.ready.wait(state).ok()?;
        }
    }

    fn stop(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.shutdown = true;
            state.pending.clear();
        }
        self.ready.notify_all();
    }
}

type LaunchResults = Arc<Mutex<Vec<LaunchResult>>>;

/// Owns the background launch queue for one dock window.
pub struct LauncherService {
    hwnd: HWND,
    queue: Arc<LaunchQueue>,
    results: LaunchResults,
}

impl LauncherService {
    /// Creates a service that wakes `hwnd` with `WM_APP_LAUNCH_RESULT`.
    #[must_use]
    pub fn new(hwnd: HWND) -> Self {
        let queue = Arc::new(LaunchQueue::new());
        let results = Arc::new(Mutex::new(Vec::new()));
        for index in 0..WORKER_COUNT {
            let queue = Arc::clone(&queue);
            let results = Arc::clone(&results);
            let hwnd = hwnd.0 as isize;
            let _ = std::thread::Builder::new()
                .name(format!("glide-launch-{index}"))
                .spawn(move || worker_loop(queue, results, hwnd));
        }
        Self {
            hwnd,
            queue,
            results,
        }
    }

    /// Starts `item` on a background thread. The UI thread is never blocked by
    /// ShellExecuteEx/CreateProcessW.
    pub fn request(&self, item: &DockItem) {
        tracing::info!(
            id = item.id,
            target = %item.target_path,
            launch_type = ?item.launch_type,
            "launch requested"
        );
        let item = item.clone();
        if !self.queue.try_push(item.clone()) {
            tracing::warn!(id = item.id, "launch queue is full");
            publish_result(
                &self.results,
                self.hwnd.0 as isize,
                item.id,
                Err(QUEUE_FULL_MESSAGE.to_string()),
            );
        }
    }

    /// Drains completed launch results.
    #[must_use]
    pub fn drain(&self) -> Vec<LaunchResult> {
        self.results
            .lock()
            .map(|mut queue| queue.drain(..).collect())
            .unwrap_or_default()
    }
}

impl Drop for LauncherService {
    fn drop(&mut self) {
        self.queue.stop();
    }
}

fn worker_loop(queue: Arc<LaunchQueue>, results: LaunchResults, hwnd: isize) {
    let _com = ComGuard::sta();
    while let Some(item) = queue.pop() {
        let result = launch(&item);
        publish_result(&results, hwnd, item.id, result);
    }
}

fn publish_result(results: &LaunchResults, hwnd: isize, id: u32, result: Result<(), String>) {
    if let Ok(mut queue) = results.lock() {
        queue.push((id, result));
    }
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut c_void)),
            WM_APP_LAUNCH_RESULT,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_drain_is_empty() {
        let service = LauncherService::new(HWND::default());
        assert!(service.drain().is_empty());
    }

    #[test]
    fn queue_accepts_sixteen_pending_requests_and_rejects_the_next() {
        let queue = LaunchQueue::new();
        for id in 1..=MAX_QUEUED_REQUESTS as u32 {
            assert!(queue.try_push(DockItem {
                id,
                ..DockItem::default()
            }));
        }
        assert!(!queue.try_push(DockItem::default()));
    }

    #[test]
    fn stopping_queue_rejects_new_requests_and_discards_pending_work() {
        let queue = LaunchQueue::new();
        assert!(queue.try_push(DockItem::default()));
        queue.stop();
        assert!(!queue.try_push(DockItem::default()));
        assert!(queue.pop().is_none());
    }
}
