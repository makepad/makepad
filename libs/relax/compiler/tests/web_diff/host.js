// node host.js <module.wasm>: runs `run` of a wasm32 build of web_diff.rs, printing what it
// passes to the page-side import env.relax_web_print (to stderr, as Relax's web runtime does).
const fs = require('fs');
const m = new WebAssembly.Module(fs.readFileSync(process.argv[2]));
let mem = null;
const imports = { env: { relax_web_print: (p, n) => process.stderr.write(Buffer.from(new Uint8Array(mem.buffer, p, n))) } };
for (const i of WebAssembly.Module.imports(m)) {
    if (!(imports[i.module] && imports[i.module][i.name])) {
        (imports[i.module] ??= {})[i.name] = () => { throw new Error('import ' + i.module + '.' + i.name); };
    }
}
const x = new WebAssembly.Instance(m, imports).exports;
mem = x.memory;
x.run();
