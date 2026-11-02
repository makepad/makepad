#!/bin/bash
# ios_bundle_size.sh [<package>]: what the makepad run host app for iOS ships and how big it is, from
# the workspace root on a Mac with the aarch64-apple-ios target (a devhub build job). Builds the
# device bundle (`cargo makepad apple ios build-host`), then prints the bundle's files, the binary
# as built and stripped, its segments, its code and data per crate (the linker map of the same
# build) and the source store's breakdown (pack-src's own report).
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
pkg=${1:-makepad-example-counter}
T=${CARGO_TARGET_DIR:-$PWD/target}
work=$(mktemp -d /tmp/iossize.XXXXXX) || exit 2
cargo build --release -q -p cargo-makepad || { echo "SIZE-FAIL: cargo-makepad build"; exit 1; }
if ! target/release/cargo-makepad apple ios --org=dev.makepad build-host "$pkg" > "$work/build" 2>&1; then
    grep -v '^\s*$' "$work/build" | grep -v -E '^ +(Compiling|Checking)' | tail -60 | cut -c1-300
    echo "SIZE-FAIL: build-host"; exit 1
fi
sed -n '/^pack-src: /,$p' "$work/build" | head -40
app=$(sed -n 's/^Host app built: \(.*\.app\) (.*/\1/p' "$work/build")
echo "--- bundle $app: $(du -sk "$app" | cut -f1) KB"
ls -l "$app" | awk 'NR > 1 { printf "  %10d  %s\n", $5, $9 }'
bin=$app/makepad
cp "$bin" "$work/stripped"
strip -x "$work/stripped" 2>/dev/null
echo "binary: $(stat -f %z "$bin") bytes in the bundle, $(stat -f %z "$work/stripped") after strip -x"
# what a download is (an .ipa is a zip of the bundle)
ditto -c -k --keepParent "$app" "$work/app.zip" && echo "bundle zipped (download size): $(stat -f %z "$work/app.zip") bytes"
size -m "$work/stripped" | grep -E '^Segment|Section (__text|__const|__data|__bss|__cstring|__unwind_info|__eh_frame)' | head -30
# the per-crate breakdown: the same host-profile link with a linker map
map=$work/map.txt
if cargo rustc --profile host -q -p makepad-run --bin makepad --target aarch64-apple-ios -- -C link-arg=-Wl,-map,"$map" > "$work/rustc" 2>&1 && [ -s "$map" ]; then
    echo "--- linked size per library (the map's symbols by their object file's library):"
    # per library and section kind (code = __text, data = the rest), the top 40, then the 25
    # largest symbols
    perl -ne '
        if (/^# Object files:/) { $s = 1; next }
        if (/^# Sections:/) { $s = 3; next }
        if (/^# Symbols:/) { $s = 2; next }
        if (/^# Dead Stripped Symbols:/) { $s = 0 }
        if ($s == 1 && /^\[\s*(\d+)\]\s+(.*)$/) {
            my ($i, $f) = ($1, $2); $f =~ s/\(.*$//; $f =~ s/^.*\///; $f =~ s/^lib//; $f =~ s/-[0-9a-f]+\.rlib$//; $f =~ s/\.(rlib|o|a|tbd|dylib)$//;
            $lib{$i} = $f;
        }
        if ($s == 3 && /^(0x[0-9A-Fa-f]+)\s+(0x[0-9A-Fa-f]+)\s+(\S+)\s+(\S+)/) { push @sec, [hex($1), hex($1) + hex($2), $4] }
        if ($s == 2 && /^(0x[0-9A-Fa-f]+)\s+(0x[0-9A-Fa-f]+)\s+\[\s*(\d+)\]\s+(.*)$/) {
            my ($a, $z, $l, $n) = (hex($1), hex($2), $lib{$3}, $4);
            my $k = "data"; for my $x (@sec) { if ($a >= $x->[0] && $a < $x->[1]) { $k = $x->[2] eq "__text" ? "code" : $x->[2]; last } }
            $by{"$l $k"} += $z; $t += $z; push @big, [$z, "$l $k $n"];
            # Relax crates by module (v0 mangling: <len><crate><len><module>)
            if ($n =~ /\d+(relax_(?:compiler|backend|types|front|codegen|base))(\d+)/) { my ($c, $m) = ($1, $2); my $r = substr($n, $+[2]); $mod{"$c::" . substr($r, 0, $m)} += $z }
        }
        END {
            my $c = 0; for my $k (sort { $by{$b} <=> $by{$a} } keys %by) { last if $c++ >= 40; printf "%12d  %5.1f%%  %s\n", $by{$k}, 100 * $by{$k} / $t, $k }
            printf "%12d  total\n--- the 25 largest symbols:\n", $t;
            $c = 0; for my $x (sort { $b->[0] <=> $a->[0] } @big) { last if $c++ >= 25; printf "%12d  %s\n", $x->[0], substr($x->[1], 0, 150) }
            print "--- Relax crates by module (the 40 largest):\n";
            $c = 0; for my $k (sort { $mod{$b} <=> $mod{$a} } keys %mod) { last if $c++ >= 40; printf "%12d  %s\n", $mod{$k}, $k }
        }
    ' "$map"
else
    echo "(no linker map: $(tail -3 "$work/rustc"))"
fi
rm -r -- "${work:?}"
echo "SIZE-DONE"
