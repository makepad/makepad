//! Script-level `str.decode_base64()` / `bytes.to_base64()`: sandboxed Splash apps turn
//! host-supplied base64 into U8 bytes for `Image.load_image_from_data_async` in one native call.

use makepad_script::*;

/// The cap `splash_host_respond` gives each host callback (`WIDGET_SCRIPT_INSTRUCTION_LIMIT`).
const CALLBACK_INSTRUCTION_CAP: usize = 200_000;

/// Bytes 0..=255, encoded by Python's `base64.b64encode`.
const ALL_BYTES_B64: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn+AgYKDhIWGh4iJiouMjY6PkJGSk5SVlpeYmZqbnJ2en6ChoqOkpaanqKmqq6ytrq+wsbKztLW2t7i5uru8vb6/wMHCw8TFxsfIycrLzM3Oz9DR0tPU1dbX2Nna29zd3t/g4eLj5OXm5+jp6uvs7e7v8PHy8/T19vf4+fr7/P3+/w==";

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host,
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn script(name: &str, code: &str) -> ScriptMod {
    ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: format!("base64_{name}"),
        line: 0,
        column: 0,
        code: code.to_string(),
        values: vec![],
    }
}

/// Evaluates `code` under the callback cap, returning its value and every script error.
fn eval(vm: &mut ScriptVm, name: &str, code: &str) -> (ScriptValue, Vec<String>) {
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(CALLBACK_INSTRUCTION_CAP, |vm| {
        vm.eval(script(name, code))
    });
    (value, vm.take_errors())
}

/// Runs `code` to its end, failing on any script error, including a failed `assert`.
fn run_asserts(vm: &mut ScriptVm, name: &str, code: &str) {
    let (value, errors) = eval(vm, name, &format!("use mod.std.assert\n{code}\n\"done\""));
    assert!(errors.is_empty(), "{name}: {errors:#?}");
    assert_eq!(string(vm, value).as_deref(), Some("done"), "{name} stopped early");
}

fn string(vm: &mut ScriptVm, value: ScriptValue) -> Option<String> {
    vm.string_with(value, |_vm, s| s.to_string())
}

/// The bytes of a U8-storage array, the only storage `load_image_from_data_async` accepts.
fn u8_bytes(vm: &ScriptVm, value: ScriptValue) -> Vec<u8> {
    let array = value.as_array().expect("expected an array");
    match vm.bx.heap.array_storage(array) {
        ScriptArrayStorage::U8(bytes) => bytes.clone(),
        _ => panic!("expected U8 array storage"),
    }
}

/// A host callback that decodes its payload, returning the bytes or "invalid".
fn media_callback(vm: &mut ScriptVm) -> ScriptValue {
    let code = "\
fn on_media(b64) {
    let bytes = b64.decode_base64()
    if bytes == nil { return \"invalid\" }
    bytes
}
on_media";
    let (callback, errors) = eval(vm, "callback", code);
    assert!(errors.is_empty(), "{errors:#?}");
    callback
}

/// Calls `callback(payload)` under the same cap as `splash_host_respond`.
fn deliver(
    vm: &mut ScriptVm,
    callback: ScriptValue,
    payload: &str,
) -> (ScriptValue, usize, Vec<String>) {
    let payload = vm.bx.heap.new_string_from_str(payload);
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(CALLBACK_INSTRUCTION_CAP, |vm| {
        vm.call(callback, &[payload])
    });
    (value, vm.last_limit_consumed(), vm.take_errors())
}

/// Exposes `payload` as `mod.media.payload()`, created outside any allocation budget.
/// Pinned static: GC can't see the closure that holds it.
fn register_payload(vm: &mut ScriptVm, payload: &str) {
    let payload = vm.bx.heap.new_string_from_str(payload);
    vm.bx.heap.set_static(payload);
    let media = vm.new_module(id!(media));
    vm.add_method(media, id_lut!(payload), script_args_def!(), move |_, _| payload);
}

/// Evaluates `code` under a heap allocation budget of `limit` bytes.
fn run_budgeted(
    vm: &mut ScriptVm,
    limit: usize,
    code: &str,
) -> (ScriptValue, ScriptAllocationReport, Vec<String>) {
    vm.bx.captured_errors = Some(Vec::new());
    let (value, report) =
        vm.with_heap_allocation_limit(limit, |vm| vm.eval(script("budget", code)));
    (value, report, vm.take_errors())
}

