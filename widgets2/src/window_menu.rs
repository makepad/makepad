// a window menu implementation
use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw.KeyCode

    mod.widgets.MenuItem = mod.std.set_type_default() do #(MenuItem::script_api(vm)),

    mod.widgets.WindowMenuBase = #(WindowMenu::register_widget(vm))
    mod.widgets.WindowMenu = set_type_default() do mod.widgets.WindowMenuBase{
        height: 0 width: 0
    }
}

#[derive(Clone, Debug, Script, ScriptHook)]
pub enum MenuItem {
    #[live { items: Vec::new() }]
    Main { items: Vec<LiveId> },

    #[live { name: String::new(), items: Vec::new() }]
    Sub { name: String, items: Vec<LiveId> },

    #[live { name: String::new(), shift: false, key: KeyCode::Unknown, enabled: true }]
    Item {
        name: String,
        shift: bool,
        key: KeyCode,
        enabled: bool,
    },

    #[pick]
    Line,
}

#[derive(Script, Widget)]
pub struct WindowMenu {
    #[walk]
    walk: Walk,
    #[redraw]
    #[rust]
    area: Area,
    #[layout]
    layout: Layout,
    #[rust]
    menu_items: HashMap<LiveId, MenuItem>,
    #[rust]
    initialized: bool,
}

#[derive(Clone, Default)]
pub enum WindowMenuAction {
    Command(LiveId),
    #[default]
    None,
}

impl ScriptHook for WindowMenu {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Handle menu items from the object's vec (children with $id prefix)
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    // Only process vec key ids ($main, $app, $quit, etc.)
                    if kv.key.as_id().is_some() {
                        if let Some(id) = kv.key.as_id() {
                            // Check if this is a MenuItem by checking its type
                            if let Some(val_obj) = kv.value.as_object() {
                                if vm
                                    .bx
                                    .heap
                                    .type_matches_id(val_obj, MenuItem::script_type_id_static())
                                {
                                    let item = MenuItem::script_from_value(vm, kv.value);
                                    self.menu_items.insert(id, item);
                                }
                            }
                        }
                    }
                }
            });
        }

        // Initialize the macOS menu after applying (defer to first draw)
        self.initialized = false;
    }
}

impl WindowMenu {
    fn update_macos_menu(&self, cx: &mut Cx) {
        #[cfg(target_os = "macos")]
        {
            fn recur_menu(command: LiveId, menu_items: &HashMap<LiveId, MenuItem>) -> MacosMenu {
                if let Some(item) = menu_items.get(&command) {
                    match item.clone() {
                        MenuItem::Main { items } => {
                            let mut out = Vec::new();
                            for item in items {
                                out.push(recur_menu(item, menu_items));
                            }
                            return MacosMenu::Main { items: out };
                        }
                        MenuItem::Item {
                            name,
                            shift,
                            key,
                            enabled,
                        } => {
                            return MacosMenu::Item {
                                command,
                                name,
                                shift,
                                key,
                                enabled,
                            }
                        }
                        MenuItem::Sub { name, items } => {
                            let mut out = Vec::new();
                            for item in items {
                                out.push(recur_menu(item, menu_items));
                            }
                            return MacosMenu::Sub { name, items: out };
                        }
                        MenuItem::Line => return MacosMenu::Line,
                    }
                } else {
                    log!("Menu cannot find item {}", command);
                    MacosMenu::Line
                }
            }

            // Find the Main menu item (the root)
            let main_id = self
                .menu_items
                .iter()
                .find(|(_, item)| matches!(item, MenuItem::Main { .. }))
                .map(|(id, _)| *id);

            if let Some(main_id) = main_id {
                let menu = recur_menu(main_id, &self.menu_items);
                cx.update_macos_menu(menu)
            }
        }
        let _ = cx;
    }
}

impl Widget for WindowMenu {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event {
            Event::MacosMenuCommand(item) => {
                if *item == live_id!(quit) {
                    cx.quit();
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        // Initialize the macOS menu on first draw
        if !self.initialized {
            self.initialized = true;
            self.update_macos_menu(cx);
        }
        DrawStep::done()
    }
}

impl WindowMenuRef {
    pub fn command(&self) -> Option<LiveId> {
        if let Some(mut _dock) = self.borrow_mut() {}
        None
    }
}
