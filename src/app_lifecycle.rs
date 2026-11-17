//! Application lifecycle parity with Flutter's five-state machine.
//!
//! Mechanism source: Flutter 3.50 `services/binding.dart:313-381` (generated
//! intermediate transitions and the legality table) and
//! `scheduler/binding.dart:388-430` (frame gate). `Lifecycle::Destroyed` is this
//! crate's name for Flutter's `detached`: the engine outlives its view.

use crate::Lifecycle;

/// Flutter enum order: detached, resumed, inactive, hidden, paused.
const ORDER: [Lifecycle; 5] = [
    Lifecycle::Destroyed,
    Lifecycle::Resumed,
    Lifecycle::Inactive,
    Lifecycle::Hidden,
    Lifecycle::Paused,
];

fn index(state: Lifecycle) -> usize {
    match state {
        Lifecycle::Destroyed => 0,
        Lifecycle::Resumed => 1,
        Lifecycle::Inactive => 2,
        Lifecycle::Hidden => 3,
        Lifecycle::Paused => 4,
    }
}

/// The stable wire string, equal to Flutter's `AppLifecycleState.<lowercase>`
/// suffix (`services/binding.dart:448-457`).
#[cfg(test)]
pub(crate) fn as_str(state: Lifecycle) -> &'static str {
    match state {
        Lifecycle::Resumed => "resumed",
        Lifecycle::Inactive => "inactive",
        Lifecycle::Hidden => "hidden",
        Lifecycle::Paused => "paused",
        Lifecycle::Destroyed => "detached",
    }
}

/// Whether frames and ticks may run in this state. Flutter enables them for
/// `resumed`/`inactive` and stops them for `hidden`/`paused`/`detached`
/// (`scheduler/binding.dart:421-429`).
pub fn frames_enabled(state: Lifecycle) -> bool {
    matches!(state, Lifecycle::Resumed | Lifecycle::Inactive)
}

/// One legal adjacent step of Flutter's lifecycle graph
/// (`services/binding.dart:360-381`).
pub(crate) fn can_transition(previous: Option<Lifecycle>, next: Lifecycle) -> bool {
    match previous {
        None => true,
        Some(previous) if previous == next => false,
        Some(Lifecycle::Resumed) => next == Lifecycle::Inactive,
        Some(Lifecycle::Destroyed) => matches!(next, Lifecycle::Resumed | Lifecycle::Paused),
        Some(Lifecycle::Inactive) => matches!(next, Lifecycle::Resumed | Lifecycle::Hidden),
        Some(Lifecycle::Hidden) => matches!(next, Lifecycle::Paused | Lifecycle::Inactive),
        Some(Lifecycle::Paused) => matches!(next, Lifecycle::Hidden | Lifecycle::Destroyed),
    }
}

/// Every state change from `previous` to `next`, including the intermediates
/// Flutter synthesizes instead of jumping (`services/binding.dart:313-358`).
pub(crate) fn transitions(previous: Option<Lifecycle>, next: Lifecycle) -> Vec<Lifecycle> {
    match previous {
        None => vec![next],
        Some(previous) if previous == next => Vec::new(),
        Some(previous) => {
            let (from, to) = (index(previous), index(next));
            let mut changes = Vec::new();
            if next == Lifecycle::Destroyed {
                changes.extend_from_slice(&ORDER[from + 1..]);
                changes.push(Lifecycle::Destroyed);
            } else if from > to {
                for state in &ORDER[to..from] {
                    changes.insert(0, *state);
                }
            } else {
                changes.extend_from_slice(&ORDER[from + 1..=to]);
            }
            debug_assert!(
                can_transition(Some(previous), changes[0]),
                "first generated state must follow {previous:?}"
            );
            debug_assert!(
                changes
                    .windows(2)
                    .all(|pair| can_transition(Some(pair[0]), pair[1])),
                "generated states must be legal adjacent steps"
            );
            changes
        }
    }
}

/// Per-host lifecycle cursor: hosts report the state their OS signal implies and
/// push whatever transitions come back into their event FIFO.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LifecycleTracker {
    current: Option<Lifecycle>,
}

impl LifecycleTracker {
    pub(crate) const fn new() -> Self {
        Self { current: None }
    }

    #[cfg(test)]
    pub(crate) fn current(self) -> Option<Lifecycle> {
        self.current
    }

