# makepad-xr-ui

Headset UI for makepad apps that stream their XR view to a headset. Every app gets the same
wrist menu, cards, pointing, haptic ticks and grab-to-edit.

`examples/xr` shows all of it end to end in one file.

## Adding XR to an app

1. **Crates.**
   - `makepad-xr-stream`: the XR server, the caption control and the desktop mirror.
   - `makepad-xr-nav`: move, turn and scale the scene with the grips and sticks.
   - `makepad-xr-ui`: this crate.

   The headset runs the `apps/xr_remote` viewer. The app itself never builds for the headset.

2. **Register the script modules** in `AppMain::script_mod`:

   ```rust
   makepad_xr_stream::script_mod(vm);
   makepad_xr_stream::serve_script_mod(vm);
   makepad_xr_ui::script_mod(vm);
   ```

3. **Put the caption control in the window's caption:**
   `xr := XrCaptionButton{app: "my-app"}`. It starts the app's XR server and pairs a headset
   by its PIN. When a headset links, the process gets an XR display (`cx.xr_display()`).
   Optionally, an `XrMirrorView{}` somewhere shows the headset's left eye.

4. **Write a widget that draws the XR pass.** It can be 1x1 in the caption.
   - Once `cx.xr_display()` is true, make a `DrawPass` with colour and depth textures.
   - Call `set_as_xr_pass`, then `cx.xr_start_presenting()`, plus
     `cx.xr_set_see_through(true)` for mixed reality.
   - Each `Event::XrUpdate` carries the head, controllers and hands. Step your UI and edits
     from it, feed `XrNav::update` what is left, then redraw.
   - When drawing, begin the pass and render the panels' textures inside it
     (`menu.render(cx)`). Then draw the scene in a `Cx3d`, with the cards last
     (`menu.draw(cx3d)`, `panel.draw(cx3d, &card)`). The server takes the frame when the pass
     ends.

## What the app supplies

- **Menu rows.** Declare `#[live] menu: XrWristMenu`. Each frame, call
  `menu.panel.set(key, |rows| ...)` with a key for what the rows show. The rows are written
  only when the key changes:
  - `row(Height::Fixed(52.0))` or `row(Height::Fill)` (for lists);
  - then cells: `button`, `toggle`, `entry` (pressable text in a `Tone`), `text`,
    `heading`, `blank` and `slider(id, value, width)` (value in 0..1), each with a `Width`.
    Update a slider with `panel.set_slider_value(id, value)` without changing the rows' key:
    its track and knob are geometry and do not repaint the text texture.

  `menu.step(state, &buttons, now)` opens the menu, places it and finds the hover. It returns
  the cell pressed this frame and the hover and press haptics. `MenuStep::slider` reports
  `SliderDrag { id, value, ended }`: trigger/index pinch captures a slider until release,
  following the aim or touching fingertip even outside the cell, clamped to its ends.
  Release reports `ended: true`; travel ticks every 5% with `haptic::HOVER`.

  The menu opens with `MenuOpen::Button(XrController::CLICK_Y)` by default. Apps whose buttons
  all belong to a game use `MenuOpen::WristTurn` instead. With tracked hands, the menu shows
  while the left palm is open and turned up.

- **Cards.** Declare `#[live] card: XrPanel` with its texel size and width in metres
  (`XrPanel{height: 128.0 meters: 0.12}`). Write its rows the same way, then place it:
  - `Card::beside(hand, head, size)`: next to a hand;
  - `Card::facing(at, head, size)`: turned to the head;
  - `Card::pinned(at, yaw, size)`: fixed in the room.

  `XrPanel::draw_in` takes a room-to-scene matrix, for passes whose scene is not the room.

- **Edits.** Implement `EditSink<YourEdit>` on the app's edit path. One commit is one undo
  step. Keep a `GrabEdits` for the items the hands move:
  - When a grab ends, call `commit(items, edit, sink)`.
  - Once the edit has a job id, call `sent(job)`. A synchronous sink calls it with the
    document's new version.
  - When the document is rebuilt, call `settle(job, held, settled)` so items stop showing
    their local move.
  - Until then, `waiting(i)` says an item is still on its way.

- **Haptics.** Use `haptic::{HOVER, PRESS, GRAB, SNAP, LIMIT}`. Call `.play(cx, hand)` for one
  tick, or collect a frame's ticks in `Haptics` and call `play` once.

- **Input.** `input::{aim, tip, trigger, grab}` work for controllers and tracked hands.
  The controller interaction point is its aim pose, with no added offset (see `input`'s
  OpenXR pose definitions). A hand touches at its index fingertip and grabs at its
  thumb/index pinch midpoint. Either controller trigger or grip holds a grab, with
  press/release hysteresis; releasing both drops it. `place_tip` puts synthetic input
  at exactly the requested point; its illustrative palm spacing affects synthetic data only.

  Keep an `InteractionFilter` and call `apply(state, state.time)` once per input update.
  Use that copy for pointing, hover and dragging; keep the incoming state for rendering
  controllers/hands and for navigation. The timestamp follows the display grid, avoiding
  UI/network arrival jitter in the filter's time step. Tracking loss and jumps reset it.

  Declare `#[live] tips: XrTips`, call `tips.set_input(state)` to distinguish controllers
  from tracked hands, then `tips.draw(cx3d, input::tips(filtered), touching)` (or `draw_in`).
  Controllers show a 6 cm tapered stylus with its apex exactly at the interaction point;
  touching lights and lengthens it toward the controller. A bright 3 mm dot marks the
  apex. Tracked hands show only that dot at the fingertip.

  Step one `Buttons` per frame. With draggables, use `menu.prepare(state, &buttons, now)`
  to place/hit-test the menu, let touched draggables claim each hand via `buttons.claim(i)`,
  then use `menu.interact(state, &buttons, &mut step)`. Existing captures own the hand
  through release; a new near drag wins over a button or selection from the same press.
  An existing menu slider reserves its hand before new drags. Advance all edge detectors
  on the original button values even while claimed; gate actions on `buttons.claimed(i)`.
  Apps without draggables can keep using `menu.step`. `mask_grab` clears both controller
  grab controls and the index pinch for downstream consumers that do not track edges.
  Pass raw grips to navigation except for hands actually captured by edits; empty-space
  grip navigation is unchanged.

Panel colours come from the system theme: `color_bg_app`, `color_outset`, `color_focus` for
toggles that are on, `color_text`, and the error, warning, info and success colours for tones.
