# Signing and notarization

**This is done.** Signing went on 2026-07-26; every release since is signed,
notarized and stapled, and `brew install --cask` and browser downloads are both
live channels. Cutting a release today is `git push --tags` plus publishing the
draft — nothing below is on a schedule.

What is still live is at the end: **Rotating or renewing later** and **Backing it
out**. Both send you back into the numbered steps — a certificate renewal is steps
1, 2, 5 and 6 again, verbatim — which is why all twelve stay here in full rather
than collapsed into a summary. Read them as the record of how it was turned on,
in the order it had to happen; the order still matters in one place (step 5 before
step 6) and is noted there.

---

## Why it was off, and what the two switches are

`com.apple.quarantine` is written by the *downloading application*, not by the
server. `curl` never writes it, so an unsigned `.app` fetched by
`packaging/install.sh` never meets Gatekeeper and opens fine. A browser download
and `brew install --cask` both **do** quarantine, so an unsigned app arriving that
way opens as *"dreamd is damaged and can't be opened"* — which reads to a user as a
broken release rather than as a signing policy.

So the two quarantining channels were deliberately paused and curl was the only
supported one. This document undid that. It was not fixing a bug; it was making a
different choice, which became available once there was a Developer ID certificate
to sign with.

Two switches hold the decision, in whichever direction it is set:

- `NO_SIGN` in `.github/workflows/release.yml`. It makes `check-signing.sh` stand
  down and `build.sh` pass `--no-sign`. One variable governs the whole pipeline,
  so a workflow condition here can never disagree with a flag over there. **It is
  absent now, and that absence is the switch** — do not reintroduce it to get past
  a red build; run `check-signing.sh` against the secrets instead.
- The `PUBLISH_CASK` repository variable, which the `tap` job is gated on. **Now
  `true`.** `PUBLISH_AUR` gates the `aur` job the same way and is its own decision.

---

## Current state

| Thing | State |
|---|---|
| Developer ID Application certificate | **present** in the login keychain — `Developer ID Application: OLIVER ONSTOTT FONG (34VGHNCG6J)` |
| Team ID | `34VGHNCG6J` |
| Six `APPLE_*` repo secrets | **set and validated** — `verify` runs `check-signing.sh` against them before every build matrix |
| `TAP_GITHUB_TOKEN` secret | **set** (step 7) |
| `PUBLISH_CASK` variable | **`true`** |
| `bongofongo/homebrew-tap` repo | exists, public, and carries a `Casks/dreamd.rb` generated from `packaging/cask.rb.tmpl` |

Confirm the certificate for yourself with:

```sh
security find-identity -v -p codesigning
```

You want a line reading `Developer ID Application: ...`. An **Apple Development**
or **Apple Distribution** certificate is not a substitute — both sign happily and
are then rejected by the notary twenty minutes into the build. `check-signing.sh`
asserts this specifically for that reason.

If a rotation ever leaves you unsure which secret is stale, redo steps 1–6 and
re-upload all six rather than guessing. That is cheaper than bisecting a
twenty-minute matrix — which is how these steps came to be written in that order
in the first place.

---

## 1. Export the certificate as a `.p12`

The Tauri bundler imports a **PKCS#12 bundle** — certificate *and* private key —
into a temporary keychain on the runner. A `.cer` downloaded from the developer
portal is the public half only and will not work.

**Keychain Access** → **login** keychain → **My Certificates** category → find:

```
Developer ID Application: OLIVER ONSTOTT FONG (34VGHNCG6J)
```

There must be a **disclosure triangle** next to it, with a private key underneath.
If there is no triangle, the private key is not on this machine: you must request
a new certificate from the developer portal with a fresh CSR generated here, or
export the key from wherever it does live.

Right-click the certificate → **Export "Developer ID Application: …"** → file
format **Personal Information Exchange (.p12)** → save as `~/dreamd-signing.p12`.

> **Set a password when prompted. Do not leave it blank.**
>
> A `.p12` exported with an empty password fails `security import` with
> `errSecParam` — byte-for-byte the same error as a corrupt file or a `.cer`. This
> is the single most common way this whole process goes wrong, and the bundler's
> message for it names none of the six secrets:
>
> ```
> security: SecKeychainItemImport: One or more parameters passed to a function
> were not valid.
> Error failed to bundle project: failed codesign application
> ```

macOS may then ask for your login keychain password to authorise the export. That
is a different password from the one you just chose; do not confuse them.

