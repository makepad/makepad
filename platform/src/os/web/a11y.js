// The browser consumes the same semantic tree and action wire format as native adapters.
// The codec, role names, state bits and action bits come from Rust at bridge startup.
export class MakepadA11y {
    constructor(app, enabled) {
        this.app = app;
        this.codec = app.msg_class.a11y;
        this.enabled = enabled;
        this.nodes = new Map();
        this.elements = new Map();
        this.parents = new Map();
        this.focus = 0n;
        this.request = 0n;
        this.syncing = false;
        this.composing = new Set();
        this.inputState = new WeakMap();
        this.encoder = new TextEncoder();
        this.decoder = new TextDecoder();
        this.window = 0;
        this.layer = document.createElement("div");
        this.layer.dataset.makepadA11y = "";
        this.layer.style.cssText = "position:absolute;pointer-events:none;opacity:0;overflow:hidden;z-index:1000";
        this.layer.hidden = !enabled;
        document.body.appendChild(this.layer);
        this.live = [null, this.liveRegion("polite"), this.liveRegion("assertive")];
        this.checkAtStartup = new URLSearchParams(location.search).get("a11y_selfcheck") === "1";
        window.makepadA11ySelfCheck = () => this.selfCheck();
        window.makepadA11ySetEnabled = enabled => this.setEnabled(Boolean(enabled));
        app.canvas.tabIndex = -1;
        for (const [type, down] of [["keydown", true], ["keyup", false]]) {
            app.canvas.addEventListener(type, event => {
                if (this.enabled && event.key !== "Tab") this.rawKey(event, down);
            });
        }
        window.addEventListener("focus", () => this.syncFocus());
        window.addEventListener("scroll", () => this.syncGeometry(), true);
        this.resizeObserver = new ResizeObserver(() => this.syncGeometry());
        this.resizeObserver.observe(app.canvas);
        this.syncGeometry();
    }

    liveRegion(politeness) {
        const el = document.createElement("div");
        el.setAttribute("aria-live", politeness);
        el.setAttribute("aria-atomic", "true");
        this.layer.appendChild(el);
        return el;
    }

    setEnabled(enabled) {
        if (enabled === this.enabled) return;
        this.enabled = enabled;
        this.layer.hidden = !enabled;
        // The legacy IME textarea exists only while the semantic mirror is disabled.
        // With the mirror enabled, text_area is the focused semantic input itself.
        if (enabled) {
            const old = this.app.text_area;
            this.app.text_area = null;
            if (old) old.remove();
        } else {
            this.clear();
            this.app.bind_keyboard();
        }
        this.app.to_wasm.ToWasmA11yState({enabled});
        this.app.do_wasm_pump();
    }

    clear() {
        for (const el of this.elements.values()) el.remove();
        this.nodes.clear();
        this.elements.clear();
        this.parents.clear();
        this.composing.clear();
        this.focus = 0n;
        this.app.text_area = null;
        for (const el of this.live.slice(1)) el.replaceChildren();
    }

    syncGeometry() {
        if (!this.enabled) return;
        // A single layout read before writes, never interleaved with per-node writes.
        const r = this.app.canvas.getBoundingClientRect();
        Object.assign(this.layer.style, {left: `${r.left + window.scrollX}px`,
            top: `${r.top + window.scrollY}px`, width: `${r.width}px`, height: `${r.height}px`});
    }

    has(node, state) { return (node.states & this.codec.states[state]) !== 0n; }
    supports(node, action) { return (node.actions & this.codec.actions[action]) !== 0; }
    isText(node) { return node && (["text_field", "text_area", "search_field", "password_field"].includes(this.codec.roleNames[node.role]) || this.has(node, "editable")); }
    multiline(node) { return this.has(node, "multiline") || this.codec.roleNames[node.role] === "text_area"; }
    tag(node) {
        if (this.isText(node)) return this.multiline(node) ? "textarea" : "input";
        if (this.codec.roles[node.role] === "button") return "button";
        return "div";
    }
    domId(id) { return `makepad-a11y-${this.window}-${id.toString(16)}`; }
    attr(el, name, value) {
        if (value === null || value === undefined || value === "") {
            if (el.hasAttribute(name)) el.removeAttribute(name);
        } else if (el.getAttribute(name) !== String(value)) el.setAttribute(name, String(value));
    }

