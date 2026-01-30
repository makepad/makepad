//! "Did you mean?" suggestions for property lookup errors
//! 
//! This module provides fuzzy matching for property names to help users
//! identify typos when a property lookup fails.

use crate::value::*;
use crate::heap::*;
use crate::makepad_live_id::*;
use crate::pod::*;
use crate::traits::ScriptTypeObject;
use std::fmt::Write;

/// Compute Levenshtein distance between two strings
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();
    
    if a_len == 0 { return b_len; }
    if b_len == 0 { return a_len; }
    
    // Use two rows instead of full matrix for efficiency
    let mut prev_row: Vec<usize> = (0..=b_len).collect();
    let mut curr_row: Vec<usize> = vec![0; b_len + 1];
    
    for (i, ca) in a.chars().enumerate() {
        curr_row[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr_row[j + 1] = (prev_row[j + 1] + 1)       // deletion
                .min(curr_row[j] + 1)                     // insertion  
                .min(prev_row[j] + cost);                 // substitution
        }
        std::mem::swap(&mut prev_row, &mut curr_row);
    }
    prev_row[b_len]
}

/// Format a ScriptValue briefly for display in suggestions.
/// Shows type and a short preview of the value, e.g.:
/// - `#ff0000` for colors
/// - `fn123` for functions  
/// - `obj` for objects
/// - `3.14` for numbers
/// - `"hello"` for strings (truncated)
pub fn format_value_brief(heap: &ScriptHeap, value: ScriptValue) -> String {
    // Handle nil
    if value.is_nil() {
        return "nil".to_string();
    }
    
    // Handle bool
    if value.is_bool() {
        return if value == TRUE { "true" } else { "false" }.to_string();
    }
    
    // Handle numbers (f64, f32, u32, i32, u40)
    if let Some(n) = value.as_f64() {
        if n.fract() == 0.0 && n.abs() < 1e10 {
            return format!("{}", n as i64);
        }
        return format!("{:.3}", n);
    }
    if let Some(n) = value.as_u40() {
        return format!("{}", n);
    }
    
    // Handle color
    if let Some(color) = value.as_color() {
        return format!("#{:02x}{:02x}{:02x}{:02x}", 
            (color >> 24) as u8,
            (color >> 16) as u8, 
            (color >> 8) as u8,
            color as u8);
    }
    
    // Handle LiveId
    if let Some(id) = value.as_id() {
        return id.as_string(|s| s.unwrap_or("id").to_string());
    }
    
    // Handle inline strings
    if let Some(s) = value.as_inline_string(|s| s.to_string()) {
        let truncated = if s.len() > 12 { format!("{}...", &s[..12]) } else { s };
        return format!("\"{}\"", truncated);
    }
    
    // Handle heap strings
    if let Some(s) = value.as_string() {
        if let Some(str_data) = &heap.strings[s.index as usize] {
            let s = &str_data.string.0;
            let truncated = if s.len() > 12 { format!("{}...", &s[..12]) } else { s.to_string() };
            return format!("\"{}\"", truncated);
        }
        return "\"\"".to_string();
    }
    
    // Handle objects - check if it's a function
    if let Some(obj) = value.as_object() {
        if heap.is_fn(obj) {
            return format!("fn{}", obj.index);
        }
        // Regular object - show object id
        return format!("obj{}", obj.index);
    }
    
    // Handle arrays
    if let Some(arr) = value.as_array() {
        let len = heap.arrays[arr.index as usize].storage.len();
        return format!("[{}]", len);
    }
    
    // Handle pod values
    if let Some(pod) = value.as_pod() {
        let pod_data = &heap.pods[pod.index as usize];
        if let Some(name) = heap.pod_type_name(pod_data.ty) {
            return name.as_string(|s| s.unwrap_or("pod").to_string());
        }
        return "pod".to_string();
    }
    
    // Handle pod types
    if let Some(pod_ty) = value.as_pod_type() {
        if let Some(name) = heap.pod_type_name(pod_ty) {
            return format!("type:{}", name.as_string(|s| s.unwrap_or("?").to_string()));
        }
        return "type".to_string();
    }
    
    // Handle errors
    if value.is_err() {
        return "err".to_string();
    }
    
    // Fallback
    format!("{:?}", value.value_type())
}

