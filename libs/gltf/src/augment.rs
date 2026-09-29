//! Shared machinery for lossless GLB augmentation.
//!
//! Animation and skinning both need to append accessors without rebuilding
//! the source mesh. Keeping the JSON/BIN rewrite here gives both paths the
//! same preservation, validation, deterministic serialization and chunk
//! handling contract.

use crate::{parse_glb_bytes, GlbChunk, GltfDocument, GltfError};
use makepad_micro_serde::{DeJson, JsonValue};
use std::collections::HashMap;

pub(crate) fn validation(message: impl Into<String>) -> GltfError {
    GltfError::Validation(message.into())
}

pub(crate) fn number(value: usize) -> JsonValue {
    JsonValue::U64(value as u64)
}

pub(crate) fn string(value: impl Into<String>) -> JsonValue {
    JsonValue::String(value.into())
}

pub(crate) fn object(
    fields: impl IntoIterator<Item = (&'static str, JsonValue)>,
) -> JsonValue {
    JsonValue::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

fn json_escape(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < '\u{20}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Deterministic JSON serialization. `JsonValue` stores object fields in a
/// randomized `HashMap`; stable key ordering makes generated GLBs reproducible
/// across processes and platforms.
fn write_json(value: &JsonValue, out: &mut String) -> Result<(), GltfError> {
    match value {
        JsonValue::String(value) => json_escape(value, out),
        JsonValue::Char(value) => json_escape(&value.to_string(), out),
        JsonValue::U64(value) => out.push_str(&value.to_string()),
        JsonValue::U128(value) => out.push_str(&value.to_string()),
        JsonValue::I64(value) => out.push_str(&value.to_string()),
        JsonValue::I128(value) => out.push_str(&value.to_string()),
        JsonValue::F64(value) if value.is_finite() => out.push_str(&value.to_string()),
        JsonValue::F64(_) => return Err(validation("glTF JSON contains a non-finite number")),
        JsonValue::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        JsonValue::Null => out.push_str("null"),
        JsonValue::Array(values) => {
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                write_json(value, out)?;
            }
            out.push(']');
        }
        JsonValue::Object(fields) => {
            out.push('{');
            let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
            keys.sort_unstable();
            for (index, key) in keys.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                json_escape(key, out);
                out.push(':');
                write_json(&fields[*key], out)?;
            }
            out.push('}');
        }
        JsonValue::BareIdent(_) | JsonValue::Undefined => {
            return Err(validation("glTF JSON contains a non-JSON value"));
        }
    }
    Ok(())
}

/// Parsed, mutable representation of a self-contained GLB rewrite. The
/// original BIN bytes remain the exact prefix of `bin` throughout the edit.
pub(crate) struct GlbRewrite {
    pub(crate) document: GltfDocument,
    root: HashMap<String, JsonValue>,
    pub(crate) bin: Vec<u8>,
    extra_chunks: Vec<GlbChunk>,
}

impl GlbRewrite {
    pub(crate) fn begin(input: &[u8]) -> Result<Self, GltfError> {
        let parsed = parse_glb_bytes(input)?;
        if parsed.document.buffers_slice().len() != 1
            || parsed.document.buffers_slice()[0].uri.is_some()
        {
            return Err(GltfError::Unsupported(
                "GLB augmentation requires one embedded buffer".to_string(),
            ));
        }
        let json_len = u32::from_le_bytes(input[12..16].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&input[20..20 + json_len])?
            .trim_end_matches(['\0', ' ', '\n', '\r', '\t']);
        let root = JsonValue::deserialize_json_lenient(json).map_err(GltfError::Json)?;
        let JsonValue::Object(root) = root else {
            return Err(validation("glTF JSON root is not an object"));
        };
        Ok(Self {
            document: parsed.document,
            root,
            bin: parsed.bin_chunk.unwrap_or_default(),
            extra_chunks: parsed.extra_chunks,
        })
    }

    pub(crate) fn take_array(&mut self, name: &str) -> Result<Vec<JsonValue>, GltfError> {
        match self.root.remove(name) {
            Some(JsonValue::Array(values)) => Ok(values),
            Some(_) => Err(validation(format!("glTF '{name}' is not an array"))),
            None => Ok(Vec::new()),
        }
    }

    pub(crate) fn put_array(&mut self, name: &str, values: Vec<JsonValue>) {
        self.root
            .insert(name.to_string(), JsonValue::Array(values));
    }

    pub(crate) fn remove(&mut self, name: &str) {
        self.root.remove(name);
    }

    pub(crate) fn array_mut(&mut self, name: &str) -> Result<&mut Vec<JsonValue>, GltfError> {
        let value = self
            .root
            .entry(name.to_string())
            .or_insert_with(|| JsonValue::Array(Vec::new()));
        match value {
            JsonValue::Array(items) => Ok(items),
            _ => Err(validation(format!("glTF '{name}' is not an array"))),
        }
    }

    pub(crate) fn insert(&mut self, name: &str, value: JsonValue) {
        self.root.insert(name.to_string(), value);
    }

    /// Append one tightly-packed accessor and its buffer view.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_accessor(
        &mut self,
        views: &mut Vec<JsonValue>,
        accessors: &mut Vec<JsonValue>,
        bytes: &[u8],
        component_type: usize,
        count: usize,
        accessor_type: &'static str,
        normalized: bool,
        target: Option<usize>,
        min: Option<JsonValue>,
        max: Option<JsonValue>,
    ) -> usize {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        let byte_offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let view_index = views.len();
        let mut view = HashMap::from([
            ("buffer".to_string(), number(0)),
            ("byteOffset".to_string(), number(byte_offset)),
            ("byteLength".to_string(), number(bytes.len())),
        ]);
        if let Some(target) = target {
            view.insert("target".to_string(), number(target));
        }
        views.push(JsonValue::Object(view));

        let mut accessor = HashMap::from([
            ("bufferView".to_string(), number(view_index)),
            ("componentType".to_string(), number(component_type)),
            ("count".to_string(), number(count)),
            ("type".to_string(), string(accessor_type)),
        ]);
        if normalized {
            accessor.insert("normalized".to_string(), JsonValue::Bool(true));
        }
        if let Some(min) = min {
            accessor.insert("min".to_string(), min);
        }
        if let Some(max) = max {
            accessor.insert("max".to_string(), max);
        }
        accessors.push(JsonValue::Object(accessor));
        accessors.len() - 1
    }

    pub(crate) fn finish(mut self) -> Result<Vec<u8>, GltfError> {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        let bin_len = self.bin.len();
        let buffers = self.array_mut("buffers")?;
        let Some(JsonValue::Object(buffer_zero)) = buffers.first_mut() else {
            return Err(validation("glTF buffer 0 is missing"));
        };
        buffer_zero.insert("byteLength".to_string(), number(bin_len));

        let mut json = String::new();
        write_json(&JsonValue::Object(self.root), &mut json)?;
        let mut json_bytes = json.into_bytes();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        let extra_len: usize = self
            .extra_chunks
            .iter()
            .map(|chunk| 8 + chunk.data.len())
            .sum();
        let total = 12 + 8 + json_bytes.len() + 8 + self.bin.len() + extra_len;
        let mut output = Vec::with_capacity(total);
        output.extend_from_slice(b"glTF");
        output.extend_from_slice(&2u32.to_le_bytes());
        output.extend_from_slice(&(total as u32).to_le_bytes());
        output.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        output.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        output.extend_from_slice(&json_bytes);
        output.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
        output.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        output.extend_from_slice(&self.bin);
        for chunk in self.extra_chunks {
            output.extend_from_slice(&(chunk.data.len() as u32).to_le_bytes());
            output.extend_from_slice(&chunk.chunk_type.to_le_bytes());
            output.extend_from_slice(&chunk.data);
        }
        // Run the normal structural/index validator over our own output.
        parse_glb_bytes(&output)?;
        Ok(output)
    }
}

