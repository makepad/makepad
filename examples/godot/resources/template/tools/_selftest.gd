extends CharacterBody2D

## Smoke test for the agent harness: proves input injection, deterministic
## physics and frame capture all work. Not part of the game.

const SPEED := 300.0
const JUMP_VELOCITY := -650.0
const GRAVITY := 1600.0


func _physics_process(delta: float) -> void:
	if not is_on_floor():
		velocity.y += GRAVITY * delta
	if Input.is_action_just_pressed("ui_accept") and is_on_floor():
		velocity.y = JUMP_VELOCITY
	velocity.x = Input.get_axis("ui_left", "ui_right") * SPEED
	move_and_slide()
