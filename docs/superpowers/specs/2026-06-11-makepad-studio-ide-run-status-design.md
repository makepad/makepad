# Spec: Makepad Studio IDE Project Status Indicators

## 1. Goal
Provide real-time project compilation and running state feedback in the Makepad Studio sidebar project list, making the workspace behave and feel more like a professional IDE.

## 2. Constraints & Scope
- All states must use existing backend/protocol events: `BuildStarted` and `AppStarted` and `BuildStopped` and `BuildCleared`.
- State tracking must leverage the existing `AppData::run_tab_state` map.
- No compat shims or compatibility aliases will be left behind; we are cleanly integrating status to row items.

## 3. UI/UX Specifications

### 3.1 Sidebar Run List States
Each row in the sidebar Run list (`DesktopRunList`) will display a colored text status pill next to the project name when active:
- **`BUILDING` State**:
  - Text: `"BUILDING"`
  - Background Color: `#x3a2e1d` (dark yellow/orange)
  - Text Color: `#xe2c08d` (light yellow/orange)
- **`RUNNING` State**:
  - Text: `"RUNNING"`
  - Background Color: `#x1c352d` (dark green)
  - Text Color: `#x89ca78` (light green)
- **`IDLE` / `STOPPED` State**:
  - No badge visible (keeps row clean).

### 3.2 Status Transitions
- Triggered by `BuildStarted`: package state becomes `"building"`.
- Triggered by `AppStarted`: package state becomes `"running"`.
- Triggered by `BuildStopped`: package state becomes `"stopped"`.
- Triggered by `BuildCleared`: package state is removed.

## 4. Proposed Changes

### 4.1 `studio/desktop/src/app_messages.rs`
- In `HubToClient::BuildStarted` match arm, change the initial status of the running tab state to `"building"`.
- Handle `HubToClient::AppStarted { build_id }`:
  - Locate the corresponding tab ID from `self.data.run_tab_by_build`.
  - Update `state.status` to `"running"`.
  - Redraw the tab in the dock.
  - Refresh the run list by calling `self.refresh_active_mount_run_list(cx)`.

### 4.2 `studio/desktop/src/desktop_run_list.rs`
- Define `status_badge` in `RunListItem` DSL.
- In `DesktopRunList::draw_entries`:
  - Check `data.run_tab_state` for a state matching `entry.name` and the active mount.
  - Set badge visibility, text, background color, and text color based on the status value (`"building"` vs `"running"`).

## 5. Verification Plan
1. Launch Makepad Studio.
2. Select an example target (e.g. `makepad-example-todo`).
3. Click "Play" to compile and run.
4. Verify that the row in the sidebar shows the yellow `BUILDING` badge.
5. Verify that once compilation completes and the app starts, the badge transitions to the green `RUNNING` badge.
6. Stop/clear the build, and verify that the badge is hidden.
