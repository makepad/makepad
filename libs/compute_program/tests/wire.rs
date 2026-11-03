//! The output codecs and the session wire: every codec gives back the bytes, every frame the
//! values, and a bad frame is an error, never a panic.
use makepad_compute_program::codec::{decode, encode, Codec, BLOCK};
use makepad_compute_program::wire;
use makepad_compute_program::*;

const CODECS: [Codec; 5] = [Codec::Raw, Codec::Lz4, Codec::ShuffleLz4, Codec::DeltaLz4, Codec::DeltaDeflate];

/// A smooth field and some noise, `count` elements of `width` bytes.
fn field(count: usize, width: usize) -> Vec<u8> {
    let mut seed = 0x9e37_79b9u32;
    let mut out = Vec::with_capacity(count * width);
    for i in 0..count {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let v = match width {
            4 => ((i as f32 * 0.001).sin() * 100.0 + (seed % 3) as f32 * 0.0001).to_bits(),
            2 => (i as u32 / 7 + seed % 2) & 0xffff,
            _ => (i as u32 / 300 + seed % 2) & 0xff,
        };
        out.extend_from_slice(&v.to_le_bytes()[..width]);
    }
    out
}

#[test]
fn every_codec_gives_back_the_bytes_on_any_core_count() {
    // Sizes around the block edge, every element width.
    for width in [1, 2, 4] {
        for count in [0, 1, 7, BLOCK - 1, BLOCK, BLOCK + 1, 3 * BLOCK + 5] {
            let bytes = field(count, width);
            for codec in CODECS {
                let one = encode(&bytes, width, codec, 1);
                assert_eq!(encode(&bytes, width, codec, 8), one, "{codec:?} {width} {count}: the bytes do not depend on the cores");
                let mut back = vec![0u8; bytes.len()];
                decode(&one, width, codec, &mut back, 8).unwrap_or_else(|e| panic!("{codec:?} {width} {count}: {e}"));
                assert!(back == bytes, "{codec:?} {width} {count}");
            }
        }
    }
    // Delta coding pays where plain LZ4 finds no repeats: a ramp.
    let ramp: Vec<u8> = (0..4 * BLOCK as u32).flat_map(|i| (i * 3).to_le_bytes()).collect();
    assert!(encode(&ramp, 4, Codec::DeltaLz4, 4).len() * 8 < encode(&ramp, 4, Codec::Lz4, 4).len());
}

#[test]
fn a_damaged_payload_is_an_error() {
    let bytes = field(2 * BLOCK + 3, 4);
    for codec in [Codec::Lz4, Codec::ShuffleLz4, Codec::DeltaLz4, Codec::DeltaDeflate] {
        let coded = encode(&bytes, 4, codec, 2);
        let mut back = vec![0u8; bytes.len()];
        for cut in [0, 3, coded.len() / 2, coded.len() - 1] {
            assert!(decode(&coded[..cut], 4, codec, &mut back, 2).is_err(), "{codec:?} cut at {cut}");
        }
        let mut short = vec![0u8; bytes.len() - 4];
        assert!(decode(&coded, 4, codec, &mut short, 2).is_err(), "{codec:?} into too few bytes");
    }
}

#[test]
fn frames_carry_their_values() {
    let value = Value::object(&[
        ("view", Value::Array(vec![Value::Number(-2.0), Value::Number(1e-300), Value::Number(f64::MAX)])),
        ("name", Value::String("grow ✓".into())),
        ("on", Value::Bool(true)),
        ("none", Value::Nil),
    ]);
    let open = wire::Open {
        program: Program::new("p.splash", "{run: fn(s, p) { return 1 }}"),
        params: value.clone(),
        limits: Limits { fuel: 12345, ..Limits::default() },
        seed: 7,
        codec: Codec::DeltaLz4,
        class: Class::Background,
        inputs: vec![Input::f32("field", &[1.0, 2.5, -3.0]), Input::words("view", ElemTy::F64, vec![1, 2, 3, 4].into())],
    };
    let back = wire::decode_open(&wire::encode_open(&open, 2), 1 << 20, 2).unwrap();
    assert_eq!(back.program.source, open.program.source);
    assert_eq!(back.params, value);
    assert_eq!(back.limits.fuel, 12345);
    assert_eq!(back.seed, 7);
    assert_eq!(back.class, Class::Background);
    assert_eq!(back.inputs.len(), 2);
    assert_eq!(back.inputs[0].data.to_vec(), [1.0f32, 2.5, -3.0].map(f32::to_bits).to_vec());
    assert_eq!(back.inputs[1].ty, ElemTy::F64);
    // The node's input budget holds.
    assert!(wire::decode_open(&wire::encode_open(&open, 2), 6, 2).is_err());

    assert_eq!(wire::decode_msg(&wire::encode_msg("view", &value, true)).unwrap(), ("view".to_string(), value.clone(), true));
    assert_eq!(wire::decode_ack(&wire::encode_ack("tile", false, 9)).unwrap(), ("tile".to_string(), false, 9));

    let updates = Updates {
        outputs: vec![
            Output { name: "tile".into(), state: false, version: 3, rev: 2, ty: ElemTy::U32, data: OutputData::Words(vec![1, 2, 3, u32::MAX].into()) },
            Output { name: "half".into(), state: true, version: 1, rev: 2, ty: ElemTy::F32, data: OutputData::F16(vec![0x4100, 0].into()) },
            Output { name: "density".into(), state: false, version: 1, rev: 0, ty: ElemTy::F32, data: OutputData::U8 { lo: 0.0, hi: 1.0, levels: vec![0, 128, 255].into() } },
        ],
        values: vec![("iteration".into(), value.clone())],
        progress: Some((0.5, "solving".into())),
        end: Some(Err(ProgramError::OverBudget("work"))),
    };
    for codec in CODECS {
        let back = wire::decode_update(&wire::encode_update(&updates, codec, 2), 1 << 20, 2).unwrap();
        assert_eq!(format!("{back:?}"), format!("{updates:?}"), "{codec:?}");
    }
    // Garbage is an error.
    let coded = wire::encode_update(&updates, Codec::DeltaLz4, 1);
    for cut in [1, coded.len() / 3, coded.len() - 1] {
        assert!(wire::decode_update(&coded[..cut], 1 << 20, 1).is_err(), "cut at {cut}");
    }
    assert!(wire::decode_update(&coded, 2, 1).is_err(), "an output over the element budget");
}