## 2. Base64-encode it with no line breaks

```sh
base64 -i ~/dreamd-signing.p12 | tr -d '\n' > ~/dreamd-cert.b64
```

`tr -d '\n'` is **required**. `base64` wraps output at 76 columns by default, and
the bundler decodes with a strict engine that treats an embedded newline as fatal —
even though a local `base64 -d` would swallow it happily. `check-signing.sh`
rejects any whitespace in the value for exactly this reason.

Sanity check that it decoded to something real:

```sh
wc -c < ~/dreamd-cert.b64          # expect a few thousand bytes, not 0
```

## 3. Mint an app-specific password

The notary service rejects a plain Apple ID password. It wants an app-specific one.

**appleid.apple.com** → sign in → **Sign-In and Security** → **App-Specific
Passwords** → **+** → name it something like `dreamd notarization`.

The result is always four groups of four lowercase letters:
`abcd-efgh-ijkl-mnop`. Copy it now — Apple will not show it again.

The Apple ID you use must be a member of team `34VGHNCG6J`. If it is a personal
Apple ID that merely owns the certificate, that is fine; if it is a different
account entirely, notarization will fail with an authentication error.

## 4. Assemble the six values

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | the contents of `~/dreamd-cert.b64` |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password chosen in step 1 |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: OLIVER ONSTOTT FONG (34VGHNCG6J)` — exactly, including the parenthesised team ID |
| `APPLE_ID` | the Apple ID email address that owns the developer account |
| `APPLE_PASSWORD` | the app-specific password from step 3 |
| `APPLE_TEAM_ID` | `34VGHNCG6J` |

`APPLE_SIGNING_IDENTITY` is matched against the keychain **by name**. A near-miss
— a missing space, a truncated name, the wrong case — produces `no identity found`
and says nothing about this secret. Copy it from `security find-identity` output
rather than retyping it.

## 5. Validate locally — **before** uploading anything

This is what `packaging/check-signing.sh` exists for. It reads the same environment
variables `build.sh` does and checks all six in about ten seconds, naming the one
that is wrong and how to remake it. The alternative is discovering it after a
~20-minute build, on both runners at once.

```sh
cd /Users/oliverfong/toadmountain/dreamd

unset NO_SIGN                       # otherwise the script exits 0 without checking

export APPLE_CERTIFICATE="$(cat ~/dreamd-cert.b64)"
read -rs -p "p12 password: "         APPLE_CERTIFICATE_PASSWORD; export APPLE_CERTIFICATE_PASSWORD; echo
export APPLE_SIGNING_IDENTITY="Developer ID Application: OLIVER ONSTOTT FONG (34VGHNCG6J)"
export APPLE_ID="you@example.com"
read -rs -p "app-specific password: " APPLE_PASSWORD; export APPLE_PASSWORD; echo
export APPLE_TEAM_ID="34VGHNCG6J"

packaging/check-signing.sh
```

`read -rs` keeps both passwords out of your shell history and off the screen. Keep
this shell open — step 6 uses it.

Expected output:

```
signing secrets look usable
    identity: Developer ID Application: OLIVER ONSTOTT FONG (34VGHNCG6J)
    expires:  <date>
```

Nothing secret is printed. The subject and expiry are public — they are stamped
into every signed binary you ship.

### What it can tell you, and what to do

| Message | Cause | Fix |
|---|---|---|
| `NO_SIGN is set — …not checked` | you forgot `unset NO_SIGN` | unset it and re-run |
| `<VAR> is empty or unset` | typo in an export | re-export |
| `APPLE_CERTIFICATE_PASSWORD is empty` | blank-password `.p12` | redo step 1 with a password |
| `APPLE_CERTIFICATE contains whitespace` | missed `tr -d '\n'` | redo step 2 |
| `APPLE_CERTIFICATE is not valid base64` | truncated paste | redo step 2 |
| `…is a DER certificate (.cer)` | exported from the developer portal, no private key | export from Keychain Access instead |
| `…is a PEM file` | wrong export format | choose `.p12` in the format dropdown |
| `APPLE_CERTIFICATE_PASSWORD does not open the .p12` | wrong password | you likely typed the login-keychain password |
| `certificate is 'Apple Development: …'` | wrong certificate type | request a Developer ID Application certificate |
| `APPLE_SIGNING_IDENTITY does not match the certificate` | name string is off | copy it verbatim from the error output |
| `APPLE_TEAM_ID is not the team in …` | wrong team | use the ID in the parentheses |
| `the signing certificate has expired` | past its five-year life | renew in the developer portal |
| `APPLE_PASSWORD is not an app-specific password` | you used the account password | redo step 3 |

A `warning: certificate expires within 30 days` is not a failure, but do renew.

## 6. Upload the secrets from the same shell

Same shell means the values GitHub receives are byte-identical to the ones that
just passed validation. Re-typing them is how a trailing space gets in.

```sh
for v in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY \
         APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  printf '%s' "${!v}" | gh secret set "$v"
