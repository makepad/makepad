extends Node

## Agent harness. Boots a target scene, replays a deterministic input tape, and
## prints probe state to stdout so a capture run is machine-readable.
##
##   godot --scene res://tools/harness.tscn --write-movie .agent/cap/f.png \
##         --fixed-fps 60 --quit-after 120 \
##         -- --target res://scenes/level.tscn --tape res://tools/tapes/jump.json

const DEFAULT_PROBE_EVERY := 15

var _events: Array = []
var _probe_names: PackedStringArray = []
var _probe_every := DEFAULT_PROBE_EVERY
var _frame := 0
var _target: Node = null


func _ready() -> void:
	# Captures share the desk with the kid's game: this window must never take
	# focus or cover the screen. `gd shot` already launches us in the background
	# (`open -g`); the flag and the offscreen position are defense in depth.
	var wid := DisplayServer.MAIN_WINDOW_ID
	DisplayServer.window_set_flag(DisplayServer.WINDOW_FLAG_NO_FOCUS, true, wid)
	DisplayServer.window_set_position(Vector2i(20000, 20000), wid)

	var args := _cmdline()

	var target_path: String = args.get("target", "")
	if target_path.is_empty():
		push_error("harness: --target res://scene.tscn is required")
		get_tree().quit(2)
		return

	var packed: PackedScene = load(target_path)
	if packed == null:
		push_error("harness: failed to load %s" % target_path)
		get_tree().quit(2)
		return

	_target = packed.instantiate()
	add_child(_target)
	print("[harness] target=%s" % target_path)

	if args.has("tape"):
		_load_tape(args["tape"])
	if args.has("probe_every"):
		_probe_every = int(args["probe_every"])


# Runs before the target's own _physics_process: parents tick before children.
func _physics_process(_delta: float) -> void:
	for e in _events:
		if int(e.get("f", -1)) != _frame:
			continue
		if e.has("press"):
			_set_action(e["press"], true)
		if e.has("release"):
			_set_action(e["release"], false)

	if _probe_every > 0 and _frame % _probe_every == 0:
		_print_probe()

	_frame += 1


func _set_action(action: String, pressed: bool) -> void:
	if not InputMap.has_action(action):
		push_error("harness: unknown input action '%s'" % action)
		return
	if pressed:
		Input.action_press(action)
	else:
		Input.action_release(action)
	print("[harness] f=%d %s %s" % [_frame, "press" if pressed else "release", action])


func _print_probe() -> void:
	for n in _probe_names:
		var node: Node = _target if _target.name == n else _target.find_child(n, true, false)
		if node == null:
			print("[probe] f=%d %s MISSING" % [_frame, n])
			continue
		if node is Node2D:
			var p: Vector2 = (node as Node2D).global_position
			var line := "[probe] f=%d %s pos=(%.1f, %.1f)" % [_frame, n, p.x, p.y]
			if node is CharacterBody2D:
				var b := node as CharacterBody2D
				line += " vel=(%.1f, %.1f) floor=%s" % [b.velocity.x, b.velocity.y, b.is_on_floor()]
			print(line)


func _load_tape(path: String) -> void:
	if not FileAccess.file_exists(path):
		push_error("harness: tape not found %s" % path)
		return
	var data: Variant = JSON.parse_string(FileAccess.get_file_as_string(path))
	if typeof(data) != TYPE_DICTIONARY:
		push_error("harness: malformed tape %s" % path)
		return
	_events = data.get("events", [])
	for p in data.get("probe", []):
		_probe_names.append(str(p))
	print("[harness] tape=%s events=%d" % [path, _events.size()])


func _cmdline() -> Dictionary:
	var out := {}
	var argv := OS.get_cmdline_user_args()
	var i := 0
	while i < argv.size():
		if argv[i].begins_with("--") and i + 1 < argv.size():
			out[argv[i].substr(2)] = argv[i + 1]
			i += 2
		else:
			i += 1
	return out
