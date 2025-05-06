The most basic layout container-element.
## [Layouting](Layouting.md)
Complete layouting feature set support.
## DrawShaders
### draw_bg ([DrawColor](DrawColor.md))
DrawShader that determines the appearance of the background.
## Fields
### cursor ([MouseCursor](MouseCursor.md))
Controls which cursor type is shown when hovering over this widget.
### debug ([ViewDebug](#viewdebug))
Controls the debug visualization mode of the `View`.

#### ViewDebug
- **None**: No debug visualization.
- **R**: Red overlay.
- **G**: Green overlay.
- **B**: Blue overlay.
- **M**, **Margin**: Visualizes the margins.
- **P**, **Padding**: Visualizes the padding.
- **A**, **All**: Visualizes all aspects (margins, padding, etc.).
- **Color(Vec4)**: Uses a custom color for debug visualization.

### event_order ([EventOrder](#eventorder))
Event propagation bubbling direction. Default direction is `Up`.

#### EventOrder
- **Down**: Events propagate from parent to child.
- **Up**: Events propagate from child to parent.
- **List(Vec\<LiveId\>)**: Custom event propagation order defined by a list of widget IDs.

### grab_key_focus (bool = true)
Determines whether the `View` should capture keyboard focus when interacted with.

### optimize ([ViewOptimize](#viewoptimize))
Selects the optimization mode for the `View`.

#### ViewOptimize
- **None**: No optimization. The contents are drawn directly, which can be the fastest in many cases.
- **DrawList**: Uses an own `DrawList`. Allows incremental rendering and partial redraws.
- **Texture**: Renders the entire `View` to a texture. Useful for applying pixel shader effects and reducing redraws when content is static.

### show_bg (bool = false)
Determines if the `View` has a background.

### visible (bool = true)
Controls the `View`'s visibility.

### dpi_factor (Option\<f64\>)
Specifies a custom DPI factor for the `View`. When set to `None`, the default DPI is used.

### block_signal_event (bool = false)
When `true`, prevents signal events from propagating to child widgets.

### capture_overload (bool = false)
When `true`, the `View` will capture input events even if they are already captured by another widget.

### scroll_bars (Option\<LivePtr\>)
Optional reference to the desired `ScrollBars` widget.

## Widget presets & variations
### CachedRoundedView
A `<View>` with rounded corners that is suitable for being layered onto non uniformly colored backgrounds.
Clips its content. To be used with care as many instances can exceed texture size limitations.
### CachedView
A performance optimized `<View>` that uses the optimization mode "texture". This mode requires a shader which is shipped by this preset.
To be used with care as many instances can exceed texture size limitations.
### CircleView
A circular layout container.
* color (Vec4): Background color
* border_width (float): Width of the border
* border_color (Vec4): Border color
* inset (Vec4): Moves the background drawing of the view inside without affecting the layout.
* radius (float): Rounded corners factor
### GradientXView
A `<View>`with a horizontal gradient background.
* color (Vec4): Gradient start color
* color2 (Vec4): Gradient end color
* dither (float): Dither factor to prevent gradient banding.
### GradientYView
A `<View>`with a vertical gradient background.
* color (Vec4): Gradient start color
* color2 (Vec4): Gradient end color
* dither (float): Dither factor to prevent gradient banding
### HexagonView
A hexagonal layout container.
* color (Vec4): Background color
* border_width (float): Width of the border
* border_color (Vec4): Border color
* inset (Vec4): Moves the background drawing of the view inside without affecting the layout.
* radius (float): Rounded corners factor
### RectShadowView
A `<View>` with a dropshadow.
* color (Vec4): Background color
* border_width (float): Width of the border
* border_color (Vec4): Border color
* shadow_color (Vec4): Color of the drop shadow
* shadow_offset (Vec2): Amount and direction of the dropshadow
* shadow_radius (float): Shadow softness factor
### RectView
A `<View>`with border and inset support.
	* color (Vec4): Background color
	* border_width (float): Width of the border
	* border_color (Vec4): Border color
	* inset (Vec4): Moves the background drawing of the view inside without affecting the layout.