/// Format the type of a ScriptValue as a human-readable string for error messages.
/// Unlike format_value_brief which shows the value, this shows the type.
/// Examples: "number", "string", "bool", "vec4f", "MyStruct", "object", "function", "array"
pub fn format_value_type(heap: &ScriptHeap, value: ScriptValue) -> String {
    // Handle nil
    if value.is_nil() {
        return "nil".to_string();
    }
    
    // Handle bool
    if value.is_bool() {
        return "bool".to_string();
    }
    
    // Handle numbers (f64, f32, u32, i32, u40)
    if value.as_f64().is_some() || value.as_u40().is_some() {
        return "number".to_string();
    }
    
    // Handle color
    if value.as_color().is_some() {
        return "color".to_string();
    }
    
    // Handle LiveId
    if value.as_id().is_some() {
        return "id".to_string();
    }
    
    // Handle strings
    if value.as_inline_string(|_| ()).is_some() || value.as_string().is_some() {
        return "string".to_string();
    }
    
    // Handle objects - check if it's a function
    if let Some(obj) = value.as_object() {
        if heap.is_fn(obj) {
            return "function".to_string();
        }
        // Check if it has a proto that indicates its type
        let proto = heap.proto(obj);
        if let Some(id) = proto.as_id() {
            return id.as_string(|s| s.unwrap_or("object").to_string());
        }
        return "object".to_string();
    }
    
    // Handle arrays
    if value.as_array().is_some() {
        return "array".to_string();
    }
    
    // Handle pod values - show the pod type name
    if let Some(pod) = value.as_pod() {
        let pod_data = &heap.pods[pod.index as usize];
        return format_pod_type_name(heap, pod_data.ty);
    }
    
    // Handle pod types
    if let Some(pod_ty) = value.as_pod_type() {
        return format!("type:{}", format_pod_type_name(heap, pod_ty));
    }
    
    // Handle errors
    if value.is_err() {
        return "error".to_string();
    }
    
    // Fallback
    format!("{:?}", value.value_type())
}

/// Format the expected type name from a ScriptTypeObject for error messages.
/// This is used when type checking fails and we need to show what type was expected.
pub fn format_expected_type(heap: &ScriptHeap, type_object: &ScriptTypeObject) -> String {
    // First check if we have an explicit type name set (from derive macro)
    if let Some(name) = type_object.name {
        return name.as_string(|s| s.unwrap_or("object").to_string());
    }
    // Check if proto is a pod value (e.g., Vec4f, Mat4f primitives)
    if let Some(pod) = type_object.proto.as_pod() {
        let pod_data = &heap.pods[pod.index as usize];
        return format_pod_type_name(heap, pod_data.ty);
    }
    // Check if proto is an object
    if let Some(proto_obj) = type_object.proto.as_object() {
        let obj_data = &heap.objects[proto_obj.index as usize];
        // Check if it's a pod type first - this handles Vec4f, etc.
        if let Some(pod_type) = obj_data.tag.as_pod_type() {
            return format_pod_type_name(heap, pod_type);
        }
        // Check if the object itself has a name via its proto
        let proto_val = heap.proto(proto_obj);
        if let Some(id) = proto_val.as_id() {
            return id.as_string(|s| s.unwrap_or("object").to_string());
        }
        // Check if the object has entries that indicate it's a known type
        if let Some(name_val) = obj_data.map_get(&id!(name).into()) {
            if let Some(id) = name_val.as_id() {
                return id.as_string(|s| s.unwrap_or("object").to_string());
            }
        }
    }
    // Proto is directly an id
    if let Some(id) = type_object.proto.as_id() {
        return id.as_string(|s| s.unwrap_or("object").to_string());
    }
    // Fallback
    "object".to_string()
}

/// Format a ScriptPodType as a human-readable type name for error messages.
/// Returns names like "f32", "vec2f", "vec4f", "MyStruct", etc.
pub fn format_pod_type_name(heap: &ScriptHeap, pod_ty: ScriptPodType) -> String {
    // First try to get the registered name
    if let Some(name) = heap.pod_type_name(pod_ty) {
        return name.as_string(|s| s.unwrap_or("?").to_string());
    }
    
    // Fallback: try to describe based on the type structure
    format_pod_type_from_ty(heap, pod_ty)
}

