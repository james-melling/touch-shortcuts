//! Serial executor: runs one keys action against the foreground window.
//!
//! One press = one `SendInput` batch (cannot interleave with other input).
//! The target is captured at action start; before and after every batch the
//! foreground is compared with it. Platform access goes through traits so
//! every rule here is tested on any OS with recording fakes.

use crate::combo::{Combo, KeyEvent, press_events};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elevation { No, Yes, Unknown }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Window handle (HWND) as an opaque id.
    pub id: u64,
    pub title: String,
    /// Process image name, e.g. `R1.exe`.
    pub process: String,
    pub is_pad: bool,
    pub elevation: Elevation,
}

pub trait KeySink {
    /// Sends one batch; returns how many events were inserted.
    fn send(&mut self, events: &[KeyEvent]) -> usize;
}

pub trait Foreground {
    fn current(&mut self) -> Target;
}

pub trait Clock {
    fn now_ms(&self) -> u64;
    fn sleep_until_ms(&mut self, t: u64);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SideEffects { None, Partial, Unknown }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok { sent: u32, unverified: bool },
    Refused { code: &'static str },
    Aborted { sent: u32, of: u32, side_effects: SideEffects },
    Cancelled { sent: u32, of: u32 },
    SendIncomplete { sent: u32, of: u32 },
}

pub struct Run<'a> {
    pub combo: &'a Combo,
    pub repeat: u32,
    pub interval_ms: u32,
    pub target_apps: Option<&'a [String]>,
    pub pad_elevated: bool,
}

fn refusal(t: &Target, run: &Run) -> Option<&'static str> {
    if t.is_pad {
        return Some("target-is-pad");
    }
    if t.title.trim().is_empty() {
        return Some("target-untitled");
    }
    if let Some(apps) = run.target_apps {
        if !apps.iter().any(|a| a.eq_ignore_ascii_case(&t.process)) {
            return Some("target-not-allowed");
        }
    }
    // UIPI silently drops input into a more elevated window: never send.
    if t.elevation == Elevation::Yes && !run.pad_elevated {
        return Some("target-elevated");
    }
    None
}

/// Key-ups for every key a partially inserted batch pressed but did not
/// release, newest first. Keys the user holds physically are never touched.
fn releases_for(batch: &[KeyEvent], inserted: usize) -> Vec<KeyEvent> {
    let mut held: Vec<KeyEvent> = Vec::new();
    for e in &batch[..inserted.min(batch.len())] {
        if e.down {
            held.push(*e);
        } else {
            held.retain(|h| h.vk != e.vk);
        }
    }
    held.iter().rev().map(|e| KeyEvent { down: false, ..*e }).collect()
}

