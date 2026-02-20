use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::str;

use crate::error::GitError;
use crate::index::{write_index, Index, IndexEntry};
use crate::object::{write_loose_object_fast, Object, ObjectKind};
use crate::oid::{hash_object, ObjectId};
use crate::refs::{self, RefTarget};
use crate::repo::Repository;
use crate::tree::Tree;
use crate::worktree;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone)]
pub struct GitHttpRequest {
    pub method: GitHttpMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct GitHttpResponse {
    pub status_code: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RemoteHead {
    pub oid: ObjectId,
    pub ref_name: Option<String>,
    pub capabilities: Vec<String>,
    pub etag: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HttpSyncReport {
    pub imported_objects: usize,
    pub checked_out_files: usize,
    pub checked_out_bytes: u64,
}

pub trait HttpSyncHooks {
    fn on_checkout_file(&mut self, _path: &str) {}
}

pub struct NoopHttpSyncHooks;

impl HttpSyncHooks for NoopHttpSyncHooks {}

pub fn build_ls_refs_head_request(remote_url: &str) -> Result<GitHttpRequest, GitError> {
    let mut body = Vec::new();
    write_pkt_line(&mut body, b"command=ls-refs\n")?;
    write_pkt_line(&mut body, b"agent=makepad-git/0.1\n")?;
    write_pkt_line(&mut body, b"object-format=sha1\n")?;
    write_delim(&mut body);
    write_pkt_line(&mut body, b"peel\n")?;
    write_pkt_line(&mut body, b"symrefs\n")?;
    write_pkt_line(&mut body, b"unborn\n")?;
    write_pkt_line(&mut body, b"ref-prefix HEAD\n")?;
    write_flush(&mut body);

    Ok(GitHttpRequest {
        method: GitHttpMethod::Post,
        url: format!("{}/git-upload-pack", normalize_remote_url(remote_url)),
        headers: vec![
            (
                "Content-Type".to_string(),
                "application/x-git-upload-pack-request".to_string(),
            ),
            (
                "Accept".to_string(),
                "application/x-git-upload-pack-result".to_string(),
            ),
            ("Git-Protocol".to_string(), "version=2".to_string()),
            ("User-Agent".to_string(), "makepad-git/0.1".to_string()),
        ],
        body,
    })
}

pub fn build_info_refs_request(remote_url: &str, if_none_match: Option<&str>) -> GitHttpRequest {
    let mut headers = vec![
        (
            "Accept".to_string(),
            "application/x-git-upload-pack-advertisement".to_string(),
        ),
        ("User-Agent".to_string(), "makepad-git/0.1".to_string()),
    ];

    if let Some(etag) = if_none_match {
        if !etag.is_empty() {
            headers.push(("If-None-Match".to_string(), etag.to_string()));
        }
    }

    GitHttpRequest {
        method: GitHttpMethod::Get,
        url: format!(
            "{}/info/refs?service=git-upload-pack",
            normalize_remote_url(remote_url)
        ),
        headers,
        body: Vec::new(),
    }
}

pub fn parse_ls_refs_head_response(response: &GitHttpResponse) -> Result<RemoteHead, GitError> {
    if response.status_code != 200 {
        return Err(GitError::InvalidRef(format!(
            "ls-refs request failed with HTTP {}",
            response.status_code
        )));
    }

    let mut reader = PktLineReader::new(&response.body);
    let mut head_oid = None;
    let mut head_symref = None;

    while let Some(pkt) = reader.next()? {
        let PktLine::Data(data) = pkt else {
            continue;
        };

        let line = trim_line_end(data);
        if line.is_empty() {
            continue;
        }

        if line.starts_with(b"ERR ") {
            return Err(GitError::InvalidRef(format!(
                "remote error: {}",
                String::from_utf8_lossy(line)
            )));
        }

        let line = str::from_utf8(line)
            .map_err(|_| GitError::InvalidRef("invalid UTF-8 in ls-refs response".to_string()))?;

        let mut parts = line.split_whitespace();
        let Some(oid_hex) = parts.next() else {
            continue;
        };
        let Some(ref_name) = parts.next() else {
            continue;
        };

        if ref_name != "HEAD" {
            continue;
        }

        let oid = ObjectId::from_hex(oid_hex)?;
        head_oid = Some(oid);

        for attr in parts {
            if let Some(symref_target) = attr.strip_prefix("symref-target:") {
                head_symref = Some(symref_target.to_string());
            }
        }
    }

    let oid = head_oid.ok_or_else(|| GitError::RefNotFound("HEAD".to_string()))?;
    Ok(RemoteHead {
        oid,
        ref_name: head_symref,
        capabilities: Vec::new(),
        etag: None,
    })
}

pub fn parse_info_refs_response(
    response: &GitHttpResponse,
    branch: Option<&str>,
) -> Result<RemoteHead, GitError> {
    if response.status_code != 200 {
        return Err(GitError::InvalidRef(format!(
            "info/refs request failed with HTTP {}",
            response.status_code
        )));
    }

    let mut refs_map = HashMap::<String, ObjectId>::new();
    let mut caps_seen = HashSet::<String>::new();
    let mut capabilities = Vec::<String>::new();

    let mut reader = PktLineReader::new(&response.body);
    let mut seen_service_separator = false;

    while let Some(pkt) = reader.next()? {
        match pkt {
            PktLine::Flush => {
                if !seen_service_separator {
                    seen_service_separator = true;
                    continue;
                }
                break;
            }
            PktLine::Data(data) => {
                let line = trim_line_end(data);

                if !seen_service_separator && line.starts_with(b"# service=git-upload-pack") {
                    continue;
                }

                if line.starts_with(b"ERR ") {
                    return Err(GitError::InvalidRef(format!(
                        "remote error: {}",
                        String::from_utf8_lossy(line)
                    )));
                }

                let Some(space_pos) = line.iter().position(|b| *b == b' ') else {
                    return Err(GitError::InvalidRef("invalid ref advertisement line".to_string()));
                };

                let oid_hex = str::from_utf8(&line[..space_pos])
                    .map_err(|_| GitError::InvalidRef("invalid oid in info/refs".to_string()))?;
                let oid = ObjectId::from_hex(oid_hex)?;

                let mut ref_name_part = &line[space_pos + 1..];
                if let Some(nul_pos) = ref_name_part.iter().position(|b| *b == 0) {
                    let caps_part = &ref_name_part[nul_pos + 1..];
                    ref_name_part = &ref_name_part[..nul_pos];
                    for cap in caps_part.split(|b| *b == b' ') {
                        if cap.is_empty() {
                            continue;
                        }
                        let cap_string = String::from_utf8_lossy(cap).to_string();
                        if caps_seen.insert(cap_string.clone()) {
                            capabilities.push(cap_string);
                        }
                    }
                }

                let ref_name = str::from_utf8(ref_name_part)
                    .map_err(|_| GitError::InvalidRef("invalid ref name in info/refs".to_string()))?;

                refs_map.insert(ref_name.to_string(), oid);
            }
        }
    }

    if refs_map.is_empty() {
        return Err(GitError::InvalidRef(
            "no refs found in info/refs response".to_string(),
        ));
    }

    let mut head_symref = None;
    for cap in &capabilities {
        if let Some(symref) = cap.strip_prefix("symref=HEAD:") {
            head_symref = Some(symref.to_string());
            break;
        }
    }

    let (oid, ref_name) = if let Some(branch_name) = branch {
        let requested_ref = if branch_name.starts_with("refs/") {
            branch_name.to_string()
        } else {
            format!("refs/heads/{}", branch_name)
        };
        let oid = refs_map
            .get(&requested_ref)
            .copied()
            .ok_or_else(|| GitError::RefNotFound(requested_ref.clone()))?;
        (oid, Some(requested_ref))
    } else if let Some(symref) = head_symref {
        let oid = refs_map
            .get(&symref)
            .copied()
            .or_else(|| refs_map.get("HEAD").copied())
            .ok_or_else(|| GitError::RefNotFound("HEAD".to_string()))?;
        (oid, Some(symref))
    } else if let Some(oid) = refs_map.get("refs/heads/main").copied() {
        (oid, Some("refs/heads/main".to_string()))
    } else if let Some(oid) = refs_map.get("refs/heads/master").copied() {
        (oid, Some("refs/heads/master".to_string()))
    } else if let Some(oid) = refs_map.get("HEAD").copied() {
        (oid, None)
    } else {
        let mut keys: Vec<_> = refs_map.keys().cloned().collect();
        keys.sort();
        let first = keys
            .first()
            .cloned()
            .ok_or_else(|| GitError::RefNotFound("no advertised refs".to_string()))?;
        let oid = refs_map
            .get(&first)
            .copied()
            .ok_or_else(|| GitError::RefNotFound(first.clone()))?;
        (oid, Some(first))
    };

    Ok(RemoteHead {
        oid,
        ref_name,
        capabilities,
        etag: find_header_value(&response.headers, "etag"),
    })
}

pub fn build_upload_pack_request(
    remote_url: &str,
    target_oid: ObjectId,
    capabilities: &[String],
    have: &[ObjectId],
    depth: Option<u32>,
) -> Result<GitHttpRequest, GitError> {
    let has_cap = |name: &str| {
        capabilities
            .iter()
            .any(|cap| cap == name || cap.starts_with(&format!("{}=", name)))
    };

    if depth.is_some() && !has_cap("shallow") {
        return Err(GitError::InvalidRef(
            "remote does not advertise shallow capability".to_string(),
        ));
    }

    let mut want_caps = Vec::new();
    if has_cap("side-band-64k") {
        want_caps.push("side-band-64k".to_string());
    } else if has_cap("side-band") {
        want_caps.push("side-band".to_string());
    }
    if has_cap("ofs-delta") {
        want_caps.push("ofs-delta".to_string());
    }
    if has_cap("thin-pack") {
        want_caps.push("thin-pack".to_string());
    }
    if has_cap("no-progress") {
        want_caps.push("no-progress".to_string());
    }

    let mut body = Vec::new();

    let want_line = if want_caps.is_empty() {
        format!("want {}\n", target_oid.to_hex())
    } else {
        format!("want {} {}\n", target_oid.to_hex(), want_caps.join(" "))
    };
    write_pkt_line(&mut body, want_line.as_bytes())?;

    // Optional depth negotiation section. This is part of the want-list and
    // must be sent before the first flush-pkt.
    if let Some(depth) = depth {
        write_pkt_line(&mut body, format!("deepen {}\n", depth).as_bytes())?;
    }

    // End of want/deepen section.
    write_flush(&mut body);

    // \"have\" section for incremental updates.
    for oid in have {
        write_pkt_line(&mut body, format!("have {}\n", oid.to_hex()).as_bytes())?;
    }

    // End negotiation.
    write_pkt_line(&mut body, b"done\n")?;

    Ok(GitHttpRequest {
        method: GitHttpMethod::Post,
        url: format!("{}/git-upload-pack", normalize_remote_url(remote_url)),
        headers: vec![
            (
                "Content-Type".to_string(),
                "application/x-git-upload-pack-request".to_string(),
            ),
            (
                "Accept".to_string(),
                "application/x-git-upload-pack-result".to_string(),
            ),
            ("User-Agent".to_string(), "makepad-git/0.1".to_string()),
        ],
        body,
    })
}

pub fn extract_pack_from_response(response: &GitHttpResponse) -> Result<Option<Vec<u8>>, GitError> {
    if response.status_code != 200 {
        return Err(GitError::InvalidRef(format!(
            "upload-pack request failed with HTTP {}",
            response.status_code
        )));
    }

    if response.body.is_empty() {
        return Ok(None);
    }

    if response.body.starts_with(b"PACK") {
        return Ok(Some(response.body.clone()));
    }

    let mut reader = PktLineReader::new(&response.body);
    let mut pack_data = Vec::new();
    let mut saw_pkt_lines = false;

    while let Some(pkt) = reader.next()? {
        saw_pkt_lines = true;

        let PktLine::Data(data) = pkt else {
            continue;
        };

        if data.starts_with(b"ERR ") {
            return Err(GitError::InvalidRef(format!(
                "remote error: {}",
                String::from_utf8_lossy(data)
            )));
        }

        if data.starts_with(b"NAK")
            || data.starts_with(b"ACK ")
            || data.starts_with(b"shallow ")
            || data.starts_with(b"unshallow ")
        {
            continue;
        }

        if data.is_empty() {
            continue;
        }

        match data[0] {
            1 => pack_data.extend_from_slice(&data[1..]),
            2 => {}
            3 => {
                let msg = String::from_utf8_lossy(&data[1..]).trim().to_string();
                return Err(GitError::InvalidRef(format!("remote fatal error: {}", msg)));
            }
            _ => {
                if data.starts_with(b"PACK") {
                    pack_data.extend_from_slice(data);
                }
            }
        }
    }

    if !pack_data.is_empty() {
        return Ok(Some(pack_data));
    }

    if !saw_pkt_lines {
        if let Some(idx) = find_subslice(&response.body, b"PACK") {
            return Ok(Some(response.body[idx..].to_vec()));
        }
    }

    Ok(None)
}

pub fn apply_pack_and_checkout(
    dst: &Path,
    remote_url: &str,
    target_oid: ObjectId,
    target_ref: Option<&str>,
    pack_data: &[u8],
    hooks: &mut dyn HttpSyncHooks,
) -> Result<HttpSyncReport, GitError> {
    let old_head = Repository::open(dst).ok().and_then(|repo| repo.head_oid().ok());

    ensure_repo_layout(dst, remote_url)?;

    let mut existing_repo = Repository::open(dst).ok();
    let imported_objects = if pack_data.is_empty() {
        0
    } else {
        import_pack_to_loose(
            &dst.join(".git"),
            pack_data,
            existing_repo.as_mut(),
        )?
    };

    let mut repo = Repository::open(dst)?;
    let (checked_out_files, checked_out_bytes) =
        checkout_commit(&mut repo, old_head, target_oid, hooks)?;

    if let Some(ref_name) = target_ref {
        refs::write_ref(&repo.git_dir, ref_name, &target_oid)?;
        if ref_name.starts_with("refs/heads/") {
            refs::update_head(&repo.git_dir, &RefTarget::Symbolic(ref_name.to_string()))?;
            if let Some(branch) = ref_name.strip_prefix("refs/heads/") {
                let _ = refs::write_ref(
                    &repo.git_dir,
                    &format!("refs/remotes/origin/{}", branch),
                    &target_oid,
                );
            }
        } else {
            refs::update_head(&repo.git_dir, &RefTarget::Direct(target_oid))?;
        }
    } else {
        refs::update_head(&repo.git_dir, &RefTarget::Direct(target_oid))?;
    }

    fs::write(
        repo.git_dir.join("shallow"),
        format!("{}\n", target_oid.to_hex()),
    )?;

    write_basic_config(&repo.git_dir, remote_url)?;

    Ok(HttpSyncReport {
        imported_objects,
        checked_out_files,
        checked_out_bytes,
    })
}

fn normalize_remote_url(remote_url: &str) -> String {
    remote_url.trim_end_matches('/').to_string()
}

fn ensure_repo_layout(dst: &Path, remote_url: &str) -> Result<(), GitError> {
    if dst.exists() {
        if !dst.is_dir() {
            return Err(GitError::InvalidRef(format!(
                "destination is not a directory: {}",
                dst.display()
            )));
        }
    } else {
        fs::create_dir_all(dst)?;
    }

    let git_dir = dst.join(".git");
    fs::create_dir_all(git_dir.join("objects/info"))?;
    fs::create_dir_all(git_dir.join("refs/heads"))?;
    fs::create_dir_all(git_dir.join("refs/tags"))?;

    if !git_dir.join("HEAD").exists() {
        refs::update_head(&git_dir, &RefTarget::Symbolic("refs/heads/main".to_string()))?;
    }

    write_basic_config(&git_dir, remote_url)?;
    Ok(())
}

fn write_basic_config(git_dir: &Path, remote_url: &str) -> Result<(), GitError> {
    let config = format!(
        "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n[remote \"origin\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        remote_url
    );
    fs::write(git_dir.join("config"), config)?;
    Ok(())
}

#[derive(Debug, Clone)]
struct ParsedPackEntry {
    offset: u64,
    kind: ParsedPackEntryKind,
}

#[derive(Debug, Clone)]
enum ParsedPackEntryKind {
    Full {
        kind: ObjectKind,
        data: Vec<u8>,
    },
    DeltaOfs {
        base_offset: u64,
        delta: Vec<u8>,
    },
    DeltaRef {
        base_oid: ObjectId,
        delta: Vec<u8>,
    },
}

#[derive(Debug, Clone)]
struct ResolvedPackObject {
    kind: ObjectKind,
    data: Vec<u8>,
    oid: ObjectId,
}

fn import_pack_to_loose(
    git_dir: &Path,
    pack_data: &[u8],
    mut existing_repo: Option<&mut Repository>,
) -> Result<usize, GitError> {
    if pack_data.len() < 12 {
        return Err(GitError::CorruptPack("pack too small".to_string()));
    }
    if &pack_data[..4] != b"PACK" {
        return Err(GitError::CorruptPack("missing PACK header".to_string()));
    }

    let version = read_u32_be(pack_data, 4)?;
    if version != 2 && version != 3 {
        return Err(GitError::CorruptPack(format!(
            "unsupported pack version {}",
            version
        )));
    }

    let num_objects = read_u32_be(pack_data, 8)? as usize;
    let mut pos = 12usize;
    let mut parsed = Vec::with_capacity(num_objects);

    for _ in 0..num_objects {
        let offset = pos as u64;
        if pos >= pack_data.len() {
            return Err(GitError::CorruptPack(
                "unexpected end of pack while reading object header".to_string(),
            ));
        }

        let first = pack_data[pos];
        pos += 1;

        let type_num = (first >> 4) & 0x07;
        let mut size = (first & 0x0f) as usize;
        let mut shift = 4usize;
        let mut byte = first;

        while (byte & 0x80) != 0 {
            if pos >= pack_data.len() {
                return Err(GitError::CorruptPack(
                    "truncated pack size varint".to_string(),
                ));
            }
            byte = pack_data[pos];
            pos += 1;
            size |= ((byte & 0x7f) as usize) << shift;
            shift += 7;
        }

        match type_num {
            1 | 2 | 3 | 4 => {
                let kind = ObjectKind::from_type_num(type_num)?;
                let (data, consumed) = decompress_object_data(&pack_data[pos..], size)?;
                pos += consumed;
                parsed.push(ParsedPackEntry {
                    offset,
                    kind: ParsedPackEntryKind::Full { kind, data },
                });
            }
            6 => {
                if pos >= pack_data.len() {
                    return Err(GitError::CorruptPack(
                        "truncated ofs-delta base offset".to_string(),
                    ));
                }

                let mut b = pack_data[pos];
                pos += 1;
                let mut delta_off = (b & 0x7f) as u64;
                while (b & 0x80) != 0 {
                    if pos >= pack_data.len() {
                        return Err(GitError::CorruptPack(
                            "truncated ofs-delta base offset".to_string(),
                        ));
                    }
                    b = pack_data[pos];
                    pos += 1;
                    delta_off = ((delta_off + 1) << 7) | (b & 0x7f) as u64;
                }

                let base_offset = offset.checked_sub(delta_off).ok_or_else(|| {
                    GitError::CorruptPack("invalid ofs-delta base offset".to_string())
                })?;

                let (delta, consumed) = decompress_object_data(&pack_data[pos..], size)?;
                pos += consumed;

                parsed.push(ParsedPackEntry {
                    offset,
                    kind: ParsedPackEntryKind::DeltaOfs { base_offset, delta },
                });
            }
            7 => {
                if pos + 20 > pack_data.len() {
                    return Err(GitError::CorruptPack(
                        "truncated ref-delta base oid".to_string(),
                    ));
                }
                let base_oid = ObjectId::from_slice(&pack_data[pos..pos + 20])?;
                pos += 20;

                let (delta, consumed) = decompress_object_data(&pack_data[pos..], size)?;
                pos += consumed;

                parsed.push(ParsedPackEntry {
                    offset,
                    kind: ParsedPackEntryKind::DeltaRef { base_oid, delta },
                });
            }
            _ => {
                return Err(GitError::CorruptPack(format!(
                    "unsupported pack object type {}",
                    type_num
                )));
            }
        }
    }

    if pos + 20 > pack_data.len() {
        return Err(GitError::CorruptPack(
            "pack missing trailing checksum".to_string(),
        ));
    }

    let mut resolved_by_offset = HashMap::<u64, ResolvedPackObject>::new();
    let mut resolved_oid_to_offset = HashMap::<ObjectId, u64>::new();
    let mut external_cache = HashMap::<ObjectId, Object>::new();

    let mut unresolved: Vec<usize> = (0..parsed.len()).collect();
    while !unresolved.is_empty() {
        let mut next = Vec::new();
        let mut progressed = false;

        for idx in unresolved {
            let entry = &parsed[idx];

            match &entry.kind {
                ParsedPackEntryKind::Full { kind, data } => {
                    let oid = hash_object(kind.as_str(), data);
                    let resolved = ResolvedPackObject {
                        kind: *kind,
                        data: data.clone(),
                        oid,
                    };
                    resolved_oid_to_offset.insert(oid, entry.offset);
                    resolved_by_offset.insert(entry.offset, resolved);
                    progressed = true;
                }
                ParsedPackEntryKind::DeltaOfs { base_offset, delta } => {
                    let Some(base) = resolved_by_offset.get(base_offset) else {
                        next.push(idx);
                        continue;
                    };

                    let data = apply_delta(&base.data, delta)?;
                    let oid = hash_object(base.kind.as_str(), &data);
                    let resolved = ResolvedPackObject {
                        kind: base.kind,
                        data,
                        oid,
                    };
                    resolved_oid_to_offset.insert(oid, entry.offset);
                    resolved_by_offset.insert(entry.offset, resolved);
                    progressed = true;
                }
                ParsedPackEntryKind::DeltaRef { base_oid, delta } => {
                    let base_object = if let Some(offset) = resolved_oid_to_offset.get(base_oid) {
                        resolved_by_offset.get(offset).map(|obj| Object {
                            kind: obj.kind,
                            data: obj.data.clone(),
                        })
                    } else if let Some(obj) = external_cache.get(base_oid).cloned() {
                        Some(obj)
                    } else if let Some(repo) = existing_repo.as_deref_mut() {
                        match repo.read_object(base_oid) {
                            Ok(obj) => {
                                external_cache.insert(*base_oid, obj.clone());
                                Some(obj)
                            }
                            Err(_) => None,
                        }
                    } else {
                        None
                    };

                    let Some(base) = base_object else {
                        next.push(idx);
                        continue;
                    };

                    let data = apply_delta(&base.data, delta)?;
                    let oid = hash_object(base.kind.as_str(), &data);
                    let resolved = ResolvedPackObject {
                        kind: base.kind,
                        data,
                        oid,
                    };
                    resolved_oid_to_offset.insert(oid, entry.offset);
                    resolved_by_offset.insert(entry.offset, resolved);
                    progressed = true;
                }
            }
        }

        if !progressed {
            return Err(GitError::CorruptPack(
                "could not resolve all delta objects (thin pack base missing?)".to_string(),
            ));
        }

        unresolved = next;
    }

    for entry in &parsed {
        let obj = resolved_by_offset.get(&entry.offset).ok_or_else(|| {
            GitError::CorruptPack("resolved object missing by offset".to_string())
        })?;
        let written_oid = write_loose_object_fast(git_dir, obj.kind, &obj.data)?;
        if written_oid != obj.oid {
            return Err(GitError::CorruptPack(
                "object hash mismatch while writing loose object".to_string(),
            ));
        }
    }

    Ok(parsed.len())
}

fn decompress_object_data(input: &[u8], expected_size: usize) -> Result<(Vec<u8>, usize), GitError> {
    let mut out = vec![0u8; expected_size];
    let (consumed, written) = makepad_fast_inflate::zlib_decompress(input, &mut out)
        .map_err(|e| GitError::CorruptPack(format!("zlib decompress failed: {}", e)))?;

    if written != expected_size {
        return Err(GitError::CorruptPack(format!(
            "zlib size mismatch: expected {}, got {}",
            expected_size, written
        )));
    }

    out.truncate(written);
    Ok((out, consumed))
}

fn checkout_commit(
    repo: &mut Repository,
    old_head: Option<ObjectId>,
    new_head: ObjectId,
    hooks: &mut dyn HttpSyncHooks,
) -> Result<(usize, u64), GitError> {
    let commit = repo.read_commit(&new_head)?;
    let tree = repo.read_tree(&commit.tree)?;

    let mut old_files = HashMap::new();
    if let Some(old_head) = old_head {
        if let Ok(old_commit) = repo.read_commit(&old_head) {
            let old_tree = repo.read_tree(&old_commit.tree)?;
            flatten_tree_recursive(repo, &old_tree, "", &mut old_files)?;
        }
    }

    let mut new_files = HashMap::new();
    flatten_tree_recursive(repo, &tree, "", &mut new_files)?;

    worktree::remove_worktree_files(&repo.workdir, &old_files, &new_files)?;

    let mut index_entries = Vec::new();
    let mut checked_out_bytes = 0u64;
    checkout_tree_recursive(
        repo,
        &tree,
        "",
        &mut index_entries,
        &mut checked_out_bytes,
        hooks,
    )?;

    index_entries.sort_by(|a, b| a.path.cmp(&b.path));
    let checked_out_files = index_entries.len();

    write_index(
        &repo.git_dir,
        &Index {
            version: 2,
            entries: index_entries,
        },
    )?;

    Ok((checked_out_files, checked_out_bytes))
}

fn flatten_tree_recursive(
    repo: &mut Repository,
    tree: &Tree,
    prefix: &str,
    out: &mut HashMap<String, ObjectId>,
) -> Result<(), GitError> {
    for entry in &tree.entries {
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{}{}", prefix, entry.name)
        };

        if entry.is_tree() {
            let sub_tree = repo.read_tree(&entry.oid)?;
            flatten_tree_recursive(repo, &sub_tree, &format!("{}/", path), out)?;
        } else if !entry.is_gitlink() {
            out.insert(path, entry.oid);
        }
    }

    Ok(())
}

fn checkout_tree_recursive(
    repo: &mut Repository,
    tree: &Tree,
    prefix: &str,
    index_entries: &mut Vec<IndexEntry>,
    checked_out_bytes: &mut u64,
    hooks: &mut dyn HttpSyncHooks,
) -> Result<(), GitError> {
    for entry in &tree.entries {
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{}{}", prefix, entry.name)
        };

        if entry.is_tree() {
            let sub_tree = repo.read_tree(&entry.oid)?;
            fs::create_dir_all(repo.workdir.join(&path))?;
            checkout_tree_recursive(
                repo,
                &sub_tree,
                &format!("{}/", path),
                index_entries,
                checked_out_bytes,
                hooks,
            )?;
            continue;
        }

        if entry.is_gitlink() {
            continue;
        }

        let data = repo.read_blob(&entry.oid)?;
        let file_path = repo.workdir.join(&path);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        if entry.is_symlink() {
            #[cfg(unix)]
            {
                let target = std::str::from_utf8(&data).unwrap_or("");
                let _ = fs::remove_file(&file_path);
                std::os::unix::fs::symlink(target, &file_path)?;
            }
            #[cfg(not(unix))]
            {
                fs::write(&file_path, &data)?;
            }
        } else {
            fs::write(&file_path, &data)?;
            #[cfg(unix)]
            if entry.mode == 0o100755 {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&file_path, fs::Permissions::from_mode(0o755))?;
            }
        }

        hooks.on_checkout_file(&path);
        *checked_out_bytes += data.len() as u64;

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = if entry.is_symlink() {
                fs::symlink_metadata(&file_path)?
            } else {
                fs::metadata(&file_path)?
            };

            index_entries.push(IndexEntry {
                ctime_sec: metadata.mtime() as u32,
                ctime_nsec: 0,
                mtime_sec: metadata.mtime() as u32,
                mtime_nsec: 0,
                dev: metadata.dev() as u32,
                ino: metadata.ino() as u32,
                mode: entry.mode,
                uid: metadata.uid(),
                gid: metadata.gid(),
                file_size: metadata.len() as u32,
                oid: entry.oid,
                flags: (path.len().min(0x0fff)) as u16,
                path,
            });
        }