/// Format a ScriptPodType based on its type structure (without using registered name).
pub fn format_pod_type_from_ty(heap: &ScriptHeap, pod_ty: ScriptPodType) -> String {
    let pod_type = &heap.pod_types[pod_ty.index as usize];
    match &pod_type.ty {
        ScriptPodTy::F32 => "f32".to_string(),
        ScriptPodTy::F16 => "f16".to_string(),
        ScriptPodTy::U32 => "u32".to_string(),
        ScriptPodTy::I32 => "i32".to_string(),
        ScriptPodTy::Bool => "bool".to_string(),
        ScriptPodTy::Vec(vt) => {
            // Use the name() method which returns the correct LiveId
            vt.name().as_string(|s| s.unwrap_or("vec").to_string())
        }
        ScriptPodTy::Mat(mt) => {
            // Use the name() method which returns the correct LiveId
            mt.name().as_string(|s| s.unwrap_or("mat").to_string())
        }
        ScriptPodTy::Struct { .. } => {
            // Try to get name if available
            if let Some(name) = heap.pod_type_name(pod_ty) {
                return name.as_string(|s| s.unwrap_or("struct").to_string());
            }
            format!("struct#{}", pod_ty.index)
        }
        ScriptPodTy::Enum { .. } => {
            if let Some(name) = heap.pod_type_name(pod_ty) {
                return name.as_string(|s| s.unwrap_or("enum").to_string());
            }
            format!("enum#{}", pod_ty.index)
        }
        ScriptPodTy::FixedArray { .. } => format!("array#{}", pod_ty.index),
        ScriptPodTy::VariableArray { .. } => format!("vararray#{}", pod_ty.index),
        ScriptPodTy::Void => "void".to_string(),
        ScriptPodTy::AtomicU32 => "atomic_u32".to_string(),
        ScriptPodTy::AtomicI32 => "atomic_i32".to_string(),
        _ => format!("type#{}", pod_ty.index),
    }
}

/// Format a ScriptPodType using builtin constants (for use in shader builtins where we don't have heap access).
/// This compares against known builtin type indices to get the name.
pub fn format_pod_type_from_builtins(pod_ty: ScriptPodType, builtins: &crate::mod_pod::ScriptPodBuiltins) -> String {
    // Check against known builtin types
    if pod_ty == builtins.pod_void { return "void".to_string(); }
    if pod_ty == builtins.pod_f32 { return "f32".to_string(); }
    if pod_ty == builtins.pod_f16 { return "f16".to_string(); }
    if pod_ty == builtins.pod_u32 { return "u32".to_string(); }
    if pod_ty == builtins.pod_i32 { return "i32".to_string(); }
    if pod_ty == builtins.pod_vec2f { return "vec2f".to_string(); }
    if pod_ty == builtins.pod_vec3f { return "vec3f".to_string(); }
    if pod_ty == builtins.pod_vec4f { return "vec4f".to_string(); }
    if pod_ty == builtins.pod_vec2h { return "vec2h".to_string(); }
    if pod_ty == builtins.pod_vec3h { return "vec3h".to_string(); }
    if pod_ty == builtins.pod_vec4h { return "vec4h".to_string(); }
    if pod_ty == builtins.pod_vec2u { return "vec2u".to_string(); }
    if pod_ty == builtins.pod_vec3u { return "vec3u".to_string(); }
    if pod_ty == builtins.pod_vec4u { return "vec4u".to_string(); }
    if pod_ty == builtins.pod_vec2i { return "vec2i".to_string(); }
    if pod_ty == builtins.pod_vec3i { return "vec3i".to_string(); }
    if pod_ty == builtins.pod_vec4i { return "vec4i".to_string(); }
    if pod_ty == builtins.pod_mat2x2f { return "mat2x2f".to_string(); }
    if pod_ty == builtins.pod_mat3x3f { return "mat3x3f".to_string(); }
    if pod_ty == builtins.pod_mat4x4f { return "mat4x4f".to_string(); }
    if pod_ty == builtins.pod_mat2x3f { return "mat2x3f".to_string(); }
    if pod_ty == builtins.pod_mat2x4f { return "mat2x4f".to_string(); }
    if pod_ty == builtins.pod_mat3x2f { return "mat3x2f".to_string(); }
    if pod_ty == builtins.pod_mat3x4f { return "mat3x4f".to_string(); }
    if pod_ty == builtins.pod_mat4x2f { return "mat4x2f".to_string(); }
    if pod_ty == builtins.pod_mat4x3f { return "mat4x3f".to_string(); }
    
    // Unknown type - just show index
    format!("type#{}", pod_ty.index)
}

