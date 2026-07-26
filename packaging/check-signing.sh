#!/usr/bin/env bash
#
# Assert that the six macOS signing secrets are actually usable, before the
# ~20-minute build matrix spends a runner discovering it.
#
#   packaging/check-signing.sh          # reads the same env vars build.sh does
#
# The failure this exists for is the Tauri bundler's own message:
#
#   security: SecKeychainItemImport: One or more parameters passed to a
#   function were not valid.
#   Error failed to bundle project: failed codesign application
#
# That is errSecParam from `security import`, and it names none of the six
# secrets. It means the bytes in APPLE_CERTIFICATE decoded to something that is
# not a PKCS#12 bundle — almost always a .cer downloaded from the developer
# portal (a bare certificate, no private key) rather than a .p12 exported from
# Keychain Access. This script says which one is wrong and how to remake it.
#
# Nothing here prints a secret. Certificate subject and expiry are printed
# because they are public: they are stamped into every signed binary shipped.
set -euo pipefail

fail() { echo "check-signing.sh: $*" >&2; exit 1; }

# Same switch build.sh reads, so one variable governs the whole pipeline rather
# than a workflow condition here disagreeing with a --no-sign flag over there.
if [[ -n "${NO_SIGN:-}" ]]; then
  echo "NO_SIGN is set — unsigned build, signing secrets not checked"
  exit 0
fi

for v in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY \
         APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  [[ -n "${!v:-}" ]] || fail "$v is empty or unset."
done

# A .p12 exported with a blank password imports as errSecParam, the same error
# as a malformed file, so Keychain Access's "no password" option is a trap.
[[ ${#APPLE_CERTIFICATE_PASSWORD} -ge 1 ]] || fail "APPLE_CERTIFICATE_PASSWORD is empty; export the .p12 with a password."

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# The bundler decodes with a strict base64 engine, so an embedded newline is
# fatal there even though `base64 -d` here would swallow it. Strip only a
# trailing newline (which the secrets UI adds), reject anything else.
B64="${APPLE_CERTIFICATE%$'\n'}"
case "$B64" in
  *[[:space:]]*)
    fail "APPLE_CERTIFICATE contains whitespace. Re-encode with: base64 -i cert.p12 | tr -d '\\n'" ;;
esac

printf '%s' "$B64" | base64 --decode > "$TMP/cert.p12" 2>/dev/null \
  || printf '%s' "$B64" | base64 -D > "$TMP/cert.p12" 2>/dev/null \
  || fail "APPLE_CERTIFICATE is not valid base64."
[[ -s "$TMP/cert.p12" ]] || fail "APPLE_CERTIFICATE decoded to zero bytes."

# Name the two near-misses explicitly. Both are things you can plausibly have on
# disk when you go looking for "the certificate", and neither is a p12.
if openssl x509 -inform DER -in "$TMP/cert.p12" -noout 2>/dev/null; then
  fail "APPLE_CERTIFICATE is a DER certificate (.cer) — public half only, no private key.
    Export the identity from Keychain Access instead (My Certificates -> right-click -> Export -> .p12)."
fi
if head -c 64 "$TMP/cert.p12" | grep -q -- '-----BEGIN'; then
  fail "APPLE_CERTIFICATE is a PEM file, not a PKCS#12 bundle. Export a .p12 from Keychain Access."
fi

# OpenSSL 3 refuses Keychain Access's RC2-encrypted p12s without -legacy;
# LibreSSL (/usr/bin/openssl on macOS) has no such flag and needs none.
#
# The password goes via a 0600 file rather than `env:`, and stdin is closed:
# openssl writes its passphrase prompt to /dev/tty, so a source it cannot read
# turns into an interactive hang (or a prompt that scrolls past, mid-CI-log)
# instead of an error. With no stdin it fails immediately and we say why.
umask 077
printf '%s' "$APPLE_CERTIFICATE_PASSWORD" > "$TMP/pw"
P12=(-in "$TMP/cert.p12" -nokeys -clcerts -passin "file:$TMP/pw" -out "$TMP/cert.pem")
err="$(openssl pkcs12 "${P12[@]}" 2>&1 </dev/null)" || {
  if openssl pkcs12 -help 2>&1 | grep -q -- '-legacy'; then
    err="$(openssl pkcs12 -legacy "${P12[@]}" 2>&1 </dev/null)" || {
      case "$err" in
        *[Mm]ac*verif*|*invalid\ password*|*[Ii]nvalid\ MAC*)
          fail "APPLE_CERTIFICATE_PASSWORD does not open the .p12." ;;
      esac
      fail "APPLE_CERTIFICATE is not a readable PKCS#12 bundle. openssl said: $err"
    }
  else
    case "$err" in
      *[Mm]ac*verif*|*invalid\ password*|*[Ii]nvalid\ MAC*)
        fail "APPLE_CERTIFICATE_PASSWORD does not open the .p12." ;;
    esac
    fail "APPLE_CERTIFICATE is not a readable PKCS#12 bundle. openssl said: $err"
  fi
}

