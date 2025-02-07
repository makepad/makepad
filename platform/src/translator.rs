use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::error::Error;

use crate::{LiveId, LiveFileId, Cx};
use crate::makepad_live_compiler::LiveScopeTarget;
use crate::makepad_live_compiler::live_registry;

#[derive(Debug, Clone)]
pub struct Translator {
    current_language: String,
    translations: HashMap<String, LiveFileId>,
}

impl Translator {
    pub fn new(default_lang: &str) -> Self {
        Translator {
            current_language: default_lang.to_string(),
            translations: HashMap::new(),
        }
    }

    /// Create a new translator using the convention of having a `translations` directory
    /// with a file for each language, e.g. `translations/en.rs` for English.
    /// 
    /// Example usage:
    /// ```rust
    /// let translator = Translator::discover_translations(&cx, "en")?;
    /// ```
    /// 
    pub fn discover_translations(cx: &Cx, default_lang: &str) -> Self {
        let mut translator = Self::new(default_lang);
        
        let live_registry = cx.live_registry.borrow();
        let mut translations = Vec::new();
        
        // Look for translation files in the conventional location
        for (file_path, _) in live_registry.file_ids() {
            if file_path.contains("/translations/") && file_path.ends_with(".rs") {
                // Extract language code from filename (e.g., "en.rs" -> "en")
                if let Some(file_name) = Path::new(file_path).file_stem() {
                    if let Some(lang_code) = file_name.to_str() {
                        translations.push((lang_code, file_path.as_str()));
                    }
                }
            }
        }

        if translations.is_empty() {
            error!("No translation files found in /translations/ directory");
        }

        let _ = translator.set_translations(cx, &translations);
        translator
    }

    /// Set the translations for the translator
    /// 
    /// Example usage:
    /// ```rust
    /// let mut translator = Translator::new("en");
    ///     translator.set_translations(cx, &[
    ///         ("en", "src/translations/en.rs"),
    ///         ("es", "src/translations/es.rs"),
    ///     ])?;
    /// ```
    pub fn set_translations(&mut self, cx: &Cx, translations: &[(&str, &str)]) -> Result<(), Box<dyn Error>> {
        let mut lang_map = HashMap::new();
        
        for (lang, file_name) in translations {
            let live_registry_ref = cx.live_registry.borrow();
            let file_id = live_registry_ref.file_name_to_file_id(file_name)
                .ok_or_else(|| format!("Failed to find file: {}", file_name))?;
            
            lang_map.insert(lang.to_string(), file_id);
        }

        if !lang_map.contains_key(&self.current_language) {
            return Err("Default language not found in translations".into());
        }

        self.translations = lang_map;
        Ok(())
    }

    pub fn add_translations(&mut self, lang: &str, file_name: &str, cx: &Cx) -> Result<(), Box<dyn Error>> {
        let live_registry_ref = cx.live_registry.borrow();
        let file_id = live_registry_ref.file_name_to_file_id(file_name)
            .ok_or_else(|| format!("Failed to find file: {}", file_name))?;
        
        self.translations.insert(lang.to_string(), file_id);
        Ok(())
    }

    /// Translate a string
    pub fn tr(&self, cx: &Cx, tr_live_id: LiveId) -> String {
        let file_id = self.translations.get(&self.current_language).expect("Failed to find translation file");

        let live_registry_ref = cx.live_registry.borrow();

        let live_file = live_registry_ref.file_id_to_file(*file_id);
        // All the nodes in the file
        let nodes = &live_file.expanded.nodes;

        // Find the target node
        let target = live_registry_ref.find_scope_target(tr_live_id, nodes).expect("Failed to find target");
        match target {
            // The target is a local pointer to a node in the same file
            live_registry::LiveScopeTarget::LocalPtr(local_ptr) => {
                let live_node = nodes.get(local_ptr).expect("Failed to find node");
                let value = live_registry_ref.live_node_as_string(live_node);
                value.expect("Translation not found")
            }
            // The target is a live pointer to a node in another file 
            live_registry::LiveScopeTarget::LivePtr(live_ptr) => {
                let node = live_registry_ref.ptr_to_node(live_ptr);
                let value = live_registry_ref.live_node_as_string(node);
                value.expect("Translation not found")
            }
        }
    }

    /// Translate a string with arguments
    pub fn tr_with_args(
        &self,
        cx: &Cx,
        tr_live_id: LiveId,
        args: &[&str],
    ) -> String {
        // Fetch the translation
        let translation = self.tr(cx, tr_live_id);
        // Replace placeholders with arguments
        self.format_with_args(&translation, args)
    }

    // Helper method to format placeholders
    fn format_with_args(&self, template: &str, args: &[&str]) -> String {
        let mut result = template.to_string();
        for (i, arg) in args.iter().enumerate() {
            let placeholder = format!("{{{}}}", i);
            result = result.replace(&placeholder, arg);
        }
        result
    }

    /// Set the current language
    pub fn set_language(&mut self, language: &str) -> Result<(), String> {
        if self.translations.contains_key(language) {
            self.current_language = language.to_string();
            Ok(())
        } else {
            Err(format!("Language not available: {}", language))
        }
    }
}
