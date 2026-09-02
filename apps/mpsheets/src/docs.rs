//! The document source/sink boundary for native and bundled-demo builds.

use crate::sheet::{self, Sheet};

pub trait SheetDocs {
    fn demos(&self) -> &[DemoDoc];
    fn load(&self, key: &str) -> Result<Sheet, String>;
    fn save(&self, key: &str, sheet: &Sheet) -> Result<(), String>;
    fn can_save(&self) -> bool;
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
}

#[cfg(not(feature = "demo"))]
pub struct FsDocs;

#[cfg(not(feature = "demo"))]
impl SheetDocs for FsDocs {
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
            std::fs::write(key, sheet::to_csv(sheet)).map_err(|e| e.to_string())
        }
    }

    fn can_save(&self) -> bool {
        cfg!(not(target_arch = "wasm32"))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::Value;
    use std::collections::HashSet;

    #[test]
    fn bundled_demos_load_with_live_formulas_and_unique_titles() {
        let docs = BundledDocs;
        let mut titles = HashSet::new();

        for demo in docs.demos() {
            assert!(titles.insert(demo.title), "duplicate title: {}", demo.title);
            let sheet = docs.load(demo.id).expect("bundled demo should load");
            let ((r0, c0), (r1, c1)) = sheet.used_range().expect("demo should not be empty");
            let has_live_formula = (r0..=r1).any(|row| {
                (c0..=c1).any(|col| {
                    sheet.input((row, col)).starts_with('=')
                        && !matches!(sheet.value((row, col)), Value::Err(_))
                })
            });
            assert!(has_live_formula, "{} needs a working formula", demo.title);
        }
    }
}
