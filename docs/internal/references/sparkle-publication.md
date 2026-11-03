---
created_on: 2026-11-03 01:22
last_modified: 2026-11-03 01:22
status: current
---

# Publishing Sparkle updates

Maintainers complete this procedure after the approved GitHub release workflow has uploaded its DMG. The workflow does not sign or publish the update feed. The feed at `https://raw.githubusercontent.com/alexgorbatchev/simple-ptt/main/appcast.xml` is served directly from `main`.

The signing private key must already exist in the maintainer's login Keychain, under Sparkle's `https://sparkle-project.org` service and the account chosen when that key was created. Its public key must equal `SUPublicEDKey` in both `scripts/build-macos-app.sh` and the published app. An unrelated Sparkle key is insufficient. If the matching key is unavailable, stop and recover it through the maintainer's key backup; generating another key does not let installed apps trust it. Never commit or print private key material.

Version 1.6.0 predates the bundled updater and requires manual installation to upgrade. Its DMG is not an update-feed entry. An empty channel is intentional until a verified Sparkle-enabled release is published.

1. Confirm the release tag/version has the user's approval and its GitHub release is published, with a successful packaging workflow. Start from a clean checkout of current `main`. Set `release_tag` to that existing tag, `sparkle_account` to the matching Keychain account, and `sparkle_tools` to an absolute directory containing the `bin` tools from the official Sparkle release matching `sparkle_version` in `scripts/build-macos-app.sh` (currently 2.9.5). These tools are included in the [official Sparkle archive](https://github.com/sparkle-project/Sparkle/releases); the packaging script extracts only the framework. Do not create a version bump or tag as part of this procedure.
2. Create a fresh staging directory under `.tmp/`, then download the actual published DMG. Do not substitute a local rebuild. Keep the variables in the same shell throughout the procedure:

   ```sh
   mkdir -p .tmp
   feed_stage=$(mktemp -d "$PWD/.tmp/sparkle-feed.XXXXXX")
   asset_name="simple-ptt-${release_tag}-macos-arm64.dmg"
   gh release download "$release_tag" --repo alexgorbatchev/simple-ptt \
     --pattern "$asset_name" --dir "$feed_stage"
   archive="$feed_stage/$asset_name"
   ```

3. Inspect the downloaded app without launching it. Mount the DMG read-only with `hdiutil attach -readonly -nobrowse -mountpoint "$feed_stage/mount" "$archive"` after creating that mount directory. Read `simple-ptt.app/Contents/Info.plist` with `/usr/libexec/PlistBuddy -c 'Print :KEY'`, substituting each key: `CFBundleVersion`, `CFBundleShortVersionString`, `LSMinimumSystemVersion`, `SUFeedURL`, and `SUPublicEDKey`. Require the approved release version, macOS 26.0 minimum, the feed URL above, and the packaging script's public key; require `Contents/Frameworks/Sparkle.framework` to exist. Compare the public key with `"$sparkle_tools/generate_keys" --account "$sparkle_account" -p` (public-key lookup only). Stop on a missing key, framework, metadata, or mismatch. Detach the mount with `hdiutil detach "$feed_stage/mount"`, including on failure, before continuing.
4. Copy the current checked-in feed into the staging directory, then let Sparkle generate the new entry from the downloaded DMG. Preserve existing entries and avoid creating unuploaded delta files:

   ```sh
   cp appcast.xml "$feed_stage/appcast.xml"
   "$sparkle_tools/generate_appcast" --account "$sparkle_account" \
     --download-url-prefix "https://github.com/alexgorbatchev/simple-ptt/releases/download/$release_tag/" \
     --maximum-versions 0 --maximum-deltas 0 \
     -o "$feed_stage/appcast.xml" "$feed_stage"
   ```

5. Validate the generated XML with `xmllint --noout "$feed_stage/appcast.xml"`. Inspect the item for the approved `sparkle:version`, not merely the first item. Require its short version and minimum OS to match the mounted bundle, its enclosure URL to equal the published asset's `browser_download_url` from `gh api "repos/alexgorbatchev/simple-ptt/releases/tags/$release_tag"`, and its `length` to equal both `stat -f %z "$archive"` and the release API's asset `size`. Copy that enclosure's `sparkle:edSignature` into `archive_signature`, then require this command to succeed:

   ```sh
   "$sparkle_tools/sign_update" --account "$sparkle_account" \
     --verify "$archive" "$archive_signature"
   ```

   Any missing field, signature failure, size mismatch, or metadata mismatch stops publication. Never hand-write a signature, copy another release's signature, or change the downloaded archive after verification.
6. Copy the verified feed to `appcast.xml`, inspect `git diff -- appcast.xml`, and obtain independent review of the generated entry and the recorded verification results. Existing entries must remain intact. Commit and push only the reviewed feed change to current `main`, following the repository's normal integration rules. Do not overwrite a concurrently updated feed: rebase, regenerate from the new feed, and repeat verification/review.
7. Fetch the live feed with `curl --fail --location 'https://raw.githubusercontent.com/alexgorbatchev/simple-ptt/main/appcast.xml'` into a separate staging file. CDN propagation may delay the new item; retry until its version, URL, length, and signature match the reviewed entry. Do not mark the release complete while the live entry differs. Never replace an uploaded DMG once its entry is live; publish a new approved version instead.
8. Record the published tag, asset URL/size, signature-verification success, feed commit, and live-feed result in the release evidence. Detach any remaining mount and remove only this procedure's reviewed staging files after verification; retain required evidence. No private key export or app launch is needed.

Tool behavior and signing guidance: [Sparkle publishing documentation](https://sparkle-project.org/documentation/publishing/). Use the matching tools' `--help` when their version changes.
