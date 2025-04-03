Displays pixel images.
## Layouting
Support for [Walk](Walk.md) layout features which define how the widget positions itself in its parent container.
No Support for layouting child elements via the feature subset defined in [Layout](Layout.md).

## DrawShaders
### draw_bg ([DrawQuad](DrawQuad.md))
The DrawShader reponsible for rendering the image.

## Fields
### fit ([ImageFit](ImageFit.md))

Display mode of pictures
* **Size**
  Original size
* **Stretch**
  Stretch to fit the parent container
* **Horizontal**
  Fill the parent container horizontally while keeping the image's aspect ratio ratio
* **Vertical**
  Fill the parent container vertically while keeping the image's aspect ratio ratio
* **Smallest**
  Fill the parent container's shorter side and keep the image's aspect ratio.
* **Biggest**
  Fill the parent container's longer side and keep the image's aspect ratio.

![imagemodes](image_modes.png)
### min_height (i64)
The image's minimal height.
### min_width (i64)
The image's minimal width.
### source (LiveDependency)
The LiveDependence path to the image file.

## Examples
### Basic
```rust
<Image> {
	source: dep("crate://self/resources/logo.png"),
	height: 200,
	width: 200
}
```

### Typical
```rust
MyImage = <Image> {
	fit: Stretch,
	min_height: 300.0,
	min_width: 50.0,
	source: dep("crate://self/resources/logo.png"),

	// LAYOUT PROPERTIES

	height: 200.0,
	// Element is 15. high.

	width: Fill,
	// Element expands to use all available horizontal space.

	margin: { top: 10.0, right: 5.0, bottom: 10.0, left: 5.0 },
	// Individual margins outside the element for all four directions.
}
```

### Advanced
```rust
MyImage = <Image> {
	draw_bg: {
		texture image: texture2d
		
		instance rotation: 0.0
		instance opacity: 1.0
		instance scale: 1.0
		
		fn rotation_vertex_expansion(rotation: float, w: float, h: float) -> vec2 {
			let horizontal_expansion = (abs(cos(rotation)) * w + abs(sin(rotation)) * h) / w - 1.0;
			let vertical_expansion = (abs(sin(rotation)) * w + abs(cos(rotation)) * h) / h - 1.0;
			
			return vec2(horizontal_expansion, vertical_expansion);
		}
		
		fn rotate_2d_from_center(coord: vec2, a: float, size: vec2) -> vec2 {
			let cos_a = cos(-a);
			let sin_a = sin(-a);
			
			let centered_coord = coord - vec2(0.5, 0.5);
			
			// Denormalize the coordinates to use original proportions (between height and width)
			let denorm_coord = vec2(centered_coord.x, centered_coord.y * size.y / size.x);
			let demorm_rotated = vec2(denorm_coord.x * cos_a - denorm_coord.y * sin_a, denorm_coord.x * sin_a + denorm_coord.y * cos_a);
			
			// Restore the coordinates to use the texture coordinates proportions (between 0 and 1 in both axis)
			let rotated = vec2(demorm_rotated.x, demorm_rotated.y * size.x / size.y);
			
			return rotated + vec2(0.5, 0.5);
		}
		
		fn get_color(self) -> vec4 {
			let rot_padding = rotation_vertex_expansion(self.rotation, self.rect_size.x, self.rect_size.y) / 2.0;
			
			// Current position is a traslated one, so let's get the original position
			let current_pos = self.pos.xy - rot_padding;
			let original_pos = rotate_2d_from_center(current_pos, self.rotation, self.rect_size);
			
			// Scale the current position by the scale factor
			let scaled_pos = original_pos / self.scale;
			
			// Take pixel color from the original image
			let color = sample2d(self.image, scaled_pos).xyzw;
			
			let faded_color = color * vec4(1.0, 1.0, 1.0, self.opacity);
			return faded_color;
		}
		
		fn pixel(self) -> vec4 {
			let rot_expansion = rotation_vertex_expansion(self.rotation, self.rect_size.x, self.rect_size.y);
			
			let sdf = Sdf2d::viewport(self.pos * self.rect_size);
			
			let translation_offset = vec2(self.rect_size.x * rot_expansion.x / 2.0, self.rect_size.y * self.scale * rot_expansion.y / 2.0);
			sdf.translate(translation_offset.x, translation_offset.y);
			
			let center = self.rect_size * 0.5;
			sdf.rotate(self.rotation, center.x, center.y);
			
			let scaled_size = self.rect_size * self.scale;
			sdf.box(0.0, 0.0, scaled_size.x, scaled_size.y, 1);
			
			sdf.fill_premul(Pal::premul(self.get_color()));
			return sdf.result
		}
		
		fn vertex(self) -> vec4 {
			let rot_expansion = rotation_vertex_expansion(self.rotation, self.rect_size.x, self.rect_size.y);
			let adjusted_pos = vec2(
				self.rect_pos.x - self.rect_size.x * rot_expansion.x / 2.0,
				self.rect_pos.y - self.rect_size.y * rot_expansion.y / 2.0
			);
			
			let expanded_size = vec2(self.rect_size.x * (self.scale + rot_expansion.x), self.rect_size.y * (self.scale + rot_expansion.y));
			let clipped: vec2 = clamp(
				self.geom_pos * expanded_size + adjusted_pos,
				self.draw_clip.xy,
				self.draw_clip.zw
			);
			
			self.pos = (clipped - adjusted_pos) / self.rect_size;
			return self.camera_projection * (self.camera_view * (
				self.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_zbias, 1.)
			));
		}
		
		shape: Solid,
		fill: Image
	}

	fit: Stretch,
	min_height: 300.0,
	min_width: 50.0,
	source: dep("crate://self/resources/logo.png"),
	width_scale: 100.0,

	// LAYOUT PROPERTIES
	height: 150.0,
	width: 100.0,
	margin: 5.0
}

<MyImage> { source: dep("crate://self/resources/my_logo.png") }
```

