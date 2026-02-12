use crate::heap::*;
use crate::regex::*;
use crate::value::*;

impl ScriptHeap {
    /// Create or intern a regex. If the same (pattern, flags) was already compiled,
    /// returns the existing ScriptRegex index.
    pub fn new_regex(&mut self, pattern: &str, flags_str: &str) -> Result<ScriptValue, String> {
        let flags = RegexFlags::parse(flags_str)?;
        let key = RegexInternKey {
            pattern: pattern.to_string(),
            flags,
        };

        // Check intern table first
        if let Some(idx) = self.regex_intern.get(&key) {
            return Ok((*idx).into());
        }

        // Compile and store
        let data = ScriptRegexData::new(pattern, flags)?;

        let idx = if let Some(re) = self.regexes_free.pop() {
            // re already has the correct generation from gc sweep
            self.regexes[re] = Some(data);
            re
        } else {
            let index = self.regexes.len();
            self.regexes.push(Some(data));
            ScriptRegex::new(index as u32, crate::value::GENERATION_ZERO)
        };

        self.regex_intern.insert(key, idx);
        Ok(idx.into())
    }

    /// Get a reference to a regex's data
    pub fn regex(&self, ptr: ScriptRegex) -> Option<&ScriptRegexData> {
        self.regexes[ptr].as_ref()
    }

    /// Get a mutable reference to a regex's data
    pub fn regex_mut(&mut self, ptr: ScriptRegex) -> Option<&mut ScriptRegexData> {
        self.regexes[ptr].as_mut()
    }
}