    create(node) {
        const el = document.createElement(this.tag(node));
        el.id = this.domId(node.id);
        el.dataset.a11yId = node.id.toString();
        el.style.cssText = "position:absolute;box-sizing:border-box;margin:0;padding:0;border:0;pointer-events:none";
        if (el.tagName === "BUTTON") el.type = "button";
        if (this.isText(node)) {
            el.autocomplete = "off";
            el.spellcheck = false;
            el.style.resize = "none";
            el.addEventListener("input", () => this.edit(node.id));
            el.addEventListener("compositionstart", () => this.composing.add(node.id));
            el.addEventListener("compositionend", () => {
                this.composing.delete(node.id);
                this.edit(node.id);
            });
            el.addEventListener("select", () => this.select(node.id));
            // Native text selection must not also hit the canvas's window-level pointer handlers.
            for (const type of ["mousedown", "mouseup", "mousemove", "touchstart", "touchmove", "touchend"]) {
                el.addEventListener(type, event => event.stopPropagation());
            }
        }
        el.addEventListener("focus", () => {
            this.app.text_area = this.isText(this.nodes.get(node.id)) ? el : null;
            if (!this.syncing) this.act(node.id, "focus");
        });
        el.addEventListener("click", event => {
            // Nested semantic nodes must not activate their ancestors too.
            event.stopPropagation();
            if (!this.isText(this.nodes.get(node.id))) {
                event.preventDefault();
                this.activate(node.id);
            }
        });
        el.addEventListener("keydown", event => this.key(node.id, event, true));
        el.addEventListener("keyup", event => this.key(node.id, event, false));
        return el;
    }

    attributes(node, el, previous) {
        const s = name => this.has(node, name), roleName = this.codec.roleNames[node.role];
        const attr = (name, value) => this.attr(el, name, value);
        attr("role", this.codec.roles[node.role]);
        attr("aria-label", node.name);
        attr("aria-description", node.description);
        attr("aria-roledescription", node.role_description);
        attr("aria-keyshortcuts", node.key_shortcut);
        attr("aria-placeholder", node.placeholder);
        attr("aria-valuenow", node.range?.now);
        attr("aria-valuemin", node.range?.min);
        attr("aria-valuemax", node.range?.max);
        attr("aria-valuetext", node.range ? node.value : null);
        const toggle = [null, "false", "true", "mixed"][node.toggle];
        attr("aria-checked", roleName === "toggle_button" ? null : toggle);
        attr("aria-pressed", roleName === "toggle_button" ? (toggle || String(s("pressed"))) : s("pressed") ? "true" : null);
        attr("aria-expanded", s("expanded") || s("collapsed") ? String(s("expanded")) : null);
        attr("aria-selected", s("selectable") || s("selected") ? String(s("selected")) : null);
        for (const [name, state] of [["disabled", "disabled"], ["hidden", "hidden"], ["required", "required"],
            ["busy", "busy"], ["modal", "modal"], ["multiselectable", "multiselectable"],
            ["invalid", "invalid"], ["haspopup", "has_popup"], ["readonly", "read_only"]]) {
            attr(`aria-${name}`, s(state) ? "true" : null);
        }
        attr("aria-level", node.level || null);
        attr("aria-posinset", node.pos_in_set || null);
        attr("aria-setsize", node.set_size || null);
        attr("aria-live", [null, "polite", "assertive"][node.live]);
        attr("aria-orientation", s("vertical") ? "vertical" : s("horizontal") ? "horizontal" : null);
        for (const [tag, name] of ["labelledby", "describedby", "controls", "flowto", "activedescendant", "errormessage"].entries()) {
            attr(`aria-${name}`, (node.relations[tag] || []).map(id => this.domId(id)).join(" "));
        }
        for (const [name, field] of [["rowcount", "rows"], ["colcount", "cols"], ["rowspan", "row_span"], ["colspan", "col_span"]]) {
            attr(`aria-${name}`, node.table?.[field] || null);
        }
        attr("aria-rowindex", node.table ? node.table.row + 1 : null);
        attr("aria-colindex", node.table ? node.table.col + 1 : null);
        el.tabIndex = s("focusable") && !s("disabled") && !s("hidden") ? 0 : -1;
        if (this.isText(node)) {
            if (el.tagName === "INPUT") el.type = s("protected") || roleName === "password_field" ? "password" : "text";
            el.readOnly = s("read_only") || !(this.supports(node, "replace_text") || this.supports(node, "set_value"));
            el.disabled = s("disabled");
            el.placeholder = node.placeholder;
            attr("aria-multiline", String(this.multiline(node)));
            // Canvas pointer input still sets widget focus. Once focused, this is also the IME
            // input and owns native selection, clipboard, mobile keyboard and composition.
            el.style.pointerEvents = this.focus === node.id ? "auto" : "none";
            if (!previous || previous.value !== node.value || previous.text?.content !== node.text?.content
                || previous.text?.selection[0] !== node.text?.selection[0]
                || previous.text?.selection[1] !== node.text?.selection[1]) this.syncText(node, el);
        } else if (!node.children.length) {
            // Static text has no ARIA role; it needs actual DOM text to be exposed.
            const text = node.text?.content || node.name || node.value;
            if (el.textContent !== text) el.textContent = text;
        } else if (previous && !previous.children.length) {
            el.textContent = "";
        }
    }

