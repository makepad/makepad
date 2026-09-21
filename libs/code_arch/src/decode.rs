use crate::*;
use makepad_toml_parser::{parse_toml, Toml, TomlTable};
use std::collections::BTreeSet;

/// Decode schema v0, rejecting unknown fields and invalid graph structure.
/// A bounded, allocation-free scan runs before the TOML parser allocates.
/// Filesystem checks are separate, in `validate`.
pub fn parse(text: &str, limits: &Limits) -> Result<Plan, ArchError> {
    let limits = limits.bounded();
    crate::guard::preflight(text, &limits)?;
    let document = parse_toml(text).map_err(|e| {
        let duplicate = e.msg.contains("duplicate key")
            || e.msg.contains("defined twice")
            || e.msg.contains("already defined");
        ArchError {
            kind: if duplicate {
                ErrorKind::Duplicate
            } else {
                ErrorKind::Syntax
            },
            field: "TOML".into(),
            message: e.msg,
            offset: Some(e.span.start),
        }
    })?;
    let root = &document.root;
    unknown(root, "", &["arch", "source", "lanes", "nodes", "edges"])?;
    let arch = table(required(root, "arch", "arch")?, "arch")?;
    unknown(
        arch,
        "arch",
        &["version", "scope", "title", "prompt", "generator"],
    )?;
    let version = required(arch, "version", "arch.version")?;
    let Toml::Num(_, span) = version else {
        return Err(type_error("arch.version", "integer"));
    };
    // The TOML API stores all numbers as f64: inspect the original lexeme to
    // prevent fractional/exponent/underflow representations passing as v0.
    let raw = text[span.start..span.end()].replace('_', "");
    let unsigned = raw.trim_start_matches(['+', '-']);
    let (digits, radix) = if let Some(digits) = unsigned.strip_prefix("0x") {
        (digits, 16)
    } else if let Some(digits) = unsigned.strip_prefix("0o") {
        (digits, 8)
    } else if let Some(digits) = unsigned.strip_prefix("0b") {
        (digits, 2)
    } else {
        (unsigned, 10)
    };
    match u64::from_str_radix(digits, radix) {
        Ok(0) => {}
        Ok(_) => {
            return Err(ArchError::new(
                ErrorKind::Version,
                "arch.version",
                "expected schema integer 0",
            ))
        }
        Err(_) => return Err(type_error("arch.version", "schema integer 0")),
    }
    let header = Header {
        version: 0,
        scope: string(arch, "scope", "arch.scope")?,
        title: string(arch, "title", "arch.title")?,
        prompt: string(arch, "prompt", "arch.prompt")?,
        generator: string(arch, "generator", "arch.generator")?,
    };
    let source = table(required(root, "source", "source")?, "source")?;
    unknown(source, "source", &["files", "overview"])?;
    let overview = string(source, "overview", "source.overview")?;
    limit(
        "source.overview",
        overview.chars().count(),
        limits.max_overview_chars,
    )?;
    let values = array(required(source, "files", "source.files")?, "source.files")?;
    limit("source.files", values.len(), limits.max_files)?;
    let mut files = Vec::with_capacity(values.len());
    let mut paths = BTreeSet::new();
    for (i, value) in values.iter().enumerate() {
        let field = format!("source.files[{i}]");
        let row = table(value, &field)?;
        unknown(row, &field, &["path", "hash"])?;
        let path = string(row, "path", &format!("{field}.path"))?;
        if !paths.insert(crate::files::normalized(PathBuf::from(&path).as_path())) {
            return Err(ArchError::new(
                ErrorKind::Duplicate,
                &field,
                format!("duplicate path `{path}`"),
            ));
        }
        let hash = string(row, "hash", &format!("{field}.hash"))?;
        check_hash(&hash, &format!("{field}.hash"))?;
        files.push(FileHash {
            path: PathBuf::from(path),
            hash,
        });
    }
    let mut lanes = BTreeMap::new();
    if let Some(value) = root.get("lanes") {
        let rows = table(value, "lanes")?;
        limit("lanes", rows.len(), limits.max_lanes)?;
        for (id, value) in rows {
            let field = format!("lanes.{id}");
            let row = table(value, &field)?;
            unknown(row, &field, &["title"])?;
            lanes.insert(
                id.clone(),
                Lane {
                    title: string(row, "title", &format!("{field}.title"))?,
                },
            );
        }
    }
    let rows = table(required(root, "nodes", "nodes")?, "nodes")?;
    limit("nodes", rows.len(), limits.max_nodes)?;
    let mut nodes = BTreeMap::new();
    for (id, value) in rows {
        let field = format!("nodes.{id}");
        let row = table(value, &field)?;
        unknown(
            row,
            &field,
            &[
                "kind", "title", "summary", "story", "lane", "refs", "budget", "child", "visual",
            ],
        )?;
        let kind = string(row, "kind", &format!("{field}.kind"))?;
        let summary = string(row, "summary", &format!("{field}.summary"))?;
        limit(
            &format!("{field}.summary"),
            summary.chars().count(),
            limits.max_summary_chars,
        )?;
        let story = string(row, "story", &format!("{field}.story"))?;
        limit(
            &format!("{field}.story"),
            story.chars().count(),
            limits.max_story_chars,
        )?;
        let refs_field = format!("{field}.refs");
        let refs = array(required(row, "refs", &refs_field)?, &refs_field)?
            .iter()
            .map(|v| text_value(v, &refs_field).map(str::to_owned))
            .collect::<Result<_, _>>()?;
        nodes.insert(
            id.clone(),
            Node {
                kind: kind.parse().map_err(|_| {
                    ArchError::new(ErrorKind::InvalidKind, &format!("{field}.kind"), kind)
                })?,
                title: string(row, "title", &format!("{field}.title"))?,
                summary,
                story,
                lane: optional(row, "lane", &field)?,
                refs,
                budget: optional(row, "budget", &field)?,
                child: optional(row, "child", &field)?,
                visual: optional(row, "visual", &field)?,
            },
        );
    }
    let mut edges = BTreeMap::new();
    if let Some(value) = root.get("edges") {
        let rows = table(value, "edges")?;
        limit("edges", rows.len(), limits.max_edges)?;
        for (id, value) in rows {
            let field = format!("edges.{id}");
            let row = table(value, &field)?;
            unknown(row, &field, &["from", "to", "kind", "label"])?;
            let kind = string(row, "kind", &format!("{field}.kind"))?;
            edges.insert(
                id.clone(),
                Edge {
                    from: string(row, "from", &format!("{field}.from"))?,
                    to: string(row, "to", &format!("{field}.to"))?,
                    kind: kind.parse().map_err(|_| {
                        ArchError::new(ErrorKind::InvalidKind, &format!("{field}.kind"), kind)
                    })?,
                    label: optional(row, "label", &field)?,
                },
            );
        }
    }
    let plan = Plan {
        arch: header,
        source: Source { files },
        overview,
        lanes,
        nodes,
        edges,
    };
    if let Some(error) = structure_errors(&plan).into_iter().next() {
        return Err(error);
    }
    // Quoting and table headers can expand compact input. Never admit a plan
    // whose canonical form cannot be read again under the same resource caps.
    crate::guard::preflight(&format(&plan), &limits)?;
    Ok(plan)
}

