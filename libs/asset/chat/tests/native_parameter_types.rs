//! XML parameter bodies have no string delimiters. Decode them using the
//! declared top-level property type, without coercing structured JSON.

use makepad_asset_chat::toolcall::{extract, Extract};
use makepad_asset_chat::tools::{encode_args, ContentToolCall};
use makepad_asset_client::json::{self, Value};

fn extracted(text: &str) -> (String, Value) {
    match extract(text) {
        Extract::Call { name, args, .. } => (name, args),
        other => panic!("expected a tool call, got {other:?}"),
    }
}

fn texture_xml(name: &str, seed: &str) -> String {
    format!(
        r#"<tool_call>
<function={name}>
<parameter=document>car</parameter>
<parameter=request_id>paint-1</parameter>
<parameter=expected>{{"generation":"9007199254740993","content":"{}"}}</parameter>
<parameter=material>1</parameter>
<parameter=layer>aged-paint</parameter>
<parameter=prompt>worn green automotive paint</parameter>
<parameter=model>flux1-schnell</parameter>
<parameter=seed>{seed}</parameter>
<parameter=width>256</parameter>
<parameter=height>512</parameter>
<parameter=channels>["base_color","roughness","normal"]</parameter>
<parameter=seamless>true</parameter>
<parameter=derive_maps>false</parameter>
</function>
</tool_call>"#,
        "a".repeat(64)
    )
}

#[test]
fn texture_xml_preserves_exact_seed_and_all_other_argument_types() {
    // First is the observed failed Astra call. The larger values expose
    // both IEEE-754 rounding and signed-integer overflow regressions.
    for seed in ["20260906", "0", "9007199254740993", "18446744073709551615"] {
        for spelling in ["model_texture", "model.texture"] {
            for encoded in [seed.to_string(), json::s(seed).to_json()] {
                let (name, args) = extracted(&texture_xml(spelling, &encoded));
                assert_eq!(name, "model.texture");
                assert_eq!(args.get("seed").and_then(Value::as_str), Some(seed));
                assert_eq!(args.get("material"), Some(&Value::Int(1)));
                assert_eq!(args.get("width"), Some(&Value::Int(256)));
                assert_eq!(args.get("height"), Some(&Value::Int(512)));
                assert_eq!(args.get("seamless"), Some(&Value::Bool(true)));
                assert_eq!(args.get("derive_maps"), Some(&Value::Bool(false)));
                assert_eq!(
                    args.get("channels").and_then(Value::as_arr).unwrap().len(),
                    3
                );
                assert_eq!(
                    args.get("expected")
                        .unwrap()
                        .get("generation")
                        .and_then(Value::as_str),
                    Some("9007199254740993")
                );
                let call = ContentToolCall::parse(&name, &args).expect("first call must validate");
                assert_eq!(
                    encode_args(&call),
                    args,
                    "typed roundtrip must remain exact"
                );
            }
        }
    }
}

#[test]
fn optional_concept_seed_uses_the_same_declared_string_type() {
    let (name, args) = extracted(
        r#"<tool_call><function=model_concepts>
<parameter=request_id>old-cars</parameter>
<parameter=prompts>["1930s saloon", "1950s pickup", "1970s wagon"]</parameter>
<parameter=seed>18446744073709551615</parameter>
</function></tool_call>"#,
    );
    assert_eq!(
        args.get("seed").and_then(Value::as_str),
        Some("18446744073709551615")
    );
    ContentToolCall::parse(&name, &args).unwrap();
}

