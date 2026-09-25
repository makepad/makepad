use crate::icns::{decode_icns, encode_argb, parse_icns};
use crate::ico::{decode_dib, encode_dib, parse_group_icon, parse_ico, write_ico};
use crate::image::Image;
use crate::tree::LANG_EN_US;
use crate::version::parse_version_resource;
use crate::*;

fn scope_icns() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/makepad_builder/resources/scope.icns");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A disc with a transparent outside and a colour gradient inside.
fn test_image(size: u32) -> Image {
    let mut rgba = Vec::new();
    let c = (size as f32 - 1.0) / 2.0;
    for y in 0..size {
        for x in 0..size {
            let inside = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt() <= c;
            rgba.extend_from_slice(&if inside { [(x * 255 / size) as u8, (y * 255 / size) as u8, 40, 255] } else { [0, 0, 0, 0] });
        }
    }
    Image::new(size, size, rgba).unwrap()
}

fn icns_file(entries: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (ostype, data) in entries {
        body.extend_from_slice(*ostype);
        body.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
        body.extend_from_slice(data);
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

#[test]
fn parses_the_scope_icns() {
    let bytes = scope_icns();
    let types: Vec<String> = parse_icns(&bytes).unwrap().iter().map(|e| String::from_utf8_lossy(&e.ostype).into_owned()).collect();
    for t in ["ic04", "ic05", "ic07", "ic08", "ic10", "ic11", "ic12"] {
        assert!(types.iter().any(|x| x == t), "{t} missing from {types:?}");
    }
    let images = decode_icns(&bytes).unwrap();
    let mut sizes: Vec<u32> = images.iter().map(|s| s.image.width).collect();
    sizes.sort();
    sizes.dedup();
    assert_eq!(sizes, [16, 32, 64, 128, 256, 512, 1024]);
    // The ARGB 16 px entry is not empty: iconutil's RLE decoded to pixels.
    let small = images.iter().find(|s| s.image.width == 16).unwrap();
    assert!(small.png.is_none());
    assert!(small.image.rgba.chunks_exact(4).any(|p| p[3] == 255));
}

#[test]
fn argb_rle_round_trips() {
    let image = test_image(16);
    let bytes = icns_file(&[(b"ic04", encode_argb(&image)), (b"info", vec![1, 2, 3])]);
    let decoded = decode_icns(&bytes).unwrap();
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].image, image);
}

#[test]
fn rejects_truncated_icns() {
    let mut bytes = icns_file(&[(b"ic07", test_image(8).encode_png().unwrap())]);
    bytes.truncate(bytes.len() - 3);
    assert!(parse_icns(&bytes).is_err());
}

#[test]
fn png_and_dib_round_trip() {
    let image = test_image(24);
    assert_eq!(Image::decode_png(&image.encode_png().unwrap()).unwrap(), image);
    assert_eq!(decode_dib(&encode_dib(&image)).unwrap(), image);
}

#[test]
fn resample_keeps_flat_colour_and_does_not_bleed() {
    let flat = Image::new(64, 64, [200u8, 100, 50, 255].repeat(64 * 64)).unwrap();
    for size in [48, 24, 17] {
        assert!(flat.resample(size, size).rgba.chunks_exact(4).all(|p| p == [200, 100, 50, 255]));
    }
    // Half transparent black, half opaque red: the edge keeps pure red colour.
    let mut rgba = Vec::new();
    for _ in 0..4 { rgba.extend_from_slice(&[[0, 0, 0, 0], [0, 0, 0, 0], [255, 0, 0, 255], [255, 0, 0, 255]].concat()); }
    let half = Image::new(4, 4, rgba).unwrap().resample(3, 3);
    let middle = &half.rgba[4..8];
    assert_eq!(&middle[..3], &[255, 0, 0]);
    assert!(middle[3] > 100 && middle[3] < 160, "{middle:?}");
}

#[test]
fn icon_images_cover_the_standard_sizes() {
    let sources = load_source(&scope_icns()).unwrap();
    let images = icon_images(&sources).unwrap();
    let sizes: Vec<u32> = images.iter().map(|i| i.width).collect();
    assert_eq!(sizes, ico::ICON_SIZES);
    for image in &images {
        assert_eq!(image.is_png(), image.width == 256, "{}", image.width);
        let decoded = image.decode().unwrap();
        assert_eq!((decoded.width, decoded.height), (image.width, image.height));
    }
    // 256 comes from the icns PNG as-is.
    let png256 = sources.iter().find(|s| s.image.width == 256).unwrap().png.clone().unwrap();
    assert_eq!(images.last().unwrap().data, png256);

    // A single small PNG gives only the sizes it can fill.
    let small = load_source(&test_image(40).encode_png().unwrap()).unwrap();
    let sizes: Vec<u32> = icon_images(&small).unwrap().iter().map(|i| i.width).collect();
    assert_eq!(sizes, [16, 24, 32]);
    let tiny = load_source(&test_image(8).encode_png().unwrap()).unwrap();
    assert_eq!(icon_images(&tiny).unwrap()[0].width, 8);
}

#[test]
fn ico_round_trips_and_is_a_source() {
    let images = icon_images(&load_source(&test_image(300).encode_png().unwrap()).unwrap()).unwrap();
    let file = write_ico(&images);
    assert_eq!(parse_ico(&file).unwrap(), images);
    let again = icon_images(&load_source(&file).unwrap()).unwrap();
    assert_eq!(again.iter().map(|i| i.width).collect::<Vec<_>>(), ico::ICON_SIZES);
    assert!(load_source(b"GIF89a").is_err());
}

