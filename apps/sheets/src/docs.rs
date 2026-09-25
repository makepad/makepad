//! The document source/sink boundary for native and bundled-demo builds.

#[cfg(any(not(target_arch = "wasm32"), feature = "demo", test))]
use crate::sheet;
use crate::sheet::{Sheet, Workbook};

pub trait SheetDocs {
    fn initial(&self) -> Workbook;
    fn demos(&self) -> &[DemoDoc];
    fn load(&self, key: &str) -> Result<Sheet, String>;
    fn save(&self, key: &str, sheet: &Sheet) -> Result<(), String>;
    fn can_save(&self) -> bool;
    /// Whether something already exists at `key`, so Save can ask before
    /// replacing a file that is not the sheet's own.
    fn exists(&self, key: &str) -> bool;
}

#[derive(Clone, Copy, Debug)]
pub struct DemoDoc {
    pub id: &'static str,
    pub title: &'static str,
}

#[cfg(any(feature = "demo", test))]
const DEMOS: [DemoDoc; 3] = [
    DemoDoc {
        id: "household-budget",
        title: "Household Budget",
    },
    DemoDoc {
        id: "project-plan",
        title: "Project Plan",
    },
    DemoDoc {
        id: "sales-table",
        title: "Sales Table",
    },
];

#[cfg(any(feature = "demo", test))]
pub struct BundledDocs;

#[cfg(any(feature = "demo", test))]
impl SheetDocs for BundledDocs {
    fn initial(&self) -> Workbook {
        let first = self.demos().first().expect("demo build needs a bundled sheet");
        let mut workbook = Workbook::default();
        workbook.sheets = vec![self.load(first.id).expect("first bundled sheet should load")];
        workbook
    }

    fn demos(&self) -> &[DemoDoc] {
        &DEMOS
    }

    fn load(&self, key: &str) -> Result<Sheet, String> {
        let (title, csv) = match key {
            "household-budget" => (
                "Household Budget",
                include_str!("../demos/household_budget.csv"),
            ),
            "project-plan" => ("Project Plan", include_str!("../demos/project_plan.csv")),
            "sales-table" => ("Sales Table", include_str!("../demos/sales_table.csv")),
            _ => return Err(format!("unknown bundled sheet: {key}")),
        };
        Ok(sheet::sheet_from_csv(title, csv))
    }

    fn save(&self, _key: &str, _sheet: &Sheet) -> Result<(), String> {
        Err("demo build: saving is off".to_string())
    }

    fn can_save(&self) -> bool {
        false
    }

    fn exists(&self, _key: &str) -> bool {
        false
    }
}

#[cfg(not(feature = "demo"))]
pub struct FsDocs;

#[cfg(not(feature = "demo"))]
impl SheetDocs for FsDocs {
    fn initial(&self) -> Workbook {
        Workbook::with_demo()
    }

    fn demos(&self) -> &[DemoDoc] {
        &[]
    }

    fn load(&self, key: &str) -> Result<Sheet, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = key;
            Err("disk access is unavailable on the web".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let text = std::fs::read_to_string(key).map_err(|e| e.to_string())?;
            let name = std::path::Path::new(key)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "Imported".into());
            Ok(sheet::sheet_from_csv(&name, &text))
        }
    }

    fn save(&self, key: &str, sheet: &Sheet) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (key, sheet);
            Err("disk access is unavailable on the web".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            write_atomic(key, sheet::to_csv(sheet).as_bytes())
        }
    }

    fn can_save(&self) -> bool {
        cfg!(not(target_arch = "wasm32"))
    }

    fn exists(&self, key: &str) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = key;
            false
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // symlink_metadata: a dangling link is still something Save
            // would replace.
            std::fs::symlink_metadata(key).is_ok()
        }
    }
}

/// Write `bytes` to `path` so a crash or a full disk never leaves a
/// half-written file: the bytes go to a temporary file in the same folder
/// (same volume, so the rename is atomic), are flushed, and only then
/// renamed over the target. On failure the target is untouched and the
/// temporary file is removed.
#[cfg(all(not(feature = "demo"), not(target_arch = "wasm32")))]
fn write_atomic(path: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::path::Path;
    if path.trim().is_empty() {
        return Err("no file name".to_string());
    }
    let target = Path::new(path);
    let name = target
        .file_name()
        .ok_or_else(|| format!("{path} is not a file name"))?
        .to_string_lossy()
        .to_string();
    let folder = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let temp = folder.join(format!(".{name}.tmp.{}", std::process::id()));
    // create_new: never write through something already at the temp name.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, target)
    })();
    if let Err(e) = result {
        // Only the temporary file this call created; the target is intact.
        let _ = std::fs::remove_file(&temp);
        return Err(e.to_string());
    }
    Ok(())
}

pub fn docs() -> Box<dyn SheetDocs> {
    #[cfg(feature = "demo")]
    {
        Box::new(BundledDocs)
    }
    #[cfg(not(feature = "demo"))]
    {
        Box::new(FsDocs)
    }
}

pub fn copy_text(cx: &mut makepad_widgets::Cx, text: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    cx.copy_to_clipboard(text);
    #[cfg(target_arch = "wasm32")]
    let _ = (cx, text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::Value;
    use std::collections::HashSet;

    #[test]
    fn bundled_demos_load_with_valid_formulas_names_and_results() {
        let docs = BundledDocs;
        let mut titles = HashSet::new();

        for demo in docs.demos() {
            assert!(titles.insert(demo.title), "duplicate title: {}", demo.title);
            let sheet = docs.load(demo.id).expect("bundled demo should load");
            assert_eq!(sheet.name, demo.title);
            let ((r0, c0), (r1, c1)) = sheet.used_range().expect("demo should not be empty");
            let mut formula_count = 0;
            for row in r0..=r1 {
                for col in c0..=c1 {
                    if sheet.input((row, col)).starts_with('=') {
                        formula_count += 1;
                        assert!(
                            !matches!(sheet.value((row, col)), Value::Err(_)),
                            "{} formula {} failed",
                            demo.title,
                            sheet::pos_name((row, col))
                        );
                    }
                }
            }
            assert!(formula_count > 0, "{} needs a formula", demo.title);

            let (cell, expected) = match demo.id {
                "household-budget" => ((7, 1), 3825.0),
                "project-plan" => ((6, 6), 12100.0),
                "sales-table" => ((8, 3), 5250.0),
                _ => unreachable!(),
            };
            let actual = sheet.value(cell).as_num().expect("key result should be numeric");
            assert!((actual - expected).abs() < 1e-9, "{} key result", demo.title);
        }
    }
}
