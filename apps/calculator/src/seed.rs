//! Deterministic empty calculator document. No clock, no RNG.

use crate::model::{AngleMode, CalculatorDoc, Phase, Session};
use std::collections::VecDeque;

/// Version 1, degrees, empty memory and history, source `"0"`.
pub fn initial() -> CalculatorDoc {
    CalculatorDoc {
        version: 1,
        angle: AngleMode::Degrees,
        memory: None,
        session: Session {
            source: "0".into(),
            phase: Phase::Editing,
            value: 0.0,
            repeat: None,
            reuse_repeat: false,
        },
        next_id: 1,
        history: VecDeque::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_seed_is_degrees_zero_and_has_no_history() {
        let d = initial();
        assert_eq!(d.version, 1);
        assert_eq!(d.angle, AngleMode::Degrees);
        assert!(d.memory.is_none());
        assert_eq!(d.session.source, "0");
        assert_eq!(d.session.value, 0.0);
        assert_eq!(d.session.phase, Phase::Editing);
        assert!(d.session.repeat.is_none());
        assert!(!d.session.reuse_repeat);
        assert_eq!(d.next_id, 1);
        assert!(d.history.is_empty());
    }
}
