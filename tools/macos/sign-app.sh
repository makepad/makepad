#!/bin/bash
# sign-app.sh <bundle.app> <bundle identifier>   -- signs an app bundle for the macOS installers.
#
# With the local identity "makepad local signing" (made once by tools/macos/make-signing-identity.sh)
# the bundle gets the designated requirement
#     identifier "<bundle identifier>" and certificate leaf = H"<the certificate's SHA-1>"
# which is the same for every build, so macOS privacy grants (Local Network, keychain access) stay.
# Without it: ad-hoc (a new code identity each build, macOS asks again), with a one-line hint.
#
# MAKEPAD_SIGN_IDENTITY=<name or SHA-1>: sign with that identity instead (it must exist; it may be
#   one not yet trusted); MAKEPAD_SIGN_IDENTITY=- forces ad-hoc.
# MAKEPAD_SIGN_KEYCHAIN=<path>: look the identity up in that keychain only.
set -eu
APP=$1
ID=$2
NAME="makepad local signing"
WANT=${MAKEPAD_SIGN_IDENTITY:-}
KC=${MAKEPAD_SIGN_KEYCHAIN:-}

find_sha1() { # find_sha1 <-v or ''> <name or sha1>: the SHA-1 of the matching code-signing identity
    security find-identity $1 -p codesigning ${KC:+"$KC"} | awk -v w="$2" '
        $1 ~ /^[0-9]+\)$/ { q = index($0, "\""); n = substr($0, q + 1); sub(/"[^"]*$/, "", n)
                           if (toupper($2) == toupper(w) || n == w) { print $2; exit } }'
}

SHA1=
if [ "$WANT" = "-" ]; then
    :
elif [ -n "$WANT" ]; then
    SHA1=$(find_sha1 '' "$WANT")
    [ -n "$SHA1" ] || { echo "MAKEPAD_SIGN_IDENTITY=$WANT: no such code-signing identity" >&2; exit 1; }
else
    SHA1=$(find_sha1 -v "$NAME")
fi

if [ -n "$SHA1" ]; then
    codesign -s "$SHA1" ${KC:+--keychain "$KC"} --identifier "$ID" --force \
        --requirements "=designated => identifier \"$ID\" and certificate leaf = H\"$SHA1\"" "$APP"
    echo "signed $ID with ${WANT:-$NAME} (stable identity: macOS privacy answers stay across reinstalls)"
else
    codesign -s - --identifier "$ID" --force "$APP"
    if [ "$WANT" != "-" ]; then
        echo "signed $ID ad-hoc: macOS asks its privacy questions again after every reinstall; run bash tools/macos/make-signing-identity.sh once to stop that"
    fi
fi
codesign --verify --strict "$APP"
