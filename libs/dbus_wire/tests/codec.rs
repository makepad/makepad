use makepad_dbus_wire::*;

fn array(sig: &str, values: Vec<Value>) -> Value {
    Value::Array {
        element: sig.into(),
        values,
    }
}

#[test]
fn every_type_and_nested_containers_round_trip_at_each_alignment() {
    let values = vec![
        Value::Byte(255),
        Value::Bool(true),
        Value::Bool(false),
        Value::I16(i16::MIN),
        Value::U16(u16::MAX),
        Value::I32(i32::MIN),
        Value::U32(u32::MAX),
        Value::I64(i64::MIN),
        Value::U64(u64::MAX),
        Value::F64(-0.125),
        Value::Fd(0),
        Value::Str("héllo".into()),
        Value::Path("/a/b_2".into()),
        Value::Sig("a{sv}(ot)".into()),
        array(
            "(yu)",
            vec![Value::Struct(vec![Value::Byte(7), Value::U32(8)])],
        ),
        array(
            "{sv}",
            vec![Value::DictEntry(Box::new((
                Value::Str("key".into()),
                Value::Variant(Box::new(array(
                    "as",
                    vec![array("s", vec![]), array("s", vec![Value::Str("x".into())])],
                ))),
            )))],
        ),
        Value::Variant(Box::new(Value::Variant(Box::new(Value::U64(99))))),
        array("x", vec![]),
        array("{sv}", vec![]),
        Value::Struct(vec![array("(uy)", vec![]), Value::Byte(1)]),
    ];
    for offset in 0..8 {
        let mut expected = vec![Value::Byte(42); offset];
        expected.extend(values.clone());
        let body = Body::from_values(&expected).unwrap();
        assert_eq!(body.values().unwrap(), expected);
        let mut reader = body.reader();
        reader.skip(body.signature()).unwrap();
        reader.finish().unwrap();
    }
}

#[test]
fn generated_style_typed_codec_matches_dynamic_and_borrows() {
    let mut writer = Writer::new();
    writer.byte(9);
    let array = writer.begin_array("(su)").unwrap();
    writer.begin_struct();
    writer.str("entry").unwrap();
    writer.u32(42);
    writer.end_array(array).unwrap();
    writer
        .variant("a{sv}", |writer| {
            let dict = writer.begin_array("{sv}")?;
            writer.begin_dict_entry();
            writer.str("enabled")?;
            writer.variant("b", |writer| {
                writer.bool(true);
                Ok(())
            })?;
            writer.end_array(dict)
        })
        .unwrap();
    let body = Body::from_writer("ya(su)v", writer).unwrap();
    let dynamic = Body::from_values(&[
        Value::Byte(9),
        Value::Array {
            element: "(su)".into(),
            values: vec![Value::Struct(vec![
                Value::Str("entry".into()),
                Value::U32(42),
            ])],
        },
        Value::Variant(Box::new(Value::Array {
            element: "{sv}".into(),
            values: vec![Value::DictEntry(Box::new((
                Value::Str("enabled".into()),
                Value::Variant(Box::new(Value::Bool(true))),
            )))],
        })),
    ])
    .unwrap();
    assert_eq!(body, dynamic);
    let mut reader = body.reader();
    assert_eq!(reader.byte().unwrap(), 9);
    let mut array = reader.array("(su)").unwrap();
    array.begin_struct().unwrap();
    let text = array.str().unwrap();
    assert_eq!(text, "entry");
    assert!(text.as_ptr() >= body.as_bytes().as_ptr());
    assert_eq!(array.u32().unwrap(), 42);
    array.finish().unwrap();
    assert_eq!(reader.variant_signature().unwrap(), "a{sv}");
    let mut dict = reader.array("{sv}").unwrap();
    dict.begin_dict_entry().unwrap();
    assert_eq!(dict.str().unwrap(), "enabled");
    assert_eq!(dict.variant_signature().unwrap(), "b");
    assert!(dict.bool().unwrap());
    dict.finish().unwrap();
    reader.finish().unwrap();
}