    position(node) {
        const el = this.elements.get(node.id), parent = this.nodes.get(this.parents.get(node.id));
        Object.assign(el.style, {left: `${node.bounds.x - (parent?.bounds.x || 0)}px`,
            top: `${node.bounds.y - (parent?.bounds.y || 0)}px`,
            width: `${Math.max(0, node.bounds.w)}px`, height: `${Math.max(0, node.bounds.h)}px`});
    }

    sameChildren(a, b) {
        return a && a.length === b.length && a.every((id, i) => id === b[i]);
    }

    apply(windowId, update) {
        if (!this.enabled) return;
        this.syncGeometry();
        this.syncing = true;
        try {
            const hadDomFocus = this.layer.contains(document.activeElement);
            if (update.full) this.clear();
            this.window = windowId;
            const oldFocus = this.focus;
            this.root = update.root;
            this.focus = update.focus;
            const position = new Set(), children = new Set(), previousNodes = new Map();
            for (const id of update.removed) {
                const node = this.nodes.get(id);
                for (const child of node?.children || []) if (this.parents.get(child) === id) this.parents.delete(child);
                this.elements.get(id)?.remove();
                this.elements.delete(id);
                this.nodes.delete(id);
                this.parents.delete(id);
                this.composing.delete(id);
            }
            // Finish the data diff before DOM writes: children can precede their parents on the wire.
            for (const node of update.nodes) {
                const previous = this.nodes.get(node.id);
                previousNodes.set(node.id, previous);
                if (!this.sameChildren(previous?.children, node.children)) {
                    for (const id of previous?.children || []) if (this.parents.get(id) === node.id) this.parents.delete(id);
                }
                this.nodes.set(node.id, node);
            }
            for (const node of update.nodes) {
                if (!this.sameChildren(previousNodes.get(node.id)?.children, node.children)) {
                    for (const id of node.children) this.parents.set(id, node.id);
                }
            }
            for (const node of update.nodes) {
                const previous = previousNodes.get(node.id);
                let el = this.elements.get(node.id);
                if (!el || el.tagName.toLowerCase() !== this.tag(node)) {
                    const replacement = this.create(node);
                    if (el) { replacement.append(...el.children); el.replaceWith(replacement); }
                    el = replacement;
                    this.elements.set(node.id, el);
                    previousNodes.delete(node.id);
                    const parent = this.parents.get(node.id);
                    if (parent !== undefined) children.add(parent);
                }
                this.attributes(node, el, previousNodes.get(node.id));
                position.add(node.id);
                if (!previous || previous.bounds.x !== node.bounds.x || previous.bounds.y !== node.bounds.y
                    || !this.sameChildren(previous.children, node.children)) {
                    for (const id of node.children) position.add(id);
                }
                if (!previous || !this.sameChildren(previous.children, node.children)) children.add(node.id);
            }
            for (const id of children) {
                const node = this.nodes.get(id), el = this.elements.get(id);
                if (!node || !el) continue;
                let next = el.firstElementChild;
                for (const childId of node.children) {
                    const child = this.elements.get(childId);
                    if (!child) continue;
                    if (child !== next) el.insertBefore(child, next);
                    next = child.nextElementSibling;
                }
                // A child detached from this parent is either moved above or no longer reachable.
                while (next) { const old = next; next = next.nextElementSibling; old.remove(); }
            }
            const root = this.elements.get(this.root);
            if (root && root.parentElement !== this.layer) this.layer.appendChild(root);
            for (const id of position) if (this.nodes.has(id)) this.position(this.nodes.get(id));
            if (oldFocus !== this.focus) {
                for (const id of [oldFocus, this.focus]) {
                    const node = this.nodes.get(id), el = this.elements.get(id);
                    if (node && this.isText(node)) el.style.pointerEvents = id === this.focus ? "auto" : "none";
                }
            }
            if (oldFocus !== this.focus || update.full || this.restoreFocus
                || hadDomFocus && document.activeElement !== this.elements.get(this.focus)) {
                this.restoreFocus = false;
                this.syncFocus();
            }
            const announcements = [[], [], []];
            for (const notification of update.notifications) {
                if (notification.tag === 8) announcements[notification.live]?.push(notification.text);
            }
            for (const live of [1, 2]) if (announcements[live].length) this.announce(announcements[live].join("\n"), live);
        } finally { this.syncing = false; }
        if (this.checkAtStartup && this.nodes.size) {
            this.checkAtStartup = false;
            requestAnimationFrame(() => console.log(this.selfCheck()));
        }
    }

