//! A debugging aid: `TANDEM_DUMP=<file.fxp>` prints the patch's
//! parameters that differ from the init patch, its routings and its
//! migrated revision (skipped without the variable).

use makepad_audio_synth::tandem::catalog::catalog;
use makepad_audio_synth::tandem::{LoadMode, Patch};

#[test]
fn dump_patch() {
    let Some(path) = std::env::var_os("TANDEM_DUMP") else { return };
    let bytes = std::fs::read(&path).expect("read");
    let p = Patch::from_fxp(&bytes, "dump", LoadMode::Patch).expect("load");
    let init = Patch::init();
    eprintln!("revision {} -> {}; warnings {:?}", p.loaded_revision, p.revision, p.warnings);
    for (i, info) in catalog().params.iter().enumerate() {
        if p.values[i] != init.values[i] {
            eprintln!("{} = {:?}", info.name, p.values[i]);
        }
    }
    for r in &p.routings {
        eprintln!("route {} <- src {} idx {} depth {}", catalog().params[r.dest as usize].name, r.source, r.source_index, r.depth);
    }
}
