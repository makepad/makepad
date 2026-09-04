use makepad_flowgraph::wire_route::{
    obstacles_in_corridor, route_wire, route_wire_sticky, Obstacle, Point, PortSide, RouteKind,
    RouteStyle, WireRoute,
};

const CLEARANCE: f64 = 12.0;

fn screenshot_cards(prompt_y: f64) -> [Obstacle; 5] {
    [
        Obstacle::from_xywh(-1359.0, prompt_y, 290.0, 150.0),
        Obstacle::from_xywh(-955.0, -422.0, 290.0, 200.0),
        Obstacle::from_xywh(-552.0, -554.0, 290.0, 200.0),
        Obstacle::from_xywh(-567.0, -885.0, 290.0, 200.0),
        Obstacle::from_xywh(-1210.0, -966.0, 290.0, 150.0),
    ]
}

fn assert_port_directions(route: &WireRoute, source: PortSide, target: PortSide) {
    let RouteKind::Orthogonal { points, .. } = &route.kind else {
        panic!("expected orthogonal geometry");
    };
    let sign = |side| if side == PortSide::Right { 1.0 } else { -1.0 };
    assert_eq!(points[1].y, route.from.y, "{:?}", route.kind);
    assert_eq!((points[1].x - route.from.x).signum(), sign(source));
    let last = points[points.len() - 2];
    assert_eq!(last.y, route.to.y, "{:?}", route.kind);
    assert_eq!((last.x - route.to.x).signum(), sign(target));
}

fn assert_short(route: &WireRoute) {
    let manhattan = (route.to.x - route.from.x).abs() + (route.to.y - route.from.y).abs();
    assert!(
        route.length() <= manhattan + 2.0 * CLEARANCE,
        "length={} manhattan={manhattan} {:?}",
        route.length(),
        route.kind
    );
}

fn assert_outside_cards(route: &WireRoute, cards: &[Obstacle]) {
    for step in 0..=(route.length().ceil() as usize * 4) {
        let point = route.point_at_distance(step as f64 * 0.25);
        for card in cards {
            assert!(
                point.x <= card.min.x
                    || point.x >= card.max.x
                    || point.y <= card.min.y
                    || point.y >= card.max.y,
                "point={point:?} card={card:?} {:?}",
                route.kind
            );
        }
    }
}

fn screenshot_fixture(mirror_y: bool, target_side: PortSide) {
    // Test both card-edge ports and the actual canvas anchor offsets:
    // output tip = 13 + 4.5; input notch = 13 - 3; first port row = 26.
    for (source_offset, target_offset) in [(0.0, 0.0), (17.5, 10.0)] {
        let mut cards = screenshot_cards(30.0);
        let from = Point::new(cards[0].max.x + source_offset, cards[0].min.y + 26.0);
        let target_x = if target_side == PortSide::Left {
            cards[1].min.x - target_offset
        } else {
            cards[1].max.x + target_offset
        };
        let to = Point::new(target_x, cards[1].min.y + 26.0);
        let reflect = |p: Point| Point::new(p.x, if mirror_y { -p.y } else { p.y });
        let from = reflect(from);
        let to = reflect(to);
        if mirror_y {
            for card in &mut cards {
                let (min, max) = (reflect(card.max), reflect(card.min));
                card.min.y = min.y;
                card.max.y = max.y;
            }
        }
        let obstacles: Vec<_> = cards.iter().map(|card| card.inflate(CLEARANCE)).collect();
        // Auto-flip/preview use every card; per-frame drawing uses the corridor.
        let style = RouteStyle::default();
        let local = obstacles_in_corridor(
            from,
            to,
            &obstacles,
            style.port_stub + style.corner_radius * 2.0,
        );
        for obstacles in [&obstacles, &local] {
            let route = route_wire(
                from,
                PortSide::Right,
                to,
                target_side,
                obstacles,
                style,
                0.0,
            );
            assert_short(&route);
            assert_port_directions(&route, PortSide::Right, target_side);
            assert_outside_cards(&route, &cards);
            for point in route.slice(0.0, route.length()) {
                assert!(
                    if mirror_y {
                        point.y <= cards[1].max.y
                    } else {
                        point.y >= cards[1].min.y
                    },
                    "{:?}",
                    route.kind
                );
                let max_x = if target_side == PortSide::Left {
                    cards[1].max.x
                } else {
                    to.x + CLEARANCE
                };
                assert!(point.x <= max_x, "{:?}", route.kind);
            }
        }
    }
}