    syncFocus() {
        if (!this.enabled || !document.hasFocus()) return;
        const el = this.elements.get(this.focus), node = this.nodes.get(this.focus);
        this.app.text_area = node && this.isText(node) ? el : null;
        if (!el && this.layer.contains(document.activeElement)) document.activeElement.blur();
        if (el && document.activeElement !== el) {
            const syncing = this.syncing;
            this.syncing = true;
            el.focus({preventScroll: true});
            this.syncing = syncing;
        }
    }

    focusKeyboard() {
        this.restoreFocus = true;
        // Preserve the canvas keyboard path when no semantic control owns focus. Defer until
        // the current pointer message has been dispatched; same-focus clicks need no tree diff.
        queueMicrotask(() => {
            if (!this.enabled || !document.hasFocus()) return;
            if (this.elements.has(this.focus)) this.syncFocus();
            else this.app.canvas.focus({preventScroll: true});
        });
    }

    announce(text, live) {
        const el = this.live[live];
        if (!el) return;
        // A new child is an addition even when two consecutive announcements have the same text.
        const message = document.createElement("span");
        message.textContent = text;
        el.replaceChildren(message);
    }

    act(id, name, arg, pump = true) {
        const node = this.nodes.get(id);
        if (!node || this.has(node, "disabled") || !this.supports(node, name)) return;
        const req = ++this.request;
        this.app.to_wasm.ToWasmA11yAction({window: this.window,
            bytes: this.codec.encodeAction(req, id, this.codec.actions[name], arg)});
        if (pump) this.app.do_wasm_pump();
        return req;
    }

    activate(id) {
        const node = this.nodes.get(id);
        if (!node) return;
        this.app.resume_audio_from_gesture();
        const action = ["press", "toggle", "select"].find(name => this.supports(node, name));
        if (action) this.act(id, action);
    }

    key(id, event, down) {
        event.stopPropagation();
        const node = this.nodes.get(id);
        if (!node || this.has(node, "disabled")) return;
        if (event.key === "Tab" || event.isComposing) return;
        const text = this.isText(node), modifier = event.ctrlKey || event.metaKey || event.altKey;
        if (!text && !modifier && ["Enter", " "].includes(event.key)) {
            event.preventDefault();
            if (down && !event.repeat) this.activate(id);
            return;
        }
        if ((!text || ["ArrowUp", "ArrowDown"].includes(event.key)) && !modifier && node.range
            && ["ArrowUp", "ArrowRight", "ArrowDown", "ArrowLeft"].includes(event.key)) {
            event.preventDefault();
            if (down) this.act(id, ["ArrowUp", "ArrowRight"].includes(event.key) ? "increment" : "decrement");
            return;
        }
        // Native inputs own editing; input/select events send semantic edits, never duplicate raw keys.
        if (text && event.key !== "Escape" && !(event.key === "Enter" && !this.multiline(node))) return;
        this.rawKey(event, down);
    }

    rawKey(event, down) {
        if (down) event.preventDefault();
        const key = {key_code: event.keyCode, char_code: event.charCode, is_repeat: event.repeat,
            time: event.timeStamp / 1000, modifiers: (event.shiftKey ? 1 : 0) | (event.ctrlKey ? 2 : 0)
                | (event.altKey ? 4 : 0) | (event.metaKey ? 8 : 0)};
        this.app.to_wasm[down ? "ToWasmKeyDown" : "ToWasmKeyUp"]({key});
        this.app.do_wasm_pump();
    }

