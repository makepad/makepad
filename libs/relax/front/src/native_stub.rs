//! The callback stubs of an `--allow-c` native library (native.rs link): C code of a native
//! package that calls a `#[no_mangle]` fn of its own crate's Rust side (blake3's NEON code calls
//! `blake3_compress_in_place_portable`). Under rustc the linker binds that call in the same
//! binary; under the JIT no loaded object defines the symbol. The native library is linked with
//! one more relocatable object, written here (no assembler): per symbol a global function that
//! jumps through a pointer slot, the slots as one table (`__relax_native_callbacks`) and the
//! symbols' names (`__relax_native_callback_names`, NUL-terminated, in table order, an empty
//! name last). The loader writes the program's compiled fns into the slots before the
//! program starts (run.rs bind_native_callbacks). The stubs are hidden (Mach-O: private
//! external): the library's C code binds to its own, never to another loaded library's stub of
//! the same name; the stubs reach the slots through a local symbol, never the exported table.
//!
//! The loader binds the slots once the library has loaded, so C code that calls the program
//! while it loads (a C constructor) runs before them. Until then each slot holds its stub's
//! unbound trap: it writes the loader's error for that symbol to stderr as one line and ends
//! the process with exit status 1 (`write`, then `exit`, whose handlers carry the line through
//! the run's stderr tee), where a zero slot would jump to address 0.

/// The table of slots, one pointer per stub.
pub const SLOTS: &str = "__relax_native_callbacks";
/// The stubs' names, NUL-terminated, in slot order; an empty name ends the list.
pub const NAMES: &str = "__relax_native_callback_names";

#[derive(Clone, Copy, PartialEq)]
enum Arch {
    X64,
    Arm64,
}

/// The relocatable object for `triple` (ELF, or Mach-O for an Apple triple) defining a stub
/// for each of `names` (C symbol names).
pub fn object(triple: &str, names: &[String]) -> Result<Vec<u8>, String> {
    let arch = if triple.starts_with("x86_64") {
        Arch::X64
    } else if triple.starts_with("aarch64") || triple.starts_with("arm64") {
        Arch::Arm64
    } else {
        return Err(format!("--allow-c: C code calling back into Rust is not supported on {triple}"));
    };
    // read-only data: the names (the NAMES table), then each trap's message
    let mut rodata = Vec::new();
    for n in names {
        rodata.extend_from_slice(n.as_bytes());
        rodata.push(0);
    }
    rodata.push(0);
    let names_len = rodata.len();
    let mut messages = Vec::new();
    for n in names {
        let m = unbound_message(n);
        if m.len() >= 1 << 16 {
            return Err(format!("--allow-c: the C symbol name `{n}` is too long for a callback stub"));
        }
        messages.push((rodata.len(), m.len()));
        rodata.extend_from_slice(m.as_bytes());
    }
    let code = text(arch, &messages);
    Ok(if triple.contains("-apple-") { macho(arch, names, &code, &rodata, names_len) } else { elf(arch, names, &code, &rodata, names_len) })
}

/// What a slot's trap writes when C code calls it before the loader bound it.
fn unbound_message(name: &str) -> String {
    format!("makepad-run: cannot load the --allow-c native library: its C code calls `{name}` while the library loads (a C constructor), before the program's fns are bound to it\n")
}

/// The object's code: the stubs, one unbound trap per stub, and the traps' shared exit.
struct Code {
    bytes: Vec<u8>,
    /// one stub's size
    stub: usize,
    /// where the traps start, one trap's size
    traps: usize,
    trap: usize,
    /// (rodata offset of each trap's message) -- trap i's message reference is at
    /// traps + i * trap (x86-64: its lea's displacement at + 3)
    messages: Vec<usize>,
    /// the calls of `write` and `exit` (x86-64: their rel32; arm64: their bl)
    write: usize,
    exit: usize,
}

