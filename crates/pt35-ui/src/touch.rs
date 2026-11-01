//! One finger on the glass, told apart as a tap, a pan or a long press.
//!
//! Pure state, so it is tested on any host. The model is foot's: nothing acts
//! on touch down. A finger that lifts before it moves is a tap, one that moves
//! past [`SLOP`] pans until it lifts, and one held still for [`LONG_PRESS`] is
//! a long press. A pan or a long press never ends in a tap.

use std::time::{Duration, Instant};

/// How far a finger may wander and still be a tap. About 1.3 mm on the panel,
/// above the GT911's jitter.
pub const SLOP: f64 = 12.0;

/// How long a still finger takes to become a long press.
pub const LONG_PRESS: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Held,
    Panning,
    Spent,
}

#[derive(Debug, Clone)]
pub struct Finger {
    id: i32,
    start: (f64, f64),
    last: (f64, f64),
    down: Instant,
    phase: Phase,
}

impl Finger {
    pub fn new(id: i32, at: (f64, f64), now: Instant) -> Self {
        Self {
            id,
            start: at,
            last: at,
            down: now,
            phase: Phase::Held,
        }
    }

    pub fn id(&self) -> i32 {
        self.id
    }

    /// Where the finger came down. A tap and a long press act there.
    pub fn start(&self) -> (f64, f64) {
        self.start
    }

    /// The finger moved to `at`. Once it pans, the distance since the last
    /// move, the first one measured from where it came down.
    pub fn moved(&mut self, at: (f64, f64)) -> Option<(f64, f64)> {
        match self.phase {
            Phase::Held => {
                let (dx, dy) = (at.0 - self.start.0, at.1 - self.start.1);
                if dx.hypot(dy) <= SLOP {
                    return None;
                }
                self.phase = Phase::Panning;
            }
            Phase::Panning => {}
            Phase::Spent => return None,
        }
        let delta = (at.0 - self.last.0, at.1 - self.last.1);
        self.last = at;
        Some(delta)
    }

    /// True if lifting the finger now is a tap.
    pub fn is_tap(&self) -> bool {
        self.phase == Phase::Held
    }

    /// When a long press is due, while the finger can still be one.
    pub fn long_press_at(&self) -> Option<Instant> {
        (self.phase == Phase::Held).then(|| self.down + LONG_PRESS)
    }

    /// The long press fired: the rest of this touch does nothing.
    pub fn spend(&mut self) {
        self.phase = Phase::Spent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finger() -> Finger {
        Finger::new(0, (100.0, 100.0), Instant::now())
    }

    #[test]
    fn a_finger_that_lifts_where_it_landed_is_a_tap() {
        let mut f = finger();
        assert_eq!(f.moved((105.0, 104.0)), None, "jitter is not a pan");
        assert!(f.is_tap());
    }

    #[test]
    fn a_pan_reports_each_step_and_never_taps() {
        let mut f = finger();
        assert_eq!(f.moved((100.0, 80.0)), Some((0.0, -20.0)));
        assert_eq!(f.moved((100.0, 70.0)), Some((0.0, -10.0)));
        assert_eq!(
            f.moved((100.0, 70.5)),
            Some((0.0, 0.5)),
            "no slop once panning"
        );
        assert!(!f.is_tap());
        assert_eq!(f.long_press_at(), None);
    }

    #[test]
    fn a_long_press_spends_the_touch() {
        let now = Instant::now();
        let mut f = Finger::new(0, (10.0, 10.0), now);
        assert_eq!(f.long_press_at(), Some(now + LONG_PRESS));
        f.spend();
        assert!(!f.is_tap());
        assert_eq!(f.moved((200.0, 200.0)), None);
        assert_eq!(f.long_press_at(), None);
    }
}