#[test]
fn empty_array_padding_and_lengths_match_wire_layout() {
    let mut writer = Writer::new();
    writer.byte(1);
    let a = writer.begin_array("x").unwrap();
    writer.end_array(a).unwrap();
    writer.byte(2);
    let a = writer.begin_array("x").unwrap();
    writer.i64(3);
    writer.end_array(a).unwrap();
    assert_eq!(
        writer.as_bytes(),
        &[1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 8, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0]
    );
    let mut writer = Writer::new();
    let a = writer.begin_array("x").unwrap();
    writer.end_array(a).unwrap();
    assert_eq!(writer.as_bytes(), &[0; 8]);
    Body::from_writer("ax", writer).unwrap();
}

#[test]
fn signatures_enforce_container_grammar_and_protocol_depth() {
    for good in ["", "ybnqiuxtdsogvh", "a{sv}", "(a(so)v)", "aa{ya{sv}}"] {
        Signature::new(good).unwrap();
    }
    for bad in [
        "a", "()", "(i", "i)", "{sv}", "a{vs}", "a{ssu}", "a{}", "r", "e", "é", "a{(i)s}",
    ] {
        assert!(Signature::new(bad).is_err(), "{bad}");
    }
    assert_eq!(
        Signature::new("ua{sv}(so)")
            .unwrap()
            .types()
            .collect::<Vec<_>>(),
        ["u", "a{sv}", "(so)"]
    );
    assert!(Signature::single("").is_err());
    assert!(Signature::single("ss").is_err());
    Signature::single(&format!("{}u", "a".repeat(32))).unwrap();
    assert!(Signature::single(&format!("{}u", "a".repeat(33))).is_err());
    Signature::new(&"s".repeat(255)).unwrap();
    assert!(Signature::new(&"s".repeat(256)).is_err());
    for path in ["/", "/a_1/B"] {
        validate_object_path(path).unwrap();
    }
    for path in ["", "a", "/a/", "//a", "/a-b", "/é"] {
        assert!(validate_object_path(path).is_err());
    }
}

#[test]
fn malformed_values_and_truncation_return_errors() {
    for (sig, bytes) in [
        ("b", vec![2, 0, 0, 0]),
        ("s", vec![1, 0, 0, 0, b'a', 1]),
        ("s", vec![2, 0, 0, 0, b'a', 0, 0]),
        ("s", vec![1, 0, 0, 0, 255, 0]),
        ("o", vec![1, 0, 0, 0, b'a', 0]),
        ("v", vec![2, b'u', b'u', 0, 0, 0, 0, 0]),
        ("au", vec![1, 0, 0, 0, 1, 0, 0, 0]),
        ("ax", vec![0, 0, 0, 0, 0, 0, 0, 1]),
        ("y", vec![0, 0]),
        ("yu", vec![0, 1, 0, 0, 0, 0, 0, 0]),
    ] {
        assert!(
            Body::from_bytes(sig, bytes, Endian::Little).is_err(),
            "{sig}"
        );
    }
    let body = Body::from_values(&[array(
        "(suv)",
        vec![Value::Struct(vec![
            Value::Str("abc".into()),
            Value::U32(8),
            Value::Variant(Box::new(Value::I64(-1))),
        ])],
    )])
    .unwrap();
    for n in 0..body.as_bytes().len() {
        assert!(Body::from_bytes(
            body.signature(),
            body.as_bytes()[..n].to_vec(),
            Endian::Little
        )
        .is_err());
    }
    assert!(Body::from_values(&[array("u", vec![Value::Str("bad".into())])]).is_err());
    let mut w = Writer::new();
    assert!(w.str("a\0b").is_err());
    let mut w = Writer::new();
    w.begin_array("u").unwrap();
    assert!(w.into_bytes().is_err());
    let mut deep = vec![1, b'v', 0].repeat(65);
    deep.extend_from_slice(&[1, b'y', 0, 1]);
    assert!(Body::from_bytes("v", deep, Endian::Little).is_err());
}