    pub(crate) fn report(&mut self, target: Lifecycle) -> Vec<Lifecycle> {
        let changes = transitions(self.current, target);
        if let Some(last) = changes.last() {
            self.current = Some(*last);
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_gate_matches_flutter() {
        assert!(frames_enabled(Lifecycle::Resumed));
        assert!(frames_enabled(Lifecycle::Inactive));
        assert!(!frames_enabled(Lifecycle::Hidden));
        assert!(!frames_enabled(Lifecycle::Paused));
        assert!(!frames_enabled(Lifecycle::Destroyed));
    }

    #[test]
    fn state_strings_are_stable_bytes() {
        assert_eq!(as_str(Lifecycle::Resumed).as_bytes(), b"resumed");
        assert_eq!(as_str(Lifecycle::Inactive).as_bytes(), b"inactive");
        assert_eq!(as_str(Lifecycle::Hidden).as_bytes(), b"hidden");
        assert_eq!(as_str(Lifecycle::Paused).as_bytes(), b"paused");
        assert_eq!(as_str(Lifecycle::Destroyed).as_bytes(), b"detached");
    }

    #[test]
    fn transitions_synthesize_the_flutter_path() {
        let paused = |previous| transitions(previous, Lifecycle::Paused);
        assert_eq!(paused(None), vec![Lifecycle::Paused]);
        assert_eq!(
            paused(Some(Lifecycle::Resumed)),
            vec![Lifecycle::Inactive, Lifecycle::Hidden, Lifecycle::Paused]
        );
        assert_eq!(
            transitions(Some(Lifecycle::Paused), Lifecycle::Resumed),
            vec![Lifecycle::Hidden, Lifecycle::Inactive, Lifecycle::Resumed]
        );
        assert_eq!(
            transitions(Some(Lifecycle::Resumed), Lifecycle::Destroyed),
            vec![
                Lifecycle::Inactive,
                Lifecycle::Hidden,
                Lifecycle::Paused,
                Lifecycle::Destroyed
            ]
        );
        assert_eq!(
            transitions(Some(Lifecycle::Paused), Lifecycle::Destroyed),
            vec![Lifecycle::Destroyed]
        );
        assert_eq!(
            transitions(Some(Lifecycle::Destroyed), Lifecycle::Resumed),
            vec![Lifecycle::Resumed]
        );
        assert!(transitions(Some(Lifecycle::Hidden), Lifecycle::Hidden).is_empty());
    }

    #[test]
    fn legality_table_matches_flutter() {
        let legal = [
            (None, Lifecycle::Resumed),
            (Some(Lifecycle::Resumed), Lifecycle::Inactive),
            (Some(Lifecycle::Inactive), Lifecycle::Resumed),
            (Some(Lifecycle::Inactive), Lifecycle::Hidden),
            (Some(Lifecycle::Hidden), Lifecycle::Paused),
            (Some(Lifecycle::Hidden), Lifecycle::Inactive),
            (Some(Lifecycle::Paused), Lifecycle::Hidden),
            (Some(Lifecycle::Paused), Lifecycle::Destroyed),
            (Some(Lifecycle::Destroyed), Lifecycle::Resumed),
            (Some(Lifecycle::Destroyed), Lifecycle::Paused),
        ];
        for (previous, next) in legal {
            assert!(can_transition(previous, next), "{previous:?} -> {next:?}");
        }
        let illegal = [
            (Some(Lifecycle::Resumed), Lifecycle::Resumed),
            (Some(Lifecycle::Resumed), Lifecycle::Hidden),
            (Some(Lifecycle::Resumed), Lifecycle::Paused),
            (Some(Lifecycle::Inactive), Lifecycle::Paused),
            (Some(Lifecycle::Hidden), Lifecycle::Resumed),
            (Some(Lifecycle::Paused), Lifecycle::Inactive),
            (Some(Lifecycle::Paused), Lifecycle::Resumed),
            (Some(Lifecycle::Destroyed), Lifecycle::Hidden),
        ];
        for (previous, next) in illegal {
            assert!(!can_transition(previous, next), "{previous:?} -> {next:?}");
        }
    }

    #[test]
    fn tracker_dedupes_and_advances() {
        let mut tracker = LifecycleTracker::new();
        assert_eq!(tracker.report(Lifecycle::Paused), vec![Lifecycle::Paused]);
        assert!(tracker.report(Lifecycle::Paused).is_empty());
        assert_eq!(
            tracker.report(Lifecycle::Resumed),
            vec![Lifecycle::Hidden, Lifecycle::Inactive, Lifecycle::Resumed]
        );
        assert_eq!(
            tracker.report(Lifecycle::Destroyed),
            vec![
                Lifecycle::Inactive,
                Lifecycle::Hidden,
                Lifecycle::Paused,
                Lifecycle::Destroyed
            ]
        );
        assert_eq!(tracker.current(), Some(Lifecycle::Destroyed));
    }
}
