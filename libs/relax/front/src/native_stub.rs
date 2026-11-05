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
    let (text, stub) = text(arch, names.len());
    let mut blob = Vec::new();
    for n in names {
        blob.extend_from_slice(n.as_bytes());
        blob.push(0);
    }
    blob.push(0);
    Ok(if triple.contains("-apple-") { macho(arch, names, &text, stub, &blob) } else { elf(arch, names, &text, stub, &blob) })
}

/// The stubs' code (relocations zeroed) and one stub's size. x86-64: `jmp *slot(%rip)`;
/// arm64: `adrp x16, slot; ldr x16, [x16, :lo12:slot]; br x16`.
fn text(arch: Arch, n: usize) -> (Vec<u8>, usize) {
    let mut t = Vec::new();
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
    (t, if arch == Arch::X64 { 8 } else { 16 })
}

fn put16(b: &mut Vec<u8>, v: u16) { b.extend_from_slice(&v.to_le_bytes()); }
fn put32(b: &mut Vec<u8>, v: u32) { b.extend_from_slice(&v.to_le_bytes()); }
fn put64(b: &mut Vec<u8>, v: u64) { b.extend_from_slice(&v.to_le_bytes()); }
fn pad(b: &mut Vec<u8>, align: usize) { while b.len() % align != 0 { b.push(0); } }

/// ELF64 ET_REL: .text, .data (the slots), .rodata (the names), .note.GNU-stack (no executable
/// stack), the symbols and .rela.text (against .data's section symbol).
fn elf(arch: Arch, names: &[String], text: &[u8], stub: usize, blob: &[u8]) -> Vec<u8> {
    let n = names.len();
    // sections: 0 null, 1 .text, 2 .data, 3 .rodata, 4 .note.GNU-stack, 5 .symtab, 6 .strtab, 7 .rela.text, 8 .shstrtab
    let mut shstr = vec![0u8];
    let mut sh_name = |s: &str| { let at = shstr.len() as u32; shstr.extend_from_slice(s.as_bytes()); shstr.push(0); at };
    let names_at: Vec<u32> = [".text", ".data", ".rodata", ".note.GNU-stack", ".symtab", ".strtab", ".rela.text", ".shstrtab"].iter().map(|s| sh_name(s)).collect();
    // symbols: 0 null, 1 .data's section symbol, then the globals (stubs, the slot table, the names)
    let mut strtab = vec![0u8];
    let mut sym = Vec::new();
    sym.extend_from_slice(&[0u8; 24]);
    put32(&mut sym, 0);
    sym.push(3); // STB_LOCAL, STT_SECTION
    sym.push(0);
    put16(&mut sym, 2);
    put64(&mut sym, 0);
    put64(&mut sym, 0);
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
        global(&mut strtab, name, 2, 2, 1, (i * stub) as u64, stub as u64); // STT_FUNC, STV_HIDDEN
    }
    global(&mut strtab, SLOTS, 1, 0, 2, 0, 8 * n as u64); // STT_OBJECT, STV_DEFAULT
    global(&mut strtab, NAMES, 1, 0, 3, 0, blob.len() as u64);
    let slots_sym = 1u64;
    let mut rela = Vec::new();
    for i in 0..n {
        let at = (i * stub) as u64;
        let slot = 8 * i as i64;
        let mut r = |off: u64, ty: u64, addend: i64| { put64(&mut rela, off); put64(&mut rela, (slots_sym << 32) | ty); put64(&mut rela, addend as u64); };
        match arch {
            Arch::X64 => r(at + 2, 2, slot - 4), // R_X86_64_PC32
            Arch::Arm64 => {
                r(at, 275, slot); // R_AARCH64_ADR_PREL_PG_HI21
                r(at + 4, 286, slot); // R_AARCH64_LDST64_ABS_LO12_NC
            }
        }
    }
    let mut out = vec![0u8; 64];
    let place = |out: &mut Vec<u8>, bytes: &[u8], align: usize| { pad(out, align); let at = out.len(); out.extend_from_slice(bytes); (at as u64, bytes.len() as u64) };
    let text_at = place(&mut out, text, 16);
    let data_at = place(&mut out, &vec![0u8; 8 * n], 8);
    let rodata_at = place(&mut out, blob, 1);
    let note_at = (out.len() as u64, 0);
    let sym_at = place(&mut out, &sym, 8);
    let str_at = place(&mut out, &strtab, 1);
    let rela_at = place(&mut out, &rela, 8);
    let shstr_at = place(&mut out, &shstr, 1);
    pad(&mut out, 8);
    let shoff = out.len() as u64;
    // (name, type, flags, (offset, size), link, info, align, entsize)
    let secs: [(u32, u32, u64, (u64, u64), u32, u32, u64, u64); 8] = [
        (names_at[0], 1, 0x6, text_at, 0, 0, 16, 0),        // PROGBITS, ALLOC|EXECINSTR
        (names_at[1], 1, 0x3, data_at, 0, 0, 8, 0),         // PROGBITS, WRITE|ALLOC
        (names_at[2], 1, 0x2, rodata_at, 0, 0, 1, 0),       // PROGBITS, ALLOC
        (names_at[3], 1, 0, note_at, 0, 0, 1, 0),
        (names_at[4], 2, 0, sym_at, 6, 2, 8, 24),           // SYMTAB, link .strtab, first global 2
        (names_at[5], 3, 0, str_at, 0, 0, 1, 0),            // STRTAB
        (names_at[6], 4, 0x40, rela_at, 5, 1, 8, 24),       // RELA, INFO_LINK, of .text
        (names_at[7], 3, 0, shstr_at, 0, 0, 1, 0),
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
    put16(&mut h, 9); // shnum
    put16(&mut h, 8); // shstrndx
    out[..64].copy_from_slice(&h);
    out
}

