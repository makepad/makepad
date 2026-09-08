// Application integration for the shared Dock presentation. This file
// is included by main.rs so the app keeps a single dispatch and ownership root.
impl App {
    fn project_dir(&self) -> PathBuf {
        self.args
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
    fn workers_finished(&self) -> bool {
        self.iterations
            .worker
            .as_ref()
            .is_none_or(IterationWorker::is_finished)
            && self
                .agent_sessions
                .worker
                .as_ref()
                .is_none_or(|w| w.is_finished())
            && self
                .disk_worker
                .as_ref()
                .is_none_or(DiskWorker::is_finished)
            && self
                .document_worker
                .as_ref()
                .is_none_or(DocumentWorker::is_finished)
            && self
                .activity_worker
                .as_ref()
                .is_none_or(ActivityWorker::is_finished)
            && self
                .usage_worker
                .as_ref()
                .is_none_or(UsageWorker::is_finished)
            && self
                .project_tree_ref
                .borrow::<StudioProjectTree>()
                .is_none_or(|tree| tree.is_finished())
    }
    fn materialize_tabs(&self, cx: &mut Cx) {
        let dock = self.ui.dock(cx, ids!(dock));
        for (id, item) in dock.clone_state().unwrap_or_default() {
            if let DockItem::Tab { kind, .. } = item {
                if let Some(tab) = dock.item_or_create(cx, id, kind) {
                    self.set_terminal_cwd(cx, &tab);
                }
            }
        }
    }
    fn restore_items(&mut self, cx: &mut Cx) {
        let file = self.state_dir().join("items.ron");
        if std::fs::metadata(&file).is_ok_and(|m| m.len() <= 1_048_576) {
            if let Some(stored) = std::fs::read_to_string(file)
                .ok()
                .and_then(|s| StoredItems::deserialize_ron(&s).ok())
            {
                let dock = self.ui.dock(cx, ids!(dock));
                let state = dock.clone_state().unwrap_or_default();
                for (id, path) in stored.documents.into_iter().take(32) {
                    if PathBuf::from(&path).is_absolute()
                        && path.len() <= 4096
                        && matches!(state.get(&LiveId(id)),Some(DockItem::Tab{kind,..})if *kind==id!(CodeTab))
                    {
                        self.code_tabs.insert(id, PathBuf::from(path));
                    }
                }
                self.designs=stored.designs.into_iter().filter(|d|d.title.len()<=120&&d.detail.len()<=8192&&d.path.as_ref().is_none_or(|p|PathBuf::from(p).is_absolute()&&p.len()<=4096)&&matches!(state.get(&LiveId(d.id)),Some(DockItem::Tab{kind,..})if *kind==id!(DesignTab))).take(64).collect();
            }
        }
        // Migrate utility tabs out of saved workspaces, and prune tabs whose
        // document/design metadata is missing or corrupt.
        let dock = self.ui.dock(cx, ids!(dock));
        for (id, item) in dock.clone_state().unwrap_or_default() {
            if let DockItem::Tab { kind, .. } = item {
                if Self::is_utility_kind(kind)
                    || (kind == id!(CodeTab) && !self.code_tabs.contains_key(&id.0))
                    || (kind == id!(DesignTab) && !self.designs.iter().any(|d| d.id == id.0))
                {
                    dock.close_tab(cx, id);
                }
            }
        }
    }

    fn save_workspace(&self, cx: &mut Cx) {
        let dir = self.state_dir();
        if let Some(surface) = self
            .ui
            .widget(cx, ids!(workspace))
            .borrow::<StudioSurface>()
        {
            if let Err(e) = surface.workspace.save(&dir) {
                log!("studio workspace save: {e}");
            }
        }
        let mut documents: Vec<_> = self
            .code_tabs
            .iter()
            .map(|(id, p)| (*id, p.to_string_lossy().into_owned()))
            .collect();
        documents.sort();
        let stored = StoredItems {
            documents,
            designs: self.designs.clone(),
        };
        let save = || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            let tmp = dir.join("items.ron.tmp");
            std::fs::write(&tmp, stored.serialize_ron())?;
            std::fs::rename(tmp, dir.join("items.ron"))
        };
        if let Err(e) = save() {
            log!("studio items save: {e}");
        }
    }
    /// The mode radios and the Project pane follow the workspace mode; the
    /// tasks view and its caption tools are shown by `set_flows_visible`.
    fn refresh_mode_controls(&mut self, cx: &mut Cx) {
        let mode = self.workspace_mode(cx);
        self.ui
            .dock(cx, ids!(dock))
            .item(id!(project_tree_tab))
            .set_visible(cx, mode == Mode::Structured);
        for (id, active) in [
            (id!(mode_structured), mode == Mode::Structured),
            (id!(mode_tasks), mode == Mode::Tasks),
            (id!(mode_architecture), mode == Mode::Architecture),
            (id!(mode_disk), mode == Mode::Disk),
        ] {
            self.ui.radio_button(cx, &[id]).set_active(cx, active, Animate::No);
        }
        self.ui.view(cx, ids!(arch_tools)).set_visible(cx, mode == Mode::Architecture);
        self.ui.view(cx, ids!(tasks_tools)).set_visible(cx, mode == Mode::Tasks);
        self.ui.view(cx, ids!(disk_tools)).set_visible(cx, mode == Mode::Disk);
        if mode == Mode::Architecture {
            self.sync_atlas_toolbar(cx);
        }
        if mode == Mode::Disk {
            self.refresh_disk_panel(cx);
        }
        self.ui.view(cx, ids!(toolbar)).redraw(cx);
    }
    fn workspace_json(&self, cx: &mut Cx) -> Value {
        json::obj(vec![
            ("mode", json::s(self.workspace_mode(cx).as_str())),
            ("coverage", json::s(&self.activity_snapshot.coverage)),
            (
                "active_item",
                self.active_item
                    .map(|id| json::s(format!("{id:x}")))
                    .unwrap_or(Value::Null),
            ),
        ])
    }
    fn refresh_workspace(&mut self, cx: &mut Cx) {
        self.materialize_tabs(cx);
        let dock = self.ui.dock(cx, ids!(dock));
        for design in &self.designs {
            let body = dock.item(LiveId(design.id));
            if body.is_empty() {
                continue;
            }
            body.label(cx, ids!(design_detail))
                .set_text(cx, &design.detail);
            body.label(cx, ids!(design_path)).set_text(
                cx,
                &design
                    .path
                    .as_ref()
                    .map(|p| format!("Code: {p}"))
                    .unwrap_or_default(),
            );
        }
        self.refresh_mode_controls(cx);
        let mut report = format!(
            "{}\n{}\n\nPROCESSES\n",
            self.project_dir().display(),
            self.activity_snapshot.coverage
        );
        for p in &self.activity_snapshot.processes {
            report.push_str(&format!("{:?} · {} · {}\n", p.state, p.pid, p.command));
        }
        report.push_str("\nSOURCE CHANGES\n");
        for f in &self.activity_snapshot.changed_files {
            report.push_str(&format!("{:?} · {}\n", f.state, f.path.display()));
        }
        report.push_str("\nSTUDIO ACTIONS\n");
        for a in self.activity.iter().rev() {
            report.push_str(a);
            report.push('\n');
        }
        for e in &self.activity_snapshot.errors {
            report.push_str(&format!("\nObservation: {e}"));
        }
        self.ui
            .label(cx, ids!(activity_report))
            .set_text(cx, &report);
    }
    fn is_utility_kind(kind: LiveId) -> bool {
        [
            id!(SettingsTab),
            id!(DiskTab),
            id!(UsageTab),
            id!(ActivityTab),
        ]
        .contains(&kind)
    }

