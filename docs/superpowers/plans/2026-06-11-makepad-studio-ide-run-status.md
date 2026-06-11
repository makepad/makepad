# IDE Project Run Status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement real-time `BUILDING` (yellow) and `RUNNING` (green) status badges on the Makepad Studio Run list sidebar rows.

**Architecture:** Modify `app_messages.rs` to set the package status to `"building"` on `BuildStarted` and transition it to `"running"` on `AppStarted`. Update `desktop_run_list.rs` to declare a status badge widget and render it dynamically on each project row.

**Tech Stack:** Rust, Makepad framework, UI Script (script_mod!)

---

### Task 1: Update State Transitions in message dispatcher

**Files:**
- Modify: `studio/desktop/src/app_messages.rs:280-290,670-682`

- [ ] **Step 1: Modify BuildStarted to set status to building**
In `studio/desktop/src/app_messages.rs`, change the initial status assigned under `HubToClient::BuildStarted` from `"running"` to `"building"`.
Replace:
```rust
                    if let Some(state) = self.data.run_tab_state.get_mut(&tab_id) {
                        state.mount = mount.clone();
                        state.package = package.clone();
                        state.status = "running".to_string();
                        window_id = state.window_id;
                    }
```
With:
```rust
                    if let Some(state) = self.data.run_tab_state.get_mut(&tab_id) {
                        state.mount = mount.clone();
                        state.package = package.clone();
                        state.status = "building".to_string();
                        window_id = state.window_id;
                    }
```

- [ ] **Step 2: Add AppStarted handler**
Add the `HubToClient::AppStarted` match arm to handle the transition to `"running"` and refresh the Run list:
```rust
            HubToClient::AppStarted { build_id } => {
                if let Some(tab_id) = self.data.run_tab_by_build.get(&build_id).copied() {
                    if let Some(state) = self.data.run_tab_state.get_mut(&tab_id) {
                        state.status = "running".to_string();
                    }
                    let mount = self.data.build_to_mount.get(&build_id).cloned();
                    if let Some(mount) = mount {
                        if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                            dock.redraw_tab(cx, tab_id);
                        }
                        self.refresh_active_mount_run_list(cx);
                    }
                }
            }
```
Insert this right before the `_ => {}` catch-all match arm (around line 680).

- [ ] **Step 3: Run compilation check**
Verify the code compiles without warnings or errors.
Run: `cargo check -p makepad-studio`
Expected: Process exits with `0` and compilation succeeds.

- [ ] **Step 4: Commit state transition changes**
Commit these changes before editing the UI:
```bash
git add studio/desktop/src/app_messages.rs
git commit -m "feat: implement building and running state transitions in app messages"
```

---

### Task 2: Declare status_badge Widget in Run list DSL

**Files:**
- Modify: `studio/desktop/src/desktop_run_list.rs:70-91`

- [ ] **Step 1: Add status_badge to RunListItem DSL**
Add the `status_badge := RoundedView` inside `mod.widgets.RunListItem` right after `row_button`.
Replace:
```rust
        row_button := ButtonFlat {
            width: Fill
            height: Fill
            align: Align {x: 0.0 y: 0.5}
            label_walk: Walk {width: Fit height: Fit}
            padding: Inset {left: 4.0 right: 0.0 top: 0.0 bottom: 0.0}
            text: ""
            draw_bg +: {
                color: #0000
                color_hover: #0000
                color_pressed: #0000
                border_color: #0000
            }
            draw_text +: {
                color: theme.color_label_inner
                color_hover: #xFFFFFF
                color_pressed: #xFFFFFF
                color_focus: #xFFFFFF
            }
        }
```
With:
```rust
        row_button := ButtonFlat {
            width: Fill
            height: Fill
            align: Align {x: 0.0 y: 0.5}
            label_walk: Walk {width: Fit height: Fit}
            padding: Inset {left: 4.0 right: 0.0 top: 0.0 bottom: 0.0}
            text: ""
            draw_bg +: {
                color: #0000
                color_hover: #0000
                color_pressed: #0000
                border_color: #0000
            }
            draw_text +: {
                color: theme.color_label_inner
                color_hover: #xFFFFFF
                color_pressed: #xFFFFFF
                color_focus: #xFFFFFF
            }
        }

        status_badge := RoundedView {
            visible: false
            width: Fit
            height: Fit
            margin: Inset {left: 4.0 right: 4.0 top: 0.0 bottom: 0.0}
            padding: Inset {left: 5.0 right: 5.0 top: 3.0 bottom: 3.0}
            draw_bg +: {
                color: #x252526
                border_radius: 3.0
            }
            label := Label {
                text: ""
                draw_text +: {
                    font_size: theme.font_size_p - 2.0
                    color: #fff
                }
            }
        }
```