/// The stubs' code (relocations zeroed). x86-64: `jmp *slot(%rip)`; arm64: `adrp x16, slot; ldr
/// x16, [x16, :lo12:slot]; br x16`. Trap i: the address and length of its message in the second
/// and third argument registers, then the shared exit: `write(2, ..)`, `exit(1)`.
fn text(arch: Arch, messages: &[(usize, usize)]) -> Code {
    let n = messages.len();
    let mut t = Vec::new();
    let (stub, trap) = if arch == Arch::X64 { (8, 24) } else { (16, 16) };
    for _ in 0..n {
        match arch {
            // jmp *0(%rip), int3 x2
            Arch::X64 => t.extend_from_slice(&[0xff, 0x25, 0, 0, 0, 0, 0xcc, 0xcc]),
            Arch::Arm64 => {
                for w in [0x9000_0010u32, 0xf940_0210, 0xd61f_0200, 0xd420_0000] {
                    t.extend_from_slice(&w.to_le_bytes());
                }
            }
        }
    }
    let traps = t.len();
    let common = traps + n * trap;
    for (i, &(_, len)) in messages.iter().enumerate() {
        let at = traps + i * trap;
        match arch {
            Arch::X64 => {
                // lea 0(%rip), %rsi; mov $len, %edx; jmp common; int3 padding
                t.extend_from_slice(&[0x48, 0x8d, 0x35, 0, 0, 0, 0, 0xba]);
                put32(&mut t, len as u32);
                t.push(0xe9);
                put32(&mut t, (common as i64 - (at + 17) as i64) as u32);
                t.resize(at + trap, 0xcc);
            }
            Arch::Arm64 => {
                // adrp x1, msg; add x1, x1, :lo12:msg; movz x2, #len; b common
                let b = ((common as i64 - (at + 12) as i64) / 4) as u32 & 0x03ff_ffff;
                for w in [0x9000_0001u32, 0x9100_0021, 0xd280_0002 | (len as u32) << 5, 0x1400_0000 | b] {
                    t.extend_from_slice(&w.to_le_bytes());
                }
            }
        }
    }
    let (write, exit) = match arch {
        Arch::X64 => {
            // sub $8, %rsp (the call's alignment); mov $2, %edi; call write; mov $1, %edi; call exit; int3
            t.extend_from_slice(&[0x48, 0x83, 0xec, 0x08, 0xbf, 2, 0, 0, 0, 0xe8, 0, 0, 0, 0, 0xbf, 1, 0, 0, 0, 0xe8, 0, 0, 0, 0, 0xcc]);
            (common + 10, common + 20)
        }
        Arch::Arm64 => {
            // movz x0, #2; bl write; movz x0, #1; bl exit; brk #0
            for w in [0xd280_0040u32, 0x9400_0000, 0xd280_0020, 0x9400_0000, 0xd420_0000] {
                t.extend_from_slice(&w.to_le_bytes());
            }
            (common + 4, common + 12)
        }
    };
    Code { bytes: t, stub, traps, trap, messages: messages.iter().map(|m| m.0).collect(), write, exit }
}

fn put16(b: &mut Vec<u8>, v: u16) { b.extend_from_slice(&v.to_le_bytes()); }
fn put32(b: &mut Vec<u8>, v: u32) { b.extend_from_slice(&v.to_le_bytes()); }
fn put64(b: &mut Vec<u8>, v: u64) { b.extend_from_slice(&v.to_le_bytes()); }
fn pad(b: &mut Vec<u8>, align: usize) { while b.len() % align != 0 { b.push(0); } }