    // DOM selection uses UTF-16 code units; the semantic wire uses UTF-8 byte offsets.
    byteOffset(text, offset) { return this.encoder.encode(text.slice(0, offset)).length; }
    charOffset(text, offset) {
        return this.decoder.decode(this.encoder.encode(text).slice(0, offset)).length;
    }
    selection(el) {
        const a = this.byteOffset(el.value, el.selectionStart), b = this.byteOffset(el.value, el.selectionEnd);
        return el.selectionDirection === "backward" ? [b, a] : [a, b];
    }
    syncText(node, el) {
        if (this.composing.has(node.id)) return;
        const value = node.text?.content ?? node.value;
        if (el.value !== value) el.value = value;
        if (node.text) {
            const [a, b] = node.text.selection.map(offset => this.charOffset(value, offset));
            if (el.selectionStart !== Math.min(a, b) || el.selectionEnd !== Math.max(a, b)
                || (a !== b && el.selectionDirection !== (a > b ? "backward" : "forward"))) {
                el.setSelectionRange(Math.min(a, b), Math.max(a, b), a > b ? "backward" : "forward");
            }
        }
        this.inputState.set(el, {value, selection: this.selection(el)});
    }
    edit(id) {
        if (this.syncing || this.composing.has(id)) return;
        const node = this.nodes.get(id), el = this.elements.get(id);
        if (!node || this.has(node, "read_only")) return;
        const old = this.inputState.get(el)?.value ?? node.text?.content ?? node.value;
        if (old === el.value) return;
        const [start, end] = this.selection(el);
        // A browser may emit input immediately after compositionend, before the next draw.
        // Keep only the latest submitted edit, so that event cannot send the same edit twice.
        this.inputState.set(el, {value: el.value, selection: [start, end]});
        // Send both operations in one pump so a redraw cannot overwrite the browser's new caret.
        if (this.supports(node, "replace_text")) {
            let a = 0, b = old.length, c = el.value.length;
            while (a < b && a < c && old[a] === el.value[a]) a++;
            // Do not split a surrogate pair when a single astral character changes.
            if (a && /[\uD800-\uDBFF]/.test(old[a - 1])) a--;
            while (b > a && c > a && old[b - 1] === el.value[c - 1]) { b--; c--; }
            if (b < old.length && /[\uDC00-\uDFFF]/.test(old[b])) { b++; c++; }
            this.act(id, "replace_text", {tag: 4, start: this.byteOffset(old, a),
                end: this.byteOffset(old, b), text: el.value.slice(a, c)}, false);
        } else this.act(id, "set_value", {tag: 2, text: el.value}, false);
        this.act(id, "set_text_selection", {tag: 3, start, end}, false);
        this.app.do_wasm_pump();
    }
    select(id) {
        if (this.syncing || this.composing.has(id)) return;
        const node = this.nodes.get(id), el = this.elements.get(id);
        const state = this.inputState.get(el);
        if (!node?.text || el.value !== (state?.value ?? node.text.content)) return;
        const [start, end] = this.selection(el);
        const selection = state?.selection ?? node.text.selection;
        if (start !== selection[0] || end !== selection[1]) {
            this.inputState.set(el, {value: el.value, selection: [start, end]});
            this.act(id, "set_text_selection", {tag: 3, start, end});
        }
    }

    selfCheck() {
        if (!this.enabled) return "makepad a11y self-check: disabled";
        const errors = [], canvas = this.app.canvas.getBoundingClientRect();
        if (!this.nodes.size) errors.push("no semantic tree received");
        const domNodes = this.layer.querySelectorAll("[data-a11y-id]");
        if (domNodes.length !== this.nodes.size) errors.push("DOM/tree node count");
        for (const el of domNodes) {
            if (this.elements.get(BigInt(el.dataset.a11yId)) !== el) errors.push(`${el.id}: unexpected element`);
        }
        for (const [id, node] of this.nodes) {
            const el = this.elements.get(id), label = id.toString(16);
            if (!el || !this.layer.contains(el)) { errors.push(`${label}: missing element`); continue; }
            if ((el.getAttribute("role") || "") !== this.codec.roles[node.role]) errors.push(`${label}: role`);
            if ((el.getAttribute("aria-label") || "") !== node.name) errors.push(`${label}: name`);
            const focusable = this.has(node, "focusable") && !this.has(node, "disabled") && !this.has(node, "hidden");
            if (el.tabIndex !== (focusable ? 0 : -1)) errors.push(`${label}: tabindex`);
            const r = el.getBoundingClientRect(), b = node.bounds;
            if (Math.abs(r.left - canvas.left - b.x) > 0.5 || Math.abs(r.top - canvas.top - b.y) > 0.5
                || Math.abs(r.width - Math.max(0, b.w)) > 0.5 || Math.abs(r.height - Math.max(0, b.h)) > 0.5) errors.push(`${label}: bounds`);
            const parent = this.elements.get(this.parents.get(id)) || this.layer;
            if (el.parentElement !== parent) errors.push(`${label}: parent`);
            if (node.children.some((child, i) => el.children[i] !== this.elements.get(child))) errors.push(`${label}: child order`);
        }
        return `makepad a11y self-check: ${errors.length ? "FAIL" : "PASS"} (${this.nodes.size} nodes)${errors.length ? "\n" + errors.join("\n") : ""}`;
    }
}
