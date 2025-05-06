## path(LiveDependency)
LiveDependency path to the desired font file.
## Example
```rust
<TextInput> {
	width: 200, height: Fit,
	draw_label: {
		text_style: {
			font: { path: dep("crate://self/resources/GoNotoKurrent-Regular.ttf"),	
			font_size: 12.0,
			line_spacing: 1.5,
			top_drop: 1.2,
			height_factor: 1.4,
		}
	}
}
```