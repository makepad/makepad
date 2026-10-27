// The rustc reference and Relax export use only the page's console import.
const fs = require('fs');
const assert = require('node:assert/strict');
const m = new WebAssembly.Module(fs.readFileSync(process.argv[2]));
for (const i of WebAssembly.Module.imports(m)) {
    assert.equal(i.module, 'env'); assert.equal(i.name, 'js_console_log');
}
function instance() {
    let x;
    x = new WebAssembly.Instance(m, {env: {js_console_log: (p,n) => process.stdout.write(Buffer.from(new Uint8Array(x.memory.buffer,Number(p),Number(n))))}}).exports;
    return x;
}
if (process.argv[3] === 'probes') {
    for (let n=0; n<13; n++) {
        const x=instance();
        assert.throws(() => x.panic_probe(n), WebAssembly.RuntimeError);
    }
} else {
    const x=instance(); (x.t0 || x.run)();
}
