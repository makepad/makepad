// node relax_host.js <module.wasm>: runs Relax's wasm export of web_diff.rs (relax_backend::wasm,
// MAKEPAD_RUN_WASM_EXPORT) as the page would: the root export `t0` is main; the imports are the
// export's runtime entries (r.*) and C symbols (c.*), given here with their Relax semantics.
const fs = require('fs');
const m = new WebAssembly.Module(fs.readFileSync(process.argv[2]));
let x = null;
const u8 = () => new Uint8Array(x.m.buffer);
let top = 0;
const sizes = new Map();
function alloc(size, align) {
    const a = Math.max(Number(align), 8);
    top = Math.ceil(top / a) * a;
    const p = top;
    top += Math.max(Number(size), 1);
    const need = top - x.m.buffer.byteLength;
    if (need > 0) x.m.grow(Math.ceil(need / 65536));
    sizes.set(p, Number(size));
    return BigInt(p);
}
const M = (1n << 64n) - 1n;
const imports = {
    r: {
        panic_bounds: (i, n) => { throw new Error('index out of bounds: ' + i + ' >= ' + n); },
        panic_site: (s) => { throw new Error('panic at site ' + s); },
        divrem128: (mode, alo, ahi, blo, bhi) => {
            const U = (lo, hi) => ((BigInt.asUintN(64, hi) << 64n) | BigInt.asUintN(64, lo));
            let a = U(alo, ahi), b = U(blo, bhi), r;
            if (mode & 2n) {
                a = BigInt.asIntN(128, a); b = BigInt.asIntN(128, b);
                r = (mode & 1n) ? a % b : a / b;
            } else {
                r = (mode & 1n) ? a % b : a / b;
            }
            r = BigInt.asUintN(128, r);
            return [BigInt.asIntN(64, r & M), BigInt.asIntN(64, r >> 64n)];
        },
    },
    c: {
        relax_web_print: (p, n) => process.stderr.write(Buffer.from(u8().subarray(Number(p), Number(p) + Number(n)))),
        relax_alloc: (size, align) => alloc(size, align),
        relax_dealloc: () => {},
        relax_realloc: (p, old, align, size) => {
            const q = alloc(size, align);
            u8().copyWithin(Number(q), Number(p), Number(p) + Math.min(Number(old), Number(size)));
            return q;
        },
    },
};
for (const i of WebAssembly.Module.imports(m)) {
    if (!(imports[i.module] && imports[i.module][i.name])) {
        (imports[i.module] ??= {})[i.name] = () => { throw new Error('import ' + i.module + '.' + i.name); };
    }
}
x = new WebAssembly.Instance(m, imports).exports;
top = x.h.value;
x.t0();
