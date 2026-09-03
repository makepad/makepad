//! Faces (DESIGN.md §3): one splash isolate per open instance. The flow file
//! is evaluated in it with the REAL face prelude (`faces.splash`) in scope,
//! each node's `ui` object is mounted with `WidgetRef::script_from_value`
//! inside that isolate — so every inline handler routes back to it — and the
//! canvas draws the mounted roots inside its node frames.
//!
//! The `flow` bridge the handlers see never re-enters the canvas: every call
//! is posted as a [`FaceBridgeCall`] action and the app acts on it on the
//! next event dispatch.

use crate::values::ValueCache;
use makepad_code_editor::code_view::CodeView;
use makepad_flow::{
    Graph, InstanceRow, Literal, Node, NodeTypeCatalog, PortType, ValueBytes, ValueRef,
    PRELUDE,
};
use makepad_widgets::makepad_micro_serde::SerJson;
use makepad_widgets::makepad_script::*;
use makepad_widgets::widget_async::{enter_isolate, leave_isolate, CxSplashVmExt, SplashVmId};
use makepad_widgets::widget_tree::CxWidgetExt;
use makepad_widgets::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const FACES: &str = include_str!("faces.splash");
const RECIPE_PRELUDE: &str = include_str!("../../../libs/flow/recipes/prelude_recipes.splash");
const PRELUDE_FILE: &str = "<makepad-flow-prelude>";
const FACES_FILE: &str = "<flow-ui-faces>";
const RECIPE_FILE: &str = "<makepad-flow-recipe-prelude>";
const FLOW_INSTRUCTION_LIMIT: usize = 5_000_000;
const HANDLER_INSTRUCTION_LIMIT: usize = 200_000;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.flow.ui.ValueImageBase = #(ValueImage::register_widget(vm))
    mod.flow.ui.ValueImage = set_type_default() do mod.flow.ui.ValueImageBase{
        width: Fill
        height: 180
        flow: Down
        align: Align{x: 0.5 y: 0.5}
        image := Image{
            width: Fill
            height: Fill
            fit: ImageFit.Smallest
        }
        note := Label{
            width: Fill
            height: Fit
            text: ""
            draw_text +: {color: theme.color_text_meta}
        }
    }

    mod.flow.ui.ValueTextBase = #(ValueText::register_widget(vm))
    mod.flow.ui.ValueText = set_type_default() do mod.flow.ui.ValueTextBase{
        width: Fill
        height: Fit
        flow: Down
        text := Label{
            width: Fill
            height: Fit
            text: ""
            draw_text +: {text_style: theme.font_code{}}
        }
    }

    mod.flow.ui.ValueViewBase = #(ValueView::register_widget(vm))
    mod.flow.ui.ValueView = set_type_default() do mod.flow.ui.ValueViewBase{
        width: Fill
        height: Fit
        flow: Down
        image := Image{
            width: Fill
            height: 180
            visible: false
            fit: ImageFit.Smallest
        }
        text := Label{
            width: Fill
            height: Fit
            text: ""
        }
    }

    mod.flow.ui.ModelPickerBase = #(ModelPicker::register_widget(vm))
    mod.flow.ui.ModelPicker = set_type_default() do mod.flow.ui.ModelPickerBase{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{y: 0.5}
        Label{text: "model"}
        name := TextInput{
            width: Fill
            height: 28
            empty_text: "hub picks"
        }
    }

    mod.flow.ui.PortDot = RoundedView{
        width: 10
        height: 10
        draw_bg +: {
            border_radius: 5.0
            color: theme.color_makepad
        }
    }
}

/// An image value: the PNG/JPEG bytes become the texture.
#[derive(Script, ScriptHook, Widget)]
pub struct ValueImage {
    #[deref]
    view: View,
}

impl Widget for ValueImage {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl ValueImage {
    pub fn set_value(&mut self, cx: &mut Cx, value: &ValueBytes) {
        let image = self.view.image(cx, ids!(image));
        let loaded = if value.content_type.contains("jpeg") || value.content_type.contains("jpg") {
            image.load_jpg_from_data(cx, &value.bytes)
        } else {
            image.load_png_from_data(cx, &value.bytes)
        };
        let note = match loaded {
            Ok(()) => String::new(),
            Err(error) => format!("{} · {:?}", value.content_type, error),
        };
        self.view.label(cx, ids!(note)).set_text(cx, &note);
        self.view.redraw(cx);
    }

    pub fn set_note(&mut self, cx: &mut Cx, text: &str) {
        self.view.label(cx, ids!(note)).set_text(cx, text);
        self.view.redraw(cx);
    }
}

/// A text / json value in the code font.
#[derive(Script, ScriptHook, Widget)]
pub struct ValueText {
    #[deref]
    view: View,
    #[rust]
    value: String,
}

impl Widget for ValueText {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.value = v.to_string();
        self.view.label(cx, ids!(text)).set_text(cx, v);
        self.view.redraw(cx);
    }
    fn text(&self) -> String {
        self.value.clone()
    }
}

/// Shows whatever arrives: an image as a picture, text and json inline,
/// anything else as its content type and byte count.
#[derive(Script, ScriptHook, Widget)]
pub struct ValueView {
    #[deref]
    view: View,
    #[rust]
    value: String,
}

impl Widget for ValueView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.value = v.to_string();
        self.view.image(cx, ids!(image)).set_visible(cx, false);
        self.view.label(cx, ids!(text)).set_text(cx, v);
        self.view.redraw(cx);
    }
    fn text(&self) -> String {
        self.value.clone()
    }
}