fn pseudo_random_bytes(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 32) as u8
        })
        .collect()
}

/// Bit-stream encoder, written independently of the crate's chunked one.
fn reference_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        acc = acc << 8 | byte as u32;
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            out.push(ALPHABET[(acc >> bits & 63) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[(acc << (6 - bits) & 63) as usize] as char);
    }
    while !out.len().is_multiple_of(4) {
        out.push('=');
    }
    out
}

#[test]
fn rfc4648_vectors_encode_padded_and_decode_either_way() {
    let mut vm = test_vm();
    run_asserts(
        &mut vm,
        "rfc4648",
        r#"
assert("".decode_base64().to_string() == "")
assert("Zg==".decode_base64().to_string() == "f")
assert("Zm8=".decode_base64().to_string() == "fo")
assert("Zm9v".decode_base64().to_string() == "foo")
assert("Zm9vYg==".decode_base64().to_string() == "foob")
assert("Zm9vYmE=".decode_base64().to_string() == "fooba")
assert("Zm9vYmFy".decode_base64().to_string() == "foobar")
assert("Zg".decode_base64().to_string() == "f")
assert("Zm8".decode_base64().to_string() == "fo")
assert("Zm9vYg".decode_base64().to_string() == "foob")
assert("Zm9vYmE".decode_base64().to_string() == "fooba")
assert("".to_bytes().to_base64() == "")
assert("f".to_bytes().to_base64() == "Zg==")
assert("fo".to_bytes().to_base64() == "Zm8=")
assert("foo".to_bytes().to_base64() == "Zm9v")
assert("foob".to_bytes().to_base64() == "Zm9vYg==")
assert("fooba".to_bytes().to_base64() == "Zm9vYmE=")
assert("foobar".to_bytes().to_base64() == "Zm9vYmFy")"#,
    );
}

#[test]
fn every_byte_value_round_trips() {
    let mut vm = test_vm();
    register_payload(&mut vm, ALL_BYTES_B64);
    vm.gc();
    let (value, errors) = eval(&mut vm, "all_bytes", "mod.media.payload().decode_base64()");
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(u8_bytes(&vm, value), (0..=255).collect::<Vec<u8>>());

    // Sweeps that decoded array, so the next decode can reuse its slot.
    vm.gc();
    let code = "mod.media.payload().decode_base64().to_base64()";
    let (value, errors) = eval(&mut vm, "all_bytes_round_trip", code);
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(string(&mut vm, value).as_deref(), Some(ALL_BYTES_B64));
}

#[test]
fn ascii_whitespace_is_skipped_but_other_spaces_are_invalid() {
    let mut vm = test_vm();
    run_asserts(
        &mut vm,
        "whitespace",
        r#"
assert(" Zm 9v\nYmFy\r\n".decode_base64().to_string() == "foobar")
assert("Zm9v\tYg\x0c=\n=".decode_base64().to_string() == "foob")
assert(" \n ".decode_base64().len() == 0)
assert("Zm9v\x0bYg".decode_base64() == nil)
assert("Zm9v Yg".decode_base64() == nil)"#,
    );
}

#[test]
fn invalid_input_returns_nil_without_aborting_the_callback() {
    let mut vm = test_vm();
    run_asserts(
        &mut vm,
        "invalid",
        r#"
let invalid = ["Z" "Zm9vY" "=" "====" "Zg=" "Zg===" "Zm8==" "Zm9v=" "Zg==Zg==" "Zm=9v" "Zm-v" "Zm_v" "Zm9v!" "Zm9v\0" "Zm9vé" "data:text/plain;base64,aGk="]
let checked = 0
for s in invalid {
    assert(s.decode_base64() == nil)
    checked += 1
}
assert(checked == 16)"#,
    );

    let callback = media_callback(&mut vm);
    for payload in ["Zg=", "Zm-v", "not base64!"] {
        let (value, _, errors) = deliver(&mut vm, callback, payload);
        assert!(errors.is_empty(), "{payload:?} raised {errors:#?}");
        assert_eq!(string(&mut vm, value).as_deref(), Some("invalid"), "{payload:?}");
    }
}

