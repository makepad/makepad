//! Why a function is still in a module: `wasm_why <file.wasm> <name part>`
//! prints each matching function's direct callers and its table slots.
use makepad_wasm_strip::opt::demangle::demangle;
use makepad_wasm_strip::opt::ir::{decode, ElemItems, ElemMode, Instr};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let module = decode(&std::fs::read(&args[1]).expect("read")).expect("decode");
    let names: std::collections::HashMap<u32, String> = module
        .names
        .as_ref()
        .map(|n| n.funcs.iter().map(|(i, s)| (*i, demangle(s).map(|d| d.name).unwrap_or_else(|| s.clone()))).collect())
        .unwrap_or_default();
    let imported = module.num_imported_funcs();
    let name = |i: u32| names.get(&i).cloned().unwrap_or_else(|| format!("#{i}"));
    if args[2] == "--addr" {
        // Code and data pointing at or up to 400 bytes before an address.
        let addr: u32 = args[3].parse().expect("addr");
        for (j, f) in module.funcs.iter().enumerate() {
            for x in &f.body {
                if let Instr::I32Const(v) = x {
                    let v = *v as u32;
                    if v <= addr && addr - v < 400 {
                        let c = name(imported + j as u32);
                        println!("   {} points at {v} (+{})", &c[..c.len().min(160)], addr - v);
                    }
                }
            }
        }
        return;
    }
    if args[2] == "--slot" {
        // Where a table index is held in the data, and the code that points
        // just before it.
        let slot: u32 = args[3].parse().expect("slot");
        for (j, f) in module.funcs.iter().enumerate() {
            if f.body.contains(&Instr::I32Const(slot as i32)) {
                let c = name(imported + j as u32);
                println!("const {slot} in {}", &c[..c.len().min(160)]);
            }
        }
        let mut passive = std::collections::HashMap::new();
        for f in &module.funcs {
            for w in f.body.windows(4) {
                if let [Instr::I32Const(dst), Instr::I32Const(src), Instr::I32Const(_), Instr::MemoryInit { data, .. }] = w {
                    passive.insert(*data as usize, (*dst as u32).wrapping_sub(*src as u32));
                }
            }
        }
        for (di, d) in module.datas.iter().enumerate() {
            let start = match &d.mode {
                makepad_wasm_strip::opt::ir::DataMode::Active { offset, .. } => match offset.first() { Some(Instr::I32Const(v)) => *v as u32, _ => continue },
                _ => match passive.get(&di) { Some(s) => *s, None => continue },
            };
            for at in (0..d.bytes.len().saturating_sub(3)).step_by(4) {
                if u32::from_le_bytes([d.bytes[at], d.bytes[at + 1], d.bytes[at + 2], d.bytes[at + 3]]) != slot { continue }
                let addr = start + at as u32;
                println!("slot {slot} held at {addr}");
                for (dj, d2) in module.datas.iter().enumerate() {
                    let Some(s2) = passive.get(&dj).copied().or_else(|| match &d2.mode { makepad_wasm_strip::opt::ir::DataMode::Active { offset, .. } => match offset.first() { Some(Instr::I32Const(v)) => Some(*v as u32), _ => None }, _ => None }) else { continue };
                    for at2 in (0..d2.bytes.len().saturating_sub(3)).step_by(4) {
                        let w = u32::from_le_bytes([d2.bytes[at2], d2.bytes[at2 + 1], d2.bytes[at2 + 2], d2.bytes[at2 + 3]]);
                        if w <= addr && addr - w < 400 {
                            println!("   data word at {} points at {w} (+{})", s2 + at2 as u32, addr - w);
                        }
                    }
                }
                for (j, f) in module.funcs.iter().enumerate() {
                    for x in &f.body {
                        if let Instr::I32Const(v) = x {
                            let v = *v as u32;
                            if v <= addr && addr - v < 400 {
                                let c = name(imported + j as u32);
                                println!("   {} points at {v} (+{})", &c[..c.len().min(160)], addr - v);
                            }
                        }
                    }
                }
            }
        }
        return;
    }
    let mut shown = 0;
    for (&i, n) in &names {
        if !n.contains(args[2].as_str()) || shown >= 12 {
            continue;
        }
        shown += 1;
        println!("{i} {}", &n[..n.len().min(200)]);
        for (j, f) in module.funcs.iter().enumerate() {
            if f.body.iter().any(|x| matches!(x, Instr::Call(c) | Instr::ReturnCall(c) | Instr::RefFunc(c) if *c == i)) {
                let c = name(imported + j as u32);
                println!("   called by {}", &c[..c.len().min(200)]);
            }
        }
        for e in &module.elems {
            if let (ElemMode::Active { offset, .. }, ElemItems::Funcs(fs)) = (&e.mode, &e.items) {
                for (k, f) in fs.iter().enumerate() {
                    if *f == i {
                        println!("   table slot {:?}+{k}", offset.first());
                    }
                }
            }
        }
    }
}
