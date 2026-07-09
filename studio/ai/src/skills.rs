#[derive(Clone, Debug)]
pub struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub content: String,
}

pub fn parse_skill_markdown(content: &str) -> Option<ParsedSkill> {
    let lines: Vec<&str> = content.lines().collect();
    let mut first_dash = None;
    let mut second_dash = None;
    for (idx, line) in lines.iter().enumerate() {
        if line.trim() == "---" {
            if first_dash.is_none() {
                first_dash = Some(idx);
            } else {
                second_dash = Some(idx);
                break;
            }
        }
    }

    let (frontmatter_lines, body_lines) = match (first_dash, second_dash) {
        (Some(start), Some(end)) if start < end => (&lines[start + 1..end], &lines[end + 1..]),
        _ => (&[][..], &lines[..]),
    };

    let mut name = String::new();
    let mut description = String::new();

    for line in frontmatter_lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, val)) = line.split_once(':') {
            let key = key.trim();
            let val = val.trim().trim_matches('"').trim_matches('\'').trim();
            if key == "name" {
                name = val.to_string();
            } else if key == "description" {
                description = val.to_string();
            }
        }
    }

    let body_content = body_lines.join("\n");

    if name.trim().is_empty() || body_content.trim().is_empty() {
        return None;
    }

    Some(ParsedSkill {
        name,
        description,
        content: body_content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_markdown() {
        let content = r#"---
name: "Semantic Compression"
description: "Guidelines for compressing and summarizing files"
---
# Semantic Compression
This is the body content of the skill.
It has multiple lines.
"#;
        let parsed = parse_skill_markdown(content).unwrap();
        assert_eq!(parsed.name, "Semantic Compression");
        assert_eq!(
            parsed.description,
            "Guidelines for compressing and summarizing files"
        );
        assert!(parsed.content.contains("# Semantic Compression"));
        assert!(parsed.content.contains("It has multiple lines."));
    }

    #[test]
    fn test_parse_skill_markdown_validation() {
        let content_no_name = r#"---
description: "Guidelines for compressing and summarizing files"
---
# Semantic Compression
This is the body content of the skill.
"#;
        assert!(parse_skill_markdown(content_no_name).is_none());

        let content_no_body = r#"---
name: "Semantic Compression"
---
"#;
        assert!(parse_skill_markdown(content_no_body).is_none());
    }
}