impl ValueView {
    pub fn set_image(&mut self, cx: &mut Cx, value: &ValueBytes) {
        let image = self.view.image(cx, ids!(image));
        let loaded = if value.content_type.contains("jpeg") || value.content_type.contains("jpg") {
            image.load_jpg_from_data(cx, &value.bytes)
        } else {
            image.load_png_from_data(cx, &value.bytes)
        };
        match loaded {
            Ok(()) => {
                image.set_visible(cx, true);
                self.view.label(cx, ids!(text)).set_text(cx, "");
            }
            Err(error) => {
                image.set_visible(cx, false);
                self.view
                    .label(cx, ids!(text))
                    .set_text(cx, &format!("{} · {:?}", value.content_type, error));
            }
        }
        self.view.redraw(cx);
    }
}

/// The model name: free text in v1 (the catalog carries no model list).
#[derive(Script, ScriptHook, Widget)]
pub struct ModelPicker {
    #[deref]
    view: View,
    #[rust]
    value: String,
}

impl Widget for ModelPicker {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.value = v.to_string();
        self.view.text_input(cx, ids!(name)).set_text(cx, v);
    }
    fn text(&self) -> String {
        self.value.clone()
    }
}

/// Registers the Rust-backed face widgets into `mod.flow.ui` of an isolate.
pub fn register_face_widgets(vm: &mut ScriptVm) {
    self::script_mod(vm);
}

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

/// What a face handler asked the flow for; posted as an action, acted on by
/// the app on the next dispatch (never inside the handler).
#[derive(Clone, Debug)]
pub enum BridgeCall {
    Input {
        node: String,
        port: String,
        /// The value as JSON text (a string value arrives quoted).
        value_json: String,
    },
    Run {
        outputs: Option<Vec<String>>,
    },
    Cancel,
    Param {
        node: String,
        key: String,
        value_json: String,
    },
}

#[derive(Clone, Debug)]
pub struct FaceBridgeCall {
    pub instance: String,
    pub call: BridgeCall,
}

type NodeObjects = Rc<RefCell<HashMap<ScriptObject, String>>>;

fn value_text(vm: &ScriptVm<'_>, value: ScriptValue) -> Option<String> {
    if let Some(id) = value.as_id() {
        return id.as_string(|name| name.map(str::to_string));
    }
    vm.bx
        .heap
        .string_with(value, |_heap, text| text.to_string())
}

fn node_name(vm: &ScriptVm<'_>, nodes: &NodeObjects, value: ScriptValue) -> Option<String> {
    if let Some(obj) = value.as_object() {
        return nodes.borrow().get(&obj).cloned();
    }
    value_text(vm, value)
}

fn make_bridge(vm: &mut ScriptVm, instance: String, nodes: NodeObjects) -> ScriptObject {
    let bridge = vm.bx.heap.new_object();
    let empty = vm.bx.heap.new_object();
    vm.bx
        .heap
        .set_value_def(bridge, id!(inputs).into(), empty.into());
    let values = vm.bx.heap.new_object();
    vm.bx
        .heap
        .set_value_def(bridge, id!(values).into(), values.into());
    let state = vm.bx.heap.new_string_from_str("idle");
    vm.bx.heap.set_value_def(bridge, id!(state).into(), state);

    let post = {
        let instance = instance.clone();
        move |call: BridgeCall| {
            Cx::post_action(FaceBridgeCall {
                instance: instance.clone(),
                call,
            });
        }
    };

    {
        let post = post.clone();
        let nodes = nodes.clone();
        vm.add_method(
            bridge,
            id_lut!(input),
            script_args_def!(node = NIL, port = NIL, value = NIL),
            move |vm, args| {
                let node = script_value!(vm, args.node);
                let port = script_value!(vm, args.port);
                let value = script_value!(vm, args.value);
                let (Some(node), Some(port)) =
                    (node_name(vm, &nodes, node), value_text(vm, port))
                else {
                    return script_err_invalid_args!(
                        vm.trap(),
                        "flow.input(node, port, value): node and port are required"
                    );
                };
                let mut value_json = String::new();
                vm.bx.heap.to_json_inner(value, &mut value_json);
                post(BridgeCall::Input {
                    node,
                    port,
                    value_json,
                });
                TRUE
            },
        );
    }
    {
        let post = post.clone();
        vm.add_method(
            bridge,
            id_lut!(run),
            script_args_def!(options = NIL),
            move |vm, args| {
                let options = script_value!(vm, args.options);
                let mut outputs = None;
                if let Some(obj) = options.as_object() {
                    let list = vm.bx.heap.value(
                        obj.into(),
                        id!(outputs).into(),
                        vm.bx.threads.cur_ref().trap.pass(),
                    );
                    if let Some(array) = list.as_array() {
                        let mut names = Vec::new();
                        for index in 0..vm.bx.heap.array_len(array) {
                            let item = vm.bx.heap.array_index_unchecked(array, index);
                            if let Some(name) = value_text(vm, item) {
                                names.push(name);
                            }
                        }
                        outputs = Some(names);
                    }
                }
                post(BridgeCall::Run { outputs });
                TRUE
            },
        );
    }
    {
        let post = post.clone();
        vm.add_method(bridge, id_lut!(cancel), script_args_def!(), move |_vm, _args| {
            post(BridgeCall::Cancel);
            TRUE
        });
    }
    {
        let post = post.clone();
        let nodes = nodes.clone();
        vm.add_method(
            bridge,
            id_lut!(param),
            script_args_def!(node = NIL, key = NIL, value = NIL),
            move |vm, args| {
                let node = script_value!(vm, args.node);
                let key = script_value!(vm, args.key);
                let value = script_value!(vm, args.value);
                let (Some(node), Some(key)) = (node_name(vm, &nodes, node), value_text(vm, key))
                else {
                    return script_err_invalid_args!(
                        vm.trap(),
                        "flow.param(node, key, value): node and key are required"
                    );
                };
                let mut value_json = String::new();
                vm.bx.heap.to_json_inner(value, &mut value_json);
                post(BridgeCall::Param {
                    node,
                    key,
                    value_json,
                });
                TRUE
            },
        );
    }
    {
        let nodes = nodes.clone();
        vm.add_method(
            bridge,
            id_lut!(value),
            script_args_def!(node = NIL, port = NIL),
            move |vm, args| {
                let me = script_value!(vm, args.self);
                let node = script_value!(vm, args.node);
                let port = script_value!(vm, args.port);
                let (Some(node), Some(port)) =
                    (node_name(vm, &nodes, node), value_text(vm, port))
                else {
                    return NIL;
                };
                let Some(me) = me.as_object() else {
                    return NIL;
                };
                let values = vm.bx.heap.value(
                    me.into(),
                    id!(values).into(),
                    vm.bx.threads.cur_ref().trap.pass(),
                );
                let Some(values) = values.as_object() else {
                    return NIL;
                };
                let by_node = vm.bx.heap.value(
                    values.into(),
                    LiveId::from_str(&node).into(),
                    vm.bx.threads.cur_ref().trap.pass(),
                );
                let Some(by_node) = by_node.as_object() else {
                    return NIL;
                };
                vm.bx.heap.value(
                    by_node.into(),
                    LiveId::from_str(&port).into(),
                    vm.bx.threads.cur_ref().trap.pass(),
                )
            },
        );
    }
    bridge
}

