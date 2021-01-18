use std::{convert::Infallible, mem, str::FromStr};

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Text {
    lines: Vec<Vec<char>>,
}

impl Text {
    pub fn new() -> Text {
        Text::default()
    }

    pub fn from_lines(lines: Vec<Vec<char>>) -> Text {
        Text { lines }
    }

    pub fn as_lines(&self) -> &[Vec<char>] {
        &self.lines
    }

    pub fn into_lines(self) -> Vec<Vec<char>> {
        self.lines
    }
}

impl FromStr for Text {
    type Err = Infallible;

    fn from_str(string: &str) -> Result<Text, Infallible> {
        let mut lines = Vec::new();
        let mut line = Vec::new();
        for ch in string.chars() {
            match ch {
                '\n' => lines.push(mem::replace(&mut line, Vec::new())),
                ch => line.push(ch),
            }
        }
        lines.push(line);
        Ok(Text { lines })
    }
}
