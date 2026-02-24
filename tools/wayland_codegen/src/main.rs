use std::{
    env,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

#[path = "../../../libs/linux/wayland-scanner/src/c_interfaces.rs"]
mod c_interfaces;
#[path = "../../../libs/linux/wayland-scanner/src/client_gen.rs"]
mod client_gen;
#[path = "../../../libs/linux/wayland-scanner/src/common.rs"]
mod common;
#[path = "../../../libs/linux/wayland-scanner/src/interfaces.rs"]
mod interfaces;
#[path = "../../../libs/linux/wayland-scanner/src/parse.rs"]
mod parse;
#[path = "../../../libs/linux/wayland-scanner/src/protocol.rs"]
mod protocol;
#[path = "../../../libs/linux/wayland-scanner/src/util.rs"]
mod util;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Side {
    Client,
    Server,
}

fn main() -> io::Result<()> {
    let root = find_repo_root()?;
    let wayland_root = root.join("libs/linux/wayland-protocols");
    let protocols = wayland_root.join("protocols");
    let wayland_client_root = root.join("libs/linux/wayland-client");

    let wp_rs = generate_wp(&protocols)?;
    let xdg_rs = generate_xdg(&protocols)?;
    let core_protocol_rs = generate_wayland_client_core(&wayland_client_root)?;

    fs::write(wayland_root.join("src/wp.rs"), wp_rs)?;
    fs::write(wayland_root.join("src/xdg.rs"), xdg_rs)?;
    fs::write(wayland_client_root.join("src/protocol.rs"), core_protocol_rs)?;

    Ok(())
}

fn find_repo_root() -> io::Result<PathBuf> {
    let mut dir = env::current_dir()?;
    loop {
        if dir.join("libs/linux/wayland-protocols").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "could not find repo root containing libs/linux/wayland-protocols",
            ));
        }
    }
}

fn generate_wp(protocols: &Path) -> io::Result<String> {
    let mut out = String::new();
    out.push_str("//! Generated from protocol XML by `tools/wayland_codegen`.\n");
    out.push_str("//! Do not edit manually; regenerate instead.\n\n");

    out.push_str("pub mod tablet {\n");
    out.push_str("    pub mod zv2 {\n");
    out.push_str(&indent(
        &generate_client_module(&protocols.join("stable/tablet/tablet-v2.xml"), &[])?,
        8,
    ));
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out.push_str("#[cfg(all(feature = \"staging\", feature = \"unstable\"))]\n");
    out.push_str("pub mod cursor_shape {\n");
    out.push_str("    pub mod v1 {\n");
    out.push_str(&indent(
        &generate_client_module(
            &protocols.join("staging/cursor-shape/cursor-shape-v1.xml"),
            &["crate::wp::tablet::zv2"],
        )?,
        8,
    ));
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out.push_str("#[cfg(feature = \"staging\")]\n");
    out.push_str("pub mod fractional_scale {\n");
    out.push_str("    pub mod v1 {\n");
    out.push_str(&indent(
        &generate_client_module(
            &protocols.join("staging/fractional-scale/fractional-scale-v1.xml"),
            &[],
        )?,
        8,
    ));
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out.push_str("#[cfg(feature = \"unstable\")]\n");
    out.push_str("pub mod text_input {\n");
    out.push_str("    pub mod zv3 {\n");
    out.push_str(&indent(
        &generate_client_module(
            &protocols.join("unstable/text-input/text-input-unstable-v3.xml"),
            &[],
        )?,
        8,
    ));
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out.push_str("pub mod viewporter {\n");
    out.push_str(&indent(
        &generate_client_module(&protocols.join("stable/viewporter/viewporter.xml"), &[])?,
        4,
    ));
    out.push_str("}\n");

    Ok(out)
}

