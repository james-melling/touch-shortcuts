//! Tap queue rules (pure; the worker thread owns one instance).
//!
//! - At most 4 pending taps; extra taps are dropped (`dropped-queue-full`).
//! - Pending taps older than 750 ms are dropped when dequeued (stale).
//! - A second tap on the running button within 300 ms of its start queues
//!   (double tap = run twice); a later tap cancels the running repeat.
//! - Stop-all cancels the running action and clears the queue.

use std::collections::VecDeque;

pub const MAX_PENDING: usize = 4;
pub const STALE_MS: u64 = 750;
pub const DOUBLE_TAP_MS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapResult { Queued, Dropped, CancelRunning }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending<T> {
    pub button_id: String,
    pub tapped_at: u64,
    pub action: T,
}

#[derive(Debug)]
pub struct TapQueue<T> {
    pending: VecDeque<Pending<T>>,
    running: Option<(String, u64)>,
    pub cancel_requested: bool,
    pub stale_dropped: Vec<String>,
}

impl<T> Default for TapQueue<T> {
    fn default() -> Self {
        Self { pending: VecDeque::new(), running: None, cancel_requested: false, stale_dropped: Vec::new() }
    }
}

impl<T> TapQueue<T> {
    pub fn tap(&mut self, button_id: &str, action: T, now: u64) -> TapResult {
        if let Some((running, started)) = &self.running {
            if running == button_id && now.saturating_sub(*started) >= DOUBLE_TAP_MS {
                self.cancel_requested = true;
                return TapResult::CancelRunning;
            }
        }
        if self.pending.len() >= MAX_PENDING {
            return TapResult::Dropped;
        }
        self.pending.push_back(Pending { button_id: button_id.to_string(), tapped_at: now, action });
        TapResult::Queued
    }

    /// Next action to run, skipping stale taps. Marks it running.
    pub fn start_next(&mut self, now: u64) -> Option<Pending<T>> {
        while let Some(p) = self.pending.pop_front() {
            if now.saturating_sub(p.tapped_at) > STALE_MS {
                self.stale_dropped.push(p.button_id);
                continue;
            }
            self.running = Some((p.button_id.clone(), now));
            return Some(p);
        }
        None
    }

    pub fn finish(&mut self) {
        self.running = None;
        self.cancel_requested = false;
    }

    pub fn stop_all(&mut self) {
        self.pending.clear();
        if self.running.is_some() {
            self.cancel_requested = true;
        }
    }

    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_queue_in_order_up_to_the_cap() {
        let mut q = TapQueue::default();
        for (i, id) in ["a", "b", "c", "d"].iter().enumerate() {
            assert_eq!(q.tap(id, i, 0), TapResult::Queued);
        }
        assert_eq!(q.tap("e", 4, 0), TapResult::Dropped);
        let order: Vec<String> = std::iter::from_fn(|| { let p = q.start_next(10); q.finish(); p }).map(|p| p.button_id).collect();
        assert_eq!(order, ["a", "b", "c", "d"]);
    }

    #[test]
    fn stale_taps_are_skipped_and_recorded() {
        let mut q = TapQueue::default();
        q.tap("old", 0, 0);
        q.tap("new", 1, 600);
        let p = q.start_next(800).unwrap();
        assert_eq!(p.button_id, "new");
        assert_eq!(q.stale_dropped, vec!["old".to_string()]);
    }

    #[test]
    fn double_tap_on_running_button_queues_a_second_run() {
        let mut q = TapQueue::default();
        q.tap("down10", 0, 1000);
        q.start_next(1000).unwrap();
        assert_eq!(q.tap("down10", 1, 1200), TapResult::Queued);
        assert!(!q.cancel_requested);
        assert_eq!(q.pending_len(), 1);
    }

    #[test]
    fn later_tap_on_running_button_cancels_it() {
        let mut q = TapQueue::default();
        q.tap("down10", 0, 1000);
        q.start_next(1000).unwrap();
        assert_eq!(q.tap("down10", 1, 1400), TapResult::CancelRunning);
        assert!(q.cancel_requested);
        assert_eq!(q.pending_len(), 0);
    }

    #[test]
    fn tap_on_other_button_while_running_queues() {
        let mut q = TapQueue::default();
        q.tap("down10", 0, 0);
        q.start_next(0).unwrap();
        assert_eq!(q.tap("enter", 1, 900), TapResult::Queued);
        assert!(!q.cancel_requested);
    }

    #[test]
    fn finish_clears_running_and_cancel_flag() {
        let mut q = TapQueue::default();
        q.tap("a", 0, 0);
        q.start_next(0).unwrap();
        q.tap("a", 1, 500);
        assert!(q.cancel_requested);
        q.finish();
        assert!(!q.cancel_requested);
        assert_eq!(q.tap("a", 2, 600), TapResult::Queued);
    }

    #[test]
    fn stop_all_cancels_running_and_clears_queue() {
        let mut q = TapQueue::default();
        q.tap("a", 0, 0);
        q.start_next(0).unwrap();
        q.tap("b", 1, 10);
        q.tap("c", 2, 20);
        q.stop_all();
        assert!(q.cancel_requested);
        assert_eq!(q.pending_len(), 0);
    }

    #[test]
    fn clear_pending_keeps_the_running_action() {
        let mut q = TapQueue::default();
        q.tap("a", 0, 0);
        q.start_next(0).unwrap();
        q.tap("b", 1, 10);
        q.clear_pending();
        assert_eq!(q.pending_len(), 0);
        assert!(!q.cancel_requested);
    }
}