/// ELF64 ET_REL: .text, .data (the slots), .rodata (the names, the messages), .note.GNU-stack (no
/// executable stack), the symbols, .rela.text (against the section symbols and `write`/`exit`)
/// and .rela.data (each slot's trap).
fn elf(arch: Arch, names: &[String], code: &Code, rodata: &[u8], names_len: usize) -> Vec<u8> {
    let n = names.len();
    // sections: 0 null, 1 .text, 2 .data, 3 .rodata, 4 .note.GNU-stack, 5 .symtab, 6 .strtab, 7 .rela.text, 8 .rela.data, 9 .shstrtab
    let mut shstr = vec![0u8];
    let mut sh_name = |s: &str| { let at = shstr.len() as u32; shstr.extend_from_slice(s.as_bytes()); shstr.push(0); at };
    let names_at: Vec<u32> = [".text", ".data", ".rodata", ".note.GNU-stack", ".symtab", ".strtab", ".rela.text", ".rela.data", ".shstrtab"].iter().map(|s| sh_name(s)).collect();
    // symbols: 0 null, 1-3 the section symbols of .data, .text and .rodata, then the globals
    // (stubs, the slot table, the names, then the undefined `write` and `exit`)
    let (data_sym, text_sym, rodata_sym) = (1u64, 2u64, 3u64);
    let mut strtab = vec![0u8];
    let mut sym = Vec::new();
    sym.extend_from_slice(&[0u8; 24]);
    for shndx in [2u16, 1, 3] {
        put32(&mut sym, 0);
        sym.push(3); // STB_LOCAL, STT_SECTION
        sym.push(0);
        put16(&mut sym, shndx);
        put64(&mut sym, 0);
        put64(&mut sym, 0);
    }
    let mut global = |strtab: &mut Vec<u8>, name: &str, kind: u8, other: u8, shndx: u16, value: u64, size: u64| {
        let at = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        put32(&mut sym, at);
        sym.push((1 << 4) | kind); // STB_GLOBAL
        sym.push(other);
        put16(&mut sym, shndx);
        put64(&mut sym, value);
        put64(&mut sym, size);
    };
    for (i, name) in names.iter().enumerate() {
        global(&mut strtab, name, 2, 2, 1, (i * code.stub) as u64, code.stub as u64); // STT_FUNC, STV_HIDDEN
    }
    global(&mut strtab, SLOTS, 1, 0, 2, 0, 8 * n as u64); // STT_OBJECT, STV_DEFAULT
    global(&mut strtab, NAMES, 1, 0, 3, 0, names_len as u64);
    global(&mut strtab, "write", 0, 0, 0, 0, 0); // STT_NOTYPE, undefined
    global(&mut strtab, "exit", 0, 0, 0, 0, 0);
    let (write_sym, exit_sym) = (4 + n as u64 + 2, 4 + n as u64 + 3);
    let rel = |out: &mut Vec<u8>, off: usize, sym: u64, ty: u64, addend: i64| { put64(out, off as u64); put64(out, (sym << 32) | ty); put64(out, addend as u64); };
    let mut rela = Vec::new();
    let mut rela_data = Vec::new();
    for i in 0..n {
        let at = i * code.stub;
        let slot = 8 * i as i64;
        let trap = code.traps + i * code.trap;
        let msg = code.messages[i] as i64;
        match arch {
            Arch::X64 => {
                rel(&mut rela, at + 2, data_sym, 2, slot - 4); // R_X86_64_PC32
                rel(&mut rela, trap + 3, rodata_sym, 2, msg - 4);
                rel(&mut rela_data, 8 * i, text_sym, 1, trap as i64); // R_X86_64_64
            }
            Arch::Arm64 => {
                rel(&mut rela, at, data_sym, 275, slot); // R_AARCH64_ADR_PREL_PG_HI21
                rel(&mut rela, at + 4, data_sym, 286, slot); // R_AARCH64_LDST64_ABS_LO12_NC
                rel(&mut rela, trap, rodata_sym, 275, msg);
                rel(&mut rela, trap + 4, rodata_sym, 277, msg); // R_AARCH64_ADD_ABS_LO12_NC
                rel(&mut rela_data, 8 * i, text_sym, 257, trap as i64); // R_AARCH64_ABS64
            }
        }
    }
    match arch {
        Arch::X64 => {
            rel(&mut rela, code.write, write_sym, 4, -4); // R_X86_64_PLT32
            rel(&mut rela, code.exit, exit_sym, 4, -4);
        }
        Arch::Arm64 => {
            rel(&mut rela, code.write, write_sym, 283, 0); // R_AARCH64_CALL26
            rel(&mut rela, code.exit, exit_sym, 283, 0);
        }
    }
    let mut out = vec![0u8; 64];
    let place = |out: &mut Vec<u8>, bytes: &[u8], align: usize| { pad(out, align); let at = out.len(); out.extend_from_slice(bytes); (at as u64, bytes.len() as u64) };
    let text_at = place(&mut out, &code.bytes, 16);
    let data_at = place(&mut out, &vec![0u8; 8 * n], 8);
    let rodata_at = place(&mut out, rodata, 1);
    let note_at = (out.len() as u64, 0);
    let sym_at = place(&mut out, &sym, 8);
    let str_at = place(&mut out, &strtab, 1);
    let rela_at = place(&mut out, &rela, 8);
    let rela_data_at = place(&mut out, &rela_data, 8);
    let shstr_at = place(&mut out, &shstr, 1);
    pad(&mut out, 8);
    let shoff = out.len() as u64;
    // (name, type, flags, (offset, size), link, info, align, entsize)
    let secs: [(u32, u32, u64, (u64, u64), u32, u32, u64, u64); 9] = [
        (names_at[0], 1, 0x6, text_at, 0, 0, 16, 0),        // PROGBITS, ALLOC|EXECINSTR
        (names_at[1], 1, 0x3, data_at, 0, 0, 8, 0),         // PROGBITS, WRITE|ALLOC
        (names_at[2], 1, 0x2, rodata_at, 0, 0, 1, 0),       // PROGBITS, ALLOC
        (names_at[3], 1, 0, note_at, 0, 0, 1, 0),
        (names_at[4], 2, 0, sym_at, 6, 4, 8, 24),           // SYMTAB, link .strtab, first global 4
        (names_at[5], 3, 0, str_at, 0, 0, 1, 0),            // STRTAB
        (names_at[6], 4, 0x40, rela_at, 5, 1, 8, 24),       // RELA, INFO_LINK, of .text
        (names_at[7], 4, 0x40, rela_data_at, 5, 2, 8, 24),  // of .data
        (names_at[8], 3, 0, shstr_at, 0, 0, 1, 0),
    ];
    out.extend_from_slice(&[0u8; 64]);
    for (name, ty, flags, (off, size), link, info, align, entsize) in secs {
        put32(&mut out, name);
        put32(&mut out, ty);
        put64(&mut out, flags);
        put64(&mut out, 0);
        put64(&mut out, off);
        put64(&mut out, size);
        put32(&mut out, link);
        put32(&mut out, info);
        put64(&mut out, align);
        put64(&mut out, entsize);
    }
    let mut h = Vec::new();
    h.extend_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0]);
    h.extend_from_slice(&[0u8; 8]);
    put16(&mut h, 1); // ET_REL
    put16(&mut h, if arch == Arch::X64 { 62 } else { 183 }); // EM_X86_64 / EM_AARCH64
    put32(&mut h, 1);
    put64(&mut h, 0); // entry
    put64(&mut h, 0); // phoff
    put64(&mut h, shoff);
    put32(&mut h, 0); // flags
    put16(&mut h, 64); // ehsize
    put16(&mut h, 0);
    put16(&mut h, 0);
    put16(&mut h, 64); // shentsize
    put16(&mut h, 10); // shnum
    put16(&mut h, 9); // shstrndx
    out[..64].copy_from_slice(&h);
    out
}

