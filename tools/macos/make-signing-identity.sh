#!/bin/bash
# Creates the stable local code-signing identity "makepad local signing", ONCE per Mac.
#
#   bash tools/macos/make-signing-identity.sh
#
# Why: the installers (tools/aihub, tools/desklink, tools/devhub install-mac.sh) sign their apps.
# Ad-hoc signatures are a new code identity on every build, so macOS privacy (Local Network,
# keychain "Always Allow") asks again after each reinstall. Signed with this identity, every build
# carries the same designated requirement (bundle identifier + this certificate), so macOS asks once.
#
# What it does, with the system's own tools (/usr/bin/openssl, /usr/bin/security, /usr/bin/codesign):
# 1. a self-signed certificate "makepad local signing" (RSA 3072, 20 years) whose only use is code
#    signing (extendedKeyUsage codeSigning, not a CA),
# 2. imports it with its private key into the login keychain; only codesign may use the key,
# 3. trusts the certificate for code signing only, for this user (macOS asks for your password once),
# 4. checks that codesign can sign with it (no further question: the key's access list names codesign).
# The private key lives only in the keychain: the files it is made from are in a private temporary
# folder that is removed when the script ends. Nothing secret is printed.
# Idempotent: when the identity already exists it does nothing. Delete it in Keychain Access
# (login > My Certificates > "makepad local signing") to start over; apps signed with the old one
# then ask once more.
#
# MAKEPAD_SIGN_KEYCHAIN=<path>: use that keychain instead of the login keychain and skip the trust
# step (used by the verification job with a throwaway keychain that it puts in the search list; not
# for normal use).
set -eu
NAME="makepad local signing"
KC=${MAKEPAD_SIGN_KEYCHAIN:-}
if [ -z "$KC" ]; then
    KC=$(security default-keychain -d user | tr -d ' "')
fi

identity_sha1() { # the SHA-1 of the identity named $NAME in $KC (valid = trusted only with -v), or nothing
    security find-identity $1 -p codesigning "$KC" | awk -v w="$NAME" '
        $1 ~ /^[0-9]+\)$/ { q = index($0, "\""); n = substr($0, q + 1); sub(/"[^"]*$/, "", n)
                           if (n == w) { print $2; exit } }'
}

if [ -z "${MAKEPAD_SIGN_KEYCHAIN:-}" ] && [ -n "$(identity_sha1 -v)" ]; then
    echo "the code-signing identity \"$NAME\" already exists in $KC: nothing to do"
    exit 0
fi
if [ -n "${MAKEPAD_SIGN_KEYCHAIN:-}" ] && [ -n "$(identity_sha1 '')" ]; then
    echo "the code-signing identity \"$NAME\" already exists in $KC: nothing to do"
    exit 0
fi

TMP=$(mktemp -d "${TMPDIR:-/tmp}/makepad-signing.XXXXXX")
chmod 700 "$TMP"
cleanup() {
    rm -f "$TMP/key.pem" "$TMP/cert.pem" "$TMP/id.p12" "$TMP/req.cnf" "$TMP/probe"
    rmdir "$TMP"
}
trap cleanup EXIT

if [ -z "$(security find-certificate -c "$NAME" "$KC" 2>/dev/null | head -1)" ]; then
    echo "creating the code-signing certificate \"$NAME\""
    cat > "$TMP/req.cnf" <<CNF
[ req ]
distinguished_name = dn
prompt = no
x509_extensions = ext
[ dn ]
CN = $NAME
O = makepad (this Mac only)
[ ext ]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
CNF
    (umask 077 && /usr/bin/openssl req -x509 -newkey rsa:3072 -nodes -sha256 -days 7300 \
        -config "$TMP/req.cnf" -keyout "$TMP/key.pem" -out "$TMP/cert.pem" 2>/dev/null)
    # pkcs12 only carries key + certificate into the keychain in one import; its password protects
    # nothing (the file is removed below), it just has to be the same on both sides
    (umask 077 && /usr/bin/openssl pkcs12 -export -name "$NAME" -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
        -out "$TMP/id.p12" -passout pass:makepad)
    rm -f "$TMP/key.pem"
    security import "$TMP/id.p12" -k "$KC" -f pkcs12 -P makepad -T /usr/bin/codesign > /dev/null
    rm -f "$TMP/id.p12"
else
    echo "the certificate \"$NAME\" is in $KC already; finishing its setup"
    security find-certificate -c "$NAME" -p "$KC" > "$TMP/cert.pem"
fi

if [ -z "${MAKEPAD_SIGN_KEYCHAIN:-}" ]; then
    echo
    echo "macOS now asks for your password ONCE, to trust \"$NAME\" for code signing (this user only)."
    security add-trusted-cert -r trustRoot -p codeSign -k "$KC" "$TMP/cert.pem"
fi

# codesign must be able to use the key (the import gave codesign access to it)
cp /usr/bin/true "$TMP/probe"
SHA1=$(identity_sha1 '')
[ -n "$SHA1" ] || { echo "the identity \"$NAME\" was not found in $KC after the import" >&2; exit 1; }
if ! codesign -s "$SHA1" --keychain "$KC" --force "$TMP/probe" 2> /dev/null; then
    echo "codesign cannot sign with \"$NAME\"; check its key in Keychain Access (login > My Certificates)" >&2
    exit 1
fi

echo
echo "done: \"$NAME\" (SHA-1 $SHA1) is ready."
echo "reinstall the apps once (bash tools/aihub/install-mac.sh, tools/desklink/install-mac.sh, tools/devhub/install-mac.sh);"
echo "macOS asks its privacy questions once more for each, and never again after later reinstalls."
