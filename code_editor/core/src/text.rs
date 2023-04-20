use super::{Delta, Size};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_lines(mut lines: Vec<String>) -> Self {
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self { lines }
    }

    pub fn len(&self) -> Size {
        Size {
            line: self.lines.len() - 1,
            byte: self.lines.last().unwrap().len(),
        }
    }

    pub fn as_lines(&self) -> &[String] {
        self.lines.as_slice()
    }

    pub fn apply_delta(&mut self, mut delta: Delta) {
        delta.replace_with.lines.first_mut().unwrap().replace_range(
            ..0,
            &self.lines[delta.range.start().line][..delta.range.start().byte],
        );
        delta
            .replace_with
            .lines
            .last_mut()
            .unwrap()
            .push_str(&self.lines[delta.range.end().line][delta.range.end().byte..]);
        self.lines.splice(
            delta.range.start().line..=delta.range.end().line,
            delta.replace_with.lines,
        );
    }

    pub fn into_lines(self) -> Vec<String> {
        self.lines
    }
}

impl Default for Text {
    fn default() -> Self {
        Self::from_lines(vec!["".into()])
    }
}

impl<const N: usize> From<[String; N]> for Text {
    fn from(array: [String; N]) -> Self {
        array.into_iter().collect::<Vec<_>>().into()
    }
}

impl From<Vec<String>> for Text {
    fn from(lines: Vec<String>) -> Self {
        Self::from_lines(lines)
    }
}

impl From<Text> for Vec<String> {
    fn from(text: Text) -> Self {
        text.into_lines()
    }
}