        #[cfg(not(unix))]
        {
            index_entries.push(IndexEntry {
                ctime_sec: 0,
                ctime_nsec: 0,
                mtime_sec: 0,
                mtime_nsec: 0,
                dev: 0,
                ino: 0,
                mode: entry.mode,
                uid: 0,
                gid: 0,
                file_size: data.len() as u32,
                oid: entry.oid,
                flags: (path.len().min(0x0fff)) as u16,
                path,
            });
        }
    }

    Ok(())
}

fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>, GitError> {
    let mut pos = 0;

    let (_, read) = read_delta_size(delta, pos)?;
    pos += read;

    let (result_size, read) = read_delta_size(delta, pos)?;
    pos += read;

    let mut result = Vec::with_capacity(result_size as usize);

    while pos < delta.len() {
        let cmd = delta[pos];
        pos += 1;

        if (cmd & 0x80) != 0 {
            let mut copy_offset = 0u32;
            let mut copy_size = 0u32;

            if (cmd & 0x01) != 0 {
                copy_offset |= delta[pos] as u32;
                pos += 1;
            }
            if (cmd & 0x02) != 0 {
                copy_offset |= (delta[pos] as u32) << 8;
                pos += 1;
            }
            if (cmd & 0x04) != 0 {
                copy_offset |= (delta[pos] as u32) << 16;
                pos += 1;
            }
            if (cmd & 0x08) != 0 {
                copy_offset |= (delta[pos] as u32) << 24;
                pos += 1;
            }
            if (cmd & 0x10) != 0 {
                copy_size |= delta[pos] as u32;
                pos += 1;
            }
            if (cmd & 0x20) != 0 {
                copy_size |= (delta[pos] as u32) << 8;
                pos += 1;
            }
            if (cmd & 0x40) != 0 {
                copy_size |= (delta[pos] as u32) << 16;
                pos += 1;
            }

            if copy_size == 0 {
                copy_size = 0x10000;
            }

            let start = copy_offset as usize;
            let end = start + copy_size as usize;
            if end > base.len() {
                return Err(GitError::CorruptPack(format!(
                    "delta copy out of range: {}..{} but base len is {}",
                    start,
                    end,
                    base.len()
                )));
            }

            result.extend_from_slice(&base[start..end]);
        } else if cmd > 0 {
            let n = cmd as usize;
            if pos + n > delta.len() {
                return Err(GitError::CorruptPack("delta insert truncated".to_string()));
            }
            result.extend_from_slice(&delta[pos..pos + n]);
            pos += n;
        } else {
            return Err(GitError::CorruptPack(
                "delta reserved opcode 0x00".to_string(),
            ));
        }
    }

    if result.len() != result_size as usize {
        return Err(GitError::CorruptPack(format!(
            "delta result size mismatch: expected {}, got {}",
            result_size,
            result.len()
        )));
    }

    Ok(result)
}

