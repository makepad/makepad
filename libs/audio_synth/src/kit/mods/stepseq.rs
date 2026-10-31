//! The step sequencer's data model (patch `<stepsequences><sequence …/>`):
//! 16 step values, inclusive loop markers, the stored shuffle and the 64-bit
//! EG trigger mask. Plain data the patch loader fills; the LFO evaluates it.

/// One LFO's step sequence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepSeq {
    /// Step values, -1..1.
    pub steps: [f32; 16],
    pub loop_start: i32,
    /// Inclusive.
    pub loop_end: i32,
    /// As stored; the engine's shuffle comes from the LFO phase parameter.
    pub shuffle: f32,
    /// Bit i (0..15): retrigger both EGs on entering step i; bit 16+i: the
    /// filter EG only; bit 32+i: the amp EG only. Voice LFOs only.
    pub trigmask: u64,
}

impl Default for StepSeq {
    /// All steps 0, loop 0..15, shuffle 0, mask 0.
    fn default() -> Self {
        StepSeq { steps: [0.0; 16], loop_start: 0, loop_end: 15, shuffle: 0.0, trigmask: 0 }
    }
}

impl StepSeq {
    /// The trigger mask as the patch stores it: the legacy `trigmask` first,
    /// then each present 16-bit lane (`trigmask_0to15`, `_16to31`,
    /// `_32to47`) overwrites its slot.
    pub fn set_trigmask_attrs(&mut self, legacy: Option<u64>, lanes: [Option<u16>; 3]) {
        let mut m = legacy.unwrap_or(0);
        for (w, lane) in lanes.iter().enumerate() {
            if let Some(v) = lane {
                let sh = 16 * w as u32;
                m = (m & !(0xffffu64 << sh)) | ((*v as u64) << sh);
            }
        }
        self.trigmask = m;
    }

    /// The three 16-bit lanes for writing.
    pub fn trigmask_lanes(&self) -> [u16; 3] {
        [self.trigmask as u16, (self.trigmask >> 16) as u16, (self.trigmask >> 32) as u16]
    }

    #[inline]
    pub fn value(&self, i: i32) -> f32 {
        self.steps[(i & 15) as usize]
    }

    /// The step after `step`, honouring the loop markers (reversed markers:
    /// stepping at or past the start jumps to just after the end).
    pub fn next_step(&self, step: i32) -> i32 {
        let mut s = step + 1;
        if self.loop_end >= self.loop_start {
            if s > self.loop_end {
                s = self.loop_start;
            }
        } else if s >= self.loop_start {
            s = self.loop_end + 1;
        }
        s & 15
    }

    /// Loop length in steps (at least 1).
    #[inline]
    pub fn loop_len(&self) -> i32 {
        (self.loop_end - self.loop_start + 1).max(1)
    }

    /// (both, filter EG, amp EG) trigger flags of entering `step`.
    #[inline]
    pub fn triggers(&self, step: i32) -> (bool, bool, bool) {
        let b = (step & 15) as u32;
        let m = self.trigmask;
        ((m >> b) & 1 != 0, (m >> (16 + b)) & 1 != 0, (m >> (32 + b)) & 1 != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_and_masks() {
        let mut s = StepSeq { loop_start: 2, loop_end: 5, ..StepSeq::default() };
        assert_eq!(s.next_step(4), 5);
        assert_eq!(s.next_step(5), 2);
        assert_eq!(s.next_step(15), 2);
        s.loop_start = 6;
        s.loop_end = 3;
        assert_eq!(s.next_step(5), 4);
        assert_eq!(s.next_step(6), 4);
        s.set_trigmask_attrs(Some(0xffff_ffff_ffff), [None, Some(0x0001), None]);
        assert_eq!(s.trigmask, 0xffff_0001_ffff);
        assert_eq!(s.triggers(0), (true, true, true));
        assert_eq!(s.triggers(1), (true, false, true));
        assert_eq!(s.trigmask_lanes(), [0xffff, 0x0001, 0xffff]);
    }
}
