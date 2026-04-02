# XR Remote shell+adb verification (worker-2)

Date: 2026-04-01/02
Worker: worker-2
Host: Mac 192.168.2.23
Quest serial used: 192.168.2.120:5555
Host log source: `/tmp/xr_remote_worker1/host-20260401T202118.log`
Host process: `target/release/makepad-example-xr-remote` pid `57893`

## Build / tests

- `cargo check -p makepad-example-xr-remote`
  - PASS
- `cargo test -p makepad-example-xr-remote -- --nocapture`
  - PASS (4 tests)

## Host shell evidence

Host runtime was already active from shell in release mode:

- `ps -p 57893 -o pid,ppid,etime,command=`
  - `57893 57732 01:59 target/release/makepad-example-xr-remote`
- `lsof -nP -iUDP:41546 -iUDP:41547 -iTCP:41548 -iTCP:44510 -iUDP:44511`
  - host listening on xr_net `41546/41547/41548` and xr_remote `44510/44511`
- Host log contained release-launch runtime proof:
  - `xr_remote host: GPU readback pipeline active (Mac VT encoder)`
  - `xr_remote host: dual-eye media client connected, forcing keyframe`
  - `xr_remote remote-log [info] quest-client ... startup mode=stereo`
  - `... left/right prepared`
  - `... left/right configured`

## Quest adb verification

### 1) Relaunch without `MAKEPAD_XR_REMOTE_HOST`

Command:

```sh
adb -s 192.168.2.120:5555 shell am force-stop dev.makepad.makepad_example_xr_remote
adb -s 192.168.2.120:5555 shell am start -W -n \
  dev.makepad.makepad_example_xr_remote/dev.makepad.makepad_example_xr_remote.MakepadApp
```

Observed after ~15s:

- Activity switched into XR activity:
  - `ResumedActivity: ... dev.makepad.makepad_example_xr_remote/.MakepadAppXr`
- xr_net transport came up:
  - Quest sockets: UDP `41546/41547`, TCP listen `41548`
  - Host socket: `192.168.2.23:41548->192.168.2.120:34050 (ESTABLISHED)`
- But xr_remote control/media did **not** come up:
  - no Quest TCP `44510` ESTAB
  - no fresh host `44510` accept / remote-log entries
  - no fresh host `dual-eye media client connected` after this relaunch

Interpretation: xr_net peer discovery/sync is alive, but autodiscovery is not promoting the discovered peer IP into the xr_remote control/media connection path.

### 2) Relaunch with explicit host extra

Command:

```sh
adb -s 192.168.2.120:5555 shell am force-stop dev.makepad.makepad_example_xr_remote
adb -s 192.168.2.120:5555 shell am start -W -n \
  dev.makepad.makepad_example_xr_remote/dev.makepad.makepad_example_xr_remote.MakepadApp \
  --es MAKEPAD_XR_REMOTE_HOST 192.168.2.23
```

Observed after ~15s:

- Activity again switched into XR activity:
  - `ResumedActivity: ... dev.makepad.makepad_example_xr_remote/.MakepadAppXr (has extras)`
- xr_remote control connection established:
  - Quest: `192.168.2.120:51924 -> 192.168.2.23:44510 (ESTABLISHED)`
  - Host: `192.168.2.23:44510 -> 192.168.2.120:51924 (ESTABLISHED)`
- xr_net sync also established simultaneously:
  - Quest: `192.168.2.120:52582 -> 192.168.2.23:41548 (ESTABLISHED)`
  - Host: `192.168.2.23:41548 -> 192.168.2.120:52582 (ESTABLISHED)`
- Host log appended fresh proof:
  - `xr_remote host: dual-eye media client connected, forcing keyframe`
  - `xr_remote remote-log [info] quest-client ... startup mode=stereo`
  - `... left prepared`
  - `... right prepared`
  - `... left configured`
  - `... right configured`

Interpretation: shell+adb launch works when the host IP is explicitly injected, and the Mac host is doing the render/encode side successfully enough to reach dual-eye media registration and decoder configuration.

## Main finding

Current blocker is **not** host shell launch or adb relaunch. The blocker is the no-extra / autodiscovery path:

- Quest reaches the host over xr_net (`41548` established)
- but Quest never opens xr_remote control/media (`44510/44511`) unless `MAKEPAD_XR_REMOTE_HOST=192.168.2.23` is provided

That isolates the remaining issue to the client-side handoff from xr_net-discovered peer address to xr_remote control/media connection.
