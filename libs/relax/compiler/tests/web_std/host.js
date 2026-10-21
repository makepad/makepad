// node host.js <module.wasm>: runs run_export of a wasm32 build of web_std.rs; relax_web_print writes to stdout.
const fs = require('fs');
const m = new WebAssembly.Module(fs.readFileSync(process.argv[2]));
let mem = null;
const env = {
    relax_web_print: (ptr, len) => {
        process.stdout.write(Buffer.from(new Uint8Array(mem.buffer, ptr, len)));
    },
};
const imports = { env };
for (const i of WebAssembly.Module.imports(m)) {
    if (i.module === 'env' && i.name === 'relax_web_print') continue;
    (imports[i.module] ??= {})[i.name] = () => { throw new Error('import ' + i.module + '.' + i.name); };
}
const x = new WebAssembly.Instance(m, imports).exports;
mem = x.memory;
x.run_export();