/// Rewrite a GLB's embedded image payloads: `replace(image_index, bytes)`
/// returns the new payload (for example a small reference to a texture stored
/// elsewhere). The BIN chunk is rebuilt compactly: every buffer view keeps its
/// place in order, image views shrink or grow, the rest are copied as they were.
/// Images without a buffer view are left alone.
pub fn rewrite_glb_images(input: &[u8], mut replace: impl FnMut(usize, &[u8]) -> Vec<u8>) -> Result<Vec<u8>, GltfError> {
    rewrite_images(input, &mut |index, bytes| Some(replace(index, bytes)), None)
}

/// Replace embedded images with KTX2 (Basis Universal) payloads, the glTF
/// `KHR_texture_basisu` way: `encode(image_index, bytes)` returns the KTX2 or
/// `None` to keep the image as it is. A replaced image gets mimeType
/// `image/ktx2`, each texture using it the extension (its `source` stays, so
/// readers that follow `source` read the KTX2 bytes), and the extension is
/// listed as used and required.
pub fn embed_ktx2_images(input: &[u8], mut encode: impl FnMut(usize, &[u8]) -> Option<Vec<u8>>) -> Result<Vec<u8>, GltfError> {
    rewrite_images(input, &mut encode, Some("image/ktx2"))
}

