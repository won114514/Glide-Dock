//! Opt-in submission statistics for the manual refresh-rate acceptance run.

use std::time::Instant;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

use crate::presenter::Submission;

// One histogram bin per 0.1 ms. The last bin covers intervals >= 1 second.
const BIN_US: u128 = 100;
const BINS: usize = 10_001;

pub(super) struct FrameStats {
    posted_pointer: bool,
    cpu_start: Option<u64>,
    first: Option<Instant>,
    last: Option<Instant>,
    bins: Box<[u32]>,
    wakes: u32,
    submitted: u32,
    busy: u32,
}

impl FrameStats {
    pub(super) fn from_env() -> Option<Self> {
        let posted_pointer = match std::env::var("GLIDE_DOCK_FRAME_STATS").as_deref() {
            Ok("1") => false,
            Ok("posted") if cfg!(debug_assertions) => true,
            _ => return None,
        };
        Some(Self {
            posted_pointer,
            cpu_start: None,
            first: None,
            last: None,
            bins: vec![0; BINS].into_boxed_slice(),
            wakes: 0,
            submitted: 0,
            busy: 0,
        })
    }

    /// A benchmark posts mouse movement and leave messages itself.
    pub(super) fn uses_posted_pointer(&self) -> bool {
        self.posted_pointer
    }

    pub(super) fn begin_frame(&mut self) {
        if self.wakes == 0 {
            self.cpu_start = process_cpu_time();
        }
        self.wakes += 1;
    }

    pub(super) fn record(&mut self, result: Submission) {
        match result {
            Submission::Presented => {
                let now = Instant::now();
                self.first.get_or_insert(now);
                if let Some(last) = self.last.replace(now) {
                    let interval = now.duration_since(last).as_micros() / BIN_US;
                    self.bins[interval.min((BINS - 1) as u128) as usize] += 1;
                }
                self.submitted += 1;
            }
            Submission::Busy => self.busy += 1,
            Submission::Failed => {}
        }
    }

    pub(super) fn finish(&mut self, gpu: bool) {
        if self.wakes == 0 {
            return;
        }
        let intervals = self.submitted.saturating_sub(1);
        let elapsed = self.first.zip(self.last).map_or(0.0, |(first, last)| {
            last.duration_since(first).as_secs_f64()
        });
        let p95_ms = percentile(&self.bins, intervals) as f64 * BIN_US as f64 / 1000.0;
        let cpu_ms_per_frame =
            self.cpu_start
                .zip(process_cpu_time())
                .map_or(0.0, |(start, end)| {
                    (end.saturating_sub(start)) as f64 / 10_000.0 / f64::from(self.submitted.max(1))
                });
        tracing::info!(
            gpu,
            wakes = self.wakes,
            submitted = self.submitted,
            busy = self.busy,
            elapsed_s = elapsed,
            fps = f64::from(intervals) / elapsed.max(1e-9),
            p95_ms,
            cpu_ms_per_frame,
            "Dock frame run finished"
        );
        self.first = None;
        self.last = None;
        self.bins.fill(0);
        self.wakes = 0;
        self.submitted = 0;
        self.busy = 0;
    }
}

fn percentile(bins: &[u32], intervals: u32) -> usize {
    let rank = (u64::from(intervals) * 95).div_ceil(100) as u32;
    let mut total = 0;
    bins.iter()
        .position(|count| {
            total += count;
            total >= rank
        })
        .unwrap_or_default()
}

fn process_cpu_time() -> Option<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    }
    .ok()?;
    let ticks =
        |time: FILETIME| u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime);
    Some(ticks(kernel) + ticks(user))
}