/// Mach-O MH_OBJECT: __TEXT,__text, __DATA,__data (the slots, each its trap's address),
/// __TEXT,__const (the names, the messages), each section one atom (no subsections via
/// symbols). Each stub's and trap's relocations name a local symbol of its own slot or message
/// (arm64 page relocations carry no addend of their own); a slot's is against __text itself.
fn macho(arch: Arch, names: &[String], code: &Code, rodata: &[u8], names_len: usize) -> Vec<u8> {
    let n = names.len();
    let text = &code.bytes;
    let data_addr = text.len().next_multiple_of(8);
    let const_addr = data_addr + 8 * n;
    let vmsize = const_addr + rodata.len();
    debug_assert!(names_len <= rodata.len());
    // symbols: the slots and the messages (local), the external definitions sorted by name,
    // then the undefined `exit` and `write` (sorted)
    let mut strtab = vec![b' ', 0];
    let mut nlist = Vec::new();
    let mut entry = |strtab: &mut Vec<u8>, name: &str, kind: u8, sect: u8, value: usize| {
        put32(&mut nlist, strtab.len() as u32);
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        nlist.push(kind);
        nlist.push(sect);
        put16(&mut nlist, 0);
        put64(&mut nlist, value as u64);
    };
    for i in 0..n {
        entry(&mut strtab, &format!("l_relax_native_callback_slot{i}"), 0x0e, 2, data_addr + 8 * i); // N_SECT
    }
    for i in 0..n {
        entry(&mut strtab, &format!("l_relax_native_callback_unbound{i}"), 0x0e, 3, const_addr + code.messages[i]);
    }
    // the stubs private external (0x1f: N_SECT | N_PEXT | N_EXT), the tables exported (0x0f)
    let mut ext: Vec<(String, u8, u8, usize)> = names.iter().enumerate().map(|(i, s)| (format!("_{s}"), 0x1f, 1, i * code.stub)).collect();
    ext.push((format!("_{SLOTS}"), 0x0f, 2, data_addr));
    ext.push((format!("_{NAMES}"), 0x0f, 3, const_addr));
    ext.sort();
    for (name, kind, sect, value) in &ext {
        entry(&mut strtab, name, *kind, *sect, *value);
    }
    let undef = 2 * n + ext.len();
    for name in ["_exit", "_write"] {
        entry(&mut strtab, name, 0x01, 0, 0); // N_UNDF | N_EXT
    }
    let (exit_sym, write_sym) = (undef, undef + 1);
    pad(&mut strtab, 8);
    // (address, symbol or section ordinal, pcrel, log2 length, extern, type)
    let reloc = |out: &mut Vec<u8>, address: usize, symbol: usize, pcrel: bool, length: u32, ext: bool, ty: u32| {
        put32(out, address as u32);
        put32(out, symbol as u32 | (pcrel as u32) << 24 | length << 25 | (ext as u32) << 27 | ty << 28);
    };
    let mut relocs = Vec::new();
    for i in (0..n).rev() {
        let trap = code.traps + i * code.trap;
        match arch {
            Arch::X64 => {
                reloc(&mut relocs, i * code.stub + 2, i, true, 2, true, 1); // X86_64_RELOC_SIGNED
                reloc(&mut relocs, trap + 3, n + i, true, 2, true, 1);
            }
            Arch::Arm64 => {
                reloc(&mut relocs, i * code.stub + 4, i, false, 2, true, 4); // ARM64_RELOC_PAGEOFF12
                reloc(&mut relocs, i * code.stub, i, true, 2, true, 3); // ARM64_RELOC_PAGE21
                reloc(&mut relocs, trap + 4, n + i, false, 2, true, 4);
                reloc(&mut relocs, trap, n + i, true, 2, true, 3);
            }
        }
    }
    // X86_64_RELOC_BRANCH / ARM64_RELOC_BRANCH26
    reloc(&mut relocs, code.write, write_sym, true, 2, true, 2);
    reloc(&mut relocs, code.exit, exit_sym, true, 2, true, 2);
    // each slot: its trap's address in __text (section 1), X86_64_RELOC_UNSIGNED / ARM64_RELOC_UNSIGNED
    let mut data_relocs = Vec::new();
    let mut slots = Vec::new();
    for i in 0..n {
        reloc(&mut data_relocs, 8 * i, 1, false, 3, false, 0);
        put64(&mut slots, (code.traps + i * code.trap) as u64);
    }
    let nsects = 3usize;
    let sizeofcmds = (72 + 80 * nsects) + 24 + 24 + 80;
    let text_off = 32 + sizeofcmds;
    let data_off = text_off + data_addr;
    let const_off = text_off + const_addr;
    let reloff = (text_off + vmsize).next_multiple_of(8);
    let data_reloff = reloff + relocs.len();
    let symoff = data_reloff + data_relocs.len();
    let stroff = symoff + nlist.len();
    let mut out = Vec::new();
    put32(&mut out, 0xfeed_facf);
    put32(&mut out, if arch == Arch::X64 { 0x0100_0007 } else { 0x0100_000c });
    put32(&mut out, if arch == Arch::X64 { 3 } else { 0 });
    put32(&mut out, 1); // MH_OBJECT
    put32(&mut out, 4);
    put32(&mut out, sizeofcmds as u32);
    put32(&mut out, 0);
    put32(&mut out, 0);
    // LC_SEGMENT_64
    put32(&mut out, 0x19);
    put32(&mut out, (72 + 80 * nsects) as u32);
    out.extend_from_slice(&[0u8; 16]);
    put64(&mut out, 0);
    put64(&mut out, vmsize as u64);
    put64(&mut out, text_off as u64);
    put64(&mut out, vmsize as u64);
    put32(&mut out, 7);
    put32(&mut out, 7);
    put32(&mut out, nsects as u32);
    put32(&mut out, 0);
    let fixed = |s: &str| { let mut b = [0u8; 16]; b[..s.len()].copy_from_slice(s.as_bytes()); b };
    // (sect, seg, addr, size, offset, align, reloff, nreloc, flags)
    let text_align = if arch == Arch::X64 { 0 } else { 2 };
    let secs = [
        ("__text", "__TEXT", 0, text.len(), text_off, text_align, reloff, relocs.len() / 8, 0x8000_0400u32),
        ("__data", "__DATA", data_addr, 8 * n, data_off, 3, data_reloff, data_relocs.len() / 8, 0),
        ("__const", "__TEXT", const_addr, rodata.len(), const_off, 0, 0, 0, 0),
    ];
    for (sect, seg, addr, size, offset, align, roff, nreloc, flags) in secs {
        out.extend_from_slice(&fixed(sect));
        out.extend_from_slice(&fixed(seg));
        put64(&mut out, addr as u64);
        put64(&mut out, size as u64);
        put32(&mut out, offset as u32);
        put32(&mut out, align);
        put32(&mut out, roff as u32);
        put32(&mut out, nreloc as u32);
        put32(&mut out, flags);
        put32(&mut out, 0);
        put32(&mut out, 0);
        put32(&mut out, 0);
    }
    // LC_BUILD_VERSION: macOS 11.0
    put32(&mut out, 0x32);
    put32(&mut out, 24);
    put32(&mut out, 1);
    put32(&mut out, 11 << 16);
    put32(&mut out, 0);
    put32(&mut out, 0);
    // LC_SYMTAB
    put32(&mut out, 0x2);
    put32(&mut out, 24);
    put32(&mut out, symoff as u32);
    put32(&mut out, (undef + 2) as u32);
    put32(&mut out, stroff as u32);
    put32(&mut out, strtab.len() as u32);
    // LC_DYSYMTAB: the locals, the external definitions, the undefined symbols
    put32(&mut out, 0xb);
    put32(&mut out, 80);
    for v in [0, 2 * n, 2 * n, ext.len(), undef, 2] {
        put32(&mut out, v as u32);
    }
    out.extend_from_slice(&[0u8; 48]);
    debug_assert_eq!(out.len(), text_off);
    out.extend_from_slice(text);
    pad(&mut out, 8);
    out.resize(data_off, 0);
    out.extend_from_slice(&slots);
    out.extend_from_slice(rodata);
    out.resize(reloff, 0);
    out.extend_from_slice(&relocs);
    out.extend_from_slice(&data_relocs);
    out.extend_from_slice(&nlist);
    out.extend_from_slice(&strtab);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn stub_objects_have_one_stub_and_one_slot_per_name() {
        let names = vec!["first_callback".to_string(), "second".to_string()];
        let elf = super::object("x86_64-unknown-linux-gnu", &names).unwrap();
        assert_eq!(&elf[..4], b"\x7fELF");
        // .text holds two 8-byte stubs, each `jmp *0(%rip)`
        let text = elf.windows(6).filter(|w| *w == [0xff, 0x25, 0, 0, 0, 0]).count();
        assert_eq!(text, 2);
        let arm = super::object("aarch64-apple-darwin", &names).unwrap();
        assert_eq!(u32::from_le_bytes(arm[..4].try_into().unwrap()), 0xfeed_facf);
        // every external name is in the string table, with Mach-O's leading underscore
        for s in ["_first_callback\0", "_second\0", "___relax_native_callbacks\0", "___relax_native_callback_names\0", "_write\0", "_exit\0"] {
            assert!(arm.windows(s.len()).any(|w| w == s.as_bytes()), "{s}");
        }
        assert!(super::object("riscv64gc-unknown-linux-gnu", &names).is_err());
    }
}