### RoundedAllView
A `<View>`with border, inset and rounded corners support.
* color (Vec4): Background color
* border_width (float): Width of the border
* border_color (Vec4): Border color
* inset (Vec4): Moves the background drawing of the view inside without affecting the layout.
* radius (Vec4): Corner rounding. Allows individual control for each corner. Starts at the top left and then continues clockwise.
### RoundedShadowView
A `<View>` with rounded corner, border and drop shadow support 
* color (Vec4): Background color
* border_width (float): Width of the border
* border_color (Vec4): Border color
* shadow_color (Vec4): Color of the drop shadow
* shadow_offset (Vec2): Amount and direction of the dropshadow
* shadow_radius (float): Shadow softness factor
* radius (float): Rounded corners factor
### RoundedView
A `<View>` with rounded corners.
* color (Vec4): Background color
* border_width (float): Width of the border
* border_color (Vec4): Border color
* inset (Vec4): Moves the background drawing of the view inside without affecting the layout.
*  radius (float): Rounded corners factor
### RoundedXView
* color (Vec4): Background color
* border_width (float): Width of the border
* border_color (Vec4): Border color
* inset (Vec4): Moves the background drawing of the view inside without affecting the layout.
* radius (Vec2): Left / right radius corners as two individual values.
### RoundedYView
* color (Vec4): Background color
* border_width (float): Width of the border
* border_color (Vec4): Border color
* inset (Vec4): Moves the background drawing of the view inside without affecting the layout.
* radius (Vec2): top / bottom radius corners as two individual values.
### SolidView
A `<View>` with a solid backround.
* color (Vec4): Background color
## Examples
### Basic
```rust
<View> {}
```
### Typical
```rust
<View> {
	// LAYOUT PROPERTIES

	height: Fill,
	// Element expands to use all available vertical space.

	width: Fill,
	// Element expands to use all available horizontal space.

	margin: { top: 10.0, right: 5.0, bottom: 10.0, left: 5.0 },
	// Individual margins outside the element for all four directions.

	padding: { top: 15.0, left: 0.0 },
	// Individual space between the element's border and its content
	// for top and left.

	flow: Right,
	// Stacks children from left to right.

	spacing: 10.0,
	// A spacing of 10.0 between children.

	align: { x: 0.0, y: 1.0 },
	// Positions children at the left (x) bottom (y) corner of the parent.
}
```
### Advanced
```rust
MyView = <View> {
    block_signal_event: false, // Allows signal events to propagate to child widgets
    cursor: Crosshair,         // Changes cursor to a crosshair when hovering over the View
    debug: Margin,             // Visualizes the margins for debugging purposes
    event_order: Down,         // Events propagate from parent to child
    grab_key_focus: true,      // Captures keyboard focus when interacted with
    optimize: Texture,         // Renders the View to a texture for optimization
    show_bg: true,             // Displays a background
    visible: true,             // The View is visible
    capture_overload: false,   // Does not capture events already captured by another widget
    scroll_bars: None,         // No scroll bars are attached
    design_mode: false,        // Design mode features are disabled
    dpi_factor: None,          // Uses the default DPI

    draw_bg: {
        instance border_width: 1.0,
        instance border_color: #f,
        instance inset: vec4(1.0, 1.0, 1.0, 1.0),
        instance radius: 2.5,
        instance dither: 1.0,
        color: #FFFFFF55,
        color2: #FFFFFF10,
        instance border_color2: #x6,
        instance border_color3: #x3A,

        fn get_color(self) -> vec4 {
            let dither = Math::random_2d(self.pos.xy) * 0.04 * self.dither;
            return mix(self.color, self.color2, self.pos.y + dither);
        }

        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.box(
                self.inset.x + self.border_width,
                self.inset.y + self.border_width,
                self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                max(1.0, self.radius)
            );
            sdf.fill_keep(self.get_color());
            if self.border_width > 0.0 {
                sdf.stroke(
                    mix(
                        mix(self.border_color, self.border_color2, clamp(self.pos.y * 10.0, 0.0, 1.0)),
                        mix(self.border_color2, self.border_color3, self.pos.y),
                        self.pos.y
                    ),
                    self.border_width
                );
            }
            return sdf.result;
        }
    },

    // LAYOUT PROPERTIES

    height: Fit,     // Element height adjusts to fit its content
    width: Fill,     // Element width expands to fill available space
    margin: {
        top: 10.0, right: 5.0, bottom: 10.0, left: 5.0
    },               // Margins around the element
    padding: 10.0,   // Space between the border and content
    flow: Down,      // Stacks children vertically
    spacing: 10.0,   // Spacing between children
    align: {
        x: 0.0, y: 0.0
    },               // Aligns children to the top-left corner
    line_spacing: 1.5,
    scroll: vec2(0.0, 300.0)
}

<MyView> {}
```