use super::objects::{parse, path, roots, state_words};
use super::*;
use makepad_a11y::{mirror::text, ActionResult, Actions};
use std::collections::HashSet;

pub(super) fn selfcheck(key: u64, act: bool) -> String {
    let began = Instant::now();
    match check(key, act) {
        Ok((nodes, action)) => format!(
            "oscheck linux atspi nodes={nodes} mismatches=0{action} elapsed_ms={:.1}",
            began.elapsed().as_secs_f64() * 1000.
        ),
        Err(error) => format!("oscheck linux atspi mismatches=1 error={error}"),
    }
}
fn check(key: u64, act: bool) -> Result<(usize, String), String> {
    let deadline = Instant::now() + TIMEOUT;
    let (address, bus) = loop {
        if let Some(pair) = lock(identity()).connection.clone() {
            break pair;
        }
        if Instant::now() >= deadline {
            return Err("adapter has not registered on the accessibility bus".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut conn = Connection::connect(&address, TIMEOUT).map_err(|e| e.to_string())?;
    macro_rules! call { ($function:ident, $object:expr $(, $arg:expr)*) => {{
        let object=&$object;
        $function(&mut conn,&object.bus,&object.path,$($arg,)* TIMEOUT).map_err(|e| e.to_string())?
    }}; }
    macro_rules! equal {
        ($actual:expr,$expected:expr,$context:expr) => {{
            let actual = $actual;
            let expected = $expected;
            if actual != expected {
                return Err(format!(
                    "{}: got {:?}, expected {:?}",
                    $context, actual, expected
                ));
            }
        }};
    }
    let registry = ObjRef {
        bus: REGISTRY.into(),
        path: ROOT.into(),
    };
    let apps = call!(call_accessible_get_children, registry);
    let app = apps
        .into_iter()
        .find(|a| a.bus == bus && a.path == ROOT)
        .ok_or("our application is absent from registry children")?;
    equal!(
        call!(call_accessible_get_role, app),
        AtspiRole::Application as u32,
        "application role"
    );
    equal!(
        call!(call_application_get_toolkit_name, app),
        "Makepad",
        "toolkit"
    );
    let windows = call!(call_accessible_get_children, app);
    let expected_roots = roots();
    equal!(
        windows.len(),
        expected_roots.len(),
        "application child count"
    );
    equal!(
        call!(call_accessible_get_child_count, app),
        windows.len() as i32,
        "application ChildCount"
    );
    let root = expected_roots
        .iter()
        .find(|(k, _)| *k == key)
        .ok_or("window tree disappeared")?
        .1;
    let window = windows
        .iter()
        .find(|r| r.path == path(key, root))
        .cloned()
        .ok_or("window is absent from application children")?;
    let revision = a11y::with_tree(key, |t| t.updates).ok_or("window tree disappeared")?;
    let expected_ids = a11y::with_tree(key, |t| t.nodes.keys().copied().collect::<HashSet<_>>())
        .unwrap_or_default();
    let cache_ref = ObjRef {
        bus: bus.clone(),
        path: CACHE.into(),
    };
    let cache = call!(call_cache_get_items, cache_ref);
    let cached: HashSet<_> = cache
        .iter()
        .filter_map(|item| {
            parse(&item.0.path)
                .filter(|(k, _)| *k == key)
                .map(|(_, n)| n)
        })
        .collect();
    equal!(cached, expected_ids.clone(), "cache nodes");
    let mut seen = HashSet::new();
    let mut stack = vec![(window, app.clone())];
    let mut press = None;
    while let Some((object, parent)) = stack.pop() {
        let (actual_key, id) =
            parse(&object.path).ok_or_else(|| format!("invalid node path {}", object.path))?;
        equal!(actual_key, key, "window identity");
        if !seen.insert(id) {
            return Err(format!("duplicate/cyclic node {}", object.path));
        }
        let v = objects::view(&object.path).map_err(|e| e.detail)?;
        let n = &v.node;
        equal!(
            call!(call_accessible_get_name, object),
            n.name.as_str(),
            format!("{} Name", object.path)
        );
        equal!(
            call!(call_accessible_get_description, object),
            n.description.as_str(),
            "Description"
        );
        equal!(
            call!(call_accessible_get_role, object),
            n.role.atspi_role(),
            "GetRole"
        );
        equal!(call!(call_accessible_get_parent, object), parent, "Parent");
        equal!(
            call!(call_accessible_get_state, object),
            state_words(n, v.focused),
            "GetState"
        );
        let children = call!(call_accessible_get_children, object);
        equal!(
            children.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
            n.children
                .iter()
                .map(|id| path(key, *id))
                .collect::<Vec<_>>(),
            "GetChildren"
        );
        equal!(
            call!(call_accessible_get_child_count, object),
            children.len() as i32,
            "ChildCount"
        );
        for (index, child) in children.iter().enumerate() {
            equal!(
                call!(call_accessible_get_child_at_index, object, index as i32),
                child.clone(),
                "GetChildAtIndex"
            );
            equal!(
                call!(call_accessible_get_index_in_parent, child),
                index as i32,
                "GetIndexInParent"
            );
            stack.push((child.clone(), object.clone()));
        }
        let interfaces = call!(call_accessible_get_interfaces, object);
        equal!(
            interfaces.contains(&TEXT_INTERFACE.to_string()),
            n.text.is_some(),
            "Text interface"
        );
        equal!(
            interfaces.contains(&VALUE_INTERFACE.to_string()),
            n.range.is_some(),
            "Value interface"
        );
        let rect = call!(
            call_component_get_extents,
            object,
            AtspiCoordType::Window as u32
        );
        equal!(
            rect,
            (
                n.bounds.x.floor() as i32,
                n.bounds.y.floor() as i32,
                (n.bounds.x + n.bounds.w).ceil() as i32 - n.bounds.x.floor() as i32,
                (n.bounds.y + n.bounds.h).ceil() as i32 - n.bounds.y.floor() as i32
            ),
            "GetExtents"
        );
        if let Some(t) = &n.text {
            equal!(
                call!(call_text_get_text, object, 0, -1),
                t.content.as_str(),
                "GetText"
            );
            equal!(
                call!(call_text_get_character_count, object),
                t.content.chars().count() as i32,
                "CharacterCount"
            );
            equal!(
                call!(call_text_get_caret_offset, object),
                text::byte_to_char(&t.content, t.selection.1) as i32,
                "CaretOffset"
            );
            if !t.content.is_empty() {
                let character = call!(
                    call_text_get_string_at_offset,
                    object,
                    0,
                    AtspiTextGranularity::Char as u32
                );
                equal!(
                    character,
                    (t.content.chars().next().unwrap().to_string(), 0, 1),
                    "GetStringAtOffset"
                );
            }
        }
        if let Some(r) = n.range {
            equal!(
                call!(call_value_get_current_value, object),
                r.now,
                "CurrentValue"
            );
            equal!(
                call!(call_value_get_minimum_value, object),
                r.min,
                "MinimumValue"
            );
            equal!(
                call!(call_value_get_maximum_value, object),
                r.max,
                "MaximumValue"
            );
            equal!(
                call!(call_value_get_minimum_increment, object),
                r.step,
                "MinimumIncrement"
            );
        }
        if n.actions.contains(Actions::PRESS)
            && press.is_none()
            && !n
                .states
                .intersects(makepad_a11y::States::DISABLED | makepad_a11y::States::HIDDEN)
        {
            let actions = call!(call_action_get_actions, object);
            let index = actions
                .iter()
                .position(|(name, _, _)| name == "press")
                .ok_or("press action missing")?;
            press = Some((object.clone(), index as i32, id));
        }
    }
    equal!(&seen, &expected_ids, "reachable nodes");
    equal!(
        a11y::with_tree(key, |t| t.updates),
        Some(revision),
        "tree changed during self-check; rerun on settled content"
    );
    let mut result = String::new();
    if act {
        let (object, index, node) = press.ok_or("no enabled pressable node")?;
        let previous = lock(identity()).last_request;
        equal!(
            call!(call_action_do_action, object, index),
            true,
            "DoAction"
        );
        let until = Instant::now() + TIMEOUT;
        let (req, status) = loop {
            let observed = {
                let identity = lock(identity());
                identity
                    .last_request
                    .filter(|r| Some(*r) != previous && r.1 == key && r.2 == node)
                    .and_then(|(req, _, _)| {
                        identity.completed.get(&req).map(|result| (req, *result))
                    })
            };
            if let Some(done) = observed {
                break done;
            }
            if Instant::now() >= until {
                return Err("DoAction did not complete through the semantic action path".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if !matches!(
            ActionResult::from_u8(status),
            ActionResult::Handled | ActionResult::Fallback
        ) {
            return Err(format!(
                "DoAction: {}",
                ActionResult::from_u8(status).short()
            ));
        }
        result = format!(
            " action=press node={} req={req} result={}",
            node.short(),
            ActionResult::from_u8(status).short()
        );
    }
    Ok((expected_ids.len(), result))
}
