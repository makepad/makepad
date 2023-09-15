# TODO

## General Bugs

- moving window by dragging titlebar is incorrect (only left part of bar works)
- cursor updates missing sometimes
- mouse events to RunView seem weird in Windows, check with other platforms

## Double Buffering

* windows: client process sends back which texture to display as soon as it's barrier-ready, run_view displays the right one
* windows: why only displaying texture 0? -> create textures before sending them off to the client process (lazy init issue)
+ macos: investigate situation
+ linux: investigate situation
- macos stdin: add second texture and correct messaging
- linux stdin: add second texture and correct messaging
- windows, macos, linux: polish

## And Also...

- Test other example apps
- Test other systems
- Gaussian Blur
