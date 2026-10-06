//! Decides which monitor is streamed, with a short dwell time so brushing past a screen edge
//! doesn't flip the stream back and forth.

use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct Selector {
    current: Option<String>,
    pending: Option<(String, Instant)>,
}

impl Selector {
    #[cfg(test)]
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// True while the cursor sits on another monitor and the dwell time hasn't passed yet.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Forgets the current choice, e.g. after the monitor layout changed.
    pub fn reset(&mut self) {
        self.current = None;
        self.pending = None;
    }

    /// Feeds the monitor under the cursor (`None` when the cursor is on no streamable monitor)
    /// and returns the monitor to stream.
    pub fn update(
        &mut self,
        under_cursor: Option<&str>,
        now: Instant,
        delay: Duration,
        locked: bool,
    ) -> Option<&str> {
        if locked && self.current.is_some() {
            self.pending = None;
            return self.current.as_deref();
        }
        let Some(candidate) = under_cursor else {
            self.pending = None;
            return self.current.as_deref();
        };
        if self.current.as_deref() == Some(candidate) {
            self.pending = None;
        } else if self.current.is_none() || delay.is_zero() {
            self.switch_to(candidate);
        } else {
            match &self.pending {
                Some((name, since)) if name == candidate => {
                    if now.duration_since(*since) >= delay {
                        self.switch_to(candidate);
                    }
                }
                _ => self.pending = Some((candidate.to_owned(), now)),
            }
        }
        self.current.as_deref()
    }

    fn switch_to(&mut self, name: &str) {
        self.current = Some(name.to_owned());
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELAY: Duration = Duration::from_millis(150);

    #[test]
    fn first_monitor_is_picked_immediately() {
        let mut s = Selector::default();
        let t = Instant::now();
        assert_eq!(s.update(Some("A"), t, DELAY, false), Some("A"));
    }

    #[test]
    fn switch_waits_for_dwell_time() {
        let mut s = Selector::default();
        let t = Instant::now();
        s.update(Some("A"), t, DELAY, false);
        assert_eq!(s.update(Some("B"), t, DELAY, false), Some("A"));
        assert!(s.is_pending());
        assert_eq!(s.update(Some("B"), t + Duration::from_millis(100), DELAY, false), Some("A"));
        assert_eq!(s.update(Some("B"), t + Duration::from_millis(150), DELAY, false), Some("B"));
        assert!(!s.is_pending());
    }

    #[test]
    fn brushing_past_an_edge_does_not_switch() {
        let mut s = Selector::default();
        let t = Instant::now();
        s.update(Some("A"), t, DELAY, false);
        s.update(Some("B"), t, DELAY, false);
        // Back on A before the delay: the pending switch is dropped.
        assert_eq!(s.update(Some("A"), t + Duration::from_millis(50), DELAY, false), Some("A"));
        assert_eq!(s.update(Some("B"), t + Duration::from_millis(160), DELAY, false), Some("A"));
    }

    #[test]
    fn moving_to_a_third_monitor_restarts_the_timer() {
        let mut s = Selector::default();
        let t = Instant::now();
        s.update(Some("A"), t, DELAY, false);
        s.update(Some("B"), t, DELAY, false);
        s.update(Some("C"), t + Duration::from_millis(100), DELAY, false);
        assert_eq!(s.update(Some("C"), t + Duration::from_millis(200), DELAY, false), Some("A"));
        assert_eq!(s.update(Some("C"), t + Duration::from_millis(250), DELAY, false), Some("C"));
    }

    #[test]
    fn zero_delay_switches_at_once() {
        let mut s = Selector::default();
        let t = Instant::now();
        s.update(Some("A"), t, Duration::ZERO, false);
        assert_eq!(s.update(Some("B"), t, Duration::ZERO, false), Some("B"));
    }

    #[test]
    fn lock_keeps_current_monitor() {
        let mut s = Selector::default();
        let t = Instant::now();
        s.update(Some("A"), t, DELAY, false);
        assert_eq!(s.update(Some("B"), t + Duration::from_secs(5), DELAY, true), Some("A"));
        // Lock with nothing selected yet still picks something.
        let mut fresh = Selector::default();
        assert_eq!(fresh.update(Some("B"), t, DELAY, true), Some("B"));
    }

    #[test]
    fn cursor_off_all_monitors_keeps_current() {
        let mut s = Selector::default();
        let t = Instant::now();
        assert_eq!(s.update(None, t, DELAY, false), None);
        s.update(Some("A"), t, DELAY, false);
        assert_eq!(s.update(None, t, DELAY, false), Some("A"));
        s.reset();
        assert_eq!(s.current(), None);
    }
}
