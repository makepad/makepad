# Lets the game-making agent peek at the running game without disturbing it.
#
# `tools/gd peek` drops `.agent/peek_request` into the project; this autoload
# notices within a quarter second, saves a short burst of screenshots plus the
# player's state to `.agent/live/`, and finishes with a `done` marker. The kid
# keeps playing the whole time — nothing is paused or restarted.
extends Node

const SNAPS := 4
const SNAP_GAP := 0.4

var _poll := 0.0
var _busy := false

func _ready() -> void:
	# Godot's default ui_accept knows Enter and Space but no gamepad button, so
	# a kid on a controller could walk (ui_left/right carry D-pad and stick
	# bindings) but never jump. Bind A here, where every game gets it for free.
	var jump := InputEventJoypadButton.new()
	jump.button_index = JOY_BUTTON_A
	InputMap.action_add_event("ui_accept", jump)

func _agent_dir() -> String:
	return ProjectSettings.globalize_path("res://") + ".agent"

func _process(delta: float) -> void:
	_poll += delta
	if _poll < 0.25 or _busy:
		return
	_poll = 0.0
	var request := _agent_dir() + "/peek_request"
	if FileAccess.file_exists(request):
		DirAccess.remove_absolute(request)
		_snap_burst()

func _snap_burst() -> void:
	_busy = true
	var live := _agent_dir() + "/live"
	# Start fresh so a sheet never mixes two peeks.
	if DirAccess.dir_exists_absolute(live):
		for file in DirAccess.get_files_at(live):
			DirAccess.remove_absolute(live + "/" + file)
	else:
		DirAccess.make_dir_recursive_absolute(live)

	var report := ""
	for index in range(SNAPS):
		var image := get_viewport().get_texture().get_image()
		image.save_png("%s/f%04d.png" % [live, index])
		report += _state_line(index)
		if index < SNAPS - 1:
			await get_tree().create_timer(SNAP_GAP).timeout

	var state := FileAccess.open(live + "/state.txt", FileAccess.WRITE)
	if state:
		state.store_string(report)
		state.close()
	var done := FileAccess.open(live + "/done", FileAccess.WRITE)
	if done:
		done.store_string("ok")
		done.close()
	_busy = false

func _state_line(index: int) -> String:
	var scene := get_tree().current_scene
	if scene == null:
		return "snap %d: no scene\n" % index
	var player := scene.find_child("Player", true, false)
	if player == null:
		return "snap %d: no node named Player\n" % index
	var line := "snap %d: %s pos=%s" % [index, player.name, player.position]
	if player is CharacterBody2D:
		line += " vel=%s floor=%s" % [player.velocity, player.is_on_floor()]
	return line + "\n"