# sep_multiline because OpenSSL and LibreSSL (/usr/bin/openssl) disagree about
# how a one-line subject is punctuated; they agree about this one.
cn="$(openssl x509 -in "$TMP/cert.pem" -noout -subject -nameopt sep_multiline,utf8 2>/dev/null \
      | sed -n 's/^ *CN=//p' | head -1)"
[[ -n "$cn" ]] || fail "the .p12 opened but holds no usable client certificate."

# Notarization only accepts Developer ID. An Apple Development or Apple
# Distribution cert signs fine and then fails at the notary, twenty minutes in.
case "$cn" in
  "Developer ID Application: "*) ;;
  *) fail "certificate is '$cn'; notarized distribution outside the App Store needs a 'Developer ID Application' certificate." ;;
esac

# codesign matches APPLE_SIGNING_IDENTITY against the keychain by name, and a
# near-miss yields "no identity found" rather than anything about this secret.
if [[ "$APPLE_SIGNING_IDENTITY" != "$cn" ]]; then
  fail "APPLE_SIGNING_IDENTITY does not match the certificate.
    secret: $APPLE_SIGNING_IDENTITY
    cert:   $cn"
fi

case "$cn" in
  *"($APPLE_TEAM_ID)") ;;
  *) fail "APPLE_TEAM_ID ($APPLE_TEAM_ID) is not the team in '$cn'." ;;
esac

openssl x509 -in "$TMP/cert.pem" -noout -checkend 0 >/dev/null \
  || fail "the signing certificate has expired ($(openssl x509 -in "$TMP/cert.pem" -noout -enddate))."
# Five years is the Developer ID maximum, so a renewal is never urgent-but-silent.
openssl x509 -in "$TMP/cert.pem" -noout -checkend 2592000 >/dev/null \
  || echo "    warning: certificate expires within 30 days ($(openssl x509 -in "$TMP/cert.pem" -noout -enddate))"

# The notary rejects an account password; it wants an app-specific one, minted at
# appleid.apple.com, and it is always four groups of four lowercase letters.
[[ "$APPLE_PASSWORD" =~ ^[a-z]{4}-[a-z]{4}-[a-z]{4}-[a-z]{4}$ ]] \
  || fail "APPLE_PASSWORD is not an app-specific password (xxxx-xxxx-xxxx-xxxx). Generate one at appleid.apple.com -> Sign-In and Security."
[[ "$APPLE_ID" == *@* ]] || fail "APPLE_ID is not an email address."
[[ "$APPLE_TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || fail "APPLE_TEAM_ID is not a 10-character team id."

echo "signing secrets look usable"
echo "    identity: $cn"
echo "    expires:  $(openssl x509 -in "$TMP/cert.pem" -noout -enddate | cut -d= -f2)"