/// Maximum number of items to show in "Available:" list
const MAX_AVAILABLE_ITEMS: usize = 4;

/// Format suggestions from a list of candidate names
/// Returns a string like: `. Did you mean: foo or bar, baz (+2 more)`
pub fn suggest_from_iter<'a>(key_str: &str, candidates: impl Iterator<Item = &'a str>) -> String {
    let mut result = String::new();
    let mut scored: Vec<(&str, usize)> = candidates
        .map(|name| (name, levenshtein(key_str, name)))
        .collect();
    
    if scored.is_empty() {
        return result;
    }
    
    // Sort by distance to find best matches
    scored.sort_by_key(|(_, dist)| *dist);
    
    let total_count = scored.len();
    
    // Format: "Did you mean: first or second, third, fourth (+x more)"
    write!(result, ". Did you mean: {}", scored[0].0).ok();
    
    if total_count > 1 {
        write!(result, " or ").ok();
        for (i, (name, _)) in scored.iter().skip(1).take(MAX_AVAILABLE_ITEMS).enumerate() {
            if i > 0 { result.push_str(", "); }
            write!(result, "{}", name).ok();
        }
        if total_count > MAX_AVAILABLE_ITEMS + 1 {
            write!(result, " (+{} more)", total_count - MAX_AVAILABLE_ITEMS - 1).ok();
        }
    }
    
    result
}

/// A candidate field with its name, value preview, and distance score
struct FieldCandidate {
    name: String,
    value_preview: String,
    distance: usize,
}

/// Format suggestions from a list of candidate names with values
fn suggest_from_candidates(candidates: Vec<FieldCandidate>) -> String {
    let mut result = String::new();
    
    if candidates.is_empty() {
        return result;
    }
    
    // Sort by distance to find best matches
    let mut sorted = candidates;
    sorted.sort_by_key(|c| c.distance);
    
    let total_count = sorted.len();
    
    // Format first item
    let first = &sorted[0];
    if first.value_preview.is_empty() {
        write!(result, ". Did you mean: {}", first.name).ok();
    } else {
        write!(result, ". Did you mean: {}({})", first.name, first.value_preview).ok();
    }
    
    // Format remaining items
    if total_count > 1 {
        write!(result, " or ").ok();
        for (i, candidate) in sorted.iter().skip(1).take(MAX_AVAILABLE_ITEMS).enumerate() {
            if i > 0 { result.push_str(", "); }
            if candidate.value_preview.is_empty() {
                write!(result, "{}", candidate.name).ok();
            } else {
                write!(result, "{}({})", candidate.name, candidate.value_preview).ok();
            }
        }
        if total_count > MAX_AVAILABLE_ITEMS + 1 {
            write!(result, " (+{} more)", total_count - MAX_AVAILABLE_ITEMS - 1).ok();
        }
    }
    
    result
}

/// Format suggestions from a list of LiveId names  
pub fn suggest_from_live_ids(key: LiveId, candidates: &[LiveId]) -> String {
    let key_str = key.as_string(|s| s.unwrap_or("").to_string());
    let names: Vec<String> = candidates.iter()
        .filter_map(|id| id.as_string(|s| s.map(|s| s.to_string())))
        .collect();
    suggest_from_iter(&key_str, names.iter().map(|s| s.as_str()))
}

/// Try to get a value from an object without erroring, returns NIL if not found
fn value_or_nil(heap: &ScriptHeap, obj_ptr: ScriptObject, key: ScriptValue) -> ScriptValue {
    let mut ptr = obj_ptr;
    let mut visited = 0;
    loop {
        if visited > 100 { break; }
        visited += 1;
        
        let object = &heap.objects[ptr.index as usize];
        if let Some(value) = object.map.get(&key) {
            return value.value;
        }
        if let Some(next_ptr) = object.proto.as_object() {
            ptr = next_ptr;
        } else {
            break;
        }
    }
    NIL
}

