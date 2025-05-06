# Animator

The `Animator` defines how visual elements of widgets respond to various user interactions, such as hover and focus states, by controlling animations between different states.

## Attributes

### KeyFrame

A `KeyFrame` represents a specific point in an animation timeline, defining the target value at a given time with an optional easing function.

- **ease** ([Ease](#ease) = Ease::Linear): The easing function used to interpolate between the previous keyframe and this keyframe.
- **time** (f64 = 1.0): The normalized time for this keyframe in the timeline, ranging from `0.0` to `1.0`.
- **value** (LiveValue = LiveValue::None): The target value at this keyframe.

### Play

Defines the playback mode of an animation.

- **duration** (f64): Duration of the animation in seconds.
- **end** (f64): The end time for looping animations. It defines the point at which the animation loops back or reverses, typically ranging from `0.0` to `1.0`.

#### Modes

- **Forward**: Plays the animation forward from start to end once, then stops.
  - `{ duration: f64 }`: Duration in seconds.
- **Snap**: Instantly changes to the final state without any interpolated animation.
- **Reverse**: Plays the animation backward from end to start once, then stops.
  - `{ duration: f64, end: f64 }`: Duration in seconds, end time (usually `1.0`).
- **Loop**: Continuously loops the animation from start to `end`.
  - `{ duration: f64, end: f64 }`: Duration of one loop cycle, end time where the loop restarts.
- **ReverseLoop**: Continuously loops the animation in reverse from `end` to start.
  - `{ duration: f64, end: f64 }`: Duration of one loop cycle, end time where the loop restarts.
- **BounceLoop**: Continuously plays the animation forward and backward between start and `end`, creating a ping-pong effect.
  - `{ duration: f64, end: f64 }`: Duration of one full forward and backward cycle, end time where the bounce reverses.

## Types

### Ease

Defines the easing functions available for animations. Easing functions determine how the animation progresses over time, affecting the acceleration and deceleration of the animation.

- **Linear**: Animates at a constant speed.
- **None**: No animation; the value changes instantly.
- **Constant(f64 = 1.0)**: Maintains a constant value during the animation.
- **InQuad**: Eases in with a quadratic (x²) curve, starting slow and accelerating towards the end.
- **OutQuad**: Eases out with a quadratic (x²) curve, starting fast and decelerating towards the end.
- **InOutQuad**: Combines easing in and out with a quadratic curve.
- **InCubic**: Eases in with a cubic (x³) curve.
- **OutCubic**: Eases out with a cubic (x³) curve.
- **InOutCubic**: Combines easing in and out with a cubic curve.
- **InQuart**: Eases in with a quartic (x⁴) curve.
- **OutQuart**: Eases out with a quartic (x⁴) curve.
- **InOutQuart**: Combines easing in and out with a quartic curve.
- **InQuint**: Eases in with a quintic (x⁵) curve.
- **OutQuint**: Eases out with a quintic (x⁵) curve.
- **InOutQuint**: Combines easing in and out with a quintic curve.
- **InSine**: Eases in with a sinusoidal curve.
- **OutSine**: Eases out with a sinusoidal curve.
- **InOutSine**: Combines easing in and out with a sinusoidal curve.
- **InExp**: Eases in with an exponential curve.
- **OutExp**: Eases out with an exponential curve.
- **InOutExp**: Combines easing in and out with an exponential curve.
- **InCirc**: Eases in with a circular curve.
- **OutCirc**: Eases out with a circular curve.
- **InOutCirc**: Combines easing in and out with a circular curve.
- **InElastic**: Eases in with an elastic curve with a bounce at the start.
- **OutElastic**: Eases out with an elastic curve with a bounce at the end.
- **InOutElastic**: Combines easing in and out with an elastic curve.
- **InBack**: Eases in with a "backward" overshoot effect before moving forward.
- **OutBack**: Eases out with a "backward" overshoot effect after moving forward.
- **InOutBack**: Combines easing in and out with a "backward" overshoot effect.
- **InBounce**: Eases in with a bounce effect.
- **OutBounce**: Eases out with a bounce effect.
- **InOutBounce**: Combines easing in and out with a bounce effect.
- **ExpDecay**: Uses an exponential decay curve, creating a natural slow-down effect.
  - `{ d1: f64, d2: f64, max: usize }`: Decay parameters.
- **Pow**: Uses a power function for easing.
  - `{ begin: f64, end: f64 }`: Power function parameters.
- **Bezier**: Uses a Bézier curve for easing.
  - `{ cp0: f64, cp1: f64, cp2: f64, cp3: f64 }`: Control points for the curve.

## Example

```rust
<Button> {
    // Allows instantiation of custom-styled elements.

    // BUTTON SPECIFIC PROPERTIES

    draw_bg: { // Shader object that draws the background.
        fn get_color(self) -> vec4 { // Override the shader's fill method.
            return mix( // State transition animations.
                mix(
                    self.color,
                    mix(self.color, #f, 0.5),
                    self.hover
                ),
                self.color_pressed,
                self.pressed
            )
        }
    },

    draw_icon: { // Shader object that draws the icon.
        svg_file: dep("crate://self/resources/icons/back.svg"),
        // Icon file dependency.

        fn get_color(self) -> vec4 { // Override the shader's fill method.
            return mix( // State transition animations.
                mix(
                    self.color,
                    mix(self.color, #f, 0.5),
                    self.hover
                ),
                self.color_pressed,
                self.pressed
            )
        }
    },

    draw_text: { // Shader object that draws the text.
        wrap: Word, // Wraps text between words.
        text_style: {
            // Controls the appearance of text.
            font: { path: dep("crate://self/resources/GoNotoKurrent-Bold.ttf") },
            // Font file dependency.

            font_size: 12.0, // Font size of 12.0.
        },

        fn get_color(self) -> vec4 { // Override the shader's fill method.
            return mix( // State transition animations.
                mix(
                    self.color,
                    self.color_hover,
                    self.hover
                ),
                self.color_pressed,
                self.pressed
            )
        }
    },

    text: "I can be clicked", // Text label.

    animator: { // State change animations.
        hover = { // State definition.
            default: off, // The state's starting point.

            off = { // Behavior when transitioning to the 'off' state.
                from: { // Transition behaviors from prior states.
                    all: Forward { duration: 0.1 }, // Default animation direction and speed in seconds.
                    pressed: Forward { duration: 0.25 } // Specific duration when coming from 'pressed' state.
                },
                apply: { // Properties to animate.
                    draw_bg: { pressed: 0.0, hover: 0.0 }, // Target values for properties.
                    draw_icon: { pressed: 0.0, hover: 0.0 },
                    draw_text: { pressed: 0.0, hover: 0.0 }
                }
            },

            on = { // Behavior when transitioning to the 'on' state.
                from: {
                    all: Forward { duration: 0.1 },
                    pressed: Forward { duration: 0.5 }
                },
                apply: {
                    draw_bg: { pressed: 0.0, hover: [{ time: 0.0, value: 1.0 }] },
                    // 'hover' is animated from 0.0 to 1.0 starting at time 0.0.
                    draw_icon: { pressed: 0.0, hover: [{ time: 0.0, value: 1.0 }] },
                    draw_text: { pressed: 0.0, hover: [{ time: 0.0, value: 1.0 }] }
                }
            },

            pressed = { // Behavior when transitioning to the 'pressed' state.
                from: { all: Forward { duration: 0.2 } },
                apply: {
                    draw_bg: { pressed: [{ time: 0.0, value: 1.0 }], hover: 1.0 },
                    draw_icon: { pressed: [{ time: 0.0, value: 1.0 }], hover: 1.0 },
                    draw_text: { pressed: [{ time: 0.0, value: 1.0 }], hover: 1.0 }
                }
            }
        }
    },

    // LAYOUT PROPERTIES
    height: Fit,
    width: Fit,
}
```

In this example, a custom `Button` component is defined with various states (`hover`, `pressed`, etc.) and animations when transitioning between these states. The `animator` block specifies how properties like `draw_bg`, `draw_icon`, and `draw_text` should animate using keyframes and easing functions defined in `KeyFrame` and `Ease`. The `Play` modes dictate how the animations play out during these transitions.