// Node-only logic rig. No browser, layout engine, GPU, packages or permission prompts.
// Run from the repository root: node tools/a11y_rigs/web_logic.mjs
import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
const source = readFileSync(new URL("../../platform/src/os/web/a11y.js", import.meta.url), "utf8");
const {MakepadA11y} = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);

// Minimal DOM double: models focus, nesting and selection, but makes no claim about browser AX or pixels.
class Element {
    constructor(tag) {
        this.tagName = tag.toUpperCase(); this.children = []; this.attributes = new Map();
        this.dataset = {}; this.events = new Map(); this.writes = 0;
        this.style = new Proxy({}, {set: (o, k, v) => { this.writes++; o[k] = v; return true; }});
        this.value = ""; this.selectionStart = 0; this.selectionEnd = 0; this.selectionDirection = "none";
        this.text = ""; this.tabIndex = -1;
    }
    appendChild(el) { this.insertBefore(el, null); return el; }
    append(...els) { for (const el of els) this.appendChild(el); }
    insertBefore(el, next) {
        el.remove(); const i = next ? this.children.indexOf(next) : this.children.length;
        assert.ok(i >= 0); this.children.splice(i, 0, el); el.parentElement = this;
    }
    remove() {
        if (this.parentElement) {
            const parent = this.parentElement; parent.children.splice(parent.children.indexOf(this), 1);
            this.parentElement = null;
        }
    }
    replaceWith(el) { this.parentElement.insertBefore(el, this); this.remove(); }
    replaceChildren(...els) { for (const child of [...this.children]) child.remove(); this.text = ""; this.append(...els); }
    set textContent(text) { this.replaceChildren(); this.text = text; }
    get textContent() { return this.text + this.children.map(el => el.textContent).join(""); }
    get firstElementChild() { return this.children[0] || null; }
    get nextElementSibling() { return this.parentElement?.children[this.parentElement.children.indexOf(this) + 1] || null; }
    setAttribute(name, value) { this.writes++; this.attributes.set(name, String(value)); }
    getAttribute(name) { return this.attributes.get(name) ?? null; }
    hasAttribute(name) { return this.attributes.has(name); }
    removeAttribute(name) { this.writes++; this.attributes.delete(name); }
    addEventListener(name, callback) { this.events.set(name, callback); }
    focus() { document.activeElement = this; this.events.get("focus")?.(); }
    blur() { document.activeElement = document.body; }
    contains(el) { return el === this || this.children.some(child => child.contains(el)); }
    querySelectorAll() {
        return this.children.flatMap(el => [...(el.dataset.a11yId ? [el] : []), ...el.querySelectorAll()]);
    }
    setSelectionRange(a, b, direction) { this.selectionStart = a; this.selectionEnd = b; this.selectionDirection = direction; }
    getBoundingClientRect() {
        const parent = this.parentElement?.getBoundingClientRect() || {left: 0, top: 0};
        return {left: parent.left + (parseFloat(this.style.left) || 0), top: parent.top + (parseFloat(this.style.top) || 0),
            width: parseFloat(this.style.width) || 0, height: parseFloat(this.style.height) || 0};
    }
}
globalThis.document = {body: new Element("body"), createElement: tag => new Element(tag), hasFocus: () => true};
globalThis.window = {scrollX: 0, scrollY: 0, addEventListener() {}};
globalThis.location = {search: ""};
globalThis.ResizeObserver = class {observe() {}};
globalThis.requestAnimationFrame = callback => callback();
const states = {focusable: 1n, focused: 2n, disabled: 4n, hidden: 8n, selected: 16n, selectable: 32n,
    expanded: 64n, collapsed: 128n, pressed: 256n, read_only: 512n, required: 1024n, busy: 2048n,
    modal: 4096n, multiselectable: 8192n, editable: 32768n, invalid: 262144n, has_popup: 524288n,
    multiline: 1048576n, protected: 2097152n, vertical: 4194304n, horizontal: 8388608n};
const actions = {focus: 1, press: 2, toggle: 4, increment: 32, decrement: 64, set_value: 128,
    select: 256, set_text_selection: 32768, replace_text: 65536};