    fn open_activity(&mut self, cx: &mut Cx) {
        self.show_utility(cx, id!(activity_tab), id!(ActivityTab), "Activity");
        self.refresh_workspace(cx);
    }
    fn open_code(&mut self, cx: &mut Cx, path: PathBuf, focus: bool) -> Result<String, String> {
        if !path.is_absolute() {
            return Err("Source path must be absolute".into());
        }
        if focus {
            self.ensure_visible_host(cx);
        }
        if let Some((&id, _)) = self.code_tabs.iter().find(|(_, p)| {
            **p == path
                || {
                    let documents = self.documents.borrow();
                    documents
                        .get(p)
                        .zip(documents.get(&path))
                        .is_some_and(|(a, b)| a.same_document(&b))
                }
        }) {
            if focus {
                self.ui.dock(cx, ids!(dock)).select_tab(cx, LiveId(id));
                self.active_item = Some(id);
            }
            return Ok(format!("{id:x}"));
        }
        self.document_worker
            .as_mut()
            .ok_or("Document reader unavailable")?
            .watch(path.clone())?;
        let parent = self.tabs_container(cx).ok_or("No tab container")?;
        let dock = self.ui.dock(cx, ids!(dock));
        let id = dock.unique_id(LiveId::from_str(&format!("studio-code:{}", path.display())).0);
        let title = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let tab = if focus {
            dock.create_and_select_tab(cx, parent, id, id!(CodeTab), title, id!(CloseableTab), None)
        } else {
            dock.create_tab(cx, parent, id, id!(CodeTab), title, id!(CloseableTab), None)
        };
        if tab.is_none() {
            self.document_worker.as_mut().unwrap().unwatch(&path);
            return Err("Could not create code view".into());
        }
        self.code_tabs.insert(id.0, path);
        if focus {
            self.active_item = Some(id.0);
        }
        self.refresh_workspace(cx);
        self.save_dock(cx);
        self.workspace_dirty = true;
        Ok(format!("{:x}", id.0))
    }
    fn code_document(&self, cx: &mut Cx, tab: u64) -> Result<DocumentHandle, String> {
        if !self.code_tabs.contains_key(&tab) {
            return Err("No code view with this tab ID".into());
        }
        self.ui
            .dock(cx, ids!(dock))
            .item(LiveId(tab))
            .borrow::<StudioCodeEditor>()
            .and_then(|e| e.document())
            .ok_or("File is still loading or unavailable".into())
    }
    fn close_item(&mut self, cx: &mut Cx, id: u64) -> Result<(), String> {
        if id == id!(project_tree_tab).0 {
            return Err("The Project pane stays available in Structured mode".into());
        }
        if let Some(path) = self.code_tabs.get(&id).cloned() {
            let last_tab = !self.code_tabs.iter().any(|(other, other_path)| {
                *other != id && self.documents_share_path(other_path, &path)
            });
            if last_tab && !self.map_holds_document(&path) {
                self.documents.borrow_mut().remove(&path)?;
                if let Some(worker) = &mut self.document_worker {
                    worker.unwatch(&path);
                }
            }
            self.code_tabs.remove(&id);
            self.code_errors.remove(&id);
        }
        self.designs.retain(|d| d.id != id);
        self.ui.dock(cx, ids!(dock)).close_tab(cx, LiveId(id));
        self.workspace_dirty = true;
        Ok(())
    }
    fn save_code(&mut self, cx: &mut Cx, tab: u64) -> Result<String, String> {
        let document = self.code_document(cx, tab)?;
        let id = self.queue_document_save(&document)?;
        Ok(format!(
            "Save {id} queued; read_code reports dirty=false only after the write is acknowledged"
        ))
    }
    /// Explicit save of one registry document. Conflicts stay unsaved.
    fn queue_document_save(&mut self, document: &DocumentHandle) -> Result<u64, String> {
        if document.has_conflict() {
            return Err(
                "Disk changed while this buffer has edits; resolve the conflict before saving"
                    .into(),
            );
        }
        self.document_worker
            .as_mut()
            .ok_or("Document reader unavailable")?
            .save(
                document.path().to_owned(),
                document.disk_text(),
                Arc::new(document.current_text()),
            )
    }
    /// Save every dirty registry document (map-only buffers included). A
    /// conflict still has to be resolved before that file is written.
    fn save_all_documents(&mut self, cx: &mut Cx) -> Result<String, String> {
        let dirty: Vec<DocumentHandle> = self
            .documents
            .borrow()
            .handles()
            .filter(|document| document.is_dirty() && !document.has_conflict())
            .cloned()
            .collect();
        if dirty.is_empty() {
            return Ok("No unsaved documents".into());
        }
        let mut queued = 0usize;
        for document in &dirty {
            self.queue_document_save(document)?;
            queued += 1;
        }
        self.ui.label(cx, ids!(status_state)).set_text(
            cx,
            &format!("Save all: {queued} queued"),
        );
        Ok(format!(
            "Save all queued {queued} document{}",
            if queued == 1 { "" } else { "s" }
        ))
    }
    fn unsaved_registry_documents(&self) -> Vec<DocumentHandle> {
        self.documents
            .borrow()
            .handles()
            .filter(|document| document.is_dirty() || document.has_conflict())
            .cloned()
            .collect()
    }
    fn documents_share_path(&self, a: &Path, b: &Path) -> bool {
        if a == b {
            return true;
        }
        let documents = self.documents.borrow();
        documents
            .get(a)
            .zip(documents.get(b))
            .is_some_and(|(left, right)| left.same_document(&right))
    }
    fn tab_holds_document(&self, path: &Path) -> bool {
        self.code_tabs
            .values()
            .any(|tab| self.documents_share_path(tab, path))
    }
    fn map_holds_document(&self, path: &Path) -> bool {
        if self
            .map_document_watchers
            .get(path)
            .is_some_and(|views| !views.is_empty())
        {
            return true;
        }
        self.map_document_watchers.iter().any(|(watched, views)| {
            !views.is_empty() && self.documents_share_path(watched, path)
        })
    }
    fn document_is_held(&self, path: &Path) -> bool {
        self.tab_holds_document(path) || self.map_holds_document(path)
    }
    fn document_watch_paths(&self) -> Vec<PathBuf> {
        let mut paths = HashSet::new();
        paths.extend(self.code_tabs.values().cloned());
        paths.extend(self.map_document_watchers.keys().cloned());
        for handle in self.documents.borrow().handles() {
            if handle.is_dirty() || handle.has_conflict() {
                paths.insert(handle.path().to_owned());
            }
        }
        paths.into_iter().collect()
    }
    fn retain_map_document(&mut self, uid: WidgetUid, path: PathBuf) {
        if self
            .map_document_watchers
            .get(&path)
            .is_some_and(|views| views.contains(&uid))
        {
            return;
        }
        let held = self.document_is_held(&path);
        if !held {
            match self.document_worker.as_mut() {
                Some(worker) => {
                    if let Err(error) = worker.watch(path.clone()) {
                        log!("atlas: cannot watch {}: {error}", path.display());
                        return;
                    }
                }
                None => {
                    log!("atlas: document reader unavailable for {}", path.display());
                    return;
                }
            }
        }
        self.map_document_watchers
            .entry(path)
            .or_default()
            .insert(uid);
    }
    fn release_map_document(&mut self, uid: WidgetUid, path: PathBuf) {
        if let Some(views) = self.map_document_watchers.get_mut(&path) {
            views.remove(&uid);
            if views.is_empty() {
                self.map_document_watchers.remove(&path);
            }
        }
        self.release_unwatched_document(&path);
    }
    fn release_map_view(&mut self, uid: WidgetUid) {
        let paths: Vec<PathBuf> = self
            .map_document_watchers
            .iter()
            .filter(|(_, views)| views.contains(&uid))
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths {
            self.release_map_document(uid, path);
        }
    }
    /// Drop a worker watch and registry entry when neither the map nor a tab
    /// still wants the path, unless the buffer is dirty or conflicted.
    fn release_unwatched_document(&mut self, path: &Path) {
        if self.document_is_held(path) {
            return;
        }
        let dirty = self
            .documents
            .borrow()
            .get(path)
            .is_some_and(|document| document.is_dirty() || document.has_conflict());
        if dirty {
            return;
        }
        if self.documents.borrow_mut().remove(path).is_ok() {
            if let Some(worker) = &mut self.document_worker {
                worker.unwatch(path);
            }
            self.map_document_watchers.remove(path);
        }
    }
    fn drain_documents(&mut self, cx: &mut Cx) {
        let saved = self
            .document_worker
            .as_mut()
            .map(DocumentWorker::poll_saves)
            .unwrap_or_default();
        for save in saved {
            match save.result {
                Ok(()) => {
                    let doc = self.documents.borrow().get(&save.path);
                    if let (Some(doc), Some(text), Some(revision)) =
                        (doc, save.text, save.revision)
                    {
                        let path = doc.path().to_owned();
                        doc.save_succeeded(text, revision);
                        drop(doc);
                        self.release_unwatched_document(&path);
                        self.ui
                            .label(cx, ids!(status_state))
                            .set_text(cx, "File saved");
                    }
                }
                Err(error) => {
                    self.ui
                        .label(cx, ids!(status_state))
                        .set_text(cx, &format!("Save refused: {error}"));
                    log!("studio save: {error}");
                }
            }
        }
        let deliveries = self
            .document_worker
            .as_mut()
            .map(DocumentWorker::poll_prepared)
            .unwrap_or_default();
        let mut changed = false;
        for delivery in deliveries {
            let snapshot = delivery.snapshot;
            // the worker prepared the editor state: admission attaches it
            let applied = self.documents.borrow_mut().apply_prepared(&snapshot, delivery.prepared);
            match applied {
                Ok(handle) => {
                    log!("studio document: prepared {}", snapshot.path.display());
                    let tabs: Vec<_> = self
                        .code_tabs
                        .iter()
                        .filter(|(_, p)| **p == snapshot.requested_path || **p == snapshot.path)
                        .map(|(id, _)| *id)
                        .collect();
                    for id in tabs {
                        let duplicate = self.code_tabs.keys().copied().find(|other| {
                            *other != id
                                && self
                                    .ui
                                    .dock(cx, ids!(dock))
                                    .item(LiveId(*other))
                                    .borrow::<StudioCodeEditor>()
                                    .and_then(|e| e.document())
                                    .is_some_and(|d| d.same_document(&handle))
                        });
                        if let Some(existing) = duplicate {
                            if let Some(path) = self.code_tabs.remove(&id) {
                                self.release_unwatched_document(&path);
                            }
                            self.ui.dock(cx, ids!(dock)).close_tab(cx, LiveId(id));
                            if self.active_item == Some(id) {
                                self.active_item = Some(existing);
                                self.ui
                                    .dock(cx, ids!(dock))
                                    .select_tab(cx, LiveId(existing));
                            }
                            self.save_dock(cx);
                            self.workspace_dirty = true;
                            continue;
                        }
                        self.code_errors.remove(&id);
                        if let Some(mut editor) = self
                            .ui
                            .dock(cx, ids!(dock))
                            .item(LiveId(id))
                            .borrow_mut::<StudioCodeEditor>()
                        {
                            editor.bind_document(cx, handle.clone());
                            editor.sync_document(cx);
                        }
                    }
                    changed = true;
                }
                Err(error) => {
                    if error.contains("Prepared ") {
                        if let Some(worker) = &mut self.document_worker {
                            worker.refresh(&snapshot.requested_path);
                            worker.refresh(&snapshot.path);
                        }
                    }
                    for (id, path) in &self.code_tabs {
                        if *path == snapshot.requested_path {
                            self.code_errors.insert(*id, error.clone());
                        }
                    }
                    changed = true;
                    self.ui.label(cx, ids!(status_state)).set_text(cx, &error);
                    log!("studio document: {error}");
                }
            }
        }
        if changed {
            self.refresh_workspace(cx);
            self.notify_atlas_documents(cx);
        }
    }
    fn drain_activity(&mut self, cx: &mut Cx) {
        let roots = self
            .agent_sessions
            .bindings
            .keys()
            .filter_map(|id| self.agent_pid_for_tab(*id))
            .collect();
        if let Some(worker) = &mut self.activity_worker {
            if let Err(error) = worker.set_external_roots(roots) {
                log!("studio agent observation: {error}");
            }
        }
        if let Some(snapshot) = self.activity_worker.as_mut().and_then(ActivityWorker::poll) {
            let changes = snapshot.changed_files.clone();
            self.activity_snapshot = snapshot;
            for file in changes {
                if self
                    .seen_files
                    .get(&file.path)
                    .is_some_and(|n| *n >= file.sequence)
                {
                    continue;
                }
                self.seen_files.insert(file.path.clone(), file.sequence);
                if file.state == FileState::Changed && self.code_tabs.len() < 32 {
                    if let Err(error) = self.open_code(cx, file.path, false) {
                        log!("studio observed source: {error}");
                    }
                }
            }
            self.refresh_workspace(cx);
        }
    }
}
