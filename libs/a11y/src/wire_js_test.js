const assert = require("node:assert/strict");
const update = codec.decodeUpdate(bytes), id = 0xffff123456789abcn;
assert.equal(update.full, true);
assert.equal(update.root, id);
assert.equal(update.focus, id);
assert.deepEqual(update.removed, [99n]);
assert.deepEqual(update.nodes[0], {
    id, role: 40, states: 32769n, actions: 65537,
    bounds: {x: 10.5, y: -20, w: 80, h: 24}, name: "Name 🦀", description: "Description",
    value: "hé🦀", placeholder: "Placeholder", key_shortcut: "Control+S", role_description: "Editor",
    class: "TextInput", path: "root.edit", range: {min: -1, max: 10, now: 2.5, step: 0.25},
    toggle: 3, level: 2, pos_in_set: 3, set_size: 4, live: 1,
    relations: {0: [3n], 2: [4n]}, custom_actions: ["archive"], children: [2n],
    text: {content: "hé🦀", selection: [7, 1],
        lines: [{start: 0, rect: {x: 10.5, y: -20, w: 80, h: 24}, baseline: 18}], glyph_x: [0, 8, 16, 32]},
    table: {rows: 5, cols: 6, row: 1, col: 2, row_span: 3, col_span: 4},
});
assert.deepEqual(update.nodes[1], {
    id: 2n, role: 0, states: 0n, actions: 0, bounds: {x: 0, y: 0, w: 0, h: 0},
    name: "", description: "", value: "", placeholder: "", key_shortcut: "", role_description: "",
    class: "", path: "", range: null, toggle: 0, level: 0, pos_in_set: 0, set_size: 0, live: 0,
    relations: {}, custom_actions: [], children: [], text: null, table: null,
});
assert.deepEqual(update.notifications, [
    {tag: 0, node: id}, {tag: 1, node: id}, {tag: 2, node: id}, {tag: 3, node: id, old: 1n, new: 32768n},
    {tag: 4, node: id}, {tag: 5, node: id, start: 1, removed: 2, inserted: 4},
    {tag: 6, node: id}, {tag: 7, node: id}, {tag: 8, text: "Saved 🦀", live: 2},
]);
assert.equal(codec.roles[40], "textbox");
assert.equal(codec.roleNames[40], "text_field");
assert.equal(codec.states.editable, 32768n);
assert.equal(codec.actions.replace_text, 65536);
for (let i = 0; i < bytes.length; i++) assert.throws(() => codec.decodeUpdate(bytes.subarray(0, i)));
assert.throws(() => codec.decodeUpdate(new Uint8Array([...bytes, 0])));
const invalid = bytes.slice(); invalid[0] = 0;
assert.throws(() => codec.decodeUpdate(invalid));
for (const arg of [{tag: 0}, {tag: 1, value: 2.5}, {tag: 2, text: "hé🦀"},
    {tag: 3, start: 7, end: 1}, {tag: 4, start: 1, end: 7, text: "é🦀"},
    {tag: 5, x: 1.5, y: -2}, {tag: 6, text: "archive"}]) {
    console.log([...codec.encodeAction(0xffffffffffffffffn, id, codec.actions.replace_text, arg)].join(","));
}