const codec = {states, actions, roles: {0: "", 1: "application", 6: "button", 7: "button", 34: "slider", 40: "textbox", 41: "textbox", 44: ""},
    roleNames: {0: "generic", 1: "window", 6: "button", 7: "toggle_button", 34: "slider", 40: "text_field", 41: "text_area", 44: "static_text"},
    encodeAction: (req, id, action, arg) => ({req, id, action, arg})};
const sent = [], canvas = new Element("canvas");
Object.assign(canvas.style, {left: "20px", top: "30px", width: "800px", height: "600px"});
let pumps = 0;
const app = {canvas, msg_class: {a11y: codec}, to_wasm: {
    ToWasmA11yAction: message => sent.push(message.bytes), ToWasmA11yState: message => sent.push(message),
    ToWasmKeyDown: message => sent.push(message), ToWasmKeyUp: message => sent.push(message)},
    do_wasm_pump: () => pumps++, resume_audio_from_gesture() {}, bind_keyboard() { this.text_area = document.createElement("textarea"); document.body.appendChild(this.text_area); }};
const mirror = new MakepadA11y(app, true); app.a11y = mirror;
function node(id, fields = {}) {
    return {id: BigInt(id), role: 0, name: "", description: "", value: "", role_description: "", key_shortcut: "",
        placeholder: "", states: 0n, actions: 0, bounds: {x: 0, y: 0, w: 100, h: 100}, range: null,
        toggle: 0, level: 0, pos_in_set: 0, set_size: 0, live: 0, relations: {}, table: null,
        children: [], text: null, ...fields};
}
function apply(nodes, fields = {}) { mirror.apply(0, {full: false, root: 1n, focus: 2n, removed: [], notifications: [], nodes, ...fields}); }
const root = node(1, {role: 1, children: [2n, 3n, 4n, 5n]}), button = node(2, {
    role: 6, name: "Save", states: states.focusable, actions: actions.focus | actions.press,
    bounds: {x: 10, y: 20, w: 80, h: 24}});
const edit = node(3, {role: 40, name: "Name", states: states.focusable | states.editable,
    actions: actions.focus | actions.replace_text | actions.set_text_selection,
    text: {content: "hé🦀", selection: [7, 1]}, bounds: {x: 20, y: 60, w: 120, h: 24}});
const slider = node(4, {role: 34, name: "Level", states: states.focusable,
    actions: actions.focus | actions.increment | actions.decrement, range: {min: 0, max: 10, now: 5}});
const label = node(5, {role: 44, name: "Static text"});
apply([edit, button, slider, label, root], {full: true});
assert.match(mirror.selfCheck(), /PASS \(5 nodes\)/);
assert.equal(sent.length, 0, "tree focus must not echo a focus action");
assert.equal(document.activeElement, mirror.elements.get(2n));
assert.equal(mirror.elements.get(5n).textContent, "Static text");
assert.equal(mirror.elements.get(3n).selectionStart, 1);
assert.equal(mirror.elements.get(3n).selectionEnd, 4);
assert.equal(mirror.elements.get(3n).selectionDirection, "backward");
const buttonElement = mirror.elements.get(2n), before = buttonElement.writes;
apply([node(5, {role: 44, name: "Updated text"})]);
assert.equal(buttonElement.writes, before, "an unrelated diff must not write the button");
assert.equal(mirror.elements.get(2n), buttonElement);
assert.equal(document.activeElement, buttonElement);

// Reparenting, sibling order and geometry work independently of wire node order.
apply([node(6, {bounds: {x: 50, y: 10, w: 200, h: 100}, children: [2n]}), node(1, {role: 1, children: [6n, 3n, 4n, 5n]})]);
assert.equal(buttonElement.parentElement, mirror.elements.get(6n));
assert.equal(buttonElement.style.left, "-40px");
assert.match(mirror.selfCheck(), /PASS/);
apply([node(6, {bounds: {x: 60, y: 10, w: 200, h: 100}, children: [2n]})]);
assert.equal(buttonElement.style.left, "-50px");
assert.match(mirror.selfCheck(), /PASS/);
apply([node(1, {role: 1, children: [2n, 4n, 3n, 5n]})], {removed: [6n]});
assert.equal(buttonElement.parentElement, mirror.elements.get(1n));
assert.match(mirror.selfCheck(), /PASS/);

