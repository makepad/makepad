#how to use adb to get the ip of a device
cargo makepad android adb shell ip route
cargo makepad android adb tcpip 5555
cargo makepad android adb connect x.x.x.x:5555
