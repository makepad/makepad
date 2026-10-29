//! The formula modulator (LFO shape 9) as patches store it: a script
//! (`<formulae><formula … formula="BASE64" interpreter="1001"/>`). The
//! script compiles and runs in [`crate::kit::formula`] (its runner writes
//! the 16 outputs into the LFO each block, see
//! [`super::Lfo::set_formula_outputs`]); a script that does not compile is
//! reported by [`Formula::check`] and outputs 0.

pub use crate::kit::formula::{DEFAULT_SCRIPT, GUIDE};

/// The only interpreter id patches use.
pub const INTERPRETER_SCRIPT: i32 = 1001;

/// Number of outputs a formula may produce.
pub const FORMULA_OUTPUTS: usize = 16;

/// One LFO's formula as stored (decoded text).
#[derive(Clone, Debug, PartialEq)]
pub struct Formula {
    pub source: String,
    pub interpreter: i32,
}

impl Default for Formula {
    /// The default script: process returns a rising ramp.
    fn default() -> Self {
        Formula { source: DEFAULT_SCRIPT.to_string(), interpreter: INTERPRETER_SCRIPT }
    }
}

impl Formula {
    /// Whether the script compiles; the load error (with its line) if not.
    pub fn check(&self) -> Result<(), String> {
        crate::kit::formula::Formula::compile(&self.source).map(|_| ()).map_err(|e| format!("formula cannot load (line {}): {}", e.line, e.message))
    }
}

/// What an evaluator reads back after a process call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FormulaIo {
    pub outputs: [f32; FORMULA_OUTPUTS],
    pub count: usize,
    pub use_envelope: bool,
    pub use_amplitude: bool,
    pub clamp_output: bool,
    pub retrigger_aeg: bool,
    pub retrigger_feg: bool,
}

impl Default for FormulaIo {
    fn default() -> Self {
        FormulaIo {
            outputs: [0.0; FORMULA_OUTPUTS],
            count: 1,
            use_envelope: true,
            use_amplitude: true,
            clamp_output: true,
            retrigger_aeg: false,
            retrigger_feg: false,
        }
    }
}

impl FormulaIo {
    /// The final outputs: e_f A_f out_i with e_f = 1 / A_f = 1 when the
    /// script opted out; outputs 1..8 clamped to +-1 when clamping (9..16
    /// not); non-finite -> 0; unipolar maps to 0.5 + 0.5 out.
    pub fn outputs(&self, env: f32, amp: f32, unipolar: bool, out: &mut [f32; FORMULA_OUTPUTS]) {
        let e = if self.use_envelope { env } else { 1.0 };
        let a = if self.use_amplitude { amp } else { 1.0 };
        for i in 0..FORMULA_OUTPUTS {
            let mut v = if i < self.count { self.outputs[i] } else { 0.0 };
            if !v.is_finite() {
                v = 0.0;
            }
            if self.clamp_output && i < 8 {
                v = v.clamp(-1.0, 1.0);
            }
            if unipolar {
                v = 0.5 + 0.5 * v;
            }
            out[i] = e * a * v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_compiles_and_errors_name_their_line() {
        assert!(Formula::default().check().is_ok());
        let bad = Formula { source: "function process(state)\n  state.output = = 1\nend\n".into(), interpreter: INTERPRETER_SCRIPT };
        let e = bad.check().unwrap_err();
        assert!(e.contains("line"), "{e}");
    }
}