pub fn execute(
    run: &Run,
    sink: &mut dyn KeySink,
    fg: &mut dyn Foreground,
    clock: &mut dyn Clock,
    cancel: &dyn Fn() -> bool,
) -> Outcome {
    let target = fg.current();
    if let Some(code) = refusal(&target, run) {
        return Outcome::Refused { code };
    }
    let unverified = target.elevation == Elevation::Unknown;
    let batch = press_events(run.combo);
    let start = clock.now_ms();
    let of = run.repeat;
    for n in 0..of {
        if cancel() {
            return Outcome::Cancelled { sent: n, of };
        }
        if fg.current().id != target.id {
            let side_effects = if n == 0 { SideEffects::None } else { SideEffects::Partial };
            return Outcome::Aborted { sent: n, of, side_effects };
        }
        let inserted = sink.send(&batch);
        if inserted < batch.len() {
            let release = releases_for(&batch, inserted);
            if !release.is_empty() {
                sink.send(&release);
            }
            return Outcome::SendIncomplete { sent: n, of };
        }
        if fg.current().id != target.id {
            return Outcome::Aborted { sent: n + 1, of, side_effects: SideEffects::Unknown };
        }
        if n + 1 < of {
            clock.sleep_until_ms(start + u64::from(n + 1) * u64::from(run.interval_ms));
        }
    }
    Outcome::Ok { sent: of, unverified }
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    pub struct RecordingSink {
        pub batches: Vec<Vec<KeyEvent>>,
        /// If set, the nth batch (0-based) inserts only this many events.
        pub short_at: Option<(usize, usize)>,
    }
    impl KeySink for RecordingSink {
        fn send(&mut self, events: &[KeyEvent]) -> usize {
            let n = match self.short_at {
                Some((i, k)) if i == self.batches.len() => k,
                _ => events.len(),
            };
            self.batches.push(events.to_vec());
            n
        }
    }

    /// Returns scripted targets in order; repeats the last one.
    pub struct ScriptedForeground {
        pub script: VecDeque<Target>,
        pub calls: usize,
    }
    impl ScriptedForeground {
        pub fn always(t: Target) -> Self {
            Self { script: VecDeque::from([t]), calls: 0 }
        }
        pub fn sequence(ts: Vec<Target>) -> Self {
            Self { script: ts.into(), calls: 0 }
        }
    }
    impl Foreground for ScriptedForeground {
        fn current(&mut self) -> Target {
            self.calls += 1;
            if self.script.len() > 1 { self.script.pop_front().unwrap() } else { self.script[0].clone() }
        }
    }

    #[derive(Default)]
    pub struct FakeClock {
        pub now: u64,
        pub sleeps: Vec<u64>,
    }
    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 { self.now }
        fn sleep_until_ms(&mut self, t: u64) {
            self.sleeps.push(t);
            if t > self.now { self.now = t; }
        }
    }

    pub fn r1() -> Target {
        Target { id: 1, title: "R1 - Arena Main".into(), process: "R1.exe".into(), is_pad: false, elevation: Elevation::No }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::*;
    use super::*;
    use crate::combo::parse_combo;

    fn run<'a>(combo: &'a Combo, repeat: u32) -> Run<'a> {
        Run { combo, repeat, interval_ms: 25, target_apps: None, pad_elevated: false }
    }

    fn go(r: &Run, sink: &mut RecordingSink, fg: &mut ScriptedForeground) -> (Outcome, FakeClock) {
        let mut clock = FakeClock::default();
        let o = execute(r, sink, fg, &mut clock, &|| false);
        (o, clock)
    }

    #[test]
    fn single_press_sends_one_batch() {
        let c = parse_combo("Num1").unwrap();
        let mut sink = RecordingSink::default();
        let (o, _) = go(&run(&c, 1), &mut sink, &mut ScriptedForeground::always(r1()));
        assert_eq!(o, Outcome::Ok { sent: 1, unverified: false });
        assert_eq!(sink.batches, vec![press_events(&c)]);
    }

    #[test]
    fn repeat_sends_exactly_n_batches_start_to_start() {
        let c = parse_combo("Down").unwrap();
        let mut sink = RecordingSink::default();
        let (o, clock) = go(&run(&c, 10), &mut sink, &mut ScriptedForeground::always(r1()));
        assert_eq!(o, Outcome::Ok { sent: 10, unverified: false });
        assert_eq!(sink.batches.len(), 10);
        // Sleeps target start + n*interval (start-to-start), none after the last press.
        assert_eq!(clock.sleeps, (1..10).map(|n| n * 25).collect::<Vec<u64>>());
    }

    #[test]
    fn refuses_when_pad_is_the_target() {
        let c = parse_combo("Num1").unwrap();
        let mut sink = RecordingSink::default();
        let pad = Target { is_pad: true, ..r1() };
        let (o, _) = go(&run(&c, 1), &mut sink, &mut ScriptedForeground::always(pad));
        assert_eq!(o, Outcome::Refused { code: "target-is-pad" });
        assert!(sink.batches.is_empty());
    }

    #[test]
    fn refuses_untitled_target() {
        let c = parse_combo("Num1").unwrap();
        let mut sink = RecordingSink::default();
        let t = Target { title: String::new(), ..r1() };
        let (o, _) = go(&run(&c, 1), &mut sink, &mut ScriptedForeground::always(t));
        assert_eq!(o, Outcome::Refused { code: "target-untitled" });
    }

    #[test]
    fn refuses_apps_outside_target_apps_case_insensitively() {
        let c = parse_combo("Enter").unwrap();
        let apps = vec!["r1.exe".to_string()];
        let mut sink = RecordingSink::default();
        let teams = Target { process: "ms-teams.exe".into(), title: "Teams".into(), ..r1() };
        let r = Run { target_apps: Some(&apps), ..run(&c, 1) };
        let (o, _) = go(&r, &mut sink, &mut ScriptedForeground::always(teams));
        assert_eq!(o, Outcome::Refused { code: "target-not-allowed" });
        let (o, _) = go(&r, &mut sink, &mut ScriptedForeground::always(r1()));
        assert!(matches!(o, Outcome::Ok { .. }));
    }

    #[test]
    fn refuses_more_elevated_target_but_not_when_pad_is_elevated() {
        let c = parse_combo("Num1").unwrap();
        let elevated = Target { elevation: Elevation::Yes, ..r1() };
        let mut sink = RecordingSink::default();
        let (o, _) = go(&run(&c, 1), &mut sink, &mut ScriptedForeground::always(elevated.clone()));
        assert_eq!(o, Outcome::Refused { code: "target-elevated" });
        assert!(sink.batches.is_empty());
        let r = Run { pad_elevated: true, ..run(&c, 1) };
        let (o, _) = go(&r, &mut sink, &mut ScriptedForeground::always(elevated));
        assert_eq!(o, Outcome::Ok { sent: 1, unverified: false });
    }

    #[test]
    fn unknown_elevation_sends_but_marks_unverified() {
        let c = parse_combo("Num1").unwrap();
        let t = Target { elevation: Elevation::Unknown, ..r1() };
        let mut sink = RecordingSink::default();
        let (o, _) = go(&run(&c, 1), &mut sink, &mut ScriptedForeground::always(t));
        assert_eq!(o, Outcome::Ok { sent: 1, unverified: true });
    }

    #[test]
    fn foreground_change_before_a_press_aborts_with_partial_effects() {
        let c = parse_combo("Down").unwrap();
        let other = Target { id: 2, title: "Teams".into(), process: "ms-teams.exe".into(), ..r1() };
        // start, then per press: check-before, check-after. Change before press 4 (index 3).
        let mut seq = vec![r1()];
        for _ in 0..3 { seq.push(r1()); seq.push(r1()); }
        seq.push(other);
        let mut sink = RecordingSink::default();
        let (o, _) = go(&run(&c, 10), &mut sink, &mut ScriptedForeground::sequence(seq));
        assert_eq!(o, Outcome::Aborted { sent: 3, of: 10, side_effects: SideEffects::Partial });
        assert_eq!(sink.batches.len(), 3);
    }

    #[test]
    fn foreground_change_right_after_a_batch_reports_unknown_effects() {
        let c = parse_combo("Down").unwrap();
        let other = Target { id: 2, ..r1() };
        let seq = vec![r1(), r1(), other];
        let mut sink = RecordingSink::default();
        let (o, _) = go(&run(&c, 10), &mut sink, &mut ScriptedForeground::sequence(seq));
        assert_eq!(o, Outcome::Aborted { sent: 1, of: 10, side_effects: SideEffects::Unknown });
    }

    #[test]
    fn short_insert_releases_only_keys_that_batch_pressed() {
        let c = parse_combo("Ctrl+Down").unwrap();
        let mut sink = RecordingSink { short_at: Some((0, 2)), ..Default::default() }; // Ctrl down, Down down inserted
        let (o, _) = go(&run(&c, 1), &mut sink, &mut ScriptedForeground::always(r1()));
        assert_eq!(o, Outcome::SendIncomplete { sent: 0, of: 1 });
        // Second batch releases Down then Ctrl (reverse order), nothing else.
        assert_eq!(sink.batches.len(), 2);
        let release: Vec<(u16, bool)> = sink.batches[1].iter().map(|e| (e.vk, e.down)).collect();
        assert_eq!(release, vec![(0x28, false), (0x11, false)]);
    }

    #[test]
    fn short_insert_with_nothing_pressed_sends_no_release() {
        let c = parse_combo("Ctrl+Down").unwrap();
        let mut sink = RecordingSink { short_at: Some((0, 0)), ..Default::default() };
        let (o, _) = go(&run(&c, 1), &mut sink, &mut ScriptedForeground::always(r1()));
        assert_eq!(o, Outcome::SendIncomplete { sent: 0, of: 1 });
        assert_eq!(sink.batches.len(), 1);
    }

    #[test]
    fn cancel_stops_between_presses() {
        let c = parse_combo("Down").unwrap();
        let mut sink = RecordingSink::default();
        let mut clock = FakeClock::default();
        let mut fg = ScriptedForeground::always(r1());
        // Cancel becomes true after 4 batches were sent.
        let sent = std::cell::Cell::new(0usize);
        struct Counting<'a> { inner: RecordingSink, sent: &'a std::cell::Cell<usize> }
        impl KeySink for Counting<'_> {
            fn send(&mut self, e: &[KeyEvent]) -> usize { self.sent.set(self.sent.get() + 1); self.inner.send(e) }
        }
        let mut counting = Counting { inner: std::mem::take(&mut sink), sent: &sent };
        let o = execute(&run(&c, 10), &mut counting, &mut fg, &mut clock, &|| sent.get() >= 4);
        assert_eq!(o, Outcome::Cancelled { sent: 4, of: 10 });
    }
}
