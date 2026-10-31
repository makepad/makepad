use crate::model::{camel, doc_attr, Enum};

pub fn emit(out: &mut String, e: &Enum) {
    let name = camel(&e.name);
    if e.bitfield { flags(out,e,&name); return; }
    doc_attr(out, &e.doc);
    out.push_str(&format!("#[repr(u32)]\n#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]\n#[non_exhaustive]\npub enum {name} {{\n"));
    for entry in &e.entries {
        doc_attr(out,&entry.doc);
        out.push_str(&format!("    {} = {},\n",camel(&entry.name),entry.value));
    }
    out.push_str(&format!("}}\nimpl TryFrom<u32> for {name} {{\n    type Error = ();\n    fn try_from(value: u32) -> Result<Self, ()> {{ match value {{\n"));
    for entry in &e.entries { out.push_str(&format!("        {} => Ok(Self::{}),\n",entry.value,camel(&entry.name))); }
    out.push_str(&format!("        _ => Err(()),\n    }} }}\n}}\nimpl From<{name}> for u32 {{ fn from(value: {name}) -> u32 {{ value as u32 }} }}\n"));
}
// Bitfield enums are compact `bitflags!` invocations, as the original scanner
// emitted them: rustc expands them through the bitflags crate and Rapid expands
// bitflags! natively. Unknown bits are retained (from_bits_retain), as before.
fn flags(out: &mut String, e: &Enum, name: &str) {
    out.push_str("bitflags::bitflags! {\n");
    let mut doc = String::new();
    doc_attr(&mut doc,&e.doc);
    for line in doc.lines() { out.push_str("    "); out.push_str(line); out.push('\n'); }
    out.push_str(&format!("    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]\n    pub struct {name}: u32 {{\n"));
    for entry in &e.entries {
        let mut doc = String::new();
        doc_attr(&mut doc,&entry.doc);
        for line in doc.lines() { out.push_str("        "); out.push_str(line); out.push('\n'); }
        out.push_str(&format!("        const {} = {};\n",camel(&entry.name),entry.value));
    }
    out.push_str(&format!("    }}\n}}\nimpl TryFrom<u32> for {name} {{\n    type Error = ();\n    fn try_from(value: u32) -> Result<Self, ()> {{ Self::from_bits(value).ok_or(()) }}\n}}\nimpl From<{name}> for u32 {{ fn from(value: {name}) -> u32 {{ value.bits() }} }}\n"));
}
