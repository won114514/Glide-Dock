//! Damped spring animation.
//!
//! Every animated scalar (icon scale, tooltip opacity, menu highlight) is a
//! spring. Springs integrate with a fixed 1/240 s substep and are considered
//! settled once they are visually indistinguishable from their target; at that
//! point the application stops all timers.

/// Default integration substep (seconds).
const SUBSTEP: f32 = 1.0 / 240.0;
/// Largest frame delta we integrate in one go. Bigger deltas (sleep, drag) are
/// clamped so the dock never jumps.
const MAX_DT: f32 = 0.034;
/// Position tolerance for "settled".
const POSITION_EPSILON: f32 = 0.001;
/// Velocity tolerance for "settled".
const VELOCITY_EPSILON: f32 = 0.01;

/// A one dimensional damped spring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    /// Current value.
    pub value: f32,
    /// Spring target.
    pub target: f32,
    /// Current velocity.
    pub velocity: f32,
}

impl Spring {
    /// Creates a spring at rest on `value`.
    #[must_use]
    pub const fn new(value: f32) -> Self {
        Self {
            value,
            target: value,
            velocity: 0.0,
        }
    }

    /// Sets a new target. No-op when the target is unchanged.
    pub fn set_target(&mut self, target: f32) {
        if (self.target - target).abs() > f32::EPSILON {
            self.target = target;
        }
    }

    /// Snaps to the target and stops.
    pub fn settle(&mut self) {
        self.value = self.target;
        self.velocity = 0.0;
    }

    /// Returns `true` when the spring is visually at rest.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < POSITION_EPSILON
            && self.velocity.abs() < VELOCITY_EPSILON
    }

    /// Advances the spring by `dt` seconds.
    ///
    /// `stiffness` is the spring constant `k`, `damping` the damping ratio
    /// `zeta`. Returns `true` while the spring still needs frames.
    pub fn step(&mut self, dt: f32, stiffness: f32, damping: f32) -> bool {
        let dt = dt.clamp(0.0, MAX_DT);
        let stiffness = stiffness.max(1.0);
        let damping = damping.clamp(0.05, 2.0);
        let omega = stiffness.sqrt();
        let drag = 2.0 * damping * omega;

        let mut remaining = dt;
        while remaining > 0.0 {
            let h = remaining.min(SUBSTEP);
            let acceleration = -stiffness * (self.value - self.target) - drag * self.velocity;
            self.velocity += acceleration * h;
            self.value += self.velocity * h;
            remaining -= h;
        }

        if !self.value.is_finite() || !self.velocity.is_finite() || self.is_settled() {
            self.settle();
        }
        !self.is_settled()
    }
}

/// A vector of springs, one per dock item.
#[derive(Debug, Clone, Default)]
pub struct SpringSet {
    springs: Vec<Spring>,
}

impl SpringSet {
    /// Creates `count` springs at rest on `1.0`.
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self {
            springs: vec![Spring::new(1.0); count],
        }
    }

    /// Returns the number of springs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.springs.len()
    }

    /// Returns `true` when there are no springs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.springs.is_empty()
    }

    /// Resizes the set, new springs start at rest on `1.0`.
    pub fn resize(&mut self, count: usize) {
        self.springs.resize(count, Spring::new(1.0));
    }

    /// Inserts a spring at `index`, starting at `value`. Used for newly added
    /// items so they grow into place.
    pub fn insert(&mut self, index: usize, value: f32) {
        let index = index.min(self.springs.len());
        self.springs.insert(index, Spring::new(value));
    }

    /// Removes the spring at `index`.
    pub fn remove(&mut self, index: usize) {
        if index < self.springs.len() {
            self.springs.remove(index);
        }
    }

    /// Reorders springs following `order`, where `order[new_index]` is the old
    /// index. Keeps animation velocities with their items when items reorder.
    pub fn permute(&mut self, order: &[usize]) {
        let old = self.springs.clone();
        self.springs = order
            .iter()
            .filter_map(|index| old.get(*index).copied())
            .collect();
    }

    /// Returns a slice of the springs.
    #[must_use]
    pub fn springs(&self) -> &[Spring] {
        &self.springs
    }

    /// Returns a slice of current values.
    #[must_use]
    pub fn values(&self) -> Vec<f32> {
        self.springs.iter().map(|spring| spring.value).collect()
    }

    /// Applies new targets. Missing targets reset to `1.0`.
    pub fn set_targets(&mut self, targets: &[f32]) {
        for (index, spring) in self.springs.iter_mut().enumerate() {
            spring.set_target(targets.get(index).copied().unwrap_or(1.0));
        }
    }

    /// Advances every spring. Returns `true` while anything is moving.
    pub fn step(&mut self, dt: f32, stiffness: f32, damping: f32) -> bool {
        let mut animating = false;
        for spring in &mut self.springs {
            animating |= spring.step(dt, stiffness, damping);
        }
        animating
    }

    /// Returns `true` when every spring is at rest.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.springs.iter().all(Spring::is_settled)
    }

    /// Snaps every spring to its target.
    pub fn settle_all(&mut self) {
        for spring in &mut self.springs {
            spring.settle();
        }
    }

    /// Gives one spring an impulse, e.g. for the launch pop.
    pub fn impulse(&mut self, index: usize, velocity: f32, target: f32) {
        if let Some(spring) = self.springs.get_mut(index) {
            spring.set_target(target);
            spring.velocity += velocity;
        }
    }
}

