//! Portable Builder bootstrap and streaming ZIP personalization. Never log bootstrap contents.
use makepad_strict_json::{self as json, Value};
use makepad_zip_file::{zip_read_central_directory, ZipMethod, ZipWriter};
use std::{fs::File, io::{Cursor, Read, Seek, SeekFrom}, path::{Path, PathBuf}};

pub const BOOTSTRAP_FILE: &str = "makepad-builder.json";
const LEGACY_BOOTSTRAP_FILE: &str = "makepad-loader.json";

pub fn email(value: &str) -> Result<String, String> {
    let value = value.trim();
    let parts: Vec<_> = value.split('@').collect();
    if value.len() > 254 || parts.len() != 2 || parts[0].is_empty()
        || parts[0].len() > 64 || parts[1].is_empty()
        || !value.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'<' | b'>' | b'"' | b'\\'))
    {
        return Err("Enter a valid email address".into());
    }
    Ok(value.to_ascii_lowercase())
}

#[derive(Clone)]
pub struct Bootstrap {
    pub email: String,
    pub app: String,
}
impl Bootstrap {
    pub fn new(address: &str, app: &str) -> Result<Self, String> {
        if app.is_empty() || app.len() > 128 || !app.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err("Invalid bootstrap app".into());
        }
        Ok(Self { email: if app == "makepad" && address.is_empty() { String::new() } else { email(address)? }, app: app.into() })
    }
    pub fn encode(&self) -> Vec<u8> {
        json::obj(vec![("version", Value::Int(1)), ("email", json::s(&self.email)), ("app", json::s(&self.app))]).to_json().into_bytes()
    }
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let value = json::parse(data).map_err(|_| "Invalid Builder bootstrap")?;
        if value.get("version").and_then(Value::as_i64) != Some(1) {
            return Err("Unsupported Builder bootstrap version".into());
        }
        Self::new(value.get("email").and_then(Value::as_str).ok_or("Missing bootstrap email")?,
            value.get("app").and_then(Value::as_str).ok_or("Missing bootstrap app")?)
    }
}

/// macOS puts the executable inside an app; the portable folder is outside the signed bundle.
pub fn directory(executable: &Path) -> Result<PathBuf, String> {
    let parent = executable.parent().ok_or("Builder has no parent directory")?;
    if parent.file_name().is_some_and(|n| n == "MacOS")
        && parent.parent().and_then(Path::file_name).is_some_and(|n| n == "Contents")
        && parent.parent().and_then(Path::parent).and_then(Path::extension).is_some_and(|n| n == "app")
    {
        return parent.parent().and_then(Path::parent).and_then(Path::parent)
            .map(Path::to_path_buf).ok_or_else(|| "Builder bundle has no parent directory".into());
    }
    Ok(parent.to_path_buf())
}

/// Call on the setup worker. A packaged bootstrap makes its containing folder the install root.
pub fn load() -> Result<Option<(PathBuf, Bootstrap)>, String> {
    let dir = directory(&std::env::current_exe().map_err(|e| e.to_string())?)?;
    Ok(load_from(&dir)?.map(|bootstrap| (dir, bootstrap)))
}

/// Older portable folders keep working after replacing their executable.
pub fn load_from(dir: &Path) -> Result<Option<Bootstrap>, String> {
    let file = match File::open(dir.join(BOOTSTRAP_FILE)).or_else(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            File::open(dir.join(LEGACY_BOOTSTRAP_FILE))
        } else {
            Err(error)
        }
    }) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Cannot read the Builder bootstrap".into()),
    };
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes).map_err(|_| "Cannot read the Builder bootstrap")?;
    if bytes.len() > 4096 { return Err("Builder bootstrap is too large".into()); }
    Ok(Some(Bootstrap::parse(&bytes)?))
}

/// Encode a UTF-8 query value without treating a literal plus as a space.
pub fn query_value(text: &str) -> String {
    let mut result = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            result.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(result, "%{byte:02X}");
        }
    }
    result
}

/// Stream the unchanged ZIP up to its central directory, followed by this suffix.
/// Existing entry attributes and signed app contents are byte-for-byte preserved.
pub fn personalize(file: &mut File, bootstrap: &Bootstrap) -> Result<(u64, Vec<u8>), String> {
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if !(22..=512 * 1024 * 1024).contains(&size) { return Err("Invalid runner ZIP size".into()); }
    file.seek(SeekFrom::End(-22)).map_err(|e| e.to_string())?;
    let end = makepad_zip_file::EndOfCentralDirectory::from_stream(file).map_err(|_| "Invalid runner ZIP directory")?;
    let prefix = end.central_directory_offset as u64;
    let central_len = end.size_of_the_central_directory as u64;
    if end.number_of_disk != 0 || end.number_of_start_central_directory_disk != 0
        || end.zip_file_comment_length != 0 || end.total_entries_all_disk != end.total_entries_this_disk
        || end.total_entries_all_disk == u16::MAX || central_len > 4 * 1024 * 1024
        || prefix + central_len + 22 != size
    { return Err("Unsupported runner ZIP directory".into()); }
    let directory = zip_read_central_directory(file).map_err(|_| "Invalid runner ZIP entries")?;
    // Keep old deployed bundles readable while rolling out the flat ZIP.
    let name = if directory.file_headers.iter().any(|h| h.file_name == "makepad-builder.exe") {
        BOOTSTRAP_FILE.to_owned()
    } else if directory.file_headers.iter().any(|h| h.file_name == "makepad-loader.exe") {
        LEGACY_BOOTSTRAP_FILE.to_owned()
    } else if directory.file_headers.iter().any(|h| h.file_name == "Makepad Loader/makepad-loader.exe") {
        format!("Makepad Loader/{LEGACY_BOOTSTRAP_FILE}")
    } else {
        return Err("Runner ZIP has no Builder executable".into());
    };
    if directory.file_headers.iter().any(|h| h.file_name == name) {
        return Err("Runner ZIP already contains a bootstrap".into());
    }
    file.seek(SeekFrom::Start(prefix)).map_err(|e| e.to_string())?;
    let mut central = vec![0; central_len as usize];
    file.read_exact(&mut central).map_err(|e| e.to_string())?;
    let mut writer = ZipWriter::new();
    writer.add(&name, &bootstrap.encode(), ZipMethod::Store).map_err(|_| "Cannot package Builder bootstrap")?;
    let mut entry = writer.finish().map_err(|_| "Cannot package Builder bootstrap")?;
    let added = zip_read_central_directory(&mut Cursor::new(&entry)).map_err(|_| "Cannot package Builder bootstrap")?;
    let entry_len = added.eocd.central_directory_offset as usize;
    let mut entry_central = entry[entry_len..entry.len()-22].to_vec();
    entry_central[42..46].copy_from_slice(&(prefix as u32).to_le_bytes());
    entry.truncate(entry_len);
    let central_offset = prefix + entry.len() as u64;
    entry.extend_from_slice(&central);
    entry.extend_from_slice(&entry_central);
    let count = end.total_entries_all_disk + 1;
    entry.extend_from_slice(&0x06054b50u32.to_le_bytes());
    entry.extend_from_slice(&0u16.to_le_bytes());
    entry.extend_from_slice(&0u16.to_le_bytes());
    entry.extend_from_slice(&count.to_le_bytes());
    entry.extend_from_slice(&count.to_le_bytes());
    entry.extend_from_slice(&((central.len() + entry_central.len()) as u32).to_le_bytes());
    entry.extend_from_slice(&(central_offset as u32).to_le_bytes());
    entry.extend_from_slice(&0u16.to_le_bytes());
    Ok((prefix, entry))
}
