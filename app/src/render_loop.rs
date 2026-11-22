//! Event-driven frame scheduling on the Dock's UI thread.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CancelWaitableTimer, CreateWaitableTimerExW,
    SetWaitableTimer, TIMER_ALL_ACCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows::core::PCWSTR;

pub const TIMER_FRAME: usize = 1;

pub struct FrameScheduler {
    hwnd: HWND,
    timer: Option<HANDLE>,
    period: Duration,
    deadline: Option<Instant>,
    last_frame: Instant,
    fallback_active: bool,
}

impl FrameScheduler {
    pub fn new(hwnd: HWND, refresh_hz: u32) -> Self {
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .inspect_err(|error| tracing::warn!(%error, "using WM_TIMER for animation"))
        .ok();
        Self {
            hwnd,
            timer,
            period: period(refresh_hz),
            deadline: None,
            last_frame: Instant::now(),
            fallback_active: false,
        }
    }

    pub fn active(&self) -> bool {
        self.deadline.is_some()
    }

    pub fn fallback_active(&self) -> bool {
        self.fallback_active
    }

    pub fn wait_handle(&self) -> Option<HANDLE> {
        self.deadline.and(self.timer)
    }

    pub fn set_refresh_rate(&mut self, refresh_hz: u32) {
        let period = period(refresh_hz);
        if self.period != period {
            let active = self.active();
            self.stop();
            self.period = period;
            if active {
                self.start();
            }
        }
    }

    pub fn start(&mut self) {
        if self.active() {
            return;
        }
        self.last_frame = Instant::now();
        self.deadline = Some(self.last_frame + self.period);
        self.arm();
    }

    pub fn begin_frame(&mut self, now: Instant) -> f32 {
        let delta = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;
        delta.as_secs_f32().min(0.034)
    }

    pub fn continue_frames(&mut self) {
        if let Some(deadline) = self.deadline
            && self.timer.is_some()
        {
            self.deadline = Some(next_deadline(deadline, self.period, Instant::now()));
            self.arm();
        } else if !self.active() {
            self.start();
        }
    }

    fn arm(&mut self) {
        let Some(deadline) = self.deadline else {
            return;
        };
        if let Some(timer) = self.timer {
            let delay = deadline.saturating_duration_since(Instant::now());
            // A negative due time is relative, in units of 100 ns. Keep the
            // absolute schedule on Instant so lateness doesn't accumulate.
            let due_time = -(delay.as_nanos().div_ceil(100).max(1) as i64);
            match unsafe { SetWaitableTimer(timer, &due_time, 0, None, None, false) } {
                Ok(()) => return,
                Err(error) => tracing::warn!(%error, "could not arm frame timer"),
            }
            self.use_fallback();
        } else {
            self.start_fallback();
        }
    }

    pub fn use_fallback(&mut self) {
        if let Some(timer) = self.timer.take() {
            unsafe {
                let _ = CancelWaitableTimer(timer);
                let _ = CloseHandle(timer);
            }
        }
        if self.active() {
            self.start_fallback();
        }
    }

    fn start_fallback(&mut self) {
        let interval_ms = (self.period.as_secs_f64() * 1000.0).round() as u32;
        self.fallback_active =
            unsafe { SetTimer(Some(self.hwnd), TIMER_FRAME, interval_ms, None) } != 0;
        if !self.fallback_active {
            tracing::error!("could not start WM_TIMER for animation");
            self.deadline = None;
        }
    }

    pub fn stop(&mut self) {
        if self.deadline.take().is_some() {
            unsafe {
                if let Some(timer) = self.timer {
                    let _ = CancelWaitableTimer(timer);
                }
                if self.fallback_active {
                    let _ = KillTimer(Some(self.hwnd), TIMER_FRAME);
                }
            }
            self.fallback_active = false;
        }
    }
}

impl Drop for FrameScheduler {
    fn drop(&mut self) {
        self.stop();
        self.use_fallback();
    }
}

fn period(hz: u32) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(hz.max(1)))
}

fn next_deadline(deadline: Instant, period: Duration, now: Instant) -> Instant {
    if now < deadline {
        return deadline;
    }
    let late = now.duration_since(deadline);
    let remainder = Duration::from_nanos((late.as_nanos() % period.as_nanos()) as u64);
    now + period - remainder
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_stays_on_the_original_cadence_and_skips_expired_slots() {
        let first = Instant::now();
        let interval = period(144);
        assert_eq!(next_deadline(first, interval, first), first + interval);
        assert_eq!(
            next_deadline(
                first,
                interval,
                first + 4 * interval + Duration::from_millis(2)
            ),
            first + 5 * interval,
        );
    }
}