// ---------------------------------------------------------------------------
// Heap helpers (the graph module keeps its own private copies)
// ---------------------------------------------------------------------------

fn own_value(vm: &ScriptVm<'_>, obj: ScriptObject, name: &str) -> Option<ScriptValue> {
    let key: ScriptValue = LiveId::from_str(name).into();
    let data = vm.bx.heap.object_data(obj);
    data.map_get(&key).or_else(|| {
        data.vec
            .iter()
            .rev()
            .find(|entry| entry.key == key)
            .map(|entry| entry.value)
    })
}

fn deep_value(vm: &ScriptVm<'_>, mut obj: ScriptObject, name: &str) -> Option<ScriptValue> {
    let mut depth = 0;
    loop {
        if let Some(value) = own_value(vm, obj, name) {
            return Some(value);
        }
        obj = vm.bx.heap.proto(obj).as_object()?;
        depth += 1;
        if depth > 64 {
            return None;
        }
    }
}

fn make_mod(file: &str, code: &str) -> ScriptMod {
    ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: file.to_string(),
        line: 0,
        column: 0,
        code: code.to_string(),
        values: vec![],
    }
}

fn eval_checked(vm: &mut ScriptVm, file: &str, code: &str) -> Result<ScriptValue, String> {
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(FLOW_INSTRUCTION_LIMIT, |vm| {
        vm.eval(make_mod(file, code))
    });
    let errors = vm.take_errors();
    vm.bx.captured_errors = Some(Vec::new());
    if let Some(error) = errors.first() {
        return Err(error.trim().to_string());
    }
    Ok(value)
}

fn json_to_script(vm: &mut ScriptVm<'_>, value: &makepad_strict_json::Value) -> ScriptValue {
    use makepad_strict_json::Value as Json;
    match value {
        Json::Null => NIL,
        Json::Bool(value) => ScriptValue::from_bool(*value),
        Json::Int(value) => ScriptValue::from_f64(*value as f64),
        Json::F64(value) => ScriptValue::from_f64(*value),
        Json::Str(value) => vm.bx.heap.new_string_from_str(value),
        Json::Arr(values) => {
            let array = vm.bx.heap.new_array();
            for value in values {
                let value = json_to_script(vm, value);
                vm.bx.heap.array_push_unchecked(array, value);
            }
            array.into()
        }
        Json::Obj(values) => {
            let object = vm.bx.heap.new_object();
            for (name, value) in values {
                let value = json_to_script(vm, value);
                vm.bx
                    .heap
                    .set_value_def(object, LiveId::from_str(name).into(), value);
            }
            object.into()
        }
    }
}

/// A value as a handler sees it: text / json inline, media as a handle.
fn value_to_script(
    vm: &mut ScriptVm<'_>,
    value: &ValueRef,
    bytes: Option<&ValueBytes>,
) -> ScriptValue {
    match value.ty {
        PortType::Text => {
            let text = bytes
                .map(|bytes| String::from_utf8_lossy(&bytes.bytes).into_owned())
                .or_else(|| preview_text(value))
                .unwrap_or_default();
            vm.bx.heap.new_string_from_str(&text)
        }
        PortType::Json | PortType::List => {
            let text = bytes
                .map(|bytes| String::from_utf8_lossy(&bytes.bytes).into_owned())
                .or_else(|| preview_text(value))
                .unwrap_or_default();
            match makepad_strict_json::parse(text.as_bytes()) {
                Ok(json) => json_to_script(vm, &json),
                Err(_) => vm.bx.heap.new_string_from_str(&text),
            }
        }
        _ => {
            let object = vm.bx.heap.new_object();
            let digest = vm.bx.heap.new_string_from_str(&value.digest);
            vm.bx
                .heap
                .set_value_def(object, id!(digest).into(), digest);
            let content_type = vm.bx.heap.new_string_from_str(&value.content_type);
            vm.bx
                .heap
                .set_value_def(object, id!(content_type).into(), content_type);
            vm.bx.heap.set_value_def(
                object,
                id!(bytes).into(),
                ScriptValue::from_f64(value.bytes as f64),
            );
            object.into()
        }
    }
}