#[test]
fn decoded_bytes_have_u8_storage() {
    let mut vm = test_vm();
    let (value, errors) = eval(&mut vm, "storage", r#""++++////AAEC".decode_base64()"#);
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(
        u8_bytes(&vm, value),
        [0xFB, 0xEF, 0xBE, 0xFF, 0xFF, 0xFF, 0x00, 0x01, 0x02]
    );

    // Empty input is valid: a zero-length U8 array, not nil.
    let (value, errors) = eval(&mut vm, "storage_empty", r#""".decode_base64()"#);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(u8_bytes(&vm, value).is_empty());

    // Array parse_json only accepts U8 storage, so this checks it from script.
    run_asserts(
        &mut vm,
        "storage_script",
        r#"
let bytes = "eyJhIjoxfQ==".decode_base64()
assert(bytes.len() == 7)
assert(bytes[0] == 123)
assert(bytes.parse_json().a == 1)"#,
    );
}

#[test]
fn megabyte_payload_decodes_in_one_native_call() {
    let bytes = pseudo_random_bytes(768 * 1024);
    let payload = reference_encode(&bytes);
    assert_eq!(payload.len(), 1024 * 1024);

    let mut vm = test_vm();
    let callback = media_callback(&mut vm);
    let (value, consumed, errors) = deliver(&mut vm, callback, &payload);
    assert!(errors.is_empty(), "{errors:#?}");
    // A per-byte script loop would need millions of instructions, far past the cap.
    assert!(consumed < 50, "decoding took {consumed} instructions");
    assert!(u8_bytes(&vm, value) == bytes, "decoded bytes differ");

    // MIME-style 76-column line wrapping decodes to the same bytes.
    let wrapped: Vec<&str> = payload
        .as_bytes()
        .chunks(76)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    let (value, _, errors) = deliver(&mut vm, callback, &wrapped.join("\r\n"));
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(u8_bytes(&vm, value) == bytes, "wrapped payload decoded differently");

    register_payload(&mut vm, &payload);
    let code = "mod.media.payload().decode_base64().to_base64()";
    let (value, errors) = eval(&mut vm, "megabyte_round_trip", code);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(vm.last_limit_consumed() < 50);
    assert!(string(&mut vm, value).as_deref() == Some(payload.as_str()), "re-encoding differs");
}

#[test]
fn decoding_and_encoding_are_charged_to_the_allocation_budget() {
    let bytes = pseudo_random_bytes(300_000);
    let payload = reference_encode(&bytes);
    let mut vm = test_vm();
    register_payload(&mut vm, &payload);
    let decode = "mod.media.payload().decode_base64()";
    let round_trip = "mod.media.payload().decode_base64().to_base64()";

    // Too small for the 300,000 decoded bytes: refused before the buffer exists.
    let (_, report, errors) = run_budgeted(&mut vm, 64 * 1024, decode);
    assert!(report.exceeded, "{report:?}");
    assert!(errors.iter().any(|e| e.contains("decoding base64")), "{errors:#?}");

    // Room for the decode but not for the 400,000-character re-encode.
    let (_, report, errors) = run_budgeted(&mut vm, 512 * 1024, round_trip);
    assert!(report.exceeded, "{report:?}");
    assert!(errors.iter().any(|e| e.contains("encoding base64")), "{errors:#?}");
    assert!(!errors.iter().any(|e| e.contains("decoding base64")), "{errors:#?}");

    let (value, report, errors) = run_budgeted(&mut vm, 2 * 1024 * 1024, decode);
    assert!(!report.exceeded && errors.is_empty(), "{report:?} {errors:#?}");
    assert!(report.allocated_bytes >= bytes.len(), "{report:?}");
    assert!(u8_bytes(&vm, value) == bytes, "decoded bytes differ");
}

#[test]
fn to_base64_requires_u8_storage() {
    let mut vm = test_vm();
    for array in ["[104 105]", "\"hi\".to_chars()"] {
        let code = format!("let encoded = {array}.to_base64()\n\"continued\"");
        let (value, errors) = eval(&mut vm, "non_u8", &code);
        assert!(
            errors.iter().any(|e| e.contains("to_base64 requires U8 byte array")),
            "{array}: {errors:#?}"
        );
        assert_eq!(string(&mut vm, value).as_deref(), Some("continued"));
    }
}
