The `PageFlip` widget manages and displays multiple pages, allowing users to flip between them dynamically. It's useful for creating multi-page interfaces within your application.

## [Layouting](Layouting.md)

Complete layouting feature set support.

## Fields

### active_page (LiveId)

Sets the active page. This determines which page is currently visible to the user.

### lazy_init (bool = false)

If set to `true`, pages will only be initialized when they are first displayed. This can improve performance by deferring the creation of pages until they are needed.

## Examples
### Basic
```Rust
example
```
### Typical
```Rust
example
```
### Advanced
```Rust
<PageFlip> {
	width: Fit, height: Fit,
	active_page: log
	lazy_init: true,
	wait = <Icon> {
		draw_bg: {
			fn pixel(self) -> vec4 {
				let sdf = Sdf2d::viewport(self.pos * self.rect_size)
				sdf.circle(5., 5., 4.)
				sdf.fill(THEME_COLOR_TEXT_META)
				sdf.move_to(3., 5.)
				sdf.line_to(3., 5.)
				sdf.move_to(5., 5.)
				sdf.line_to(5., 5.)
				sdf.move_to(7., 5.)
				sdf.line_to(7., 5.)
				sdf.stroke(#0, 0.8)
				return sdf.result
			}
		}
	},
	log = <Icon> {
		draw_bg: {
			fn pixel(self) -> vec4 {
				let sdf = Sdf2d::viewport(self.pos * self.rect_size)
				sdf.circle(5., 5., 4.);
				sdf.fill(THEME_COLOR_TEXT_META);
				let sz = 1.;
				sdf.move_to(5., 5.);
				sdf.line_to(5., 5.);
				sdf.stroke(#a, 0.8);
				return sdf.result
			}
		}
	}
	error = <Icon> {
		draw_bg: {
			fn pixel(self) -> vec4 {
				let sdf = Sdf2d::viewport(self.pos * self.rect_size)
				sdf.circle(5., 5., 4.5);
				sdf.fill(THEME_COLOR_ERROR);
				let sz = 1.5;
				sdf.move_to(5. - sz, 5. - sz);
				sdf.line_to(5. + sz, 5. + sz);
				sdf.move_to(5. - sz, 5. + sz);
				sdf.line_to(5. + sz, 5. - sz);
				sdf.stroke(#0, 0.8)
				return sdf.result
			}
		}
	},
	warning = <Icon> {
		draw_bg: {
			fn pixel(self) -> vec4 {
				let sdf = Sdf2d::viewport(self.pos * self.rect_size)
				sdf.move_to(5., 1.);
				sdf.line_to(9.25, 9.);
				sdf.line_to(0.75, 9.);
				sdf.close_path();
				sdf.fill(THEME_COLOR_WARNING);
				//  sdf.stroke(#be, 0.5);
				sdf.move_to(5., 3.5);
				sdf.line_to(5., 5.25);
				sdf.stroke(#0, 1.0);
				sdf.move_to(5., 7.25);
				sdf.line_to(5., 7.5);
				sdf.stroke(#0, 1.0);
				return sdf.result
			}
		}
	}
	panic = <Icon> {
		draw_bg: {
			fn pixel(self) -> vec4 {
				let sdf = Sdf2d::viewport(self.pos * self.rect_size)
				sdf.move_to(5., 1.);
				sdf.line_to(9., 9.);
				sdf.line_to(1., 9.);
				sdf.close_path();
				sdf.fill(THEME_COLOR_PANIC);
				let sz = 1.;
				sdf.move_to(5. - sz, 6.25 - sz);
				sdf.line_to(5. + sz, 6.25 + sz);
				sdf.move_to(5. - sz, 6.25 + sz);
				sdf.line_to(5. + sz, 6.25 - sz);
				sdf.stroke(#0, 0.8);
				return sdf.result
			}
		}
	}
}
```

