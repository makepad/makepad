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
