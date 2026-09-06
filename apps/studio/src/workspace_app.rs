// Application integration for the shared canvas/Dock presentation. This file
// is included by main.rs so the app keeps a single dispatch and ownership root.
impl App {
    fn project_dir(&self) -> PathBuf {
        self.args
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
    fn workers_finished(&self) -> bool {
        self.disk_worker
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
                log!("studio canvas save: {e}");
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
    fn focus_canvas_item(&self, cx: &mut Cx, id: u64) {
        if let Some(mut surface) = self
            .ui
            .widget(cx, ids!(workspace))
            .borrow_mut::<StudioSurface>()
        {
            if surface.workspace.mode == Mode::Canvas {
                surface.focus_card(cx, id);
            }
        }
    }
    fn refresh_canvas_controls(&self, cx: &mut Cx) {
        if let Some(surface) = self
            .ui
            .widget(cx, ids!(workspace))
            .borrow::<StudioSurface>()
        {
            let w = &surface.workspace;
            let width = self.ui.window(cx, ids!(main_window)).get_inner_size(cx).x;
            let compact = width > 0.0 && width < 800.0;
            self.ui
                .dock(cx, ids!(dock))
                .item(id!(project_tree_tab))
                .set_visible(cx, w.mode == Mode::Structured);
            self.ui
                .view(cx, ids!(canvas_controls))
                .set_visible(cx, w.mode == Mode::Canvas);
            self.ui
                .widget(cx, ids!(canvas_summary))
                .set_visible(cx, w.mode == Mode::Canvas && width >= 1100.0);
            self.ui
                .widget(cx, ids!(status_style))
                .set_visible(cx, !compact);
            self.ui
                .widget(cx, ids!(status_mode))
                .set_visible(cx, !compact);

            self.ui.radio_button(cx, ids!(mode_structured)).set_active(
                cx,
                w.mode == Mode::Structured,
                Animate::No,
            );
            self.ui.radio_button(cx, ids!(mode_canvas)).set_active(
                cx,
                w.mode == Mode::Canvas,
                Animate::No,
            );
            self.ui
                .drop_down(cx, ids!(layout_picker))
                .set_selected_item(cx, if w.layout == LayoutMode::Auto { 0 } else { 1 });
            self.ui
                .drop_down(cx, ids!(layout_picker))
                .set_disabled(cx, w.mode != Mode::Canvas);
            self.ui.label(cx, ids!(canvas_summary)).set_text(
                cx,
                &format!(
                    "{} items · {}",
                    w.cards.len(),
                    makepad_studio::workspace::zoom_label(w.camera.zoom)
                ),
            );
        }
        self.refresh_toolbar(cx);
    }
    fn workspace_json(&self, cx: &mut Cx) -> Value {
        let widget = self.ui.widget(cx, ids!(workspace));
        let Some(surface) = widget.borrow::<StudioSurface>() else {
            return Value::Null;
        };
        let w = &surface.workspace;
        let cards = w
            .cards
            .iter()
            .map(|c| {
                let g = w.geometry(c.id);
                json::obj(vec![
                    ("id", json::s(format!("{:x}", c.id))),
                    ("kind", json::s(c.kind.as_str())),
                    ("title", json::s(&c.title)),
                    ("detail", json::s(&c.detail)),
                    (
                        "parent",
                        c.parent
                            .map(|p| json::s(format!("{p:x}")))
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "rect",
                        g.map(|g| {
                            Value::Arr([g.x, g.y, g.w, g.h].into_iter().map(Value::F64).collect())
                        })
                        .unwrap_or(Value::Null),
                    ),
                ])
            })
            .collect();
        json::obj(vec![
            ("mode", json::s(w.mode.as_str())),
            ("layout", json::s(w.layout.as_str())),
            ("zoom", Value::F64(w.camera.zoom)),
            (
                "pan",
                Value::Arr(vec![Value::F64(w.camera.pan_x), Value::F64(w.camera.pan_y)]),
            ),
            ("cards", Value::Arr(cards)),
            (
                "navigator",
                surface
                    .navigator_geometry()
                    .map(|(bounds, window)| {
                        let rect = |g: makepad_studio::workspace::Geometry| {
                            Value::Arr([g.x, g.y, g.w, g.h].into_iter().map(Value::F64).collect())
                        };
                        json::obj(vec![("bounds", rect(bounds)), ("window", rect(window))])
                    })
                    .unwrap_or(Value::Null),
            ),
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
        let items = dock.clone_state().unwrap_or_default();
        let project = id!(studio_project).0;
        let root = Card::new(
            project,
            CardKind::System,
            self.project_dir()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
        );
        let mut cards = vec![root];
        let mut terminal_pids = HashMap::new();
        let mut ordered: Vec<_> = items.iter().collect();
        ordered.sort_by_key(|(id, _)| id.0);
        for (id, item) in ordered {
            let DockItem::Tab { name, kind, .. } = item else {
                continue;
            };
            let card_kind = if *kind == id!(TerminalTab) {
                CardKind::Terminal
            } else if *kind == id!(CodeTab) {
                CardKind::Code
            } else if *kind == id!(DesignTab) {
                CardKind::System
            } else {
                // Utilities never occupy agent work slots, including old layouts.
                continue;
            };
            let mut card = Card::new(id.0, card_kind, name.clone());
            card.parent = Some(project);
            if *kind == id!(TerminalTab) {
                if let Some(term) = dock.item(*id).widget(cx, ids!(term)).borrow::<MpTerm>() {
                    if let Some(pid) = term.child_pid() {
                        terminal_pids.insert(pid as u32, id.0);
                        card.detail = format!(
                            "Live PTY · PID {pid} · {}",
                            term.cwd
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default()
                        );
                    } else {
                        card.detail = "Starting live terminal".into();
                    }
                }
            } else if *kind == id!(CodeTab) {
                if let Some(editor) = dock.item(*id).borrow::<StudioCodeEditor>() {
                    card.detail = editor.status();
                }
                if let Some(error) = self.code_errors.get(&id.0) {
                    card.detail = format!("File unavailable: {error}");
                }
                if let Some(path) = self.code_tabs.get(&id.0) {
                    card.title = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    card.detail = format!("{} · {}", card.detail, path.display());
                }
            } else if *kind == id!(DesignTab) {
                if let Some(design) = self.designs.iter().find(|d| d.id == id.0) {
                    card.detail = design.detail.clone();
                    card.parent = design.parent.or(Some(project));
                    let body = dock.item(*id);
                    body.label(cx, ids!(design_detail))
                        .set_text(cx, &design.detail);
                    body.label(cx, ids!(design_path)).set_text(
                        cx,
                        &design
                            .path
                            .as_ref()
                            .map(|p| format!("Double-click card header to open {p}"))
                            .unwrap_or_default(),
                    );
                }
            }
            cards.push(card);
        }
        // The observer also sees Studio's own quota/inventory helpers. Only
        // terminal-owned jobs belong to the work graph; keep full diagnostics
        // in the separate Activity utility.
        let work_pids = self
            .activity_snapshot
            .terminal_descendants(terminal_pids.keys().copied());
        let running = self
            .activity_snapshot
            .processes
            .iter()
            .filter(|p| p.state == ProcessState::Running && work_pids.contains(&p.pid))
            .count();
        cards[0].detail = format!("{}\n\n{} live processes · {} open files\n\nTerminals, code and connected system designs. Open Activity for observation details.", self.project_dir().display(), running, self.code_tabs.len());
        if !self.activity_snapshot.errors.is_empty() {
            cards[0].detail.push_str(&format!(
                "\n\nObservation: {}",
                self.activity_snapshot.errors.join("; ")
            ));
        }
        let process_ids: HashMap<u32, u64> = self
            .activity_snapshot
            .processes
            .iter()
            .filter(|p| work_pids.contains(&p.pid) && !terminal_pids.contains_key(&p.pid))
            .map(|p| {
                (
                    p.pid,
                    LiveId::from_str_num("studio-process", p.pid as u64).0,
                )
            })
            .collect();
        for p in &self.activity_snapshot.processes {
            if !process_ids.contains_key(&p.pid) {
                continue;
            }
            let kind = process_kind(&p.command);
            let running = p.state == ProcessState::Running;
            let title = format!(
                "{} · {}",
                if running { "Running" } else { "Exited" },
                p.command
                    .split_whitespace()
                    .next()
                    .unwrap_or("process")
                    .rsplit('/')
                    .next()
                    .unwrap_or("process")
            );
            let mut card = Card::new(process_ids[&p.pid], kind, title);
            card.parent = Some(
                terminal_pids
                    .get(&p.parent_pid)
                    .copied()
                    .or_else(|| process_ids.get(&p.parent_pid).copied())
                    .unwrap_or(project),
            );
            card.detail = format!(
                "{}\n\nPID {} · parent {}\n{}",
                p.command,
                p.pid,
                p.parent_pid,
                if running {
                    "Observed running; output is in its terminal"
                } else {
                    "No longer observed; exit code and outcome unknown"
                }
            );
            cards.push(card);
        }
        cards.extend(self.command_cards.iter().cloned());
        // Closing a terminal removes the live ownership edge, not its history.
        let ids: std::collections::HashSet<_> = cards.iter().map(|c| c.id).collect();
        for c in &mut cards {
            if c.parent.is_some_and(|p| !ids.contains(&p)) {
                c.parent = Some(project);
            }
        }
        // Restored design parents may refer to a deleted item or a cycle; the
        // registry validator rejects cycles, so prune malformed restored edges.
        for i in 0..cards.len() {
            let mut seen = std::collections::HashSet::new();
            let mut next = Some(cards[i].id);
            while let Some(id) = next {
                if !seen.insert(id) {
                    cards[i].parent = Some(project);
                    break;
                }
                next = cards.iter().find(|c| c.id == id).and_then(|c| c.parent);
            }
        }
        if let Some(mut surface) = self
            .ui
            .widget(cx, ids!(workspace))
            .borrow_mut::<StudioSurface>()
        {
            if let Err(e) = surface.update_cards(cx, cards) {
                log!("studio workspace: {e}");
            }
        }
        self.refresh_canvas_controls(cx);
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
        if let Some((&id, _)) = self.code_tabs.iter().find(|(_, p)| {
            **p == path
                || self
                    .documents
                    .get(p)
                    .zip(self.documents.get(&path))
                    .is_some_and(|(a, b)| a.same_document(&b))
        }) {
            if focus {
                self.ui.dock(cx, ids!(dock)).select_tab(cx, LiveId(id));
                self.active_item = Some(id);
                self.focus_canvas_item(cx, id);
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
        if focus {
            self.focus_canvas_item(cx, id.0);
        }
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
            self.documents.remove(&path)?;
            if let Some(worker) = &mut self.document_worker {
                worker.unwatch(&path);
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
        if document.has_conflict() {
            return Err(
                "Disk changed while this buffer has edits; resolve the conflict before saving"
                    .into(),
            );
        }
        let id = self
            .document_worker
            .as_mut()
            .ok_or("Document reader unavailable")?
            .save(
                document.path().to_owned(),
                document.disk_text(),
                Arc::new(document.current_text()),
            )?;
        Ok(format!(
            "Save {id} queued; read_code reports dirty=false only after the write is acknowledged"
        ))
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
                    if let (Some(doc), Some(text), Some(revision)) =
                        (self.documents.get(&save.path), save.text, save.revision)
                    {
                        doc.save_succeeded(text, revision);
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
        let snapshots = self
            .document_worker
            .as_mut()
            .map(DocumentWorker::poll)
            .unwrap_or_default();
        let mut changed = false;
        for snapshot in snapshots {
            match self.documents.apply_snapshot(&snapshot) {
                Ok(handle) => {
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
                                if let Some(worker) = &mut self.document_worker {
                                    worker.unwatch(&path);
                                }
                            }
                            self.ui.dock(cx, ids!(dock)).close_tab(cx, LiveId(id));
                            if self.active_item == Some(id) {
                                self.active_item = Some(existing);
                                self.ui
                                    .dock(cx, ids!(dock))
                                    .select_tab(cx, LiveId(existing));
                                self.focus_canvas_item(cx, existing);
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
        }
    }
    fn drain_activity(&mut self, cx: &mut Cx) {
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
fn process_kind(command: &str) -> CardKind {
    let program = command
        .split_whitespace()
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("");
    if program == "codex" && command.split_whitespace().nth(1) == Some("app-server") {
        // A CLI service process does not establish a running model turn.
        CardKind::Run
    } else if ["claude", "codex", "fable", "aider", "opencode"].contains(&program) {
        CardKind::Agent
    } else if command.contains("cargo test")
        || command.contains("pytest")
        || command.contains("vitest")
    {
        CardKind::Test
    } else if command.contains("/target/release/") || command.contains("/target/debug/") {
        CardKind::App
    } else {
        CardKind::Run
    }
}
