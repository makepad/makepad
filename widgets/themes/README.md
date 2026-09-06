# Application styles

These Splash files define the shared widget styles used by the window manager:
`omarchy`, `macos`, `macos-dark`, `windows`, `windows-dark`, `windows-2000`,
and `nextstep`.

Each style has two phases:

- `theme.splash` installs semantic roles, typography, spacing, radii, and bevels
  before stock widgets are registered.
- `widgets.splash` applies component geometry and materials after registration.
  It can replace a widget's shader as well as its properties. Windows 2000's
  raised buttons and sunken edit fields are examples.

The loader embeds both files for installed/wasm builds. In a native source
checkout it reads `widgets/themes/<style>/` on each selection. Edit either file
and choose the same style again from the WM's style picker to hotload it.
End a generated assignment-only Splash module with `true` so its last
assignment is evaluated as a statement.

The WM keeps its Omarchy top bar. Click the style name to cycle, or right-click
it to choose directly. macOS and Windows also have a Light/Dark button. Its Applications
launcher, Windows Start menus, window chrome, and hosted apps use the selected
appearance.

`desktop_style::StyleSheet` carries both phases to existing process clients
through `StudioToApp::Custom`. `Window` receives the message and requests an
application Splash reload with `Apply::ScriptReapply`. Module clients re-evaluate
their registrations in their existing isolates with the same apply mode.
Widget identities, editable text, and application Rust state are retained.
Initialize persistent script models only when `!vm.is_reload()`; read the existing
`mod.state` from application Splash. See `examples/counter/src/main.rs`. Dynamic
`View.on_render` children and embedded Splash isolates reapply the style too.
Embedded `Splash` widgets inherit the stylesheet and reapply their existing
widget tree in their own isolate.

Applications should inherit the stock component styles and read `theme.*` for
custom drawing. Apps using the older WM palette adapter can read
`makepad_wm_theme::current_for_vm` during registration, or from their owning VM
through `Cx::with_vm`. Avoid process-wide immutable palette caches: appearances
can change while an app is open. Explicit app overrides still take precedence.

The framebuffer crossfade lives in `apps/wm/src/scene.rs`. It freezes the old
GPU render target while the live scene renders the new style; it does not take
screenshots, rebuild app instances, or put transitions in the widget system.
Shell geometry interpolates over the same 650 ms interval. Floating desktop
rectangles are stored separately from the Omarchy tiling tree, so switching back
restores the tiling arrangement.

The macOS dock overlays the desktop; dragging keeps the title bar reachable but
allows window bodies behind the dock or partly offscreen. Terminals render only
their background with opacity (`MAKEPAD_WM_TERM_OPACITY`, default `0.78 0.70` for
focused/unfocused); text and ANSI cell colors remain crisp. Their foreground and
background follow the active light/dark stylesheet without restarting the PTY.

`widgets/src/backdrop.rs` supplies ordered compositor checkpoints. Windows are
composited from back to front, and a glass surface samples a checkpoint below it.
Disjoint sampling footprints share a Gaussian stack, including the kernel's
support outside the visible surface. Intervening opaque or translucent content
that overlaps the footprint starts a new checkpoint. The dock is the final
consumer. Requested blur levels are combined before drawing the pyramid, so
unused deeper passes are skipped. Explicit producer/consumer links preserve GPU
ordering when pass IDs are recycled. `MAKEPAD_WM_TRACE_BLUR=1` logs stack and pass
counts when they change; normal operation does not log each frame.

In a checkout, launch the WM with `cargo run --release -p makepad-wm -- --remote`.
Applications launch on demand with `cargo run --release -p <package>`; there is
no binary collection to prepare. The app's window shows Cargo's compiling or
build-wait stage before the process connects, then fades into its first frame.
Installed distributions without a source checkout use sibling executables.

NeXTSTEP uses black focused title bars, gray beveled window controls, square
widgets, a right-hand vertical application dock, and its own icon family. Its
launcher opens a draggable Workspace palette with attached submenu columns.
Hold the secondary mouse button on the desktop or a window title bar, drag
through the menu and release to choose a command. The popup follows the pointer
and opens submenus to the left near the screen edge. The Omarchy top bar
and shared application state remain in place while switching styles.

Application icons use `AppIcon{name: "files" width: 32 height: 32}`. The default
`style: "auto"` follows the application's Splash stylesheet; standalone apps use
the host OS family. An explicit style ID is available for galleries. `color`
retints the Omarchy artwork; `opacity` preserves the other families' own colors.
Window captions derive their icon name from `window.app_id` (or the binary name),
and the WM's dock/taskbar/launcher use the same `AppIconDraw` renderer.

Each family has editable `icons/<app-id>.svg` assets. macOS dark and light share
app artwork, as on the OS. Unknown IDs use that family's `app.svg`. Add or edit
an SVG and reselect the style to hotload it; the complete icon sources travel in
the stylesheet to existing process and module apps. Unchanged sources keep their
parsed geometry cache. `python3 widgets/themes/build_icons.py` rebuilds the
bundled artwork families using only Python's standard library.

Use original application symbols within each OS's visual style. Files uses a
folder with documents, and Browser uses a web window with a globe; do not use
the Finder face or Safari compass artwork.

Application surfaces should use the shared `theme` roles (or app-specific aliases
that resolve to those roles), including `corner_radius` and
`container_corner_radius`. Keep content colors such as chart series and model
axes separate. Filled accent actions use `color_text_on_accent`.

Mark live fields that contain runtime UI state with `#[apply_state]` alongside
`#[live]`: stylesheet `ScriptReapply` preserves these values, while explicit
`Eval` edits and ordinary source reloads still apply. Standard panel visibility,
checkbox state, and dropdown selection use this path.

On macOS, the WM captures the complete window into a GPU texture and warps
that surface into its application icon in the dock. Minimize and restore share
the same reversible progress, without resizing or relaying out the application
during the animation. A style change invalidates minimized snapshots so the
restored window uses the current theme. `MAKEPAD_WM_TRACE_WARP=1` enables the
capture diagnostics using ordinary logs. For frame inspection,
`MAKEPAD_WM_WARP_SECONDS=5` slows the default 0.62-second animation.
