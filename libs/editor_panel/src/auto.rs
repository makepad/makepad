//! The panel a document gets when it has none written: one module per
//! group of its value sites, one control per site by its kind, each bound
//! to the site's id (resolved again on every read, like any panel path).

use makepad_edit_values::{Value, ValueKind, ValueSite};

/// A panel string literal.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn num(x: f64) -> String {
    let text = format!("{:.6}", x);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0".into() } else { text.to_string() }
}

/// One site's control line, or `None` for one a panel does not edit (code,
/// media, a derived value).
pub fn control(site: &ValueSite) -> Option<String> {
    if site.derived {
        return None;
    }
    let path = quoted(&site.id);
    let label = if site.label.is_empty() { String::new() } else { format!(" label: {}", quoted(&site.label)) };
    let range = || {
        let r = &site.range;
        let mut out = String::new();
        if r.soft_max > r.soft_min {
            out.push_str(&format!(" min: {} max: {}", num(r.soft_min), num(r.soft_max)));
        }
        if let Some(m) = r.hard_min {
            out.push_str(&format!(" hard_min: {}", num(m)));
        }
        if let Some(m) = r.hard_max {
            out.push_str(&format!(" hard_max: {}", num(m)));
        }
        if site.step > 0.0 {
            out.push_str(&format!(" step: {}", num(site.step)));
        }
        out
    };
    Some(match site.kind {
        ValueKind::Angle => format!("PanelAngle{{path: {path}{label}}}"),
        ValueKind::Count => format!("PanelKnob{{path: {path}{label}{}}}", range()),
        kind if kind.is_numeric() => format!("PanelSlider{{path: {path}{label}{}}}", range()),
        ValueKind::Bool => format!("PanelToggle{{path: {path}{label}}}"),
        ValueKind::Choice => format!("PanelChoice{{path: {path}{label} choices: {}}}", quoted(&site.choices.join(" "))),
        ValueKind::Colour => format!("PanelColor{{path: {path}{label}}}"),
        ValueKind::Ease => format!("PanelEase{{path: {path}{label}}}"),
        ValueKind::Point => match &site.value {
            Value::Point(v) if v.len() == 2 => format!("PanelXY{{path: {path}{label}{}}}", range()),
            _ => format!("PanelVec3{{path: {path}{label}}}"),
        },
        ValueKind::Profile => format!("PanelPoints{{path: {path}{label} joined: true}}"),
        ValueKind::Text => format!("PanelText{{path: {path}{label}}}"),
        ValueKind::Font => format!("PanelFont{{path: {path}{label}}}"),
        _ => return None,
    })
}

/// The panel's modules for `sites`: one `PanelModule` per group in the
/// order the groups first appear (the first one open), the rarely tuned
/// sites (`detail`) in a folded "Details" group at its end. A site with no
/// group goes in "Parameters".
pub fn auto_modules(sites: &[ValueSite]) -> Vec<String> {
    let mut groups: Vec<(&str, Vec<String>, Vec<String>)> = Vec::new();
    for site in sites {
        let Some(line) = control(site) else { continue };
        let name = if site.group.is_empty() { "Parameters" } else { site.group.as_str() };
        let at = match groups.iter().position(|(g, _, _)| *g == name) {
            Some(at) => at,
            None => {
                groups.push((name, Vec::new(), Vec::new()));
                groups.len() - 1
            }
        };
        if site.detail {
            groups[at].2.push(line);
        } else {
            groups[at].1.push(line);
        }
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(k, (name, main, details))| {
            let mut out = format!("PanelModule{{title: {}{}", quoted(name), if k == 0 { " open: true" } else { "" });
            for line in main {
                out.push_str(&format!("\n    {line}"));
            }
            if !details.is_empty() {
                out.push_str("\n    PanelGroup{title: \"Details\"");
                for line in details {
                    out.push_str(&format!("\n        {line}"));
                }
                out.push_str("\n    }");
            }
            out.push_str("\n}");
            out
        })
        .collect()
}

/// The whole panel `let <name> = fn() { View{…} }` for `sites`.
pub fn auto_panel(name: &str, sites: &[ValueSite]) -> String {
    crate::make::assemble(name, &["PanelHeader{}"], &auto_modules(sites))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_edit_values::Range;

    fn site(id: &str, group: &str, kind: ValueKind) -> ValueSite {
        ValueSite { id: id.into(), group: group.into(), kind, ..Default::default() }
    }

    #[test]
    fn sites_become_modules_of_controls() {
        let mut height = site("vase_height", "Vase", ValueKind::Length);
        height.label = "Height".into();
        height.range = Range { soft_min: 40.0, soft_max: 200.0, hard_min: Some(1.0), hard_max: None };
        let mut wall = site("wall", "Vase", ValueKind::Length);
        wall.detail = true;
        let mut osc = site("acid.osc", "acid", ValueKind::Choice);
        osc.choices = vec!["saw".into(), "square".into()];
        let mut derived = site("volume", "Vase", ValueKind::Number);
        derived.derived = true;
        let code = site("body", "", ValueKind::Code);
        let modules = auto_modules(&[height, wall, osc, derived, code]);
        assert_eq!(modules.len(), 2);
        assert_eq!(modules[0], "PanelModule{title: \"Vase\" open: true\n    PanelSlider{path: \"vase_height\" label: \"Height\" min: 40 max: 200 hard_min: 1}\n    PanelGroup{title: \"Details\"\n        PanelSlider{path: \"wall\"}\n    }\n}");
        assert_eq!(modules[1], "PanelModule{title: \"acid\"\n    PanelChoice{path: \"acid.osc\" choices: \"saw square\"}\n}");
        let all = auto_panel("editor_panel", &[site("x", "", ValueKind::Bool)]);
        assert!(all.starts_with("let editor_panel = fn() {") && all.contains("PanelModule{title: \"Parameters\" open: true\n            PanelToggle{path: \"x\"}"), "{all}");
    }
}
