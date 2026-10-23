// Only the page console service is supplied; unexpected imports fail instantiation.
const fs = require('fs');
const m = new WebAssembly.Module(fs.readFileSync(process.argv[2]));
for (const i of WebAssembly.Module.imports(m)) {
    if (i.module !== 'env' || i.name !== 'js_console_log') throw new Error('unexpected import ' + i.module + '.' + i.name);
}
let x;
const imports = {env: {js_console_log: (p, n) => process.stderr.write(Buffer.from(new Uint8Array(x.memory.buffer, Number(p), Number(n))))}};
x = new WebAssembly.Instance(m, imports).exports;
const before = x.memory.buffer.byteLength;
x.t0();
if (x.memory.buffer.byteLength <= before) throw new Error('program allocator did not grow memory');
