#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Ini {
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    pub fn parse(text: &str) -> Self {
        let mut ini = Self::default();
        let mut current = None;
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                let name = line[1..line.len() - 1].trim();
                if let Some(index) = ini
                    .sections
                    .iter()
                    .position(|(existing, _)| existing.eq_ignore_ascii_case(name))
                {
                    current = Some(index);
                } else {
                    ini.sections.push((name.to_owned(), Vec::new()));
                    current = Some(ini.sections.len() - 1);
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Some(index) = current else {
                continue;
            };
            let value = value.split_once(';').map_or(value, |(before, _)| before);
            ini.sections[index]
                .1
                .push((key.trim().to_owned(), value.trim().to_owned()));
        }
        ini
    }

    pub fn section(&self, name: &str) -> Option<&[(String, String)]> {
        self.sections
            .iter()
            .find(|(section, _)| section.eq_ignore_ascii_case(name))
            .map(|(_, entries)| entries.as_slice())
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.section(section)?
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    pub fn sections(&self) -> impl Iterator<Item = (&str, &[(String, String)])> {
        self.sections
            .iter()
            .map(|(name, entries)| (name.as_str(), entries.as_slice()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_ini_preserves_duplicates_and_ignores_case() {
        let ini = Ini::parse(
            "; heading\r\n[Waypoints]\r\n0 = 123 ; start\r\n0=456\r\n\r\n[Basic]\r\nName = Test Map\r\n",
        );
        assert_eq!(ini.get("basic", "name"), Some("Test Map"));
        assert_eq!(
            ini.section("WAYPOINTS").unwrap(),
            &[("0".into(), "123".into()), ("0".into(), "456".into())]
        );
    }
}