#[test]
fn screenshot_prompt_below_left_of_expand_does_not_wrap() {
    screenshot_fixture(false, PortSide::Left);
}

#[test]
fn screenshot_mirrored_prompt_above_left_of_expand_does_not_wrap() {
    screenshot_fixture(true, PortSide::Left);
}

#[test]
fn screenshot_flipped_expand_uses_only_its_near_corner() {
    screenshot_fixture(false, PortSide::Right);
}

#[test]
fn screenshot_expand_to_add_style_stays_between_its_ports() {
    let cards = screenshot_cards(30.0);
    let obstacles: Vec<_> = cards.iter().map(|card| card.inflate(CLEARANCE)).collect();
    let from = Point::new(cards[1].max.x + 17.5, cards[1].min.y + 26.0);
    let to = Point::new(cards[2].min.x - 10.0, cards[2].min.y + 26.0);
    let route = route_wire(
        from,
        PortSide::Right,
        to,
        PortSide::Left,
        &obstacles,
        RouteStyle::default(),
        0.0,
    );
    assert_short(&route);
    assert_port_directions(&route, PortSide::Right, PortSide::Left);
    assert_outside_cards(&route, &cards);
    assert!(
        route.slice(0.0, route.length()).iter().all(|point| {
            point.x >= from.x && point.x <= to.x && point.y >= to.y && point.y <= from.y
        }),
        "{:?}",
        route.kind
    );
}

#[test]
fn nearly_level_ports_keep_short_fillets_and_card_clearance() {
    for dy in [-31.0, -16.0, -4.0, -0.5, 0.5, 4.0, 16.0, 31.0] {
        for offset in [-4.0, 0.0, 4.0] {
            let cards = [
                Obstacle::from_xywh(0.0, 0.0, 290.0, 200.0),
                Obstacle::from_xywh(404.0, dy, 290.0, 200.0),
            ];
            let obstacles: Vec<_> = cards.iter().map(|card| card.inflate(CLEARANCE)).collect();
            let route = route_wire(
                Point::new(290.0, 26.0),
                PortSide::Right,
                Point::new(404.0, 26.0 + dy),
                PortSide::Left,
                &obstacles,
                RouteStyle::default(),
                offset,
            );
            assert_short(&route);
            assert_port_directions(&route, PortSide::Right, PortSide::Left);
            assert_outside_cards(&route, &cards);
            let RouteKind::Orthogonal { points, .. } = &route.kind else {
                unreachable!();
            };
            assert_eq!(points.len(), 4, "{:?}", route.kind);
        }
    }
}

#[test]
fn screenshot_drag_from_saved_position_never_takes_an_outside_row() {
    for (source_offset, target_offset) in [(0.0, 0.0), (17.5, 10.0)] {
        let mut previous = None;
        for y in -426..=30 {
            let cards = screenshot_cards(y as f64);
            let obstacles: Vec<_> = cards.iter().map(|card| card.inflate(CLEARANCE)).collect();
            let from = Point::new(cards[0].max.x + source_offset, cards[0].min.y + 26.0);
            let to = Point::new(cards[1].min.x - target_offset, cards[1].min.y + 26.0);
            let route = route_wire_sticky(
                from,
                PortSide::Right,
                to,
                PortSide::Left,
                &obstacles,
                RouteStyle::default(),
                0.0,
                previous.as_ref(),
            );
            assert_short(&route);
            assert_port_directions(&route, PortSide::Right, PortSide::Left);
            assert_outside_cards(&route, &cards);
            assert!(
                route.slice(0.0, route.length()).iter().all(|point| {
                    point.y >= from.y.min(to.y)
                        && point.y <= from.y.max(to.y)
                        && point.x <= cards[1].max.x
                }),
                "y={y} {:?}",
                route.kind
            );
            previous = Some(route);
        }
    }
}
