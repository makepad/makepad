#[test]
fn stick_axes_map_forward_and_turn_in_local_space() {
    let (forward, turn) = stick_deadzone_scaled_axes(vec2f(0.0, 1.0), 0.16, 1.45);
    assert!((forward + 1.0).abs() < 0.0001);
    assert!(turn.abs() < 0.0001);
}

#[test]
fn stick_axes_turn_in_place_with_horizontal_input() {
    let (forward, turn) = stick_deadzone_scaled_axes(vec2f(1.0, 0.0), 0.16, 1.45);
    assert!(forward.abs() < 0.0001);
    assert!((turn + 1.0).abs() < 0.0001);
}

#[test]
fn stick_axes_soften_partial_input_response() {
    let linear = deadzone_scaled_axis(0.5, 0.16, 1.0);
    let softened = deadzone_scaled_axis(0.5, 0.16, 1.75);

    assert!(softened > 0.0);
    assert!(softened < linear);
}

#[test]
fn tank_command_turns_for_positive_turn_input() {
    let pose = Pose::default();
    let config = TankDriveConfig::default();
    let command = tank_drive_command(
        WidgetUid(1),
        pose,
        None,
        &XrController {
            stick: vec2f(1.0, 0.0),
            ..XrController::default()
        },
        config,
    )
    .unwrap();

    assert!(command.target_angvel.y < 0.0);
    assert!(
        (command.target_angvel.y + config.turn_gain * config.max_yaw_speed_radps).abs() < 0.0001
    );
}

#[test]
fn tank_command_moves_forward_for_negative_stick_y() {
    let pose = Pose::default();
    let config = TankDriveConfig::default();
    let command = tank_drive_command(
        WidgetUid(1),
        pose,
        None,
        &XrController {
            stick: vec2f(0.0, -1.0),
            ..XrController::default()
        },
        config,
    )
    .unwrap();

    assert!(command.target_linvel.z < 0.0);
    assert!(command.target_angvel.length() < 0.0001);
}

#[test]
fn tank_command_preserves_vertical_velocity() {
    let pose = Pose::default();
    let config = TankDriveConfig::default();
    let command = tank_drive_command(
        WidgetUid(1),
        pose,
        None,
        &XrController::default(),
        config,
    )
    .unwrap();

    assert!(command.preserve_vertical_linvel);
}
