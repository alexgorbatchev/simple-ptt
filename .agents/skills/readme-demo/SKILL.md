---
name: readme-demo
description: Use when recording, re-recording, or updating the simple-ptt README demo video of the overlay, including the live transcript, the spectrum pills, and a spoken correction.
author: alexgorbatchev
metadata:
  created_on: 2026-10-02 16:15
  last_modified: 2026-10-02 22:02
  status: current
---

The demo is a scripted timeline (`assets/demo_block.rs`) that runs inside the debug tuner (`src/overlay/dev/tuner.rs`), stops the tuner's own timer, and drives the overlay through the states the app uses, over whatever the user has on screen. A developer dictates a prompt for their coding agent ("Add retries with exponential backoff to the webhook sender … cap it at three attempts."), holds the correction key to say "make that five attempts" (the correction panel grows above the transcript), releases it (transforming: the sweep runs and the inline preview underlines "five"), keeps dictating ("Then log every failure with its status code, and add a test for the timeout path."), and stops (pop out). To change the story, edit the text and phase constants at the top of the block; each phase must end before the next begins. `scripts/record-demo.ts` does every step that touches the source; run it from the repository root.

## Rules

- Record only through `scripts/record-demo.ts record`. It wires `assets/demo_block.rs` and the `overlay-visual-debugging` skill's `view_probe.rs` into the tuner, runs it twice (a dry run that measures the overlay's screen rect, then the recording), and restores `tuner.rs` and `mod.rs` byte for byte, even on failure or Ctrl-C. Launching `simple-ptt` any other way is prohibited (AGENTS.md).
- After every run, `git status --short` must list nothing under `src/` that was not there before, and `rg -n "view_probe|VIEW_PROBE|DEMO_" src/` must print nothing. If either fails, restore the files from `git` only if they had no other uncommitted changes; otherwise stop and tell the user.
- Publish the demo as an H.264 MP4 that the user uploads to GitHub in the browser. Do not commit the video to the repository and do not convert it to GIF:
  - GitHub plays a README video only from a `https://github.com/user-attachments/assets/<id>` link on a line of its own, which it renders as a collapsible player (controls, muted, no autoplay or loop). Tested on a branch: GitHub strips `<video>` tags from a README entirely, by relative path or by `raw` URL, and serves a committed `.mp4` as `application/octet-stream` with `nosniff` from `raw.githubusercontent.com`, which github.com's `media-src` policy does not allow.
  - A GIF has 256 colours and was 3–4 MB for an earlier demo; the MP4 is under 1 MB in full colour.
- `encode` finds the trim in the video: from the last frame before the pop in to the first frame after the pop out, by comparing frames with the still background before and after. It refuses a recording whose background moved (pop in outside its expected window, or an overlay duration that disagrees with the timeline's). Then ask the user to keep the screen still (no scrolling output, animations, or a moving pointer behind the overlay) and record again. Do not trim by hand to get around it.
- If the script stops with `expected one match for …` or the build fails, the tuner's code changed. Update `assets/demo_block.rs` or the anchors in `scripts/record-demo.ts`, then record again. Do not edit the tuner itself to make the script match.

## Workflow

1. **Prepare.** Tell the user that the area behind the overlay (the top centre of the main display, about 770 × 490 pt) is recorded and will be public in the README, and that it must stay still while recording. Do not run `record` until they reply that the screen is ready.
2. **Record.** Run `bun .agents/skills/readme-demo/scripts/record-demo.ts record` (about a minute; it needs Screen Recording permission for the terminal). It prints the rect it recorded, the video's size and length, and `.tmp/readme-demo/overview.png` (one frame a second).
3. **Review the story.** Read `overview.png`: the transcript fills in with pills under it, the correction panel appears above it, the text then reads "five attempts", and the second sentence follows. A frame that shows only the background where the overlay should be, or a black or wallpaper-only video, means the capture failed; stop and tell the user.
4. **Encode.** Run `bun .agents/skills/readme-demo/scripts/record-demo.ts encode`. It writes `.tmp/readme-demo/simple-ptt-demo.mp4` at the overlay's size in points (half the Retina capture), 60 fps, fails if the file is over GitHub's 10 MB limit for free plans, and prints the trim with `first.png` (the first 18 frames) and `last.png` (the last 0.6 s).
5. **Check.** Read `first.png`: its first tile is the background alone and the overlay fades in over the next tiles. Read `last.png`: the overlay fades out and the last tiles are the background alone. Then extract the MP4's frames at about 9.4 s (the correction panel), 11.4 s ("five" underlined) and the end of the second sentence (`ffmpeg -ss <t> -i <mp4> -frames:v 1 <png>`) and read them; the text must be readable.
6. **Upload.** Only the user can upload: GitHub makes attachment links only from a file dropped into its web editor. Create and push a throwaway branch from `main` (a worktree under `.workspaces/`), then give the user the MP4's full path and these steps: open `https://github.com/<owner>/<repo>/edit/<branch>/README.md`, drag the MP4 onto the demo line, wait for the `https://github.com/user-attachments/assets/<id>` line, and commit to that branch. Do not have them edit `README.md` on `main` on github.com, which commits to `main` behind any local commits.
7. **Verify the link.** Fetch the branch's README and the attachment:
   - The page `https://github.com/<owner>/<repo>/tree/<branch>` must contain a `<video` element whose `src` is on `private-user-images.githubusercontent.com` and includes the asset id.
   - `curl -sL -r 0-1023 -o /dev/null -w "%{http_code} %{content_type}" <link>` must print `206 video/mp4`. Check with GET: a HEAD request (`curl -I`) gets 403 from the signed storage URL even when the video works.
8. **Publish.** In `main`'s `README.md`, replace the demo line (the previous `user-attachments` link) with the new link on its own line, and commit and push only `README.md` when the user asks. Then, with the user's consent, delete the throwaway branch's worktree, local branch, and remote branch. The link keeps working after the branch is gone (verified: still `206 video/mp4`).

## Report

Give the MP4's path, size, dimensions, length, and the trim `encode` printed, and say what the background showed, since it will be public. After publishing, give the commit, the verified link, and what is left to clean up.
