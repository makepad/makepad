> Note: This widget is not general purpose (yet). So far it is a custom widget only used for the IronFish synthesizer demo.

A piano widget.
## Examples
### Basic
```Rust
<Piano> {}
```

### Typical
```Rust
MyPiano = <Piano> {
	// LAYOUT PROPERTIES
	black_size: vec2(100.0, 50),
	piano_key(LivePtr): // TODO: tbd
	white_size: vec2(150.0, 50)

	height: 15.0,
	// Element is 15. high.

	width: Fill,
	// Element expands to use all available horizontal space.

	margin: { top: 10.0, right: 5.0, bottom: 10.0, left: 5.0 },
	// Individual margins outside the element for all four directions.
}
```

### Advanced
```Rust
MyPiano = <Piano> {
	// LAYOUT PROPERTIES
	black_size: vec2(100.0, 50),
	piano_key(LivePtr): // TODO: tbd
	white_size: vec2(150.0, 50)

	piano_key: {
		draw_key: {
			fn height_map(self, pos: vec2) -> float {
				let fx = 1 - pow(1.2 - sin(pos.x * PI), 10.8);
				let fy = 1 - pow(1.2 - self.pressed * 0.2 - cos(pos.y * 0.5 * PI), 25.8)
				return fx + fy
			}
			
			fn black_key(self) -> vec4 {
				let delta = 0.001;
				// differentiate heightmap to get the surface normal
				let d = self.height_map(self.pos)
				let dy = self.height_map(self.pos + vec2(0, delta))
				let dx = self.height_map(self.pos + vec2(delta, 0))
				let normal = normalize(cross(vec3(delta, 0, dx - d), vec3(0, delta, dy - d)))
				let light = normalize(vec3(0.65, 0.5, 0.5))
				let light_hover = normalize(vec3(0.75, 0.5, 1.5))
				let diff = pow(max(dot(mix(light, light_hover, self.hover * (1 - self.pressed)), normal), 0), 3)
				return mix(#181818, #bc, diff)
			}
			
			fn white_key(self) -> vec4 {
				return mix(
					#DEDAD3FF,
					mix(
						mix(
							#EAE7E2FF,
							#ff,
							self.hover
						),
						mix(#96989CFF, #131820FF, pow(1.0 - sin(self.pos.x * PI), 1.8)),
						self.pressed
					),
					self.pos.y
				)
			}
			
			fn pixel(self) -> vec4 {
				//return #f00
				let sdf = Sdf2d::viewport(self.pos * self.rect_size);
				if self.is_black > 0.5 {
					sdf.box(0., -4, self.rect_size.x, self.rect_size.y + 4, 1);
					sdf.fill_keep(self.black_key())
				}
				else {
					sdf.box(0., -4.0, self.rect_size.x, self.rect_size.y + 4.0, 2.0);
					sdf.fill_keep(self.white_key())
				}
				return sdf.result
			}
		}
	}

	animator: {
		hover = {
			default: off,
			off = {
				from: {all: Forward {duration: 0.2}}
				apply: {draw_key: {hover: 0.0}}
			}
			
			on = {
				from: {all: Snap}
				apply: {draw_key: {hover: 1.0}}
			}
		}
		
		focus = {
			default: off
			
			off = {
				from: {all: Snap}
				apply: {draw_key: {focussed: 1.0}}
			}
			
			on = {
				from: {all: Forward {duration: 0.05}}
				apply: {draw_key: {focussed: 0.0}}
			}
		}
		pressed = {
			default: off
			off = {
				from: {all: Forward {duration: 0.05}}
				apply: {draw_key: {pressed: 0.0}}
			}
			
			on = {
				from: {all: Snap}
				apply: {draw_key: {pressed: 1.0}}
			}
		}
	}

	height: 220.,
	width: Fill,
	margin: 5.0
}

<MyPiano> {}
```
