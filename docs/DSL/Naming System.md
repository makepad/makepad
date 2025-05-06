# Widgets & Variants

| Name                   | Description                           |
| ---------------------- | ------------------------------------- |
| Widget                 | The default widget.                   |
| WidgetFlat             | A flat version with 3D hover states.  |
| WidgetFlatter          | A fully flat version.                 |
| WidgetGradientX        | Has vertical gradient support.        |
| WidgetGradientY        | Has horizontal gradient support.      |
| WidgetVariant          | An entirely alternative design.       |
| WidgetVariantFlat      | A flat version of this variant.       |
| WidgetVariantFlatter   | A fully flat version of this variant. |
| WidgetVariantGradientX | Has vertical gradient support.        |
| WidgetVariantGradientY | Has horizontal gradient support.      |

# Styling Attributes
`subobject_property_i_subproperty_state`

Example: `border_color_2_hover`

| Segment       | Description                                                                                                           |
| ------------- | --------------------------------------------------------------------------------------------------------------------- |
| `subobject`   | The subcomponent of the widget to style.<br>*Examples: **border**\_color, **val**\_color*                             |
| `property`    | The styling attribute.<br>*Examples: **color**, border\_**size***                                                     |
| `_i`          | Enumeration for multi-color-cases like gradients or alternating backgrounds. * *Examples: color\_**1**, color\_**2*** |
| `subproperty` | Styling attribute properties.<br>*Examples: color\_**dither**, shadow\_**offset***                                    |
| `state`       | The state.<br>*Examples: color\_**hover**, color\_**focus***                                                          |

# Available states

| state         | desc |
| ------------- | ---- |
| `hover`       | tbd  |
| `focus`       | tbd  |
| `down`        | tbd  |
| `drag`        | tbd  |
| `active`      | tbd  |
| `empty`       | tbd  |
| `empty_focus` | tbd  |
| `disabled`    | tbd  |
| `hidden`      | tbd  |


# Available shaders and their properties
All widget shaders list their supported uniforms at their top.
## draw_bg
| Attribute          | Type  | Desc                                                                                 |
| ------------------ | ----- | ------------------------------------------------------------------------------------ |
| `arrow_color`      | color | Color of the graphical indicator that suggests dropdowns have popup menus available. |
| `border_color[_i]` | color | Outline color.                                                                       |
| `border_inset`     | float | Moves the outline inside the widget.                                                 |
| `border_radius`    | float | Rounded corners.                                                                     |
| `border_size`      | float | Outline stroke width.                                                                |
| `color[_i]`        | color | The main color.                                                                      |
| `color_dither`     | float | Dither factor for gradients to prevent banding artifacts.                            |
| `handle_color[_i]` | color | Color of slider drag‑handles.                                                        |
| `mark_color[_i]`   | color | Color of activation indicators like checkbox check marks.                            |
| `shadow_color[_i]` | color | Dropshadow color.                                                                    |
| `shadow_offset`    | vec2  | Dropshadow position.                                                                 |
| `shadow_radius`    | float | Dropshadow size and blurring.                                                        |
| `val_color[_i]`    | color | Value‑bar colors of sliders.                                                         |
| `val_heat`         | float | Determines how much the tip of value‑bars is accentuated if they have gradients.     |
| `val_padding`      | float | Determines the space around the value‑bar relative to its rail.                      |
| `val_size`         | float | Size of the value‑bar.                                                               |

## draw_text

| Attribute   | Type  | Desc        |
| ----------- | ----- | ----------- |
| `color[_i]` | color | Text color. |

## draw_icon
| Attribute   | Type  | Desc       |
| ----------- | ----- | ---------- |
| `color[_i]` | color | Icon color |

## draw_highlight
| Attribute   | Type  | Desc                  |
| ----------- | ----- | --------------------- |
| `color[_i]` | color | Text selection color. |

## draw_cursor
| Attribute | Type  | Desc               |
| --------- | ----- | ------------------ |
| `color`   | color | Text cursor color. |
## round_corner
Dock-specific shader

| Attribute | Type  | Desc            |
| --------- | ----- | --------------- |
| `color`   | color | Backdrop color. |