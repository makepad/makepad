// Fixed wire primitives, header and notification layout. Node fields and constants are
// emitted by wire_js.rs. IDs and state masks stay BigInt, including high-bit part IDs.
const utf8Decoder = new TextDecoder("utf-8", {fatal: true});
const utf8Encoder = new TextEncoder();
class Reader {
    constructor(bytes) {
        this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        this.bytes = bytes;
        this.pos = 0;
    }
    take(n) {
        const p = this.pos;
        if (n > this.bytes.length - p) throw new Error("a11y wire: truncated");
        this.pos += n;
        return p;
    }
    u8() { return this.view.getUint8(this.take(1)); }
    u16() { return this.view.getUint16(this.take(2), true); }
    u32() { return this.view.getUint32(this.take(4), true); }
    u64() { return this.view.getBigUint64(this.take(8), true); }
    f32() { return this.view.getFloat32(this.take(4), true); }
    f64() { return this.view.getFloat64(this.take(8), true); }
    str() {
        const n = this.u32(), p = this.take(n);
        // Shared wasm memory is not accepted by TextDecoder on every browser.
        return utf8Decoder.decode(this.bytes.slice(p, p + n));
    }
    list(min, read) {
        const n = this.u32();
        if (n * min > this.bytes.length - this.pos) throw new Error("a11y wire: invalid count");
        return Array.from({length: n}, () => read());
    }
    ids() { return this.list(8, () => this.u64()); }
    strs() { return this.list(4, () => this.str()); }
    rect() { return {x: this.f32(), y: this.f32(), w: this.f32(), h: this.f32()}; }
    range() { return {min: this.f64(), max: this.f64(), now: this.f64(), step: this.f64()}; }
    relations() {
        const out = {}, n = this.u8();
        for (let i = 0; i < n; i++) out[this.u8()] = this.ids();
        return out;
    }
    text() {
        return {
            content: this.str(), selection: [this.u32(), this.u32()],
            lines: this.list(24, () => ({start: this.u32(), rect: this.rect(), baseline: this.f32()})),
            glyph_x: this.list(4, () => this.f32()),
        };
    }
    table() {
        return {rows: this.u32(), cols: this.u32(), row: this.u32(), col: this.u32(),
            row_span: this.u32(), col_span: this.u32()};
    }
    notification() {
        const tag = this.u8();
        if (tag === 8) return {tag, text: this.str(), live: this.u8()};
        if (tag > 8) throw new Error("a11y wire: unknown notification");
        const n = {tag, node: this.u64()};
        if (tag === 3) { n.old = this.u64(); n.new = this.u64(); }
        if (tag === 5) { n.start = this.u32(); n.removed = this.u32(); n.inserted = this.u32(); }
        return n;
    }
}
function decodeUpdate(bytes) {
    const r = new Reader(bytes);
    if (r.u32() !== MAGIC || r.u16() !== VERSION) throw new Error("a11y wire: header mismatch");
    const update = {full: r.u8() !== 0, root: r.u64(), focus: r.u64(),
        nodes: r.list(16, () => node(r)), removed: r.ids(),
        notifications: r.list(1, () => r.notification())};
    if (r.pos !== bytes.length) throw new Error("a11y wire: trailing bytes");
    return update;
}
// ActionArg uses the Rust wire tags: None, Number, Text, Range, Replace, Scroll, Custom.
function encodeAction(req, id, action, arg = {tag: 0}) {
    const text = utf8Encoder.encode(arg.text || "");
    const sizes = [0, 8, 4 + text.length, 8, 12 + text.length, 8, 4 + text.length];
    if (sizes[arg.tag] === undefined) throw new Error("a11y wire: unknown action argument");
    const bytes = new Uint8Array(21 + sizes[arg.tag]), v = new DataView(bytes.buffer);
    v.setBigUint64(0, BigInt(req), true); v.setBigUint64(8, BigInt(id), true);
    v.setUint32(16, action, true); v.setUint8(20, arg.tag);
    if (arg.tag === 1) v.setFloat64(21, arg.value, true);
    if (arg.tag === 3 || arg.tag === 4) {
        v.setUint32(21, arg.start, true); v.setUint32(25, arg.end, true);
    }
    if (arg.tag === 5) { v.setFloat32(21, arg.x, true); v.setFloat32(25, arg.y, true); }
    if (arg.tag === 2 || arg.tag === 4 || arg.tag === 6) {
        const p = arg.tag === 4 ? 29 : 21;
        v.setUint32(p, text.length, true); bytes.set(text, p + 4);
    }
    return bytes;
}
