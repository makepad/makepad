This widget provides a sliding panel that can be opened and closed, sliding in from the left, right, or top of the screen. It is useful for creating sidebars, drawers, or any sliding interface elements that appear or disappear with a sliding animation.

## Inherits

- frame: [[View]]

## Layouting

No layouting support.

## Fields

### frame ([View](View.md))

The `<View>` widget that contains the panel's content.

### side ([SlideSide](#slideside))

The direction from which the sliding panel slides in.

#### SlideSide

- **Left**: Panel slides in from the left.
- **Right**: Panel slides in from the right.
- **Top**: Panel slides in from the top.


## States

| State  | Description                             |
|--------|-----------------------------------------|
| `closed` | The closure state of the panel, where `0.0` is fully open and `1.0` is fully closed. This field is typically animated to create the sliding effect. |


## Example
### Typical
```Rust
<SlidePanel> {
	frame: <View> {
		<Label> { text: "frame" }
	}
	side: // TODO: tbd
}
```

### Advanced 
```Rust
MySlidePanel = <SlidePanel>{
	frame: <View> {
		<Label> { text: "frame" }
	}
	side: // TODO: tbd

	animator: {
		closed = {
			default: off,
			on = {
				redraw: true,
				from: {
					all: Forward {duration: 0.5}
				}
				ease: InQuad
				apply: {
					closed: 1.0
				}
			}
			
			off = {
				redraw: true,
				from: {
					all: Forward {duration: 0.5}
				}
				ease: OutQuad
				apply: {
					closed: 0.0
				}
			}
		}
	}
}

<MySlidePanel> {}
```