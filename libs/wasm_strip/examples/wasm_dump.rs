//! Prints a module's functions as decoded instructions: `wasm_dump <file.wasm> [func]`.
use makepad_wasm_strip::opt::decode::decode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let module = decode(&std::fs::read(&args[1]).expect("read")).expect("decode");
    let imported = module.num_imported_funcs() as usize;
    let only: Option<usize> = args.get(2).map(|a| a.parse().expect("func index"));
    println!("exports: {:?}", module.exports);
    println!("types: {:?}", module.types);
    println!("funcs: {:?}", module.funcs.iter().map(|f| f.ty).collect::<Vec<_>>());
    println!("elems: {:?}", module.elems);
    for (i, func) in module.funcs.iter().enumerate() {
        let index = imported + i;
        if only.map_or(false, |only| only != index) {
            continue;
        }
        println!("func {index} type {:?} locals {:?}", module.types[func.ty as usize], func.locals);
        let mut depth = 1;
        for instr in &func.body {
            if matches!(instr, makepad_wasm_strip::opt::ir::Instr::End | makepad_wasm_strip::opt::ir::Instr::Else) {
                depth -= 1;
            }
            println!("{}{:?}", "  ".repeat(depth), instr);
            if instr.is_block_start() || matches!(instr, makepad_wasm_strip::opt::ir::Instr::Else) {
                depth += 1;
            }
        }
    }
}