/// Format suggestions for a missing property on an object
/// Returns a string like: `. Did you mean 'foo'? Available: bar(obj), baz(fn12:0), foo(#ff0000)`
pub fn suggest_property(heap: &ScriptHeap, obj_ptr: ScriptObject, key: ScriptValue) -> String {
    let mut candidates: Vec<FieldCandidate> = Vec::new();
    
    // Get the key as a string for comparison
    let key_str = key_to_string(heap, key);
    
    // Helper to add a candidate if not already present
    let mut add_candidate = |name: String, value: ScriptValue| {
        if !candidates.iter().any(|c| c.name == name) {
            let distance = levenshtein(&key_str, &name);
            let value_preview = format_value_brief(heap, value);
            candidates.push(FieldCandidate { name, value_preview, distance });
        }
    };
    
    // First, check the type_check for registered type properties
    let object = &heap.objects[obj_ptr.index as usize];
    if let Some(ty_index) = object.tag.as_type_index() {
        let type_check = &heap.type_check[ty_index.0 as usize];
        for (prop_id, _prop_ty) in type_check.props.iter_ordered() {
            if let Some(name) = prop_id.as_string(|s| s.map(|s| s.to_string())) {
                // Try to get the actual value from the prototype chain
                let value = value_or_nil(heap, obj_ptr, prop_id.into());
                add_candidate(name, value);
            }
        }
    }
    
    // Collect all property names from the object and its prototype chain
    let mut ptr = obj_ptr;
    let mut visited = 0;
    loop {
        if visited > 100 { break; } // Safety limit
        visited += 1;
        
        let object = &heap.objects[ptr.index as usize];
        
        // Also check type_check on prototype objects
        if let Some(ty_index) = object.tag.as_type_index() {
            let type_check = &heap.type_check[ty_index.0 as usize];
            for (prop_id, _prop_ty) in type_check.props.iter_ordered() {
                if let Some(name) = prop_id.as_string(|s| s.map(|s| s.to_string())) {
                    let value = value_or_nil(heap, ptr, prop_id.into());
                    add_candidate(name, value);
                }
            }
        }
        
        // Collect from map
        for (map_key, map_value) in object.map.iter() {
            if let Some(name) = key_to_string_opt(heap, *map_key) {
                add_candidate(name, map_value.value);
            }
        }
        
        // Collect from vec
        for kv in object.vec.iter() {
            if let Some(name) = key_to_string_opt(heap, kv.key) {
                add_candidate(name, kv.value);
            }
        }
        
        // Follow prototype chain
        if let Some(next_ptr) = object.proto.as_object() {
            ptr = next_ptr;
        } else {
            break;
        }
    }
    
    suggest_from_candidates(candidates)
}

/// Convert a ScriptValue key to a string for display
fn key_to_string(heap: &ScriptHeap, key: ScriptValue) -> String {
    key_to_string_opt(heap, key).unwrap_or_else(|| format!("{:?}", key))
}

/// Try to convert a ScriptValue key to a string, returns None if not possible
fn key_to_string_opt(heap: &ScriptHeap, key: ScriptValue) -> Option<String> {
    if let Some(id) = key.as_id() {
        return id.as_string(|s| s.map(|s| s.to_string()));
    }
    if let Some(s) = key.as_string() {
        if let Some(s) = &heap.strings[s.index as usize] {
            return Some(s.string.0.to_string());
        }
    }
    if let Some(s) = key.as_inline_string(|s| s.to_string()) {
        return Some(s);
    }
    None
}

/// Format suggestions for scope variable lookup
pub fn suggest_scope_var(heap: &ScriptHeap, obj_ptr: ScriptObject, key: LiveId) -> String {
    suggest_property(heap, obj_ptr, key.into())
}

/// Format suggestions for pod struct field lookup
pub fn suggest_pod_field(heap: &ScriptHeap, pod_ty: ScriptPodType, field: LiveId) -> String {
    let pod_type = &heap.pod_types[pod_ty.index as usize];
    match &pod_type.ty {
        ScriptPodTy::Struct { fields, .. } => {
            let field_names: Vec<LiveId> = fields.iter().map(|f| f.name).collect();
            suggest_from_live_ids(field, &field_names)
        }
        ScriptPodTy::Vec(vt) => {
            // For vectors, suggest swizzle components based on dimension
            let key_str = field.as_string(|s| s.unwrap_or("").to_string());
            let components = match vt.dims() {
                2 => vec!["x", "y"],
                3 => vec!["x", "y", "z"],
                4 => vec!["x", "y", "z", "w"],
                _ => vec![],
            };
            suggest_from_iter(&key_str, components.into_iter())
        }
        _ => String::new()
    }
}
