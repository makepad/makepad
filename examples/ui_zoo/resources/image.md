## Image
Displays pixel images.

### Attributes
- draw_bg (DrawImage)
- min_width (float)
- min_height (float)
- width_scale (float)
- animation (ImageAnimation)
- visible (bool)
- fit (ImageFit)
    - Size: Original size
    - Stretch: Stretch to fit the parent container
    - Horizontal: Fill the parent container horizontally while keeping the image's aspect ratio ratio
    - Vertical: Fill the parent container vertically while keeping the image's aspect ratio ratio
    - Smallest: Fill the parent container's shorter side and keep the image's aspect ratio.
    - Biggest: Fill the parent container's longer side and keep the image's aspect ratio.
- source (LiveDependency)