pub(crate) fn structure_errors(plan: &Plan) -> Vec<ArchError> {
    let mut errors = Vec::new();
    for (section, ids) in [
        ("nodes", plan.nodes.keys().collect::<Vec<_>>()),
        ("lanes", plan.lanes.keys().collect()),
        ("edges", plan.edges.keys().collect()),
    ] {
        for id in ids {
            if !valid_id(id) {
                errors.push(ArchError::new(
                    ErrorKind::InvalidId,
                    &format!("{section}.{id}"),
                    "expected [a-z][a-z0-9_-]{0,63}",
                ));
            }
        }
    }
    for (id, node) in &plan.nodes {
        if let Some(lane) = &node.lane {
            if !plan.lanes.contains_key(lane) {
                errors.push(ArchError::new(
                    ErrorKind::Dangling,
                    &format!("nodes.{id}.lane"),
                    format!("unknown lane `{lane}`"),
                ));
            }
        }
    }
    for (id, edge) in &plan.edges {
        for (key, target) in [("from", &edge.from), ("to", &edge.to)] {
            if !plan.nodes.contains_key(target) {
                errors.push(ArchError::new(
                    ErrorKind::Dangling,
                    &format!("edges.{id}.{key}"),
                    format!("unknown node `{target}`"),
                ));
            }
        }
    }
    errors
}

pub(crate) fn check_hash(hash: &str, field: &str) -> Result<(), ArchError> {
    if hash.len() == 40
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(ArchError::new(
            ErrorKind::InvalidHash,
            field,
            "expected 40 lowercase hex digits (Git blob hash)",
        ))
    }
}

fn unknown(row: &TomlTable, prefix: &str, allowed: &[&str]) -> Result<(), ArchError> {
    for key in row.keys() {
        if !allowed.contains(&key.as_str()) {
            let field = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            return Err(ArchError::new(
                ErrorKind::UnknownField,
                &field,
                format!("unknown field `{key}`"),
            ));
        }
    }
    Ok(())
}
fn type_error(field: &str, expected: &str) -> ArchError {
    ArchError::new(ErrorKind::Type, field, format!("expected {expected}"))
}
fn required<'a>(row: &'a TomlTable, key: &str, field: &str) -> Result<&'a Toml, ArchError> {
    row.get(key)
        .ok_or_else(|| ArchError::new(ErrorKind::MissingField, field, "required field is missing"))
}
fn table<'a>(value: &'a Toml, field: &str) -> Result<&'a TomlTable, ArchError> {
    value.as_table().ok_or_else(|| type_error(field, "table"))
}
fn array<'a>(value: &'a Toml, field: &str) -> Result<&'a [Toml], ArchError> {
    value.as_array().ok_or_else(|| type_error(field, "array"))
}
fn text_value<'a>(value: &'a Toml, field: &str) -> Result<&'a str, ArchError> {
    value.as_str().ok_or_else(|| type_error(field, "string"))
}
fn string(row: &TomlTable, key: &str, field: &str) -> Result<String, ArchError> {
    text_value(required(row, key, field)?, field).map(str::to_owned)
}
fn optional(row: &TomlTable, key: &str, prefix: &str) -> Result<Option<String>, ArchError> {
    row.get(key)
        .map(|v| text_value(v, &format!("{prefix}.{key}")).map(str::to_owned))
        .transpose()
}