fn read_delta_size(data: &[u8], start: usize) -> Result<(u64, usize), GitError> {
    let mut pos = start;
    let mut size = 0u64;
    let mut shift = 0;

    loop {
        if pos >= data.len() {
            return Err(GitError::CorruptPack("truncated delta size".to_string()));
        }
        let b = data[pos];
        pos += 1;
        size |= ((b & 0x7f) as u64) << shift;
        shift += 7;
        if (b & 0x80) == 0 {
            break;
        }
    }

    Ok((size, pos - start))
}

fn read_u32_be(data: &[u8], offset: usize) -> Result<u32, GitError> {
    if offset + 4 > data.len() {
        return Err(GitError::CorruptPack(
            "truncated u32 in pack header".to_string(),
        ));
    }
    Ok(u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}

fn write_pkt_line(out: &mut Vec<u8>, payload: &[u8]) -> Result<(), GitError> {
    let total_len = payload.len() + 4;
    if total_len > 0xffff {
        return Err(GitError::InvalidRef("pkt-line payload too large".to_string()));
    }
    out.extend_from_slice(format!("{:04x}", total_len).as_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

fn write_flush(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0000");
}

fn write_delim(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0001");
}

#[derive(Debug)]
enum PktLine<'a> {
    Flush,
    Data(&'a [u8]),
}

struct PktLineReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> PktLineReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn next(&mut self) -> Result<Option<PktLine<'a>>, GitError> {
        if self.pos >= self.data.len() {
            return Ok(None);
        }

        if self.pos + 4 > self.data.len() {
            return Err(GitError::InvalidRef(
                "truncated pkt-line length".to_string(),
            ));
        }

        let len_hex = str::from_utf8(&self.data[self.pos..self.pos + 4])
            .map_err(|_| GitError::InvalidRef("invalid pkt-line length".to_string()))?;
        let line_len = usize::from_str_radix(len_hex, 16)
            .map_err(|_| GitError::InvalidRef("invalid pkt-line length".to_string()))?;
        self.pos += 4;

        if line_len == 0 {
            return Ok(Some(PktLine::Flush));
        }
        if line_len < 4 {
            return Err(GitError::InvalidRef(
                "invalid pkt-line length (<4)".to_string(),
            ));
        }

        let payload_len = line_len - 4;
        if self.pos + payload_len > self.data.len() {
            return Err(GitError::InvalidRef(
                "truncated pkt-line payload".to_string(),
            ));
        }

        let payload = &self.data[self.pos..self.pos + payload_len];
        self.pos += payload_len;
        Ok(Some(PktLine::Data(payload)))
    }
}

fn trim_line_end(data: &[u8]) -> &[u8] {
    let mut end = data.len();
    if end > 0 && data[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && data[end - 1] == b'\r' {
        end -= 1;
    }
    &data[..end]
}

fn find_header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }

    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkt_line_roundtrip() {
        let mut out = Vec::new();
        write_pkt_line(&mut out, b"hello\n").unwrap();
        write_flush(&mut out);

        let mut rd = PktLineReader::new(&out);
        match rd.next().unwrap().unwrap() {
            PktLine::Data(data) => assert_eq!(data, b"hello\n"),
            _ => panic!("expected data"),
        }
        match rd.next().unwrap().unwrap() {
            PktLine::Flush => {}
            _ => panic!("expected flush"),
        }
        assert!(rd.next().unwrap().is_none());
    }

    #[test]
    fn parse_info_refs_basic() {
        let mut body = Vec::new();
        write_pkt_line(&mut body, b"# service=git-upload-pack\n").unwrap();
        write_flush(&mut body);
        write_pkt_line(
            &mut body,
            b"0123456789012345678901234567890123456789 refs/heads/main\0side-band-64k shallow symref=HEAD:refs/heads/main\n",
        )
        .unwrap();
        write_flush(&mut body);

        let response = GitHttpResponse {
            status_code: 200,
            headers: vec![("ETag".into(), "\"abc\"".into())],
            body,
        };

        let head = parse_info_refs_response(&response, None).unwrap();
        assert_eq!(head.oid.to_hex(), "0123456789012345678901234567890123456789");
        assert_eq!(head.ref_name.as_deref(), Some("refs/heads/main"));
        assert_eq!(head.etag.as_deref(), Some("\"abc\""));
    }

    #[test]
    fn parse_ls_refs_head_basic() {
        let mut body = Vec::new();
        write_pkt_line(
            &mut body,
            b"0123456789012345678901234567890123456789 HEAD symref-target:refs/heads/main\n",
        )
        .unwrap();
        write_flush(&mut body);

        let response = GitHttpResponse {
            status_code: 200,
            headers: vec![],
            body,
        };

        let head = parse_ls_refs_head_response(&response).unwrap();
        assert_eq!(head.oid.to_hex(), "0123456789012345678901234567890123456789");
        assert_eq!(head.ref_name.as_deref(), Some("refs/heads/main"));
    }

    #[test]
    fn upload_pack_depth_request_orders_deepen_before_flush() {
        let req = build_upload_pack_request(
            "https://example.com/repo",
            ObjectId::from_hex("0123456789012345678901234567890123456789").unwrap(),
            &[String::from("shallow")],
            &[],
            Some(1),
        )
        .unwrap();

        let mut rd = PktLineReader::new(&req.body);
        match rd.next().unwrap().unwrap() {
            PktLine::Data(data) => assert!(data.starts_with(b"want ")),
            _ => panic!("expected want data pkt"),
        }
        match rd.next().unwrap().unwrap() {
            PktLine::Data(data) => assert_eq!(data, b"deepen 1\n"),
            _ => panic!("expected deepen data pkt"),
        }
        match rd.next().unwrap().unwrap() {
            PktLine::Flush => {}
            _ => panic!("expected flush pkt"),
        }
        match rd.next().unwrap().unwrap() {
            PktLine::Data(data) => assert_eq!(data, b"done\n"),
            _ => panic!("expected done data pkt"),
        }
        assert!(rd.next().unwrap().is_none());
    }
}