pub fn preview_text(value: &ValueRef) -> Option<String> {
    match &value.preview {
        Some(Literal::Str(text)) => Some(text.clone()),
        Some(Literal::Obj(fields)) => {
            let width = fields.iter().find(|(k, _)| k == "width").map(|(_, v)| v);
            let height = fields.iter().find(|(k, _)| k == "height").map(|(_, v)| v);
            match (width, height) {
                (Some(Literal::Num(w)), Some(Literal::Num(h))) => {
                    Some(format!("{} {}×{} · {} bytes", value.content_type, w, h, value.bytes))
                }
                _ => Some(format!("{} · {} bytes", value.content_type, value.bytes)),
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Mounted faces
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Bind {
    pub widget: WidgetRef,
    pub node: String,
    pub port: String,
}

#[derive(Default)]
pub struct MountedFace {
    pub root: WidgetRef,
    pub error: Option<String>,
    pub binds: Vec<Bind>,
    pub shows: Vec<Bind>,
    pub params: Vec<(WidgetRef, String)>,
    pub param_binds: Vec<(WidgetRef, String)>,
    pub on_value: Option<ScriptFnRef>,
    pub on_state: Option<ScriptFnRef>,
}

/// One instance's isolate and everything mounted in it.
pub struct FaceHost {
    pub instance: String,
    vm_id: SplashVmId,
    node_objects: NodeObjects,
    bridge: Option<ScriptObjectRef>,
    pub faces: HashMap<String, MountedFace>,
    pub flow_face: Option<MountedFace>,
    /// The flow file failed to evaluate in the isolate (the server's graph
    /// still draws; only the faces are missing).
    pub error: Option<String>,
    deltas: HashMap<(String, String), String>,
    /// The last output per (node, port) that reached a face, for re-pushes
    /// when its bytes arrive.
    pub last_values: HashMap<(String, String), ValueRef>,
    /// Digests a face wants the bytes of (an image preview).
    pub wanted: Vec<String>,
}

fn collect_widgets(root: &WidgetRef, out: &mut Vec<WidgetRef>) {
    if root.is_empty() {
        return;
    }
    out.push(root.clone());
    root.children(&mut |_, child| collect_widgets(&child, out));
}

/// Which port `@self` means for a node: an input-like node's value lives on
/// its output port (that is the instance's inputs table key), while an Output
/// face displays the value arriving at its input. Anything else's `@self` is
/// its first output for `show` and first input for `bind`.
fn self_port(node: &Node, for_bind: bool) -> Option<String> {
    if node.kind == "input" || node.kind == "ask" {
        return node.outputs.first().map(|port| port.name.clone());
    }
    if for_bind || node.kind == "output" {
        node.inputs.first().map(|input| input.port.clone())
    } else {
        node.outputs.first().map(|port| port.name.clone())
    }
}

impl FaceHost {
    /// Evaluate `source` in a fresh isolate and mount every node's face.
    pub fn mount(
        cx: &mut Cx,
        parent: WidgetUid,
        instance: &str,
        file_name: &str,
        source: &str,
        graph: &Graph,
        catalog: &[NodeTypeCatalog],
    ) -> Self {
        let vm_id = cx.alloc_splash_vm();
        let node_objects: NodeObjects = Rc::new(RefCell::new(HashMap::new()));
        let mut host = Self {
            instance: instance.to_string(),
            vm_id,
            node_objects: node_objects.clone(),
            bridge: None,
            faces: HashMap::new(),
            flow_face: None,
            error: None,
            deltas: HashMap::new(),
            last_values: HashMap::new(),
            wanted: Vec::new(),
        };
        let instance_name = instance.to_string();
        let nodes_for_bridge = node_objects.clone();
        let file_name = file_name.to_string();
        let source = source.to_string();
        let node_ids: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
        let result: Result<(ScriptObjectRef, ScriptObjectRef), String> = cx
            .with_script_vm_id_trusted(vm_id, |vm| {
                makepad_code_editor::script_mod(vm);
                vm.new_module(id!(flow));
                eval_checked(vm, PRELUDE_FILE, PRELUDE)?;
                register_face_widgets(vm);
                eval_checked(vm, FACES_FILE, FACES)?;
                eval_checked(vm, RECIPE_FILE, RECIPE_PRELUDE)?;
                let bridge = make_bridge(vm, instance_name, nodes_for_bridge.clone());
                vm.set_injected_global(id!(flow), bridge.into());
                // Faces are the ordinary widgets DSL, so the widget universe
                // is in scope for the flow file; the prefix shares line 1 so
                // every `loc` the server reports still points at the same line.
                let source = format!("use mod.prelude.widgets.* {source}");
                let value = eval_checked(vm, &file_name, &source)?;
                let flow = value
                    .as_object()
                    .ok_or_else(|| "the file's last expression is not a Flow{}".to_string())?;
                let mut objects = nodes_for_bridge.borrow_mut();
                for id in &node_ids {
                    if let Some(obj) = own_value(vm, flow, id).and_then(|value| value.as_object())
                    {
                        objects.insert(obj, id.clone());
                    }
                }
                Ok((
                    vm.bx.heap.new_object_ref(bridge),
                    vm.bx.heap.new_object_ref(flow),
                ))
            });
        let flow = match result {
            Ok((bridge, flow)) => {
                host.bridge = Some(bridge);
                Some(flow)
            }
            Err(error) => {
                // The file failed in the isolate (a face that names an
                // unknown widget, say): the server's graph still draws, with
                // every node wearing its type's default face.
                host.error = Some(error);
                None
            }
        };
        for node in &graph.nodes {
            let face_name = catalog
                .iter()
                .find(|entry| entry.type_name == node.type_name)
                .map(|entry| entry.face.clone())
                .unwrap_or_else(|| "NodeFace".to_string());
            let face = match flow.as_ref() {
                Some(flow) => host.mount_one(cx, parent, flow, node, Some(&face_name), graph),
                None => host.mount_value(cx, parent, None, "ui", &node.id, Some(&face_name), graph),
            };
            host.faces.insert(node.id.clone(), face);
        }
        let Some(flow) = flow else {
            cx.widget_tree_mark_dirty(parent);
            return host;
        };
        if graph.flow_ui_src.is_some() {
            let flow_obj = flow.as_object();
            let has_face = cx.with_script_vm_id_trusted(vm_id, |vm| {
                deep_value(vm, flow_obj, "ui").is_some_and(|value| value.as_object().is_some())
            });
            if has_face {
                let face = host.mount_value(cx, parent, Some(flow_obj), "ui", "flow", None, graph);
                host.flow_face = Some(face);
            }
        }
        cx.widget_tree_mark_dirty(parent);
        host
    }

    fn mount_one(
        &mut self,
        cx: &mut Cx,
        parent: WidgetUid,
        flow: &ScriptObjectRef,
        node: &Node,
        default_face: Option<&str>,
        graph: &Graph,
    ) -> MountedFace {
        let flow_obj = flow.as_object();
        let node_obj = cx.with_script_vm_id_trusted(self.vm_id, |vm| {
            own_value(vm, flow_obj, &node.id).and_then(|value| value.as_object())
        });
        let Some(node_obj) = node_obj else {
            return MountedFace {
                error: Some(format!("{} is not listed in Flow{{}}", node.id)),
                ..Default::default()
            };
        };
        self.mount_value(cx, parent, Some(node_obj), "ui", &node.id, default_face, graph)
    }

    /// Mount `owner.<field>` (or the named default face) for `node_id`.
    fn mount_value(
        &mut self,
        cx: &mut Cx,
        parent: WidgetUid,
        owner: Option<ScriptObject>,
        field: &str,
        node_id: &str,
        default_face: Option<&str>,
        graph: &Graph,
    ) -> MountedFace {
        let vm_id = self.vm_id;
        let node_objects = self.node_objects.clone();
        let graph_node = graph.nodes.iter().find(|node| node.id == node_id).cloned();
        let node_id = node_id.to_string();
        let default_face = default_face.map(str::to_string);
        let mounted = cx.with_script_vm_id_trusted(vm_id, |vm| {
            let mut face_value = owner
                .and_then(|owner| deep_value(vm, owner, field))
                .unwrap_or(NIL);
            let mut face_obj = face_value.as_object();
            vm.bx.captured_errors = Some(Vec::new());
            let mut root = if face_obj.is_some() {
                WidgetRef::script_from_value(vm, face_value)
            } else {
                WidgetRef::empty()
            };
            if root.is_empty() {
                if let Some(name) = default_face.as_deref() {
                    let flow_mod = own_value(vm, vm.bx.heap.modules, "flow")
                        .and_then(|value| value.as_object());
                    let ui = flow_mod
                        .and_then(|module| own_value(vm, module, "ui"))
                        .and_then(|value| value.as_object());
                    if let Some(value) = ui.and_then(|ui| own_value(vm, ui, name)) {
                        face_value = value;
                        face_obj = value.as_object();
                        root = WidgetRef::script_from_value(vm, face_value);
                    }
                }
            }
            let mut errors = vm.take_errors();
            vm.bx.captured_errors = Some(Vec::new());
            let mut face = MountedFace {
                root: root.clone(),
                error: None,
                ..Default::default()
            };
            if root.is_empty() {
                errors.push(format!("{node_id}: the face is not a widget"));
            }
            if !errors.is_empty() {
                face.error = Some(errors.join("\n"));
            }
            if let Some(face_obj) = face_obj {
                for (hook, slot) in [("on_value", 0usize), ("on_state", 1usize)] {
                    if let Some(fn_obj) = deep_value(vm, face_obj, hook)
                        .and_then(|value| value.as_object())
                        .filter(|obj| vm.bx.heap.is_fn(*obj))
                    {
                        let fn_ref = vm.bx.heap.new_fn_ref(fn_obj);
                        if slot == 0 {
                            face.on_value = Some(fn_ref);
                        } else {
                            face.on_state = Some(fn_ref);
                        }
                    }
                }
            }
            let mut widgets = Vec::new();
            collect_widgets(&root, &mut widgets);
            for widget in widgets {
                let src = widget.script_source();
                if src == ScriptObject::ZERO {
                    continue;
                }
                let resolve = |vm: &ScriptVm<'_>, value: ScriptValue, for_bind: bool| -> Option<(String, String)> {
                    if let Some(obj) = value.as_object() {
                        let node = own_value(vm, obj, "node")?;
                        let node = node_name(vm, &node_objects, node)?;
                        let port = own_value(vm, obj, "port").and_then(|port| value_text(vm, port))?;
                        return Some((node, port));
                    }
                    let text = value_text(vm, value)?;
                    if text == "self" {
                        let node = graph_node.as_ref()?;
                        return Some((node.id.clone(), self_port(node, for_bind)?));
                    }
                    if let Some((node, port)) = text.split_once('.') {
                        return Some((node.to_string(), port.to_string()));
                    }
                    Some((node_id.clone(), text))
                };
                if let Some(value) = own_value(vm, src, "bind").filter(|v| !v.is_nil()) {
                    if let Some((node, port)) = resolve(vm, value, true) {
                        face.binds.push(Bind {
                            widget: widget.clone(),
                            node,
                            port,
                        });
                    }
                }
                if let Some(value) = own_value(vm, src, "show").filter(|v| !v.is_nil()) {
                    if let Some((node, port)) = resolve(vm, value, false) {
                        face.shows.push(Bind {
                            widget: widget.clone(),
                            node,
                            port,
                        });
                    }
                }
                if let Some(name) = own_value(vm, src, "param")
                    .filter(|v| !v.is_nil())
                    .and_then(|v| value_text(vm, v))
                {
                    face.params.push((widget.clone(), name));
                }
                if let Some(name) = own_value(vm, src, "param_bind")
                    .filter(|v| !v.is_nil())
                    .and_then(|v| value_text(vm, v))
                {
                    face.param_binds.push((widget.clone(), name));
                }
            }
            face
        });
        let _ = parent;
        if let Some(node) = graph.nodes.iter().find(|node| node.id == node_id) {
            self.fill_params_for(cx, &mounted, node);
        }
        mounted
    }

    /// Drop every mounted widget, then the isolate.
    pub fn free(mut self, cx: &mut Cx) {
        self.faces.clear();
        self.flow_face = None;
        self.bridge = None;
        self.node_objects.borrow_mut().clear();
        cx.free_splash_vm(self.vm_id);
    }

    pub fn face(&self, node: &str) -> Option<&MountedFace> {
        self.faces.get(node)
    }

    // -- drawing and events ---------------------------------------------------

    /// Draw one node's face where the caller's turtle is. The face subtree
    /// gets an empty scope: the host's scope data is the canvas's, not
    /// the isolate's.
    pub fn draw_face(&mut self, cx: &mut Cx2d, node: &str, walk: Walk) {
        let Some(root) = self.faces.get(node).map(|face| face.root.clone()) else {
            return;
        };
        if root.is_empty() {
            return;
        }
        let entry = enter_isolate(cx, self.vm_id);
        root.draw_walk_all(cx, &mut Scope::empty(), walk);
        leave_isolate(cx, entry);
    }

    pub fn draw_flow_face(&mut self, cx: &mut Cx2d, walk: Walk) {
        let Some(root) = self.flow_face.as_ref().map(|face| face.root.clone()) else {
            return;
        };
        if root.is_empty() {
            return;
        }
        let entry = enter_isolate(cx, self.vm_id);
        root.draw_walk_all(cx, &mut Scope::empty(), walk);
        leave_isolate(cx, entry);
    }

    /// Deliver an event to every mounted face (the ones on screen own the
    /// hit; the rest see nothing they can act on).
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let roots: Vec<WidgetRef> = self
            .faces
            .values()
            .chain(self.flow_face.iter())
            .map(|face| face.root.clone())
            .filter(|root| !root.is_empty())
            .collect();
        if roots.is_empty() {
            return;
        }
        let entry = enter_isolate(cx, self.vm_id);
        for root in roots {
            root.handle_event(cx, event, scope);
        }
        leave_isolate(cx, entry);
    }

    // -- filling widgets ------------------------------------------------------

    fn fill_params_for(&self, cx: &mut Cx, face: &MountedFace, node: &Node) {
        for (widget, name) in &face.params {
            let text = param_text(node, name);
            set_widget_text(cx, widget, &text);
            if name == "options" {
                if let Some(Literal::Arr(items)) = node_param(node, "options") {
                    let labels: Vec<String> = items.iter().filter_map(literal_text).collect();
                    widget.as_drop_down().set_labels(cx, labels);
                }
            }
        }
        for (widget, name) in &face.param_binds {
            let Some(value) = node_param(node, name) else {
                continue;
            };
            if let Some(slider) = widget.borrow_mut::<Slider>() {
                if let Literal::Num(number) = value {
                    drop(slider);
                    widget.as_slider().set_value(cx, *number);
                }
                continue;
            }
            if let Some(text) = literal_text(value) {
                set_widget_text(cx, widget, &text);
            }
        }
    }

    /// The graph changed under the instance: refresh every `param` and
    /// `param_bind` widget from the new node params.
    pub fn refresh_params(&mut self, cx: &mut Cx, graph: &Graph) {
        for node in &graph.nodes {
            if let Some(face) = self.faces.get(&node.id) {
                self.fill_params_for(cx, face, node);
            }
        }
    }

    /// Fill every `bind` widget from the instance's inputs table.
    pub fn fill_inputs(&mut self, cx: &mut Cx, row: &InstanceRow) {
        for face in self.faces.values().chain(self.flow_face.iter()) {
            for bind in &face.binds {
                let Some(text) = row.input_text(&bind.node, &bind.port) else {
                    continue;
                };
                if bind.widget.borrow::<TextInput>().is_some() {
                    let input = bind.widget.as_text_input();
                    if input.text() != text {
                        input.set_text(cx, &text);
                    }
                } else if bind.widget.borrow::<DropDown>().is_some() {
                    bind.widget.as_drop_down().set_selected_by_label(&text, cx);
                } else if bind.widget.borrow::<Slider>().is_some() {
                    if let Ok(number) = text.parse::<f64>() {
                        bind.widget.as_slider().set_value(cx, number);
                    }
                }
            }
        }
        let json = row.inputs.serialize_json();
        self.set_bridge_field(cx, "inputs", &json);
        self.set_bridge_string(cx, "state", &row.state);
    }

    fn set_bridge_field(&mut self, cx: &mut Cx, field: &str, json: &str) {
        let Some(bridge) = self.bridge.as_ref().map(|bridge| bridge.as_object()) else {
            return;
        };
        let Ok(parsed) = makepad_strict_json::parse(json.as_bytes()) else {
            return;
        };
        let field = field.to_string();
        cx.with_script_vm_id_trusted(self.vm_id, |vm| {
            let value = json_to_script(vm, &parsed);
            vm.bx
                .heap
                .set_value_def(bridge, LiveId::from_str(&field).into(), value);
        });
    }

    fn set_bridge_string(&mut self, cx: &mut Cx, field: &str, text: &str) {
        let Some(bridge) = self.bridge.as_ref().map(|bridge| bridge.as_object()) else {
            return;
        };
        let field = field.to_string();
        let text = text.to_string();
        cx.with_script_vm_id_trusted(self.vm_id, |vm| {
            let value = vm.bx.heap.new_string_from_str(&text);
            vm.bx
                .heap
                .set_value_def(bridge, LiveId::from_str(&field).into(), value);
        });
    }

    /// Widget changes on `bind` widgets → `(node, port, text)`.
    pub fn bind_changes(&self, actions: &Actions) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for face in self.faces.values().chain(self.flow_face.iter()) {
            for bind in &face.binds {
                if bind.widget.borrow::<TextInput>().is_some() {
                    let input = bind.widget.as_text_input();
                    if let Some(text) = input.changed(actions) {
                        out.push((bind.node.clone(), bind.port.clone(), text));
                    } else if let Some((text, _)) = input.returned(actions) {
                        out.push((bind.node.clone(), bind.port.clone(), text));
                    }
                } else if bind.widget.borrow::<DropDown>().is_some() {
                    if let Some(label) = bind.widget.as_drop_down().changed_label(actions) {
                        out.push((bind.node.clone(), bind.port.clone(), label));
                    }
                } else if bind.widget.borrow::<Slider>().is_some() {
                    if let Some(value) = bind.widget.as_slider().end_slide(actions) {
                        out.push((bind.node.clone(), bind.port.clone(), value.to_string()));
                    }
                }
            }
        }
        out
    }

    /// Widget changes on `param_bind` widgets → `(node, key, literal)`.
    pub fn param_changes(&self, actions: &Actions) -> Vec<(String, String, Literal)> {
        let mut out = Vec::new();
        for (node, face) in &self.faces {
            for (widget, key) in &face.param_binds {
                if widget.borrow::<Slider>().is_some() {
                    if let Some(value) = widget.as_slider().end_slide(actions) {
                        out.push((node.clone(), key.clone(), Literal::Num(value)));
                    }
                } else if widget.borrow::<ModelPicker>().is_some() {
                    // Its inner text input is read by `model_changes`.
                } else if widget.borrow::<TextInput>().is_some() {
                    if let Some(text) = widget.as_text_input().changed(actions) {
                        out.push((node.clone(), key.clone(), Literal::Str(text)));
                    }
                } else if widget.borrow::<DropDown>().is_some() {
                    if let Some(label) = widget.as_drop_down().changed_label(actions) {
                        out.push((node.clone(), key.clone(), Literal::Str(label)));
                    }
                }
            }
        }
        out
    }

    /// A ModelPicker's inner text input changed → `(node, key, text)`.
    pub fn model_changes(&self, cx: &mut Cx, actions: &Actions) -> Vec<(String, String, Literal)> {
        let mut out = Vec::new();
        for (node, face) in &self.faces {
            for (widget, key) in &face.param_binds {
                if widget.borrow::<ModelPicker>().is_some() {
                    if let Some(text) = widget.text_input(cx, ids!(name)).changed(actions) {
                        out.push((node.clone(), key.clone(), Literal::Str(text)));
                    }
                }
            }
        }
        out
    }

    // -- values, deltas, states ------------------------------------------------

    /// A port produced a value. Display widgets that `show` it are filled;
    /// returns `true` when some widget needs the bytes (an image) and the
    /// caller should fetch them and call again with `bytes`.
    pub fn push_value(
        &mut self,
        cx: &mut Cx,
        node: &str,
        port: &str,
        value: &ValueRef,
        bytes: Option<&ValueBytes>,
    ) -> bool {
        self.last_values
            .insert((node.to_string(), port.to_string()), value.clone());
        self.deltas.remove(&(node.to_string(), port.to_string()));
        let mut wants_bytes = false;
        let text = bytes
            .filter(|_| !value.ty.is_media())
            .map(|bytes| String::from_utf8_lossy(&bytes.bytes).into_owned())
            .or_else(|| preview_text(value))
            .unwrap_or_else(|| format!("{} · {} bytes", value.content_type, value.bytes));
        let mut hooks = Vec::new();
        for (id, face) in self.faces.iter().chain(
            self.flow_face
                .iter()
                .map(|face| (&self.instance, face)),
        ) {
            for show in &face.shows {
                if show.node != node || show.port != port {
                    continue;
                }
                let widget = &show.widget;
                if let Some(mut image) = widget.borrow_mut::<ValueImage>() {
                    match bytes {
                        Some(bytes) if value.ty == PortType::Image => image.set_value(cx, bytes),
                        _ if value.ty == PortType::Image => {
                            wants_bytes = true;
                            image.set_note(cx, "loading…");
                        }
                        _ => image.set_note(cx, &text),
                    }
                } else if let Some(mut view) = widget.borrow_mut::<ValueView>() {
                    match bytes {
                        Some(bytes) if value.ty == PortType::Image => view.set_image(cx, bytes),
                        _ if value.ty == PortType::Image => {
                            wants_bytes = true;
                            view.set_text(cx, "loading…");
                        }
                        _ => view.set_text(cx, &text),
                    }
                } else {
                    if !value.ty.is_media() && bytes.is_none() && value.bytes > 512 {
                        wants_bytes = true;
                    }
                    set_widget_text(cx, widget, &text);
                }
            }
            if let Some(hook) = face.on_value.clone() {
                hooks.push((id.clone(), hook));
            }
        }
        if wants_bytes && !self.wanted.iter().any(|digest| *digest == value.digest) {
            self.wanted.push(value.digest.clone());
        }
        if !hooks.is_empty() {
            let node = node.to_string();
            let port = port.to_string();
            let value = value.clone();
            let bytes = bytes.cloned();
            cx.with_script_vm_id(self.vm_id, |vm| {
                for (_id, hook) in hooks {
                    let node_arg = vm.bx.heap.new_string_from_str(&node);
                    let port_arg: ScriptValue = LiveId::from_str(&port).into();
                    let value_arg = value_to_script(vm, &value, bytes.as_ref());
                    vm.with_instruction_limit(HANDLER_INSTRUCTION_LIMIT, |vm| {
                        vm.call(hook.as_object().into(), &[node_arg, port_arg, value_arg]);
                    });
                }
                for error in vm.take_errors() {
                    log!("flow-ui: on_value handler: {error}");
                }
            });
        }
        self.store_bridge_value(cx, node, port, value, bytes);
        wants_bytes
    }

    fn store_bridge_value(
        &mut self,
        cx: &mut Cx,
        node: &str,
        port: &str,
        value: &ValueRef,
        bytes: Option<&ValueBytes>,
    ) {
        let Some(bridge) = self.bridge.as_ref().map(|bridge| bridge.as_object()) else {
            return;
        };
        let node = node.to_string();
        let port = port.to_string();
        let value = value.clone();
        let bytes = bytes.cloned();
        cx.with_script_vm_id_trusted(self.vm_id, |vm| {
            let values = own_value(vm, bridge, "values")
                .and_then(|value| value.as_object())
                .unwrap_or_else(|| {
                    let values = vm.bx.heap.new_object();
                    vm.bx
                        .heap
                        .set_value_def(bridge, id!(values).into(), values.into());
                    values
                });
            let by_node = own_value(vm, values, &node)
                .and_then(|value| value.as_object())
                .unwrap_or_else(|| {
                    let by_node = vm.bx.heap.new_object();
                    vm.bx.heap.set_value_def(
                        values,
                        LiveId::from_str(&node).into(),
                        by_node.into(),
                    );
                    by_node
                });
            let script_value = value_to_script(vm, &value, bytes.as_ref());
            vm.bx
                .heap
                .set_value_def(by_node, LiveId::from_str(&port).into(), script_value);
        });
    }

    /// Streaming text for a port: appended and shown live.
    pub fn push_delta(&mut self, cx: &mut Cx, node: &str, port: &str, text: &str) {
        let key = (node.to_string(), port.to_string());
        let full = self.deltas.entry(key).or_default();
        full.push_str(text);
        let full = full.clone();
        for face in self.faces.values().chain(self.flow_face.iter()) {
            for show in &face.shows {
                if show.node == node && show.port == port {
                    set_widget_text(cx, &show.widget, &full);
                }
            }
        }
    }

    /// A node changed state; the face hooks hear about it.
    pub fn push_state(&mut self, cx: &mut Cx, node: &str, state: &str) {
        let hooks: Vec<ScriptFnRef> = self
            .faces
            .values()
            .chain(self.flow_face.iter())
            .filter_map(|face| face.on_state.clone())
            .collect();
        if hooks.is_empty() {
            return;
        }
        let node = node.to_string();
        let state = state.to_string();
        cx.with_script_vm_id(self.vm_id, |vm| {
            for hook in hooks {
                let node_arg = vm.bx.heap.new_string_from_str(&node);
                let state_arg = vm.bx.heap.new_string_from_str(&state);
                vm.with_instruction_limit(HANDLER_INSTRUCTION_LIMIT, |vm| {
                    vm.call(hook.as_object().into(), &[node_arg, state_arg]);
                });
            }
            for error in vm.take_errors() {
                log!("flow-ui: on_state handler: {error}");
            }
        });
    }

    /// A new run started: streamed text starts over.
    pub fn reset_run(&mut self) {
        self.deltas.clear();
    }

    /// Bytes arrived for a wanted digest: re-push every value that has it.
    pub fn deliver_bytes(&mut self, cx: &mut Cx, cache: &mut ValueCache, digest: &str) {
        self.wanted.retain(|wanted| wanted != digest);
        let Some(bytes) = cache.get(digest) else {
            return;
        };
        let matching: Vec<((String, String), ValueRef)> = self
            .last_values
            .iter()
            .filter(|(_, value)| value.digest == digest)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        for ((node, port), value) in matching {
            self.push_value(cx, &node, &port, &value, Some(&bytes));
        }
    }
}

fn node_param<'a>(node: &'a Node, name: &str) -> Option<&'a Literal> {
    node.params
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
        .or_else(|| {
            node.inputs.iter().find_map(|input| match &input.value {
                makepad_flow::NodeInputValue::Literal(value) if input.port == name => Some(value),
                _ => None,
            })
        })
}