fn rewrite_images(input: &[u8], replace: &mut dyn FnMut(usize, &[u8]) -> Option<Vec<u8>>, mime: Option<&str>) -> Result<Vec<u8>, GltfError> {
    let mut glb = GlbRewrite::begin(input)?;
    let mut images = glb.take_array("images")?;
    let mut views = glb.take_array("bufferViews")?;
    let index_of = |value: Option<&JsonValue>| match value { Some(JsonValue::U64(v)) => Some(*v as usize), Some(JsonValue::I64(v)) => Some(*v as usize), Some(JsonValue::F64(v)) => Some(*v as usize), _ => None };
    // Which buffer view holds which image.
    let mut image_of_view = HashMap::new();
    for (i, image) in images.iter().enumerate() {
        if let Some(v) = index_of(image.key("bufferView")) { image_of_view.insert(v, i); }
    }
    let old_bin = std::mem::take(&mut glb.bin);
    let field = |view: &JsonValue, key: &str| -> usize { index_of(view.key(key)).unwrap_or(0) };
    let mut bin = Vec::with_capacity(old_bin.len());
    let mut replaced = Vec::new();
    for (index, view) in views.iter_mut().enumerate() {
        let (offset, length) = (field(view, "byteOffset"), field(view, "byteLength"));
        let bytes = old_bin.get(offset..offset + length).ok_or_else(|| validation("buffer view outside the BIN chunk"))?;
        let payload = match image_of_view.get(&index).and_then(|&image| replace(image, bytes).map(|p| (image, p))) {
            Some((image, payload)) => { replaced.push(image); payload }
            None => bytes.to_vec(),
        };
        // Views keep their original stride alignment (accessor data needs 4).
        while bin.len() % 4 != 0 { bin.push(0); }
        let JsonValue::Object(view) = view else { return Err(validation("buffer view is not an object")) };
        view.insert("byteOffset".to_string(), number(bin.len()));
        view.insert("byteLength".to_string(), number(payload.len()));
        bin.extend_from_slice(&payload);
    }
    glb.bin = bin;
    glb.put_array("bufferViews", views);
    if let (Some(mime), false) = (mime, replaced.is_empty()) {
        for &image in &replaced {
            if let Some(JsonValue::Object(image)) = images.get_mut(image) { image.insert("mimeType".to_string(), string(mime)); }
        }
        const EXT: &str = "KHR_texture_basisu";
        for texture in glb.array_mut("textures")? {
            let JsonValue::Object(texture) = texture else { continue };
            let Some(source) = index_of(texture.get("source")).filter(|s| replaced.contains(s)) else { continue };
            let extensions = texture.entry("extensions".to_string()).or_insert_with(|| JsonValue::Object(HashMap::new()));
            if let JsonValue::Object(extensions) = extensions { extensions.insert(EXT.to_string(), object([("source", number(source))])); }
        }
        for list in ["extensionsUsed", "extensionsRequired"] {
            let list = glb.array_mut(list)?;
            if !list.iter().any(|e| matches!(e, JsonValue::String(s) if s == EXT)) { list.push(string(EXT)); }
        }
    }
    glb.put_array("images", images);
    glb.finish()
}

