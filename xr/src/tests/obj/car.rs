#[test]
fn car_stick_axes_map_forward_and_turn() {
    let (throttle, steer) = car_stick_axes(vec2f(0.0, -1.0), CarDriveConfig::default());
    assert!(throttle > 0.99);
    assert!(steer.abs() < 0.0001);
}

#[test]
fn car_command_uses_right_stick_for_steer_and_triggers_for_signed_throttle() {
    let command = car_drive_command(
        WidgetUid(7),
        None,
        vec2f(1.0, -1.0),
        1.0,
        0.5,
        CarDriveConfig::default(),
    )
    .unwrap();

    assert!(command.throttle > 0.50);
    assert!(command.steer > 0.99);
    assert_eq!(command.brake, 0.0);
}
