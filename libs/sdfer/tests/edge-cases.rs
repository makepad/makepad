use sdfer::{esdt, Image2d, Unorm8};

#[test]
fn simple_3x3() {
    fn sdf_of(inputs: [f32; 9]) -> [f32; 9] {
        let pad = 1;
        let (sdf, _) = esdt::glyph_to_sdf(
            &mut Image2d::from_storage(3, 3, inputs.map(Unorm8::encode)),
            esdt::Params {
                pad,
                ..Default::default()
            },
            None,
        );
        [0, 1, 2, 3, 4, 5, 6, 7, 8].map(|i| sdf[(pad + i % 3, pad + i / 3)].decode())
    }

    // FIXME(eddyb) add more test cases.
    assert_eq!(
        // HACK(eddyb) trailing comments keeps `rustfmt` helpful.
        sdf_of([
            0.00, 0.85, 0.00, //
            0.85, 0.90, 0.85, //
            0.00, 0.85, 0.00, //
        ]),
        [
            0.58431375, 0.8980392, 0.58431375, //
            0.89803920, 1.0000000, 0.89803920, //
            0.58431375, 0.8980392, 0.58431375, //
        ]
    );
}
