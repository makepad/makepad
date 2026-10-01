# Web packs

A web build ships only what the app was seen to use. One tool covers any
Makepad app:

```sh
cargo makepad wasm --pack=<film|app|full> [--strict] build -p <app> --release
```

1. **Collect.** The app runs once natively as `MAKEPAD_RUN=collect-web`
   (`MAKEPAD_COLLECT=<dir>`): the headless runtime with the web font set and
   the WebGL2 shader compiler. It draws a few frames, sends one `Event::Scan`
   through the widget tree, draws again and writes the manifest
   (`makepad-web-manifest`): the shader pack, kernels, glyphs per font file,
   widget types, assets, JS features and who needed each one.
2. **Pack.** The wasm is linked with the shader pack (`--strict` also leaves
   the shader compiler out), fonts are cut to the glyphs used
   (`makepad-font-subset`), and the web runtime's JS loses the `// @section`s
   nothing needed.

Presets: `film` (only what was collected), `app` (keeps a shader-compiler
fallback and full fonts), `full` (everything). Stage's Share → Web export
runs the same steps for a film, with its own scanner for documents.

## What widgets do in a scan

`Event::Scan` reaches every widget, visible or not, through `children()`.
A widget emits what it needs through the event's `ScanEvent`:

```rust
fn scan(&mut self, cx: &mut Cx, scan: &ScanEvent, _scope: &mut Scope) {
    // Typed text can be any character: ship the whole face.
    scan.need_font_full("Inter");
    scan.need_js_feature("text-input");
    scan.need_js_feature("clipboard");
}
```

Other needs: `need_glyphs(face, text)`, `need_font_range(face, "latin")`,
`need_kernel`, `need_widget::<T>()`, `need_asset(path)`. Shaders are recorded
automatically when they are applied. `cx.is_collecting()` says whether the
app runs a collect run. An app with dynamic text says so itself, for example
`need_font_range(face, "latin")` for user names.

## When a web build misses something

Widgets that create children from data spawn the templates of their
`scan:` list (or every template they hold) during the scan; see
[splash.md](splash.md). A web build that meets something not in its pack
logs it with the widget path and file:line, for example
`not in pack: draw shader 'Row' ..., add the widget that creates it to a scan: list`.
Add the template to that widget's `scan: [...]` and build again.
