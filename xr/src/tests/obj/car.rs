#[test]
fn car_stick_axes_map_forward_and_turn() {
    let (throttle, steer) = car_stick_axes(vec2f(0.0, -1.0), CarDriveConfig::default());
    assert!(throttle > 0.99);
    assert!(steer.abs() < 0.0001);
}

#[test]
fn car_command_outputs_steer_and_throttle() {
    let command = car_drive_command(
        WidgetUid(7),
        None,
        &XrController {
            stick: vec2f(1.0, -1.0),
            ..XrController::default()
        },
        CarDriveConfig::default(),
    )
    .unwrap();

    assert!(command.throttle > 0.99);
    assert!(command.steer > 0.99);
    assert_eq!(command.brake, 0.0);
}