#[test]
fn resource_object_round_trips() {
    let info = VersionInfo {
        file_description: "Scope".into(),
        product_name: "Makepad Scope".into(),
        original_filename: "scope.exe".into(),
        internal_name: "scope".into(),
        version_text: "1.2.3".into(),
        version: VersionInfo::parse_version("1.2.3-beta"),
        ..Default::default()
    };
    let icns = scope_icns();
    let resources = app_resources(Some(&icns), Some(&info)).unwrap();
    for machine in [Machine::X64, Machine::X86, Machine::Arm64] {
        let object = app_link_input(LinkFormat::Coff(machine), Some(&icns), Some(&info)).unwrap();
        let (read_machine, mut read) = read_object(&object).unwrap();
        assert_eq!(read_machine, machine);
        // The directory is ordered by type, then name, then language.
        let mut expected = resources.clone();
        expected.sort_by(|a, b| (&a.kind, &a.name, a.lang).cmp(&(&b.kind, &b.name, b.lang)));
        assert_eq!(read.len(), expected.len());
        read.iter_mut().zip(&expected).for_each(|(r, e)| assert_eq!(r, e));
    }
    // The .res form keeps the resources in writing order.
    let res = app_link_input(LinkFormat::Res, Some(&icns), Some(&info)).unwrap();
    assert_eq!(read_res(&res).unwrap(), resources);
    assert_eq!(&res[..32], &[0, 0, 0, 0, 32, 0, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

    let group = resources.iter().find(|r| r.kind == ResId::Id(RT_GROUP_ICON)).unwrap();
    assert_eq!((group.name.clone(), group.lang), (ResId::Id(APP_ICON_ID), LANG_EN_US));
    let entries = parse_group_icon(&group.data).unwrap();
    assert_eq!(entries.iter().map(|e| e.width).collect::<Vec<_>>(), ico::ICON_SIZES);
    for entry in &entries {
        let icon = resources.iter().find(|r| r.kind == ResId::Id(RT_ICON) && r.name == ResId::Id(entry.id)).unwrap();
        assert_eq!(icon.data.len() as u32, entry.bytes);
        assert_eq!(entry.bit_count, 32);
    }

    let version = resources.iter().find(|r| r.kind == ResId::Id(RT_VERSION)).unwrap();
    let (numbers, strings) = parse_version_resource(&version.data).unwrap();
    assert_eq!(numbers, [1, 2, 3, 0]);
    assert!(strings.contains(&("FileDescription".into(), "Scope".into())));
    assert!(strings.contains(&("OriginalFilename".into(), "scope.exe".into())));
    assert!(!strings.iter().any(|(k, _)| k == "CompanyName"));
}

#[test]
fn object_layout_is_well_formed() {
    let resources = vec![
        Resource::new(RT_MANIFEST, 1, b"<assembly/>".to_vec()),
        Resource::new(RT_ICON, 2, vec![1; 5]),
        Resource::new(RT_ICON, 1, vec![2; 9]),
    ];
    let object = write_object(Machine::X64, &resources).unwrap();
    let u32_at = |at: usize| u32::from_le_bytes(object[at..at + 4].try_into().unwrap());
    let u16_at = |at: usize| u16::from_le_bytes(object[at..at + 2].try_into().unwrap());
    assert_eq!(u16_at(0), 0x8664);
    assert_eq!(u16_at(2), 2);
    assert_eq!(&object[20..28], b".rsrc$01");
    assert_eq!(&object[60..68], b".rsrc$02");
    // Every relocation is ADDR32NB against a $R symbol in .rsrc$02.
    let (relocs, count) = (u32_at(20 + 24) as usize, u16_at(20 + 32) as usize);
    assert_eq!(count, 3);
    let symbols = u32_at(8) as usize;
    for r in 0..count {
        let at = relocs + r * 10;
        assert_eq!(u16_at(at + 8), 3);
        let sym = symbols + u32_at(at + 4) as usize * 18;
        assert_eq!(&object[sym..sym + 2], b"$R");
        assert_eq!(u16_at(sym + 12), 2);
        assert_eq!(u32_at(sym + 8) % 8, 0);
    }
    // Root table: two ID entries in ascending order, both subdirectories.
    let dir = 100;
    assert_eq!(u16_at(dir + 14), 2);
    assert_eq!((u32_at(dir + 16), u32_at(dir + 24)), (RT_ICON as u32, RT_MANIFEST as u32));
    assert!(u32_at(dir + 20) & 0x8000_0000 != 0);
    assert!(write_object(Machine::X64, &[Resource::new(RT_ICON, 1, vec![]), Resource::new(RT_ICON, 1, vec![])]).is_err());
}

#[test]
fn res_keeps_named_resources_and_link_format_follows_the_target() {
    let resources = vec![
        Resource { kind: ResId::Id(RT_MANIFEST), name: ResId::Id(1), lang: 0, data: b"<assembly/>".to_vec() },
        Resource { kind: ResId::Name("CUSTOM".into()), name: ResId::Name("THING".into()), lang: LANG_EN_US, data: vec![7; 3] },
    ];
    assert_eq!(read_res(&write_res(&resources)).unwrap(), resources);
    assert_eq!(LinkFormat::for_target("windows", "msvc", "x86_64").unwrap(), Some(LinkFormat::Res));
    assert_eq!(LinkFormat::for_target("windows", "gnu", "x86_64").unwrap(), Some(LinkFormat::Coff(Machine::X64)));
    assert_eq!(LinkFormat::for_target("macos", "", "aarch64").unwrap(), None);
    assert!(LinkFormat::for_target("windows", "gnu", "mips").is_err());
}
