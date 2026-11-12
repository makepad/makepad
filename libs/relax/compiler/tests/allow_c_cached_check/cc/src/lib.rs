//! `cc::Build::new().file(..).compile(..)` with the system C compiler (`$CC`, else `cc`) and `ar`:
//! object files and a static library in OUT_DIR, and the lines that link it, as the cc crate
//! prints them.
use std::process::Command;

#[derive(Default)]
pub struct Build {
    files: Vec<String>,
}

impl Build {
    pub fn new() -> Build {
        Build::default()
    }

    pub fn file(&mut self, path: &str) -> &mut Build {
        self.files.push(path.to_string());
        self
    }

    pub fn compile(&mut self, name: &str) {
        let out = std::env::var("OUT_DIR").expect("OUT_DIR");
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
        let mut objects = Vec::new();
        for (i, file) in self.files.iter().enumerate() {
            let object = format!("{out}/{name}-{i}.o");
            let ok = Command::new(&cc).args(["-O2", "-fPIC", "-c", file, "-o", &object]).status().expect("cc").success();
            assert!(ok, "cc failed on {file}");
            objects.push(object);
        }
        let lib = format!("{out}/lib{name}.a");
        let _ = std::fs::remove_file(&lib);
        let ok = Command::new("ar").arg("crs").arg(&lib).args(&objects).status().expect("ar").success();
        assert!(ok, "ar failed");
        println!("cargo:rustc-link-search=native={out}");
        println!("cargo:rustc-link-lib=static={name}");
    }
}
