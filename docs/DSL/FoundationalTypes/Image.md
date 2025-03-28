Displays pixel images.

Supported formats:
- jpg
- png
## draw_bg ([DrawQuad](DrawQuad.md))
DrawShader responsible for drawing the Image.
## fit ([ImageFit](ImageFit.md))
Selects one of the image layout modes defined in [ImageFit](ImageFit.md).
## min_height (i64)
Sets a minimal height for the image.
## min_width (i64)
Sets a minimal width for the image.
## source (LiveDependency)
The LiveDependency path to the image file.
## walk ([Walk](Walk.md))
Allows for controlling the inner layout properties of the image according to the options defined in [Walk](Walk.md).
## Example
```rust
<Image> {
	source: dep("crate://self/resources/logo.png"),
	height: 200,
	width: 200
}
```