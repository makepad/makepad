use alloc::{borrow::Cow, boxed::Box, string::String};
use core::{error::Error, fmt};

#[derive(Clone, Debug)]
pub struct ShaderError<E> {
    /// The source code of the shader.
    pub source: String,
    pub label: Option<String>,
    pub inner: Box<E>,
}

#[cfg(feature = "wgsl-in")]
impl fmt::Display for ShaderError<crate::front::wgsl::ParseError> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = self.label.as_deref().unwrap_or_default();
        let string = self.inner.emit_to_string(&self.source);
        write!(f, "\nShader '{label}' parsing {string}")
    }
}

#[cfg(feature = "glsl-in")]
impl fmt::Display for ShaderError<crate::front::glsl::ParseErrors> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = self.label.as_deref().unwrap_or_default();
        let string = self.inner.emit_to_string(&self.source);
        write!(f, "\nShader '{label}' parsing {string}")
    }
}

#[cfg(feature = "spv-in")]
impl fmt::Display for ShaderError<crate::front::spv::Error> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = self.label.as_deref().unwrap_or_default();
        let string = self.inner.emit_to_string(&self.source);
        write!(f, "\nShader '{label}' parsing {string}")
    }
}

impl fmt::Display for ShaderError<crate::WithSpan<crate::valid::ValidationError>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _label = self.label.as_deref().unwrap_or_default();
        let writer = self.inner.emit_to_string(&self.source);

        write!(f, "\nShader validation {writer}")
    }
}

#[allow(unused_imports)]
#[allow(dead_code)]
pub(crate) use core::fmt::Write as ErrorWrite;

#[allow(dead_code)]
type DiagnosticBufferInner = String;

#[allow(dead_code)]
pub(crate) struct DiagnosticBuffer {
    inner: DiagnosticBufferInner,
}

#[allow(dead_code)]
impl DiagnosticBuffer {
    pub fn new() -> Self {
        Self {
            inner: String::new(),
        }
    }

    pub fn inner_mut(&mut self) -> &mut DiagnosticBufferInner {
        &mut self.inner
    }

    pub fn into_string(self) -> String {
        self.inner
    }
}

impl<E> Error for ShaderError<E>
where
    ShaderError<E>: fmt::Display,
    E: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.inner.source()
    }
}

pub(crate) fn replace_control_chars(s: &str) -> Cow<'_, str> {
    const REPLACEMENT_CHAR: &str = "\u{FFFD}";
    debug_assert_eq!(
        REPLACEMENT_CHAR.chars().next().unwrap(),
        char::REPLACEMENT_CHARACTER
    );

    let mut res = Cow::Borrowed(s);
    let mut offset = 0;

    while let Some(found_pos) = res[offset..].find(|c: char| c.is_control() && !c.is_whitespace()) {
        offset += found_pos;
        let found_len = res[offset..].chars().next().unwrap().len_utf8();
        res.to_mut()
            .replace_range(offset..offset + found_len, REPLACEMENT_CHAR);
        offset += REPLACEMENT_CHAR.len();
    }

    res
}

#[test]
fn test_replace_control_chars() {
    // The UTF-8 encoding of \u{0080} is multiple bytes.
    let input = "Foo\u{0080}Bar\u{0001}Baz\n";
    let expected = "Foo\u{FFFD}Bar\u{FFFD}Baz\n";
    assert_eq!(replace_control_chars(input), expected);
}
