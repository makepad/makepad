A radial color picker widget that allows users to select colors by adjusting hue, saturation, and value.

## Layouting

Support for layouting child elements via the feature subset defined in [Layout](Layout.md).  
No support for [Walk](Walk.md) layout features which define how the widget positions itself in its parent container.

## Fields

### hue (f32)

Stores the selected color hue.

### sat (f32)

Stores the selected color saturation.

### val (f32)

Stores the selected color value.

## DrawShaders

### draw_wheel ([DrawColorWheel](#drawcolorwheel))

References the color picking `DrawShader` function.

## DrawColorWheel

The color picking widget's draw shader that renders the visual representation of the color wheel.

### DrawShaders

#### draw_super ([DrawQuad](../FoundationalTypes/DrawQuad.md))

The base draw shader which determines the appearance of the color picking widget.


## States

| State           | Trigger                                             |
|-----------------|-----------------------------------------------------|
| `hover (f32)`   | User moves the mouse over the element               |
| `pressed (f32)` | Mouse button is pushed down during click operations |

## Examples
### Basic
```Rust
<ColorPicker> {} 
```

### Typical
```Rust
<ColorPicker> {
	hue: 128.0, // The selected hue
	sat: 255.0, // The selected saturation
	val: 128.0, // The selected value

	// LAYOUT PROPERTIES

	height: 200.0,
	// Element is 15. high.

	width: Fit,
	// Element expands to use all available horizontal space.
}
```

### Advanced
```Rust
MyColorPicker = <ColorPicker> {
	draw_wheel: {
        instance hover: float
        instance pressed: float
        
        fn circ_to_rect(u: float, v: float) -> vec2 {
            let u2 = u * u;
            let v2 = v * v;
            return vec2(
                0.5 * sqrt(2. + 2. * sqrt(2.) * u + u2 - v2) -
                0.5 * sqrt(2. - 2. * sqrt(2.) * u + u2 - v2),
                0.5 * sqrt(2. + 2. * sqrt(2.) * v - u2 + v2) -
                0.5 * sqrt(2. - 2. * sqrt(2.) * v - u2 + v2)
            );
        }
        
        fn pixel(self) -> vec4 {
            
            let rgbv = Pal::hsv2rgb(vec4(self.hue, self.sat, self.val, 1.));
            let w = self.rect_size.x;
            let h = self.rect_size.y;
            let sdf = Sdf2d::viewport(self.pos * vec2(w, h));
            let cx = w * 0.5;
            let cy = h * 0.5;
            
            let radius = w * 0.37;
            let inner = w * 0.28;
            
            sdf.hexagon(cx, cy, w * 0.45);
            sdf.hexagon(cx, cy, w * 0.4);
            sdf.subtract();
            let ang = atan(self.pos.x * w - cx, 0.0001 + self.pos.y * h - cy) / PI * 0.5 - 0.33333;
            sdf.fill(Pal::hsv2rgb(vec4(ang, 1.0, 1.0, 1.0)));
            
            let rsize = inner / sqrt(2.0);
            sdf.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
            
            let norm_rect = vec2(self.pos.x * w - (cx - inner), self.pos.y * h - (cy - inner)) / (2. * inner);
            let circ = clamp(circ_to_rect(norm_rect.x * 2. - 1., norm_rect.y * 2. - 1.), vec2(-1.), vec2(1.));
            
            sdf.fill(Pal::hsv2rgb(vec4(self.hue, (circ.x * .5 + .5), 1. - (circ.y * .5 + .5), 1.)));
            
            let col_angle = (self.hue + .333333) * 2. * PI;
            let circle_puk = vec2(sin(col_angle) * radius + cx, cos(col_angle) * radius + cy);
            
            let rect_puk = vec2(cx + self.sat * 2. * rsize - rsize, cy + (1. - self.val) * 2. * rsize - rsize);
            
            let color = mix(mix(#3, #E, self.hover), #F, self.pressed);
            let puck_size = 0.1 * w;
            sdf.circle(rect_puk.x, rect_puk.y, puck_size);
            sdf.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
            sdf.intersect();
            sdf.fill(color);
            sdf.circle(rect_puk.x, rect_puk.y, puck_size - 1. - 2. * self.hover + self.pressed);
            sdf.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
            sdf.intersect();
            sdf.fill(rgbv);
            
            sdf.circle(circle_puk.x, circle_puk.y, puck_size);
            sdf.fill(color);
            sdf.circle(circle_puk.x, circle_puk.y, puck_size - 1. - 2. * self.hover + self.pressed);
            sdf.fill(rgbv);
            
            return sdf.result;
        }
    }

	hue: 128.0, // The selected hue
	sat: 255.0, // The selected saturation
	val: 128.0, // The selected value

	animator: {
		hover = {
			default: off
			off = {
				from: {all: Forward {duration: 0.1}}
				apply: {
					draw_wheel: {pressed: 0.0, hover: 0.0}
				}
			}
			
			on = {
				cursor: Arrow,
				from: {
					all: Forward {duration: 0.1}
					pressed: Forward {duration: 0.01}
				}
				apply: {
					draw_wheel: {
						pressed: 0.0,
						hover: [{time: 0.0, value: 1.0}],
					}
				}
			}
			
			pressed = {
				cursor: Arrow,
				from: {all: Forward {duration: 0.2}}
				apply: {
					draw_wheel: {
						pressed: [{time: -1.0, value: 1.0}],
						hover: 1.0,
					}
				}
			}
		}
	}

	// LAYOUT PROPERTIES

	height: 175.0,
	width: 100.,
	margin: 0.0
}

<MyColorPicker> {
	hue: 0.0, // The selected hue
	sat: 15.7, // The selected saturation
	val: 262.5, // The selected value
}
```