done
```

`printf '%s'` rather than `echo`, so no trailing newline is appended.
`check-signing.sh` strips one trailing newline from `APPLE_CERTIFICATE` defensively,
but the other five are compared literally and a stray newline in
`APPLE_SIGNING_IDENTITY` would break the name match.

Confirm all six are present:

```sh
gh secret list
```

The web UI (**Settings → Secrets and variables → Actions**) is equally fine, but
the textarea appends a newline to whatever you paste, which is the reason
`check-signing.sh` tolerates exactly one trailing newline on the certificate.

## 7. Add the tap token

The `tap` job checks out `bongofongo/homebrew-tap` with a token and commits the
rendered cask to it. `secrets.GITHUB_TOKEN` cannot reach another repository, so
this is a separate personal access token. **It is currently missing.**

github.com/settings/tokens → **Fine-grained tokens** → **Generate new token**:

- **Repository access**: *Only select repositories* → `bongofongo/homebrew-tap`
- **Permissions** → Repository permissions → **Contents: Read and write**
- Expiry: whatever you will actually remember to renew. A fine-grained token
  cannot be set to never expire, so the cask bump *will* break silently one day —
  put the date in a calendar.

Nothing else is needed. Do not grant it access to `bongofongo/dreamd`.

```sh
gh secret set TAP_GITHUB_TOKEN      # paste the token, then Ctrl-D
```

## 8. Flip the two switches

This is the step "Backing it out" inverts, so both directions are written here.

**a.** Delete the `env` block at the top of `.github/workflows/release.yml`:

```yaml
env:
  NO_SIGN: "1"
```

Rewrite the long comment above it — it explained why releases were unsigned,
which stops being true the moment you delete the key. Say instead that signing is
on, and point at this file for how to rotate the secrets. (Done: that comment now
says the *absence* of the key is the switch.)

**b.** Arm the cask job:

```sh
gh variable set PUBLISH_CASK --body true
gh variable list                      # confirm
```

**c.** The root `CLAUDE.md` **Packaging** section and `website/CLAUDE.md`'s
"There is no download button" gotcha both documented unsigned-on-purpose as the
standing decision. Update both in the same commit, or the next session will read
them and re-derive the wrong state of the world. (Both done — and this file was
the one that was missed, which is the whole argument for the sentence above.)

Commit these together:

```sh
git add .github/workflows/release.yml CLAUDE.md website/CLAUDE.md packaging/SIGNING.md
git commit -m "ci: turn on signing and notarization"
git push
```

## 9. Rehearse without cutting a tag

```sh
gh workflow run release
gh run watch
```

`workflow_dispatch` runs `verify` and `build` but **not** `release` — that job is
gated on `github.event_name == 'push'`. So you get a genuine
sign → notarize → staple → zip pass with no public artifact and no draft.

What to expect:

- `verify` now actually runs `check-signing.sh` against the uploaded secrets. If
  step 5 passed and step 6 uploaded cleanly, this passes in seconds.
- Each `build` job takes roughly 8 minutes to compile plus however long the notary
  takes. **Notarization is the slow, variable part** — usually a few minutes,
  occasionally much longer.
- `build.sh` asserts the result itself with `codesign --verify` and
  `stapler validate`, because notarization silently no-ops when an environment
  variable is misspelled; without that assertion the failure would first surface on
  a user's machine.

**If the notary hangs or rejects on a first submission**, log in to App Store
Connect and check for unaccepted agreements — a new team account frequently has a
pending licence agreement that blocks notarization with an unhelpful error.

### Verify a build artifact the way a browser download behaves

Downloading the artifact through the browser or `gh` does not reproduce the
quarantine flag, so set it by hand. This is the check that distinguishes a working
signed release from the current curl-only arrangement:

```sh
gh run download <run-id> -n aarch64-apple-darwin -D /tmp/dreamd-check
cd /tmp/dreamd-check
ditto -x -k dreamd-*.zip .

