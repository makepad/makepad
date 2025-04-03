> Note: This is not accessible via the DSL and must be defined from Rust.

The `MouseCursor` enum provides access to a wide variety of mouse cursors:

- **Default**: The platform's default cursor.
- **Arrow**: The standard arrow cursor, typically used for general purposes.
- **Crosshair**: A crosshair cursor for precise selection or targeting.
- **Hand**: A hand cursor, often used to indicate a clickable element or for panning operations.
- **Hidden**: Hides the cursor; no cursor is displayed.
- **Help**: Indicates that help information is available, usually depicted as a question mark.
- **Move**: A cursor with arrows pointing in all four directions, indicating that an element can be moved.
- **Text**: A text selection cursor, typically an I-beam, indicating editable or selectable text.
- **Wait**: An hourglass or spinner cursor, indicating that a process is ongoing and the user should wait.
- **NotAllowed**: Indicates that an action is not permitted, usually shown as a circle with a diagonal line.

**Directional resize cursors:**

- **NResize**: Resizes upwards (north).
- **NeResize**: Resizes diagonally upwards and to the right (northeast).
- **EResize**: Resizes to the right (east).
- **SeResize**: Resizes diagonally downwards and to the right (southeast).
- **SResize**: Resizes downwards (south).
- **SwResize**: Resizes diagonally downwards and to the left (southwest).
- **WResize**: Resizes to the left (west).
- **NwResize**: Resizes diagonally upwards and to the left (northwest).

**Bidirectional resize cursors:**

- **NsResize**: Resizes vertically (north-south).
- **EwResize**: Resizes horizontally (east-west).
- **NeswResize**: Resizes diagonally from the northeast to the southwest.
- **NwseResize**: Resizes diagonally from the northwest to the southeast.

**Special resize cursors:**

- **ColResize**: Indicates that a column can be resized; typically displays a vertical line with arrows pointing left and right.
- **RowResize**: Indicates that a row can be resized; typically displays a horizontal line with arrows pointing up and down.