- [ ] **Step 2: Verify compilation**
Run: `cargo check -p makepad-studio`
Expected: Process exits with `0`.

- [ ] **Step 3: Commit DSL changes**
```bash
git add studio/desktop/src/desktop_run_list.rs
git commit -m "feat: add status_badge widget declaration to RunListItem DSL"
```

---

### Task 3: Draw active project status badges dynamically

**Files:**
- Modify: `studio/desktop/src/desktop_run_list.rs:210-224`

- [ ] **Step 1: Modify draw_entries to render status badges**
In `studio/desktop/src/desktop_run_list.rs`, inside `draw_entries`, query `data.run_tab_state` to match the entry name and active mount. Show/hide the badge and apply custom styling (Green for `running`, Yellow/Orange for `building`).
Replace:
```rust
            let button = item.button(cx, ids!(row_button));
            button.set_text(cx, &entry.name);
            button.set_action_data(RunListRowData::RunItem {
                mount: active_mount.to_string(),
                name: entry.name.clone(),
            });
            item.draw_all(cx, &mut Scope::empty());
```
With:
```rust
            let button = item.button(cx, ids!(row_button));
            button.set_text(cx, &entry.name);
            button.set_action_data(RunListRowData::RunItem {
                mount: active_mount.to_string(),
                name: entry.name.clone(),
            });

            let badge = item.view(cx, ids!(status_badge));
            let mut status = None;
            for state in data.run_tab_state.values() {
                if state.mount == *active_mount && state.package == entry.name {
                    status = Some(state.status.clone());
                    break;
                }
            }

            if let Some(status_str) = status {
                if status_str == "building" {
                    badge.set_visible(cx, true);
                    badge.label(cx, ids!(label)).set_text(cx, "BUILDING");
                    let bg_color = Vec4f::from_hex("#x3a2e1d");
                    let txt_color = Vec4f::from_hex("#xe2c08d");
                    script_apply_eval!(cx, badge, {
                        draw_bg +: {color: #(bg_color)}
                        label: {draw_text +: {color: #(txt_color)}}
                    });
                } else if status_str == "running" {
                    badge.set_visible(cx, true);
                    badge.label(cx, ids!(label)).set_text(cx, "RUNNING");
                    let bg_color = Vec4f::from_hex("#x1c352d");
                    let txt_color = Vec4f::from_hex("#x89ca78");
                    script_apply_eval!(cx, badge, {
                        draw_bg +: {color: #(bg_color)}
                        label: {draw_text +: {color: #(txt_color)}}
                    });
                } else {
                    badge.set_visible(cx, false);
                }
            } else {
                badge.set_visible(cx, false);
            }

            item.draw_all(cx, &mut Scope::empty());
```

- [ ] **Step 2: Compile and verify code**
Run: `cargo check -p makepad-studio`
Expected: Successful check (exit 0).

- [ ] **Step 3: Commit drawing updates**
```bash
git add studio/desktop/src/desktop_run_list.rs
git commit -m "feat: dynamically draw active project status badges in run list"
```

---

### Task 4: Visual Verification

- [ ] **Step 1: Build the release binary**
Ensure a clean release build of cargo-makepad and makepad-studio:
Run: `cargo build --release -p cargo-makepad -p makepad-studio`
Expected: Build finishes successfully.

- [ ] **Step 2: Clear any running todo app build**
Clear build ID 1:
Run: `echo '{"ClearBuild":{"build_id":[1]}}' | target/release/cargo-makepad studio --studio=127.0.0.1:8001`
Expected: Build cleared response.

- [ ] **Step 3: Run the todo example**
Launch the todo example again:
Run: `echo '{"RunItem":{"mount":"makepad","name":"makepad-example-todo"}}' | target/release/cargo-makepad studio --studio=127.0.0.1:8001`
Expected: `BuildStarted` response.

- [ ] **Step 4: Take screenshot during compilation**
Immediately take a screenshot to capture the `BUILDING` status badge:
Run: `sleep 0.5 && echo '{"Screenshot":{"build_id":[1]}}' | target/release/cargo-makepad studio --studio=127.0.0.1:8001`
Expected: Screenshot path returned.

- [ ] **Step 5: Verify the app starts and takes running screenshot**
Wait for it to compile and start, then take a second screenshot:
Run: `sleep 5 && echo '{"Screenshot":{"build_id":[1]}}' | target/release/cargo-makepad studio --studio=127.0.0.1:8001`
Expected: Screenshot path returned.

- [ ] **Step 6: Inspect both screenshots**
Inspect the saved screenshots using the `read` tool to verify the yellow `BUILDING` and green `RUNNING` status badges appear correctly on the row.