xattr -w com.apple.quarantine "0083;00000000;Safari;" dreamd.app

spctl -a -vvv -t exec dreamd.app     # want: accepted, source=Notarized Developer ID
xcrun stapler validate dreamd.app    # want: The validate action worked
open dreamd.app                      # want: it opens, with no dialog at all
```

Extract with `ditto`, never `tar` — part of a `.app`'s signature lives in extended
attributes, and `tar` drops them, so a tar round-trip fails verification even for a
correctly signed app. (This is also why the release artifacts are `.zip`.)

If `spctl` accepts a *quarantined* copy, both Homebrew and browser downloads will
work. That is the whole objective.

## 10. Cut a real release

```sh
packaging/set-version.sh 0.1.1        # Cargo.toml + website/src/consts.ts + Cargo.lock
cargo build                           # must pass; touches src-tauri/
git commit -am "release: 0.1.1, signed and notarized"
git tag v0.1.1
git push && git push --tags
```

Then:

1. The tag push runs `verify` → `build` → `release`, producing a **draft** release
   with four files per arch pattern (`.zip` and `.zip.sha256`).
2. Download the draft's artifacts and re-run the step 9 verification on a machine
   that has never built dreamd — ideally one without your certificate installed,
   since a local certificate can mask a signing problem.
3. **Publish the release by hand** in the GitHub UI. The draft step is the point: a
   release is public the instant it exists and Homebrew users pull it within
   minutes, so nothing reaches anyone until a human has double-clicked the thing.
4. Publishing fires the `tap` job, which downloads the `.sha256` files the build
   already computed, substitutes the three `@@` tokens in `packaging/cask.rb.tmpl`,
   runs `brew audit --cask --online`, and commits `Casks/dreamd.rb` to the tap.

Confirm end to end:

```sh
brew update
brew install --cask bongofongo/tap/dreamd
open -a dreamd
```

If `brew audit` fails in CI, the cask is not committed and the release is still
fine — fix the template and re-run the job rather than re-cutting the tag.

## 11. Update the website

`website/src/pages/index.astro` currently offers exactly one install command and
explains why (lines ~50–61):

```
curl -fsSL {INSTALL_URL} | sh
```

> …before it installs. Homebrew follows once there is a signature to check.

Once step 10 lands, that sentence is false. Restore the Homebrew line:

```sh
brew install --cask bongofongo/tap/dreamd
```

If you add a download button, point it at `RELEASES_URL` — already exported from
`src/consts.ts` and currently unused for precisely this moment. Point it at
`/releases/latest`, **never** a pinned asset URL: the site deploys by a manual
`npm run deploy` that is independent of the release workflow, so a pinned href
would 404 in the window between cutting a tag and deploying the site.

The site is a separate deploy with no CI:

```sh
cd website
npm run build && npm run preview      # check it locally first
npm run deploy                        # publishes to the live zone
```

Also update `website/CLAUDE.md`'s gotcha about there being no download button —
it is the source of truth for that directory and will otherwise contradict the page.

## 12. Clean up

```sh
rm ~/dreamd-signing.p12 ~/dreamd-cert.b64
```

The certificate stays in your keychain; only the exported copies go. An exported
`.p12` sitting in your home directory is a signing identity anyone with disk access
can take.

---

## Rotating or renewing later

- **Certificate expires** (five-year maximum, and `check-signing.sh` warns at 30
  days): request a new Developer ID Application certificate, then redo steps 1, 2,
  5, 6. `APPLE_SIGNING_IDENTITY` usually does not change, since it is derived from
  your name and team ID.
- **App-specific password revoked**: redo step 3, then re-upload `APPLE_PASSWORD`
  only.
- **`TAP_GITHUB_TOKEN` expires**: redo step 7. The symptom is the `tap` job failing
  at checkout with a 404 on a repository that plainly exists.

Re-run `packaging/check-signing.sh` locally after any rotation. It is fifteen
seconds and it is the only thing standing between a typo and a failed release.

## Backing it out

Signing off again is one key. Add back to `.github/workflows/release.yml`:

```yaml
env:
  NO_SIGN: "1"
```

and `gh variable set PUBLISH_CASK --body false`. The secrets can stay; they are
simply not read. Then restore the curl-only copy on the website, because Homebrew
would go on serving a cask that points at unsigned artifacts.
