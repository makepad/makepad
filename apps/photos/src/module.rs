//! photos as a module (aicontrol.md §3): the app the window manager seats
//! in a tile in-process, in an isolate of its own — and the first app
//! written to the contract from its first line.
//!
//! `register` puts the tile grid's widget family and this crate's own into
//! the isolate the host prepared; `create` mints one `PhotosView{}` root
//! there, tells it which collection to open, and hands the host its
//! tools. The module never touches a file, a socket or a thread itself:
//! the baked library is read by the grid's own store worker, and a
//! collection is asked for by NAME (`smbc`), resolved under the library
//! root — never a path from the model.

use crate::view::PhotosView;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct PhotosModule;

/// The one linked instance of the module description: immutable, no state.
pub static PHOTOS_MODULE: PhotosModule = PhotosModule;

impl AppModule for PhotosModule {
    fn id(&self) -> &'static str {
        "photos"
    }

    fn label(&self) -> &'static str {
        "Photos"
    }

    fn register(&self, vm: &mut ScriptVm) {
        makepad_image_tiles::script_mod(vm);
        crate::view::script_mod(vm);
    }

    fn open_schema(&self) -> OpenSchema {
        OpenSchema::new(1).arg("collection", OpenArgKind::Text, false)
    }

    fn create(&self, vm: &mut ScriptVm, open: ValidatedOpen, _handles: InstanceHandles) -> InstanceParts {
        let value = script_eval!(vm, {
            use mod.widgets.*
            PhotosView {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        let collection = open.text("collection").filter(|c| crate::library::is_collection_name(c)).map(String::from);
        if let Some(mut view) = root.borrow_mut::<PhotosView>() {
            view.set_collection(collection);
        }
        InstanceParts {
            root: root.clone(),
            executor: Box::new(PhotosExecutor { root }),
            shutdown: Box::new(|_vm| {}),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &["storage"]
    }
}

/// The instance's tools, answered against the root at call time.
struct PhotosExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for PhotosExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::ai::manifest()
    }

    fn execute(&mut self, cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        let result = match self.root.borrow_mut::<PhotosView>() {
            Some(mut view) => crate::ai::answer(cx, &mut view, call),
            None => makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "the wall is gone"),
        };
        ExecOutcome::Done(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_describes_itself_and_opens_by_collection_name_only() {
        let m = &PHOTOS_MODULE;
        assert_eq!(m.id(), "photos");
        assert_eq!(m.label(), "Photos");
        assert!(m.capabilities().contains(&"storage"));
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        let open = schema.validate(r#"{"collection":"smbc"}"#, &[]).unwrap();
        assert_eq!(open.text("collection"), Some("smbc"));
        assert!(schema.validate(r#"{"path":"/tmp/x"}"#, &[]).is_err(), "a path is never an open argument");
    }
}
