//! Lane E: the element info tool.
//!
//! Click an element with the info card on (`I`) and a small card follows it in
//! the viewport: what it is, which storey and layer it belongs to, its GUID,
//! its size, and its quantities. The same click focuses the Properties editor
//! on the Element tab and reveals the row in the outliner, so the card is a
//! heads-up display over the full record rather than a second one.

use crate::api::*;
use crate::model::PropertyValue;
use makepad_widgets::*;

pub struct InfoCard {
    pub title: String,
    pub subtitle: String,
    pub rows: Vec<(String, String)>,
}

fn value_text(v: &PropertyValue, units: &Units) -> String {
    match v {
        PropertyValue::Text(s) => s.clone(),
        PropertyValue::Number(n) => format!("{n:.3}"),
        PropertyValue::Integer(i) => i.to_string(),
        PropertyValue::Bool(b) => if *b { "Yes" } else { "No" }.into(),
        PropertyValue::Length(m) => units.format_length(*m),
        PropertyValue::Area(a) => units.format_area(*a),
        PropertyValue::Volume(v) => units.format_volume(*v),
        PropertyValue::Angle(d) => units.format_angle(*d),
    }
}

/// Shorten a GUID so it fits the card but stays recognisable.
fn short_guid(guid: &str) -> String {
    let g = guid.trim_matches(|c| c == '{' || c == '}');
    if g.len() <= 18 {
        g.to_string()
    } else {
        format!("{}…{}", &g[..8], &g[g.len() - 6..])
    }
}

/// The card for one element, or `None` when the element is gone.
pub fn card_for(scene: &Scene, id: ElementId, units: &Units) -> Option<InfoCard> {
    let el = scene.element(id)?;
    let story = scene
        .story_of(id)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "—".into());
    let layer = el
        .layer
        .and_then(|l| scene.layers.get(l.index()))
        .map(|l| l.name.clone())
        .unwrap_or_else(|| "—".into());

    let mut rows = vec![
        ("Type".to_string(), el.class.label().to_string()),
        ("Storey".to_string(), story),
        ("Layer".to_string(), layer),
    ];
    if !el.guid.is_empty() {
        rows.push(("GUID".into(), short_guid(&el.guid)));
    }
    if el.has_geometry() {
        let e = aabb_extent(&el.bounds);
        rows.push((
            "Size".into(),
            format!(
                "{} × {} × {}",
                units.format_length(e.x as f64),
                units.format_length(e.y as f64),
                units.format_length(e.z as f64)
            ),
        ));
        rows.push(("Triangles".into(), format!("{}", el.triangle_count)));
    }
    for q in el.quantities.iter().take(4) {
        rows.push((q.name.clone(), value_text(&q.value, units)));
    }
    if el.quantities.is_empty() {
        // No quantity take-off in this file: show the first properties that
        // carry a value, which is what Fab models actually publish today.
        for p in el.properties.iter().filter(|p| !p.name.is_empty()).take(4) {
            rows.push((p.name.clone(), value_text(&p.value, units)));
        }
    }

    Some(InfoCard {
        title: el.name.clone(),
        subtitle: format!("{} · {}", el.class.label(), scene.name),
        rows,
    })
}

/// Where the card points: the top of the element's bounds.
pub fn anchor(scene: &Scene, id: ElementId) -> Option<Vec3f> {
    let el = scene.element(id)?;
    if !el.has_geometry() {
        return None;
    }
    let c = aabb_center(&el.bounds);
    Some(vec3(c.x, c.y, el.bounds.max.z))
}

/// Actions that put the full record in front of the user: Properties on the
/// Element tab, the outliner scrolled to the row.
pub fn focus_properties(cx: &mut Cx, id: ElementId) {
    cx.action(ShellAction::SetPropertiesTab(PropertiesTab::Element));
    cx.action(ShellAction::RevealInOutliner(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_reads_the_demo_house() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let wall = scene
            .elements
            .iter()
            .find(|e| e.class == crate::model::ElementClass::Wall && e.has_geometry())
            .expect("a wall");
        let card = card_for(&scene, wall.id, &scene.units).unwrap();
        assert_eq!(card.title, wall.name);
        assert!(card.rows.iter().any(|(k, _)| k == "Type"));
        assert!(card.rows.iter().any(|(k, v)| k == "Size" && v.contains('×')));
        assert!(anchor(&scene, wall.id).is_some());
    }

    #[test]
    fn guids_shorten_without_losing_their_ends() {
        let g = "{1234ABCD-5678-90EF-1234-567890ABCDEF}";
        let s = short_guid(g);
        assert!(s.starts_with("1234ABCD"));
        assert!(s.ends_with("ABCDEF"));
    }
}
