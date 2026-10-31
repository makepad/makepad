//! The two languages and the script test that tells them apart.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Zh,
}

impl Lang {
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Zh => "Chinese",
        }
    }

    /// The language's own name, for the pills.
    pub fn native(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Zh => "中文",
        }
    }

    pub fn other(self) -> Lang {
        match self {
            Lang::En => Lang::Zh,
            Lang::Zh => Lang::En,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Zh => "zh",
        }
    }
}

pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF | 0x3000..=0x303F | 0xFF00..=0xFFEF)
}

/// Which language the text is written in: Chinese when it has more CJK
/// characters than Latin words, English when it has Latin words, `None`
/// for text with neither (digits, punctuation, nothing).
pub fn detect(text: &str) -> Option<Lang> {
    let cjk = text.chars().filter(|c| is_cjk(*c)).count();
    let mut words = 0usize;
    let mut in_word = false;
    for c in text.chars() {
        let letter = c.is_ascii_alphabetic();
        if letter && !in_word {
            words += 1;
        }
        in_word = letter || (in_word && (c == '\'' || c == '-'));
    }
    match (cjk, words) {
        (0, 0) => None,
        (c, w) if c > w => Some(Lang::Zh),
        _ => Some(Lang::En),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_told_apart() {
        assert_eq!(detect("Hello, how are you?"), Some(Lang::En));
        assert_eq!(detect("你好，你好吗？"), Some(Lang::Zh));
        assert_eq!(detect("I love 北京"), Some(Lang::En));
        assert_eq!(detect("我喜欢 Rust 和 coffee"), Some(Lang::Zh));
        assert_eq!(detect("12345 !?"), None);
        assert_eq!(detect(""), None);
    }
}