#[test]
fn hand_built_big_endian_message_and_primitives() {
    // Method return: body 'u', reply serial 9, serial 7, no generated bytes.
    let bytes = [
        b'B', 2, 0, 1, 0, 0, 0, 4, 0, 0, 0, 7, 0, 0, 0, 15, 5, 1, b'u', 0, 0, 0, 0, 9, 8, 1, b'g',
        0, 1, b'u', 0, 0, 0x12, 0x34, 0x56, 0x78,
    ];
    let message = Message::decode(&bytes).unwrap();
    assert_eq!(message.header.serial, 7);
    assert_eq!(message.header.reply_serial, Some(9));
    assert_eq!(message.body.values().unwrap(), [Value::U32(0x12345678)]);
    let encoded = message.encode().unwrap();
    assert_eq!(encoded[0], b'l');
    assert_eq!(
        Message::decode(&encoded).unwrap().body.values().unwrap(),
        message.body.values().unwrap()
    );
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(-2i16).to_be_bytes());
    bytes.extend_from_slice(&65534u16.to_be_bytes());
    bytes.extend_from_slice(&(-3i32).to_be_bytes());
    bytes.extend_from_slice(&(-4i64).to_be_bytes());
    bytes.extend_from_slice(&u64::MAX.to_be_bytes());
    bytes.extend_from_slice(&0.5f64.to_be_bytes());
    assert_eq!(
        Body::from_bytes("nqixtd", bytes, Endian::Big)
            .unwrap()
            .values()
            .unwrap(),
        [
            Value::I16(-2),
            Value::U16(65534),
            Value::I32(-3),
            Value::I64(-4),
            Value::U64(u64::MAX),
            Value::F64(0.5)
        ]
    );
}

#[test]
fn header_and_all_message_kinds_round_trip() {
    let mut call = Message::method_call(
        "org.example.Service",
        "/object",
        "org.example.Interface",
        "Echo",
        Body::from_values(&[Value::Str("hello".into())]).unwrap(),
    );
    call.header.serial = 11;
    call.header.sender = Some(":1.42".into());
    call.header.flags = 2;
    call.header
        .unknown
        .push((42, array("u", vec![Value::U32(1), Value::U32(2)])));
    let mut ret = Message::method_return(&call, Body::default());
    ret.header.serial = 12;
    let mut err = Message::error(&call, "org.example.Error", "detail").unwrap();
    err.header.serial = 13;
    let mut signal = Message::signal(
        "/object",
        "org.example.Interface",
        "Changed",
        Body::default(),
    );
    signal.header.serial = 14;
    for message in [call, ret, err, signal] {
        let bytes = message.encode().unwrap();
        assert_eq!(Message::decode(&bytes).unwrap(), message);
        for n in 0..bytes.len() {
            assert!(Message::decode(&bytes[..n]).is_err());
        }
    }
}

#[test]
fn invalid_headers_are_rejected_without_panics() {
    let mut message = Message::method_call(
        "org.example.Service",
        "/object",
        "org.example.Interface",
        "Echo",
        Body::default(),
    );
    assert!(message.encode().is_err());
    message.header.serial = 1;
    let good = message.encode().unwrap();
    for (at, byte) in [(0, b'?'), (1, 0), (3, 2), (8, 0), (16, 0), (18, b'u')] {
        let mut bytes = good.clone();
        bytes[at] = byte;
        assert!(Message::decode(&bytes).is_err(), "byte {at}");
    }
    let mut bytes = good.clone();
    bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Message::decode(&bytes).is_err());
    message.header.member = Some("bad.member".into());
    assert!(message.encode().is_err());
}
