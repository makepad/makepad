//! JavaScript codec emitted into the wasm message bridge at startup. The node decoder and
//! adapter mappings are generated from the same tables as the native wire codec.
use crate::{wire, Actions, Role, States};
use std::fmt::Write;

pub fn javascript() -> String {
    let mut out = String::from("(() => {\n");
    writeln!(out, "const MAGIC = {}, VERSION = {};", wire::MAGIC, wire::VERSION).unwrap();
    out.push_str("const roles = {}, roleNames = {}, states = {}, actions = {};\n");
    for role in Role::ALL {
        writeln!(out, "roles[{}] = {:?}; roleNames[{}] = {:?};", *role as u16, role.aria(), *role as u16, role.short()).unwrap();
    }
    for (state, name) in States::NAMES {
        writeln!(out, "states.{name} = {}n;", state.0).unwrap();
    }
    for (action, name) in Actions::NAMES {
        writeln!(out, "actions.{name} = {};", action.0).unwrap();
    }
    out.push_str(include_str!("wire_js.js"));
    out.push_str("\nfunction node(r) { const id = r.u64(), mask = r.u64(); return {id,\n");
    for (bit, name, kind) in wire::NODE_FIELDS {
        let default = match *kind {
            "str" => "\"\"",
            "u64" => "0n",
            "u8" | "u16" | "u32" => "0",
            "rect" => "{x:0,y:0,w:0,h:0}",
            "ids" | "strs" => "[]",
            "relations" => "{}",
            "range" | "text" | "table" => "null",
            _ => panic!("missing JavaScript wire kind {kind}"),
        };
        writeln!(out, "{name}: (mask & (1n << {bit}n)) ? r.{kind}() : {default},").unwrap();
    }
    out.push_str("}; }\nreturn {decodeUpdate, encodeAction, roles, roleNames, states, actions};\n})()");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use std::{io::Write, process::{Command, Stdio}};

    #[test]
    fn javascript_wire_round_trip() {
        // Optional on hosts without Node; the web lane's gate runs this on a host with Node.
        if Command::new("node").arg("--version").output().is_err() {
            eprintln!("JavaScript wire check not run: node is unavailable");
            return;
        }
        let id = NodeId::part(0x123456789abc, 0xffff);
        let rect = Rect { x: 10.5, y: -20.0, w: 80.0, h: 24.0 };
        let node = Node {
            id, role: Role::TextField, states: States::FOCUSABLE | States::EDITABLE,
            actions: Actions::FOCUS | Actions::REPLACE_TEXT, bounds: rect,
            name: "Name 🦀".into(), description: "Description".into(), value: "hé🦀".into(),
            placeholder: "Placeholder".into(), key_shortcut: "Control+S".into(),
            role_description: "Editor".into(), class: "TextInput".into(), path: "root.edit".into(),
            range: Some(RangeValue {min: -1.0, max: 10.0, now: 2.5, step: 0.25}),
            toggle: Toggle::Mixed, level: 2, pos_in_set: 3, set_size: 4, live: Live::Polite,
            relations: Relations {labelled_by: vec![NodeId(3)], controls: vec![NodeId(4)], ..Default::default()},
            custom_actions: vec!["archive".into()], children: vec![NodeId(2)],
            text: Some(TextInfo {content: "hé🦀".into(), selection: (7, 1),
                lines: vec![LineBox {start: 0, rect, baseline: 18.0}], glyph_x: vec![0.0, 8.0, 16.0, 32.0]}),
            table: Some(TableInfo {rows: 5, cols: 6, row: 1, col: 2, row_span: 3, col_span: 4}),
        };
        let update = TreeUpdate {full: true, root: id, focus: id,
            nodes: vec![node, Node {id: NodeId(2), ..Default::default()}], removed: vec![NodeId(99)],
            notifications: vec![Notification::FocusChanged(id), Notification::NameChanged(id),
                Notification::ValueChanged(id), Notification::StatesChanged {node: id, old: States::FOCUSABLE, new: States::EDITABLE},
                Notification::ChildrenChanged(id), Notification::TextChanged {node: id, start: 1, removed: 2, inserted: 4},
                Notification::CaretMoved(id), Notification::LiveRegionChanged(id),
                Notification::Announce {text: "Saved 🦀".into(), live: Live::Assertive}],
        };
        let mut bytes = Vec::new();
        wire::encode_update(&update, &mut bytes);
        let mut script = format!("const codec = {};\nconst bytes = new Uint8Array({bytes:?});\n", javascript());
        script.push_str(include_str!("wire_js_test.js"));
        let mut child = Command::new("node").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let args = [ActionArg::None, ActionArg::Number(2.5), ActionArg::Text("hé🦀".into()),
            ActionArg::Range(7, 1), ActionArg::Replace(1, 7, "é🦀".into()),
            ActionArg::Scroll(1.5, -2.0), ActionArg::Custom("archive".into())];
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout.lines().count(), args.len());
        for (line, arg) in stdout.lines().zip(args) {
            let bytes: Vec<u8> = line.split(',').map(|s| s.parse().unwrap()).collect();
            assert_eq!(wire::decode_action(&bytes).unwrap(), ActionRequest {req: u64::MAX, node: id, action: Actions::REPLACE_TEXT, arg});
        }
    }
}