/// A single named spring used for fades and other scalar effects.
#[derive(Debug, Clone, Copy)]
pub struct ScalarSpring {
    /// Underlying spring.
    pub spring: Spring,
    /// Stiffness used when stepping.
    pub stiffness: f32,
    /// Damping ratio used when stepping.
    pub damping: f32,
}

impl ScalarSpring {
    /// Creates a spring at rest on `value`.
    #[must_use]
    pub fn new(value: f32, stiffness: f32, damping: f32) -> Self {
        Self {
            spring: Spring::new(value),
            stiffness,
            damping,
        }
    }

    /// Current value.
    #[must_use]
    pub fn value(&self) -> f32 {
        self.spring.value
    }

    /// Sets the target.
    pub fn set_target(&mut self, target: f32) {
        self.spring.set_target(target);
    }

    /// Advances the spring.
    pub fn step(&mut self, dt: f32) -> bool {
        self.spring.step(dt, self.stiffness, self.damping)
    }

    /// Returns `true` when at rest.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.spring.is_settled()
    }

    /// Snaps to target.
    pub fn settle(&mut self) {
        self.spring.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges_and_overshoots_slightly() {
        let mut spring = Spring::new(1.0);
        spring.set_target(1.35);
        let mut peak = 1.0f32;
        let mut frames = 0;
        for _ in 0..400 {
            spring.step(1.0 / 60.0, 560.0, 0.72);
            peak = peak.max(spring.value);
            frames += 1;
            if spring.is_settled() {
                break;
            }
        }
        assert!(frames < 90, "settled after {frames} frames");
        assert!((spring.value - 1.35).abs() < 0.01);
        assert!(peak > 1.35, "expected a little overshoot, peak={peak}");
        assert!(peak < 1.5, "overshoot too large, peak={peak}");
    }

    #[test]
    fn spring_handles_large_dt_without_nan() {
        let mut spring = Spring::new(0.0);
        spring.set_target(1.0);
        spring.step(1.0, 560.0, 0.72);
        assert!(spring.value.is_finite());
        assert!(spring.velocity.is_finite());
    }

    #[test]
    fn spring_set_tracks_targets() {
        let mut set = SpringSet::new(3);
        set.set_targets(&[1.2, 1.0, 1.35]);
        for _ in 0..240 {
            if !set.step(1.0 / 60.0, 560.0, 0.72) {
                break;
            }
        }
        assert!(set.is_settled());
        let values = set.values();
        assert!((values[0] - 1.2).abs() < 0.01);
        assert!((values[2] - 1.35).abs() < 0.01);
    }

    #[test]
    fn resize_keeps_existing_values() {
        let mut set = SpringSet::new(1);
        set.springs[0].value = 1.2;
        set.resize(3);
        assert_eq!(set.len(), 3);
        assert!((set.springs()[0].value - 1.2).abs() < f32::EPSILON);
        assert!((set.springs()[2].value - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn scalar_spring_settles() {
        let mut spring = ScalarSpring::new(0.0, 460.0, 0.82);
        spring.set_target(1.0);
        for _ in 0..120 {
            if !spring.step(1.0 / 60.0) {
                break;
            }
        }
        assert!(spring.is_settled());
        assert!((spring.value() - 1.0).abs() < 0.01);
    }
}
