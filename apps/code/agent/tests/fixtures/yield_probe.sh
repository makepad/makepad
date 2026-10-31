#!/bin/sh
# A build that fails early and keeps going: a few lines, a rustc-style
# error at 2 s, then more output once a second until it exits 1 at 22 s.
# Used to measure when run_command's result reaches the model.
echo "   Compiling probe-a v0.1.0 (/ws/probe-a)"
echo "   Compiling probe-b v0.1.0 (/ws/probe-b)"
sleep 2
cat <<'ERR'
error[E0308]: mismatched types
  --> src/main.rs:3:14
   |
3  |     let x: u32 = "five";
   |            ---   ^^^^^^ expected `u32`, found `&str`
   |            |
   |            expected due to this

ERR
i=0
while [ $i -lt 20 ]; do
    sleep 1
    i=$((i + 1))
    echo "   Compiling probe-c$i v0.1.0 (/ws/probe-c$i)"
done
echo "error: could not compile \`probe\` (bin \"probe\") due to 1 previous error"
exit 1