#[test]
fn declared_strings_preserve_literal_text_and_decode_quoted_json_strings() {
    for literal in [
        "20260906",
        "true",
        "false",
        "null",
        "{}",
        "[1,2]",
        "  42  ",
        "{\n  value: 42\n}",
        "quoted \"word\" and \\ backslash",
    ] {
        for encoded in [literal.to_string(), json::s(literal).to_json()] {
            let text = format!(
                "<tool_call><function=world_set_source>\n<parameter=source>\n{encoded}\n</parameter>\n</function></tool_call>"
            );
            let (name, args) = extracted(&text);
            assert_eq!(args.get("source").and_then(Value::as_str), Some(literal));
            let ContentToolCall::WorldSetSource { source, .. } =
                ContentToolCall::parse(&name, &args).unwrap()
            else {
                panic!("wrong tool");
            };
            assert_eq!(source, literal);
        }
    }
    // JSON strings retain intentional whitespace, Unicode and newlines,
    // including content that starts and ends with literal quote marks.
    let literal = "\"雪\"\nline two\n";
    let text = format!(
        "<tool_call><function=world_set_source><parameter=source>{}</parameter></function></tool_call>",
        json::s(literal).to_json()
    );
    assert_eq!(
        extracted(&text).1.get("source").and_then(Value::as_str),
        Some(literal)
    );
}

#[test]
fn invalid_bare_seeds_are_preserved_for_honest_validation_not_normalized() {
    for seed in [
        "01",
        "-1",
        "1.0",
        "2e7",
        "18446744073709551616",
        "true",
        "null",
        " 17 ",
    ] {
        let (name, args) = extracted(&texture_xml("model_texture", seed));
        assert_eq!(args.get("seed").and_then(Value::as_str), Some(seed));
        assert!(
            ContentToolCall::parse(&name, &args).is_err(),
            "invalid seed {seed}"
        );
    }
}

#[test]
fn json_arguments_keep_their_types_and_numeric_seeds_still_refuse() {
    let (_, valid) = extracted(&texture_xml("model_texture", "17"));
    let valid_json = valid.to_json();
    for numeric in [
        "20260906",
        "9007199254740993",
        "18446744073709551615",
        "17.0",
        "1.7e1",
    ] {
        let args_json = valid_json.replace("\"seed\":\"17\"", &format!("\"seed\":{numeric}"));
        assert_ne!(args_json, valid_json);
        // Same strict typed boundary used for structured native function
        // calls. No schema or JSON-value coercion was introduced there.
        let parsed = json::parse(args_json.as_bytes());
        if let Ok(args) = &parsed {
            assert!(args.get("seed").unwrap().as_str().is_none());
            assert!(ContentToolCall::parse("model.texture", args).is_err());
        } else {
            // The JSON codec rejects integers above i64::MAX itself.
            assert_eq!(numeric, "18446744073709551615");
        }
        for text in [
            format!("<<tool>>{{\"name\":\"model.texture\",\"args\":{args_json}}}"),
            format!("<tool_call>{{\"function\":\"model_texture\",\"arguments\":{args_json}}}</tool_call>"),
            format!("<tool_call><function=model_texture>{args_json}</function></tool_call>"),
        ] {
            match extract(&text) {
                Extract::Call { name, args, .. } => {
                    if let Ok(expected) = &parsed {
                        assert_eq!(&args, expected);
                    }
                    assert!(ContentToolCall::parse(&name, &args).is_err());
                }
                Extract::Malformed { .. } => assert!(parsed.is_err()),
                other => panic!("numeric seed must refuse, got {other:?}"),
            }
        }
    }
}

#[test]
fn nested_json_wrong_types_and_unknown_parameters_still_refuse() {
    let valid = texture_xml("model_texture", "17");
    for invalid in [
        valid.replace("<parameter=width>256", "<parameter=width>\"256\""),
        valid.replace("<parameter=seamless>true", "<parameter=seamless>\"true\""),
        valid.replace(
            "\"generation\":\"9007199254740993\"",
            "\"generation\":9007199254740993",
        ),
        valid.replace(
            "</function>",
            "<parameter=surprise>17</parameter></function>",
        ),
        valid.replace("function=model_texture", "function=unknown_texture"),
    ] {
        let (name, args) = extracted(&invalid);
        assert!(ContentToolCall::parse(&name, &args).is_err());
    }
}
