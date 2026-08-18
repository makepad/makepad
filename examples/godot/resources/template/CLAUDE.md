# testgame — a kid's 2D platformer

Godot 4.7. A child asks for changes by voice through the Game Maker app; you make them.

## Layout

- `scenes/main.tscn` — the game. Set as `run/main_scene` in `project.godot`.
- `scripts/` — GDScript.
- `tools/` — the agent harness. Not part of the game; don't ship it in the scene tree.

## Seeing what you built

The kid keeps playing the last version that worked for your WHOLE turn. The Game
Maker app relaunches their game with your changes only when you finish — so test
before you finish, and never start, stop or restart the game yourself: that would
yank it out of the kid's hands.

To test your edits, run a background capture — it plays the edited game in an
invisible instance (never takes focus), optionally replaying an input tape frame
by frame:

```
./tools/gd shot res://scenes/main.tscn 120                       # 2 seconds, no input
./tools/gd shot res://scenes/main.tscn 120 res://tools/tapes/selftest.json
```

That writes `.agent/sheet.png` (a grid of frames) and prints the probed node's
state every 15 frames, so "the jump feels floaty" is a number you can check, not
a guess. **Read that image before finishing your turn.**

Captures are deterministic (`--fixed-fps`), so the same tape gives the same frames.

To see what the kid sees right now — their running game, which does NOT include
your latest edits until the restart — peek at it:

```
./tools/gd peek
```

That asks the running game itself (the `AgentEye` autoload) for four screenshots
over ~1.2s and writes the same `.agent/sheet.png` plus the player's position,
velocity and `is_on_floor()`. The kid keeps playing while you look. Use it to
understand what the kid is talking about, not to check your own work.

An input tape scripts the controls by frame number:

```json
{ "probe": ["Player"],
  "events": [{ "f": 5, "press": "ui_right" }, { "f": 30, "press": "ui_accept" }] }
```

`./tools/gd errors` shows script errors from the last run.

## House style

- The player is a `CharacterBody2D`; ground and platforms are `StaticBody2D`.
- Art is `ColorRect` / `Polygon2D`. No external image files.
- Small, visible changes. Tune constants and add nodes; avoid big rewrites.
- Use the built-in actions `ui_left`, `ui_right`, `ui_accept` so tapes keep working —
  and never raw key checks (`Input.is_key_pressed`). The actions carry keyboard AND
  game-controller bindings (D-pad, left stick; `AgentEye` adds the A button to
  `ui_accept`), so every game stays playable with a gamepad for free.

## Gotchas found the hard way

- `--headless` **crashes** with `--write-movie`: the dummy renderer produces no
  frames. Captures need a real window, which is what `tools/gd shot` does.
- Movie Maker records at the project's viewport size and ignores `--resolution`.
- `.tscn` changes need a game restart; `.gd` changes hot-reload in the open editor.