fn literal_text(value: &Literal) -> Option<String> {
    match value {
        Literal::Null => None,
        Literal::Bool(value) => Some(value.to_string()),
        Literal::Num(value) => Some(if value.fract() == 0.0 {
            (*value as i64).to_string()
        } else {
            value.to_string()
        }),
        Literal::Str(value) | Literal::Id(value) => Some(value.clone()),
        Literal::Arr(values) => Some(
            values
                .iter()
                .filter_map(literal_text)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        Literal::Obj(_) => Some(makepad_flow::Literal::serialize_json(value)),
    }
}

/// The text a `param:` widget shows for a node param (`run` and `ui` are
/// the source spans the graph carries).
pub fn param_text(node: &Node, name: &str) -> String {
    match name {
        "run" => node.fn_src.clone().unwrap_or_default(),
        "ui" => node.face_src.clone().unwrap_or_default(),
        "domain" => node.domain.clone().unwrap_or_default(),
        _ => match node_param(node, name) {
            Some(value) => literal_text(value).unwrap_or_default(),
            None => match node.inputs.iter().find(|input| input.port == name) {
                Some(input) => match &input.value {
                    makepad_flow::NodeInputValue::Edge(edge) => {
                        format!("← {}.{}", edge.from_node, edge.from_port)
                    }
                    makepad_flow::NodeInputValue::Literal(value) => {
                        literal_text(value).unwrap_or_default()
                    }
                },
                None => String::new(),
            },
        },
    }
}

/// Set text on whatever the widget is (value widgets, labels, markdown,
/// code views, text inputs).
fn set_widget_text(cx: &mut Cx, widget: &WidgetRef, text: &str) {
    if widget.borrow::<CodeView>().is_some() {
        widget.set_text(cx, text);
        return;
    }
    if widget.borrow::<TextInput>().is_some() {
        let input = widget.as_text_input();
        if input.text() != text {
            input.set_text(cx, text);
        }
        return;
    }
    if widget.borrow::<DropDown>().is_some() {
        widget.as_drop_down().set_selected_by_label(text, cx);
        return;
    }
    widget.set_text(cx, text);
}