const key = (key, extra = {}) => ({key, keyCode: 0, charCode: 0, timeStamp: 0,
    repeat: false, preventDefault() { this.prevented = true; }, stopPropagation() {}, ...extra});
mirror.key(2n, key("Enter"), true);
mirror.key(2n, key("Enter"), false);
assert.equal(sent.pop().action, actions.press);
mirror.key(4n, key("ArrowRight"), true);
assert.equal(sent.pop().action, actions.increment);
const tab = key("Tab"); mirror.key(2n, tab, true); assert.equal(tab.prevented, undefined);
apply([node(2, {...button, states: states.focusable | states.disabled})]);
mirror.activate(2n); assert.equal(sent.length, 0);
assert.equal(buttonElement.tabIndex, -1);
assert.equal(buttonElement.getAttribute("aria-disabled"), "true");
apply([button]); assert.equal(buttonElement.getAttribute("aria-disabled"), null);

// Native input and composition yield one UTF-8 edit and a directional selection in one pump.
const input = mirror.elements.get(3n);
input.focus(); assert.equal(app.text_area, input); assert.equal(sent.pop().action, actions.focus);
apply([], {focus: 3n});
mirror.composing.add(3n);
input.value = "hé🦁"; input.setSelectionRange(4, 4, "none");
mirror.edit(3n); assert.equal(sent.length, 0);
mirror.composing.delete(3n); const oldPumps = pumps;
mirror.edit(3n);
assert.equal(pumps, oldPumps + 1);
assert.deepEqual(sent.shift().arg, {tag: 4, start: 3, end: 7, text: "🦁"});
assert.deepEqual(sent.shift().arg, {tag: 3, start: 7, end: 7});
mirror.edit(3n); assert.equal(sent.length, 0, "the final input event must not replay composition before the next frame");
apply([node(3, {...edit, text: {content: "hé🦁", selection: [7, 7]}})], {focus: 3n});
mirror.edit(3n); assert.equal(sent.length, 0, "compositionend followed by input must not duplicate the edit");
input.setSelectionRange(1, 4, "backward"); mirror.select(3n);
assert.deepEqual(sent.pop().arg, {tag: 3, start: 7, end: 1});
const backspace = key("Backspace"); mirror.key(3n, backspace, true);
assert.equal(sent.length, 0); assert.equal(backspace.prevented, undefined);

// Full replacement resets attributes and native control kind, removals disappear, disable releases IME.
apply([root, button, node(3, {...edit, states: states.focusable | states.editable | states.multiline}), slider, label], {full: true, focus: 3n});
assert.equal(mirror.elements.get(3n).tagName, "TEXTAREA");
assert.equal(app.text_area, mirror.elements.get(3n));
apply([node(1, {role: 1, children: [2n, 3n, 4n]})], {focus: 3n, removed: [5n], notifications: [{tag: 8, live: 1, text: "Saved"}]});
assert.equal(mirror.elements.has(5n), false);
assert.equal(mirror.live[1].textContent, "Saved");
mirror.setEnabled(false); assert.equal(mirror.nodes.size, 0); assert.equal(mirror.layer.hidden, true);
const fallback = app.text_area; assert.equal(fallback.tagName, "TEXTAREA");
mirror.setEnabled(true); assert.equal(app.text_area, null); assert.equal(fallback.parentElement, null);
assert.match(mirror.selfCheck(), /FAIL.*0 nodes/);
apply([root, button, edit, slider, label], {full: true, focus: 3n});
const originalInput = mirror.elements.get(3n);
apply([node(3, {...edit, states: states.focusable | states.editable | states.multiline})], {focus: 3n});
assert.notEqual(mirror.elements.get(3n), originalInput);
assert.equal(document.activeElement, mirror.elements.get(3n), "replacing the focused input must keep DOM focus");
apply([node(3, {...edit, role: 41})], {focus: 3n});
assert.equal(mirror.elements.get(3n).getAttribute("aria-multiline"), "true");
const enter = key("Enter"); mirror.key(3n, enter, true); assert.equal(enter.prevented, undefined);
console.log("web mirror logic: PASS (diffs, hierarchy, focus, actions, Unicode edits, IME, enable/disable)");
