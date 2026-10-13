mod generate;
mod metadata;
use metadata::{fail, File};
use std::io::Write;
fn main() {
    let mut args = std::env::args();
    let _ = args.next();
    let command = args.next().unwrap_or_default();
    if command == "generate" {
        generate::run(args);
        return;
    }
    if command == "inventory" {
        generate::inventory(args);
        return;
    }
    if command != "dump" {
        fail("usage: makepad-winbind dump <metadata> <type names...>");
    }
    let file = File::read(&args.next().expect("metadata file"));
    let mut text = String::new();
    for wanted in args {
        for id in 1..=file.tables[2].len() as u32 {
            let name = file.type_name(id << 2);
            let method_filter = wanted.strip_prefix('@');
            if let Some(method) = method_filter {
                let mut found = false;
                for mid in file.range(2, id, 5, 6) {
                    if file.string(file.row(6, mid)[3]) == method {
                        found = true;
                    }
                }
                if !found {
                    continue;
                }
            } else if !name.ends_with(&wanted) {
                continue;
            }
            let row = file.row(2, id);
            text.push_str("BASE ");
            text.push_str(&file.base(id));
            text.push('\n');
            text.push_str(&name);
            text.push_str(" extends ");
            text.push_str(&file.type_name(row[3]));
            text.push('\n');
            if let Some(attrs) = file.attrs.get(&(2, id)) {
                for a in attrs {
                    if file.attr_name(*a).ends_with("DocumentationAttribute") {
                        continue;
                    }
                    text.push_str("  @");
                    text.push_str(&file.attr_name(*a));
                    text.push(' ');
                    for b in file.blob(file.row(12, *a)[2]) {
                        text.push_str(&b.to_string());
                        text.push(',');
                    }
                    text.push('\n');
                }
            }
            for fid in file.range(2, id, 4, 4) {
                if method_filter.is_some() {
                    continue;
                }
                text.push_str("  field ");
                text.push_str(file.string(file.row(4, fid)[1]));
                text.push(' ');
                text.push_str(&file.field_sig(fid).describe());
                text.push('\n');
                if let Some(attrs) = file.attrs.get(&(4, fid)) {
                    for a in attrs {
                        if file.attr_name(*a).ends_with("DocumentationAttribute") {
                            continue;
                        }
                        text.push_str("    @");
                        text.push_str(&file.attr_name(*a));
                        text.push(' ');
                        for b in file.blob(file.row(12, *a)[2]) {
                            text.push_str(&b.to_string());
                            text.push(',');
                        }
                        text.push('\n');
                    }
                }
            }
            for mid in file.range(2, id, 5, 6) {
                let m = file.method(mid);
                if method_filter.is_some_and(|n| n != m.name) {
                    continue;
                }
                text.push_str("  method ");
                text.push_str(&m.name);
                text.push_str(" -> ");
                text.push_str(&m.ret.describe());
                text.push('\n');
                if let Some(attrs) = file.attrs.get(&(6, mid)) {
                    for a in attrs {
                        if file.attr_name(*a).ends_with("DocumentationAttribute") {
                            continue;
                        }
                        text.push_str("    @");
                        text.push_str(&file.attr_name(*a));
                        text.push(' ');
                        for b in file.blob(file.row(12, *a)[2]) {
                            text.push_str(&b.to_string());
                            text.push(',');
                        }
                        text.push('\n');
                    }
                }
                for p in &m.params {
                    text.push_str("    ");
                    text.push_str(&p.name);
                    text.push(' ');
                    text.push_str(&p.flags.to_string());
                    text.push(' ');
                    text.push_str(&p.sig.describe());
                    text.push('\n');
                    if let Some(attrs) = file.attrs.get(&(8, p.id)) {
                        for a in attrs {
                            text.push_str("      @");
                            text.push_str(&file.attr_name(*a));
                            text.push(' ');
                            for b in file.blob(file.row(12, *a)[2]) {
                                text.push_str(&b.to_string());
                                text.push(',');
                            }
                            text.push('\n');
                        }
                    }
                }
            }
        }
    }
    std::io::stdout()
        .write_all(text.as_bytes())
        .expect("write output");
}