fn generate_xdg(protocols: &Path) -> io::Result<String> {
    let mut out = String::new();
    out.push_str("//! Generated from protocol XML by `tools/wayland_codegen`.\n");
    out.push_str("//! Do not edit manually; regenerate instead.\n\n");

    out.push_str("#[cfg(feature = \"unstable\")]\n");
    out.push_str("pub mod decoration {\n");
    out.push_str("    pub mod zv1 {\n");
    out.push_str(&indent(
        &generate_client_module(
            &protocols.join("unstable/xdg-decoration/xdg-decoration-unstable-v1.xml"),
            &["crate::xdg::shell"],
        )?,
        8,
    ));
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out.push_str("pub mod shell {\n");
    out.push_str(&indent(
        &generate_client_module(&protocols.join("stable/xdg-shell/xdg-shell.xml"), &[])?,
        4,
    ));
    out.push_str("}\n\n");

    out.push_str("#[cfg(feature = \"staging\")]\n");
    out.push_str("pub mod toplevel_icon {\n");
    out.push_str("    pub mod v1 {\n");
    out.push_str(&indent(
        &generate_client_module(
            &protocols.join("staging/xdg-toplevel-icon/xdg-toplevel-icon-v1.xml"),
            &["crate::xdg::shell"],
        )?,
        8,
    ));
    out.push_str("    }\n");
    out.push_str("}\n");

    Ok(out)
}

fn generate_client_module(xml_path: &Path, imports: &[&str]) -> io::Result<String> {
    let protocol = parse::parse(File::open(xml_path)?);
    let interfaces = interfaces::generate(&protocol, true).to_string();
    let client_code = client_gen::generate_client_objects(&protocol).to_string();

    let mut out = String::new();
    out.push_str("pub mod client {\n");
    out.push_str("    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]\n");
    out.push_str("    #![allow(non_upper_case_globals, non_snake_case, unused_imports)]\n");
    out.push_str("    #![allow(unused_parens, irrefutable_let_patterns, unused_mut)]\n");
    out.push_str("    #![allow(missing_docs, clippy::all)]\n");
    out.push_str("    //! Client-side API of this protocol\n");
    out.push_str("    use wayland_client;\n");
    out.push_str("    use wayland_client::protocol::*;\n");
    for import in imports {
        out.push_str("    use ");
        out.push_str(import);
        out.push_str("::{client::*};\n");
    }

    out.push_str("    pub mod __interfaces {\n");
    out.push_str("        use wayland_client::protocol::__interfaces::*;\n");
    for import in imports {
        out.push_str("        use ");
        out.push_str(import);
        out.push_str("::{client::__interfaces::*};\n");
    }
    out.push_str("        ");
    out.push_str(&interfaces);
    out.push_str("\n    }\n");

    out.push_str("    use self::__interfaces::*;\n");
    out.push_str("    ");
    out.push_str(&client_code);
    out.push_str("\n}\n");

    Ok(out)
}

fn generate_wayland_client_core(wayland_client_root: &Path) -> io::Result<String> {
    let xml_path = wayland_client_root.join("wayland.xml");
    let protocol = parse::parse(File::open(xml_path)?);
    let interfaces = interfaces::generate(&protocol, true).to_string();
    let client_code = client_gen::generate_client_objects(&protocol).to_string();

    let mut out = String::new();
    out.push_str("//! Generated from `wayland.xml` by `tools/wayland_codegen`.\n");
    out.push_str("//! Do not edit manually; regenerate instead.\n\n");
    out.push_str("#![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]\n");
    out.push_str("#![allow(non_upper_case_globals, non_snake_case, unused_imports)]\n");
    out.push_str("#![allow(unused_parens, irrefutable_let_patterns, unused_mut)]\n");
    out.push_str("#![allow(missing_docs, clippy::all)]\n\n");
    out.push_str("use self::__interfaces::*;\n");
    out.push_str("use crate as wayland_client;\n\n");
    out.push_str("pub mod __interfaces {\n");
    out.push_str("    ");
    out.push_str(&interfaces);
    out.push_str("\n}\n\n");
    out.push_str(&client_code);
    out.push('\n');
    Ok(out)
}

fn indent(source: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    let mut out = String::new();
    for line in source.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&pad);
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}
