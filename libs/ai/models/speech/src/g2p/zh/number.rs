//! Digits to Chinese numerals, the way the text reads aloud.
//!
//! Misaki's front end runs `cn2an` first (`an2cn`, "low" mode: 二, not 两). This
//! is the same job for the shapes games and subtitles use: integers, decimals,
//! percentages, negatives; a four-digit year before 年 and long or
//! zero-led digit strings are read digit by digit.

const DIGITS: [char; 10] = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'];

/// `1234` -> `一千二百三十四`.
pub fn integer(value: u64) -> String {
    if value == 0 {
        return "零".into();
    }
    // Groups of four digits, most significant first: 亿亿 is out of reach of u64
    // beyond 10^19, so three group names suffice.
    let mut groups = Vec::new();
    let mut rest = value;
    while rest > 0 {
        groups.push((rest % 10_000) as u32);
        rest /= 10_000;
    }
    let names = ["", "万", "亿", "万亿", "兆"];
    let mut out = String::new();
    let mut pending_zero = false;
    for (index, group) in groups.iter().enumerate().rev() {
        if *group == 0 {
            pending_zero = !out.is_empty();
            continue;
        }
        if !out.is_empty() && (pending_zero || *group < 1000) {
            out.push('零');
        }
        pending_zero = false;
        out.push_str(&below_ten_thousand(*group));
        out.push_str(names[index.min(names.len() - 1)]);
    }
    // 十二, not 一十二, at the head of the whole number.
    if let Some(stripped) = out.strip_prefix("一十") {
        return format!("十{stripped}");
    }
    out
}

fn below_ten_thousand(group: u32) -> String {
    let digits = [group / 1000, group / 100 % 10, group / 10 % 10, group % 10];
    let units = ["千", "百", "十", ""];
    let mut out = String::new();
    let mut zero_run = false;
    for (digit, unit) in digits.iter().zip(units) {
        if *digit == 0 {
            zero_run = !out.is_empty();
            continue;
        }
        if zero_run {
            out.push('零');
            zero_run = false;
        }
        out.push(DIGITS[*digit as usize]);
        out.push_str(unit);
    }
    out
}

fn digit_by_digit(digits: &str) -> String {
    digits.chars().filter_map(|c| c.to_digit(10)).map(|d| DIGITS[d as usize]).collect()
}

/// Replace every number in `text` with its reading.
pub fn spell_numbers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !c.is_ascii_digit() {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        let whole: String = chars[start..i].iter().collect();
        let negative = start > 0 && chars[start - 1] == '-' && (start < 2 || !chars[start - 2].is_ascii_alphanumeric());
        if negative {
            out.pop();
            out.push('负');
        }
        // A decimal part: `.` followed by digits.
        let mut fraction = String::new();
        if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
            let frac_start = i + 1;
            i = frac_start;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            fraction = chars[frac_start..i].iter().collect();
        }
        let percent = i < chars.len() && (chars[i] == '%' || chars[i] == '％');
        if percent {
            out.push_str("百分之");
            i += 1;
        }
        let year = whole.len() == 4 && fraction.is_empty() && !percent && chars.get(i) == Some(&'年');
        let plain = whole.len() > 9 || (whole.len() > 1 && whole.starts_with('0'));
        if year || plain {
            out.push_str(&digit_by_digit(&whole));
        } else {
            out.push_str(&integer(whole.parse().unwrap_or(0)));
        }
        if !fraction.is_empty() {
            out.push('点');
            out.push_str(&digit_by_digit(&fraction));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_read_like_people_say_them() {
        assert_eq!(integer(0), "零");
        assert_eq!(integer(10), "十");
        assert_eq!(integer(12), "十二");
        assert_eq!(integer(100), "一百");
        assert_eq!(integer(101), "一百零一");
        assert_eq!(integer(110), "一百一十");
        assert_eq!(integer(1012), "一千零一十二");
        assert_eq!(integer(10_000), "一万");
        assert_eq!(integer(10_005), "一万零五");
        assert_eq!(integer(100_500), "十万零五百");
        assert_eq!(integer(123_456_789), "一亿二千三百四十五万六千七百八十九");
    }

    #[test]
    fn text_with_numbers() {
        assert_eq!(spell_numbers("100名"), "一百名");
        assert_eq!(spell_numbers("3.14"), "三点一四");
        assert_eq!(spell_numbers("50%"), "百分之五十");
        assert_eq!(spell_numbers("2024年"), "二零二四年");
        assert_eq!(spell_numbers("-5度"), "负五度");
        assert_eq!(spell_numbers("007"), "零零七");
        assert_eq!(spell_numbers("1小时的 Maple"), "一小时的 Maple");
    }
}
