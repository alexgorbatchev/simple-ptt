---
created_on: 2026-11-03 06:25
last_modified: 2026-11-03 06:25
status: current
---

# Developer ID distribution

Maintainers use `scripts/build-notarized-dmg.sh` for distribution builds. It signs the app and each Sparkle executable with Developer ID, a secure timestamp, and hardened runtime; notarizes and staples the app; then builds, signs, notarizes, and staples the DMG. It verifies the app extracted through a read-only DMG mount without launching it. Any failed check stops publication. The final SHA-256 file names the DMG by basename and describes its bytes after stapling.

`just bundle-release`, `just bundle-dmg`, and `just install-app` are local development recipes. Their default identity is `-` (ad hoc), and they do not notarize. Set `APPLE_SIGNING_IDENTITY=-` explicitly when a development shell also has release credentials configured. The notarized build command requires a Developer ID identity and rejects `-`.

The app entitlement is `com.apple.security.device.audio-input`; it does not disable library validation or enable debugging. Sparkle's downloader retains the sandbox entitlements from the checksum-verified upstream archive. `scripts/sign-macos-app.sh` signs the nested XPC services, executable helper, updater app, framework, and outer app in that order. Use this helper after modifying a development bundle; `codesign --deep` is for verification only.

## Local verification

1. Use a valid Developer ID Application identity with its private key, and an authenticated `notarytool` Keychain profile. `security find-identity -v -p codesigning` lists identities. Reuse the machine defaults in `${APPLE_NOTARIZE_CONFIG:-$HOME/.config/apple-notarize-app/config.json}` when available; never copy credentials into the repository. Export these variables in the current shell:

   ```sh
   export APPLE_SIGNING_IDENTITY="<Developer ID Application SHA-1 fingerprint>"
   export APPLE_TEAM_ID="<developer team ID>"
   export APPLE_NOTARY_PROFILE="<existing notarytool profile>"
   # Set these only when the identity/profile uses an explicit keychain:
   export APPLE_SIGNING_KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"
   export APPLE_NOTARY_KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"
   ```

2. Create a parent staging directory, choose a new evidence directory and unused `.dmg` path, and build:

   ```sh
   mkdir -p .tmp/signing
   just bundle-notarized .tmp/signing/simple-ptt.dmg .tmp/signing/evidence
   ```

   Existing DMGs and evidence directories are rejected. This builds the version currently in `Cargo.toml`; it does not create a release or tag. A local verification artifact is not evidence that an existing published download is signed or notarized.

3. Require the command to finish successfully. Inspect `evidence/app-signature.txt` for the intended authority/team, runtime flag, and timestamp; inspect `app-entitlements.plist` and both `*-notary/log.json` files. Both logs must say `Accepted` with no issues. The command requires successful stapler validation, strict signature verification, and Gatekeeper assessments for the staged app, DMG, and mounted app. Gatekeeper reports `Notarized Developer ID`.
4. Verify the final checksum from its directory with `shasum -a 256 --check simple-ptt.sha256`. Retain submission evidence until the distribution is verified. App launch, microphone capture, accessibility permissions, and update installation require separate authorized runtime testing; static signing checks do not establish those behaviors.

## GitHub Actions credentials and verification

The Release workflow runs on approved `v*` tags and by manual dispatch. Manual dispatch produces a verification artifact without publishing a release or changing the Sparkle feed. Both paths run the strict Rust gate before signing.

Configure these encrypted repository secrets using `gh secret set NAME --repo alexgorbatchev/simple-ptt` with values supplied through secure standard input:

| Secret | Value |
| --- | --- |
| `APPSTORE_CERTIFICATES_FILE_BASE64` | Base64 PKCS#12 containing the existing Developer ID Application certificate, private key, and issuing intermediate certificate |
| `APPSTORE_CERTIFICATES_PASSWORD` | Strong random password protecting that PKCS#12 |
| `APPLE_SIGNING_IDENTITY` | Certificate SHA-1 fingerprint |
| `APPLE_TEAM_ID` | Matching developer team |
| `APPLE_ID` | Apple Account used for notarization |
| `APPLE_NOTARY_PASSWORD` | That account's app-specific password |

The pinned Apple-Actions importer creates an ephemeral signing Keychain and deletes it in its post step. `notarytool store-credentials` validates and stores the profile in that same Keychain. Credentials are available only to the release job; PR CI does not import them. Never print secret values, enable shell tracing around credentials, upload Keychains, or rotate the signing certificate as a setup shortcut.

After a workflow change, dispatch `gh workflow run release.yml --repo alexgorbatchev/simple-ptt --ref main`, wait for every step to pass, and inspect the retained notarization evidence and verification build. Download that build and check its checksum, stapled tickets, signatures, and Gatekeeper result before considering CI distribution verified. No tag or release approval is implied by this verification run.

## Failure recovery and publication

1. Preserve the evidence directory. Each submission records its ID, submitted archive SHA-256, status, and log. A 60-second polling timeout does not cancel Apple's job; the helper polls the same ID for up to 30 minutes. For a pending submission, rerun `scripts/notarize-macos-artifact.sh <same-archive> <same-notary-evidence-directory>` to continue waiting. Do not create a second upload just because waiting timed out.
2. If `submission-pending.json` exists without `submission.json`, reconcile `notarytool history`, the artifact name/hash, and any returned ID before proceeding. Confirm the matching submission with `notarytool info`; save the verified JSON containing its ID as `submission.json` only after reconciliation. Missing local output is not evidence that Apple received nothing.
3. For a rejection, inspect `log.json`, fix the reported cause, then build the changed artifact in a new evidence directory. Do not reuse a previous ticket after re-signing. The DMG submission hash describes the signed bytes before stapling; the final `.sha256` file describes the distributed bytes after stapling.
4. After manually recovering an accepted submission, continue the remaining stapling and verification commands in `scripts/build-notarized-dmg.sh` against those same artifacts. Do not rebuild or re-sign an accepted artifact. If any command fails, preserve the artifact and its evidence and investigate that failure before publishing.
5. Obtain the repository's required version approval before committing a bump or creating a tag. Tag-triggered runs publish the verified DMG and checksum. They refuse to replace an existing release. Never replace an asset already referenced by the Sparkle feed; publish a new approved version instead. Complete feed signing and publication using [the Sparkle procedure](sparkle-publication.md), which uses the actual published DMG.

References: [Apple notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution), [nested distribution and stapling](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow), and [Sparkle signing order](https://sparkle-project.org/documentation/sandboxing/).
