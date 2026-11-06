# hello_3d

A lit cube turns slowly above a ground slab. Drag to orbit the camera, and use
the wheel to zoom. It is the smallest complete 3D scene in Makepad, and all of
it is in `src/main.rs`, meant to be read top to bottom.

```
cargo run --release -p makepad-example-hello-3d
```

## What it shows

Makepad's UI is 2D. A 3D scene is drawn into a pass of its own, a `DrawPass`
with its own colour and depth textures, and its picture is then shown in the
UI on an ordinary quad. The file's header explains each step. The sections
follow the order things happen:

1. **The mesh.** A cube built as vertices (position and normal) and
   triangles, uploaded once as a `Geometry`.
2. **The shader.** `DrawLitMesh`: the vertex stage takes each vertex from mesh
   space to world, camera and clip space. The pixel stage adds ambient, diffuse
   and specular light.
3. **The camera.** An orbit camera that turns two angles and a distance into
   the view and projection matrices.
4. **The widget and its pass.** The pass, its colour and depth textures, and
   how they are cleared.
5. **Drawing each frame.** The camera goes into the pass and two instances of
   the mesh into its draw list. Then a quad shows the colour texture in the
   widget's rectangle.
6. **Animation and input.** A next-frame loop turns the cube. Pointer hits
   orbit the camera and the wheel zooms.
7. **The app.** A window with a line of help text and the 3D view.

The header ends with a few things to try changing.

## Where to go next

- `examples/box3d`: a pile of boxes and spheres with Box3D physics, in the
  same kind of viewport.
- `examples/cad`: a full CAD viewport.
