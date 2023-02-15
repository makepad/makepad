use bender_geometry::linear_path::Command;
use bender_geometry::mesh::Writer;
use bender_geometry::{Mesh, Point};
use bender_tessellator::Tessellator;

#[test]
fn test_polygon() {
    let mut mesh = Mesh::new();
    Tessellator::new().tessellate(
        [
            Command::MoveTo(Point::new(0.49409294, 0.38215515)),
            Command::LineTo(Point::new(0.4404721, 0.3451575)),
            Command::LineTo(Point::new(0.44068092, 0.3426628)),
            Command::LineTo(Point::new(0.49409294, 0.38215515)),
            Command::Close,
        ]
        .iter()
        .cloned(),
        &mut Writer::new(&mut mesh),
        &mut Vec::new(),
        &mut Vec::new(),
    );
}
