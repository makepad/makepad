use super::*;

impl App {
    pub(crate) fn reachable_tab_bar_of_tab(
        dock: &DockRef,
        tab_id: LiveId,
    ) -> Option<(LiveId, usize)> {
        let dock_items = dock.clone_state()?;
        let mut stack = vec![id!(root)];
        while let Some(item_id) = stack.pop() {
            match dock_items.get(&item_id)? {
                DockItem::Splitter { a, b, .. } => {
                    stack.push(*b);
                    stack.push(*a);
                }
                DockItem::Tabs { tabs, .. } => {
                    if let Some(pos) = tabs.iter().position(|candidate| *candidate == tab_id) {
                        return Some((item_id, pos));
                    }
                }
                DockItem::Tab { .. } => {}
            }
        }
        None
    }

    pub(crate) fn create_dock_tab(
        dock: &DockRef,
        cx: &mut Cx,
        anchor: LiveId,
        tab_id: LiveId,
        pane_id: LiveId,
        title: String,
        select: bool,
    ) -> Option<()> {
        let (tab_bar, pos) = Self::reachable_tab_bar_of_tab(dock, anchor)?;
        let created = if select {
            dock.create_and_select_tab(
                cx,
                tab_bar,
                tab_id,
                pane_id,
                title,
                id!(CloseableTab),
                Some(pos),
            )
        } else {
            dock.create_tab(
                cx,
                tab_bar,
                tab_id,
                pane_id,
                title,
                id!(CloseableTab),
                Some(pos),
            )
        };
        created.map(|_| ())
    }

    pub(crate) fn find_anchor_tab_in<'a>(
        dock: &DockRef,
        default_id: LiveId,
        iter: impl Iterator<Item = (&'a LiveId, &'a str)>,
        mount: &str,
    ) -> Option<LiveId> {
        if dock.find_tab_bar_of_tab(default_id).is_some() {
            return Some(default_id);
        }
        for (tab_id, tab_mount) in iter {
            if tab_mount == mount && dock.find_tab_bar_of_tab(*tab_id).is_some() {
                return Some(*tab_id);
            }
        }
        None
    }
}
