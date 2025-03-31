# gloop

```glsl
fn gloop(inout self, k: float)
```

The `gloop` function blends the current shape in the `Sdf2d` drawing context with the previously stored shape, creating a smooth, organic transition between them. This blending technique is often used to produce fluid, blobby effects similar to metaballs.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the internal `shape` and `old_shape` fields of `self` to represent the blended shape.
- **k** (`float`): The smoothing factor controlling the blend between the current shape and the previously stored shape. A larger `k` value results in a smoother, more gradual transition.

## Returns

- **void**: This function does not return a value but updates the internal state of `self` to reflect the blended shape.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw the first circle at position (40.0, 50.0) with a radius of 30.0.
    sdf.circle(40.0, 50.0, 30.0);

    // Store the current shape as the starting point for blending.
    sdf.union();

    // Draw the second circle at position (70.0, 50.0) with a radius of 30.0.
    sdf.circle(70.0, 50.0, 30.0);

    // Blend the two shapes using the gloop function with smoothing factor 10.0.
    sdf.gloop(10.0);

    // Fill the blended shape with a solid red color.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context scaled to the size of the current rectangle (`self.rect_size`).
- **First Shape**: We draw a circle centered at `(40.0, 50.0)` with a radius of `30.0`.
- **Store Shape**: We call `sdf.union()` to store the current shape in `self.old_shape`, preparing for blending.
- **Second Shape**: We draw another circle centered at `(70.0, 50.0)` with the same radius.
- **Apply Gloop**: By calling `sdf.gloop(10.0)`, we blend the current shape (`self.dist`) with the stored shape (`self.old_shape`) using a smoothing factor of `10.0`. This creates a smooth transition between the two circles.
- **Fill Shape**: We use `sdf.fill(#f00)` to fill the blended shape with red color.
- **Return Result**: The function returns `sdf.result`, which contains the final rendered image with the blended shape.

### Notes

- **Smoothing Factor (`k`)**: The value of `k` determines how smoothly the shapes blend together. A larger `k` results in a softer, more fluid transition. Experiment with different values to achieve the desired effect.
- **Shape Blending**: The `gloop` function blends the signed distance fields of two shapes, allowing for complex, organic forms.
- **Shape Composition**: Before using `gloop`, ensure that you have defined both shapes you wish to blend and have stored the initial shape using `sdf.union()` or another combining function.
- **Further Transformations**: You can apply additional transformations such as `rotate`, `translate`, or `scale` to manipulate the shapes before or after blending.

### Additional Information

- **Related Functions**: Consider using `sdf.union()`, `sdf.intersect()`, or `sdf.subtract()` for other shape combination operations.
- **Applications**: The `gloop` function is useful in generating procedural graphics where smooth transitions between shapes are required, such as in fluid simulations, soft-body animations, or artistic effects.
