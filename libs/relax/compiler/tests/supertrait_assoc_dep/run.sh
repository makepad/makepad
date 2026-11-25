#!/bin/bash
# usage: run.sh <relax binary>: a call into a registry dependency reaches an impl item whose signature
# names a supertrait's associated type of a header type with a `'static` parameter bound (sad-list:
# `ShardedItem::shard_id(NonNull<Self::Target>)` for `Task<S: 'static>`, called from
# `ShardedList<L, L::Target>::push`). A registry crate is checked when its code is first built, so a
# wrong refusal of the impl item stopped the run at that call (E0053; before, a panic in instance
# resolution). The copy puts sad-list where its code is checked as a crates.io dependency's: outside the
# app's own crates (under libs/) and where the live watcher does not look (a registry folder). Prints
# "supertrait_assoc_dep: ok" or "supertrait_assoc_dep: FAILED" (exit 1); the expected line is
# `cargo run -p sad-app`'s.
H=$(cd "$(dirname "$0")" && pwd)
D=$(mktemp -d "${TMPDIR:-/tmp}/tmp.XXXXXXXXXX") || exit 1
trap 'rm -rf "$D"' EXIT
reg=libs/cache/.cargo/registry/src/index-0/sad-list-0.1.0
cp -R "$H/app" "$H/Cargo.toml" "$D/" && mkdir -p "$D/${reg%/*}" && cp -R "$H/list" "$D/$reg" || exit 1
perl -pi -e "s|\"list\"|\"$reg\"|" "$D/Cargo.toml" && perl -pi -e "s|[.][.]/list|../$reg|" "$D/app/Cargo.toml" || exit 1
export RELAX_CACHE=off CARGO_NET_OFFLINE=true
OUT=$(cd "$D" && RELAX_STD=$H/../../std "$1" run --quiet sad-app 2>"$D/err")
echo "$OUT"
if [ "$OUT" = "supertrait_assoc_dep 2 3" ]; then echo "supertrait_assoc_dep: ok"; else tail -5 "$D/err"; echo "supertrait_assoc_dep: FAILED"; exit 1; fi