/// Mach-O MH_OBJECT: __TEXT,__text, __DATA,__data (the slots), __TEXT,__const (the names), each
/// section one atom (no subsections via symbols). Each stub's relocations name a local symbol
/// of its own slot (arm64 page relocations carry no addend of their own).
fn macho(arch: Arch, names: &[String], text: &[u8], stub: usize, blob: &[u8]) -> Vec<u8> {
    let n = names.len();
    let data_addr = text.len().next_multiple_of(8);
    let const_addr = data_addr + 8 * n;
    let vmsize = const_addr + blob.len();
    // symbols: the slots (local), then the external definitions sorted by name
    let mut strtab = vec![b' ', 0];
    let mut nlist = Vec::new();
    let mut entry = |strtab: &mut Vec<u8>, name: &str, kind: u8, sect: u8, value: usize| {
        put32(&mut nlist, strtab.len() as u32);
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        nlist.push(0x0e | kind); // N_SECT, | N_EXT, | N_PEXT
        nlist.push(sect);
        put16(&mut nlist, 0);
        put64(&mut nlist, value as u64);
    };
    for i in 0..n {
        entry(&mut strtab, &format!("l_relax_native_callback_slot{i}"), 0, 2, data_addr + 8 * i);
    }
    // the stubs private external (0x11: N_PEXT | N_EXT), the tables exported (N_EXT)
    let mut ext: Vec<(String, u8, u8, usize)> = names.iter().enumerate().map(|(i, s)| (format!("_{s}"), 0x11, 1, i * stub)).collect();
    ext.push((format!("_{SLOTS}"), 0x01, 2, data_addr));
    ext.push((format!("_{NAMES}"), 0x01, 3, const_addr));
    ext.sort();
    for (name, kind, sect, value) in &ext {
        entry(&mut strtab, name, *kind, *sect, *value);
    }
    pad(&mut strtab, 8);
    let mut relocs = Vec::new();
    let mut reloc = |address: usize, symbol: usize, pcrel: bool, ty: u32| {
        put32(&mut relocs, address as u32);
        put32(&mut relocs, symbol as u32 | (pcrel as u32) << 24 | 2 << 25 | 1 << 27 | ty << 28);
    };
    for i in (0..n).rev() {
        match arch {
            Arch::X64 => reloc(i * stub + 2, i, true, 1), // X86_64_RELOC_SIGNED
            Arch::Arm64 => {
                reloc(i * stub + 4, i, false, 4); // ARM64_RELOC_PAGEOFF12
                reloc(i * stub, i, true, 3); // ARM64_RELOC_PAGE21
            }
        }
    }
    let nsects = 3usize;
    let sizeofcmds = (72 + 80 * nsects) + 24 + 24 + 80;
    let text_off = 32 + sizeofcmds;
    let data_off = text_off + data_addr;
    let const_off = text_off + const_addr;
    let reloff = (text_off + vmsize).next_multiple_of(8);
    let symoff = reloff + relocs.len();
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
        ("__data", "__DATA", data_addr, 8 * n, data_off, 3, 0, 0, 0),
        ("__const", "__TEXT", const_addr, blob.len(), const_off, 0, 0, 0, 0),
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
    put32(&mut out, (n + ext.len()) as u32);
    put32(&mut out, stroff as u32);
    put32(&mut out, strtab.len() as u32);
    // LC_DYSYMTAB: the locals, then the external definitions, no undefined symbols
    put32(&mut out, 0xb);
    put32(&mut out, 80);
    for v in [0, n, n, ext.len(), n + ext.len(), 0] {
        put32(&mut out, v as u32);
    }
    out.extend_from_slice(&[0u8; 48]);
    debug_assert_eq!(out.len(), text_off);
    out.extend_from_slice(text);
    pad(&mut out, 8);
    out.resize(data_off + 8 * n, 0);
    out.extend_from_slice(blob);
    out.resize(reloff, 0);
    out.extend_from_slice(&relocs);
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
        for s in ["_first_callback\0", "_second\0", "___relax_native_callbacks\0", "___relax_native_callback_names\0"] {
            assert!(arm.windows(s.len()).any(|w| w == s.as_bytes()), "{s}");
        }
        assert!(super::object("riscv64gc-unknown-linux-gnu", &names).is_err());
    }
}
