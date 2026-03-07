use makepad_openexr::{read_file, Box2i, Compression, LineOrder};
use std::path::{Path, PathBuf};

#[test]
fn reads_blobbies_scanline_fixture() {
    let image =
        read_file(fixture_path("ScanLines/Blobbies.exr")).expect("Blobbies.exr should decode");
    assert_eq!(image.parts.len(), 1);

    let part = &image.parts[0];
    assert_eq!(part.compression, Compression::Zip);
    assert_eq!(part.line_order, LineOrder::DecreasingY);
    assert_eq!(
        part.data_window,
        Box2i {
            min_x: -20,
            min_y: -20,
            max_x: 1019,
            max_y: 1019,
        }
    );
    assert_eq!(
        part.display_window,
        Box2i {
            min_x: 0,
            min_y: 0,
            max_x: 999,
            max_y: 999,
        }
    );

    let mut channel_names: Vec<_> = part
        .channels
        .iter()
        .map(|channel| channel.name.as_str())
        .collect();
    channel_names.sort_unstable();
    assert_eq!(channel_names, vec!["A", "B", "G", "R", "Z"]);

    let mut attr_names: Vec<_> = part
        .other_attributes
        .iter()
        .map(|attr| attr.name.as_str())
        .collect();
    attr_names.sort_unstable();
    assert!(attr_names.contains(&"capDate"));
    assert!(attr_names.contains(&"chromaticities"));
    assert!(attr_names.contains(&"owner"));
    assert!(attr_names.contains(&"preview"));
    assert!(attr_names.contains(&"utcOffset"));
    assert!(attr_names.contains(&"whiteLuminance"));
}

#[test]
fn reads_beachball_singlepart_fixture() {
    let image = read_file(fixture_path("Beachball/singlepart.0001.exr"))
        .expect("singlepart.0001.exr should decode");
    assert_eq!(image.parts.len(), 1);

    let part = &image.parts[0];
    assert_eq!(part.name.as_deref(), Some("rgba"));
    assert_eq!(part.compression, Compression::Zips);
    assert_eq!(part.line_order, LineOrder::IncreasingY);
    assert_eq!(
        part.data_window,
        Box2i {
            min_x: 654,
            min_y: 245,
            max_x: 1564,
            max_y: 1120,
        }
    );
    assert_eq!(
        part.multi_view,
        vec!["right".to_string(), "left".to_string()]
    );
    assert_eq!(part.channels.len(), 20);

    let mut channel_names: Vec<_> = part
        .channels
        .iter()
        .map(|channel| channel.name.as_str())
        .collect();
    channel_names.sort_unstable();
    assert!(channel_names.contains(&"R"));
    assert!(channel_names.contains(&"left.R"));
    assert!(channel_names.contains(&"left.Z"));
    assert!(channel_names.contains(&"forward.left.u"));
    assert!(channel_names.contains(&"forward.right.v"));
    assert!(channel_names.contains(&"whitebarmask.left.mask"));
    assert!(channel_names.contains(&"whitebarmask.right.mask"));
}

#[test]
fn reads_beachball_multipart_fixture() {
    let image = read_file(fixture_path("Beachball/multipart.0001.exr"))
        .expect("multipart.0001.exr should decode");
    assert_eq!(image.parts.len(), 10);

    let expected = [
        (
            "rgba_right",
            Some("right"),
            876,
            Box2i {
                min_x: 654,
                min_y: 245,
                max_x: 1530,
                max_y: 1120,
            },
            vec!["A", "B", "G", "R"],
        ),
        (
            "depth_left",
            Some("left"),
            876,
            Box2i {
                min_x: 688,
                min_y: 245,
                max_x: 1564,
                max_y: 1120,
            },
            vec!["Z"],
        ),
        (
            "forward_left",
            Some("left"),
            876,
            Box2i {
                min_x: 688,
                min_y: 245,
                max_x: 1564,
                max_y: 1120,
            },
            vec!["forward.u", "forward.v"],
        ),
        (
            "whitebarmask_left",
            Some("left"),
            769,
            Box2i {
                min_x: 1106,
                min_y: 245,
                max_x: 1490,
                max_y: 1013,
            },
            vec!["whitebarmask.mask"],
        ),
        (
            "rgba_left",
            Some("left"),
            876,
            Box2i {
                min_x: 688,
                min_y: 245,
                max_x: 1564,
                max_y: 1120,
            },
            vec!["A", "B", "G", "R"],
        ),
        (
            "depth_right",
            Some("right"),
            876,
            Box2i {
                min_x: 654,
                min_y: 245,
                max_x: 1530,
                max_y: 1120,
            },
            vec!["Z"],
        ),
        (
            "forward_right",
            Some("right"),
            876,
            Box2i {
                min_x: 654,
                min_y: 245,
                max_x: 1530,
                max_y: 1120,
            },
            vec!["forward.u", "forward.v"],
        ),
        (
            "disparityL",
            None,
            876,
            Box2i {
                min_x: 654,
                min_y: 245,
                max_x: 1564,
                max_y: 1120,
            },
            vec!["disparityL.x", "disparityL.y"],
        ),
        (
            "disparityR",
            None,
            876,
            Box2i {
                min_x: 654,
                min_y: 245,
                max_x: 1564,
                max_y: 1120,
            },
            vec!["disparityR.x", "disparityR.y"],
        ),
        (
            "whitebarmask_right",
            Some("right"),
            769,
            Box2i {
                min_x: 1070,
                min_y: 245,
                max_x: 1455,
                max_y: 1013,
            },
            vec!["whitebarmask.mask"],
        ),
    ];

    for (part, (name, view, chunk_count, data_window, channels)) in
        image.parts.iter().zip(expected.iter())
    {
        assert_eq!(part.name.as_deref(), Some(*name));
        assert_eq!(part.view.as_deref(), *view);
        assert_eq!(part.compression, Compression::Zips);
        assert_eq!(part.line_order, LineOrder::IncreasingY);
        assert_eq!(part.chunk_count, Some(*chunk_count));
        assert_eq!(part.data_window, *data_window);
        let mut channel_names: Vec<_> = part
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect();
        channel_names.sort_unstable();
        let mut expected_channels = channels.clone();
        expected_channels.sort_unstable();
        assert_eq!(channel_names, expected_channels);
    }
}

fn fixture_path(rel_path: &str) -> PathBuf {
    let root = std::env::var_os("MAKEPAD_OPENEXR_TESTDATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/makepad-openexr-test-images"));
    let path = root.join(rel_path);
    if Path::new(&path).exists() {
        path
    } else {
        panic!(
            "missing OpenEXR fixture at {}. Run libs/openexr/scripts/download_test_images.sh first",
            path.display()
        );
    }
}
