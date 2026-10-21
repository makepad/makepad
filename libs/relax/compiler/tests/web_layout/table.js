// node table.js <module.wasm>: prints the reference table of a wasm32 build of wasm_ref.rs.
const fs = require('fs');
const m = new WebAssembly.Module(fs.readFileSync(process.argv[2]));
const imports = {};
for (const i of WebAssembly.Module.imports(m)) {
    (imports[i.module] ??= {})[i.name] = () => { throw new Error('import ' + i.name); };
}
const x = new WebAssembly.Instance(m, imports).exports;
x.make_table();
const bytes = new Uint8Array(x.memory.buffer, x.table_ptr(), x.table_len());
process.stdout.write(new TextDecoder().decode(bytes));
