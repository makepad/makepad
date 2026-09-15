//! Disk-space treemap: a streaming scan of a folder's bytes and the widget
//! that draws them as a 2D / isometric / perspective map.

pub use makepad_widgets;
use makepad_widgets::*;

pub mod backend;
pub mod disk_map;
pub mod kind;
pub mod palette;
pub mod sizecache;
pub mod treemap;

pub use backend::{backend, install_backend, NativeScan, ScanBackend};
pub use disk_map::{
    DiskMap, DiskMapAction, DiskMapRef, DiskMapWidgetExt, DiskMapWidgetRefExt, MapCamera,
    MapProjection,
};
pub use kind::{
    display_name, extension_of, home_dir, home_scan_exclusion, is_image_file, is_playable_video,
    is_thumbnailable, kind_class, kind_for, makepad_home, scan_all, scan_exclusions, set_scan_all,
    skip_for_scan, FileKind, IMAGE_EXTS, PLAYABLE_VIDEO_EXTS,
};
pub use palette::MapPalette;
pub use sizecache::Cached;
pub use treemap::{Cell, Node, Query, ScanProgress, ScanRules, ScanStep};

pub fn script_mod(vm: &mut ScriptVm) {
    disk_map::script_mod(vm);
}

#[cfg(test)]
mod smoke {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::AtomicBool,
        time::{SystemTime, UNIX_EPOCH},
    };

    use makepad_widgets::makepad_platform::thread::TaskPool;
    use makepad_widgets::Cx;

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "makepad-diskmap-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn with_pool<R: Send>(f: impl FnOnce(&TaskPool) -> R + Send) -> R {
        let cx = Cx::new(Box::new(|_, _| {}));
        let pool = cx.task_pool();
        std::thread::scope(|scope| scope.spawn(|| f(&pool)).join().unwrap())
    }

    /// `DiskMap::set_root` scans through [`NativeScan`]; this drives that same
    /// scan against a temp folder of three files and checks the sizes it
    /// reports. The widget constructor needs a live pointer, so the scan the
    /// widget runs is the thing under test.
    #[test]
    fn diskmap_set_root_on_a_temp_dir_with_three_files_reports_their_sizes() {
        let dir = unique_dir("set-root");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.bin"), vec![0u8; 100]).unwrap();
        fs::write(dir.join("b.bin"), vec![0u8; 250]).unwrap();
        fs::write(dir.join("c.bin"), vec![0u8; 400]).unwrap();

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let reported = DiskMap::scan_root_for_test(&mut cx, &dir);

        assert_eq!(reported.files, 3, "status={reported:?}");
        assert_eq!(reported.size, 750, "status={reported:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_scan_stream_matches_the_three_file_fixture() {
        let dir = unique_dir("stream");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.bin"), vec![0u8; 100]).unwrap();
        fs::write(dir.join("b.bin"), vec![0u8; 250]).unwrap();
        fs::write(dir.join("c.bin"), vec![0u8; 400]).unwrap();

        let cancel = AtomicBool::new(false);
        let steps = std::sync::Mutex::new(Vec::new());
        assert!(with_pool(|pool| {
            NativeScan.scan_stream(
                &dir,
                &cancel,
                &|step| {
                    steps.lock().unwrap().push(step);
                },
                pool,
            )
        }));
        let mut tree = Node::dir(display_name(&dir), FileKind::Folder as u8);
        for step in steps.into_inner().unwrap() {
            tree.apply(step);
        }
        tree.seal();
        assert_eq!(tree.files, 3);
        assert_eq!(tree.size, 750);

        let _ = fs::remove_dir_all(&dir);
    }
}
