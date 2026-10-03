#!/usr/bin/env bun
// Records the README demo of the simple-ptt overlay and encodes it for GitHub.
//
//   bun <skill>/scripts/record-demo.ts record
//   bun <skill>/scripts/record-demo.ts encode
//
// Run from the repository root. `record` temporarily wires the demo timeline
// into the debug tuner, finds the overlay's screen rect with a dry run, records
// it with screencapture, restores every source file it touched, and writes an
// overview contact sheet. `encode` trims the recording from the overlay's pop
// in to the end of its pop out and writes an H.264 MP4 at the overlay's
// on-screen size, with contact sheets of its first and last frames.
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const skillDir = resolve(import.meta.dir, "..");
const repo = resolve(process.cwd());
const outDir = join(repo, ".tmp", "readme-demo");
const movPath = join(outDir, "demo.mov");
const metaPath = join(outDir, "recording.json");
const mp4Path = join(outDir, "simple-ptt-demo.mp4");

const tunerPath = join(repo, "src/overlay/dev/tuner.rs");
const modPath = join(repo, "src/overlay/dev/mod.rs");
const probePath = join(repo, "src/overlay/dev/view_probe.rs");
const probeAsset = join(repo, ".agents/skills/overlay-visual-debugging/assets/view_probe.rs");
const blockAsset = join(skillDir, "assets/demo_block.rs");
const binary = join(repo, "target/debug/simple-ptt");

/// GitHub's upload limit for videos on a free plan.
const GITHUB_VIDEO_LIMIT_BYTES = 10 * 1024 * 1024;
const MONTAGE_FONT = "/System/Library/Fonts/SFNS.ttf";
/// Frame rate the recording is resampled to for trimming and encoding;
/// screencapture writes a variable frame rate.
const FPS = 60;
/// Size of the grey frames compared to find the fades, and how far a pixel
/// must move, and in how many pixels, to count as the overlay. Measured on a
/// still screen, no pixel moved more than 4 levels between frames; the first
/// frame of the pop in moved 0.6% of pixels by more than that.
const PROBE_WIDTH = 160;
const PIXEL_CHANGE = 4;
const CHANGED_SHARE = 0.0005;
/// The timeline asks screencapture to start 1.5 s before the overlay shows,
/// and screencapture takes 0.1 to 1.2 s to begin, so the pop in lands in this
/// window of the video. A change outside it is the background moving.
const POP_IN_WINDOW_SECONDS: [number, number] = [0.3, 1.6];
/// How far the measured pop in to pop out may differ from the timeline's
/// show to hide plus the pop duration; more means the background moved.
const DURATION_TOLERANCE_SECONDS = 0.25;

const ANCHOR = "    let _keep = (tuner, window, timer);";
const MOD_FROM = "pub mod tuner;\n";
const MOD_TO = "pub mod tuner;\npub mod view_probe;\n";

type Rect = { x: number; top: number; width: number; height: number };
/// What `encode` needs from the recording run: the rect recorded, and the
/// timeline's show and hide times and pop duration, in seconds.
type Recording = { rect: Rect; show: number; hide: number; popSeconds: number };

class DemoError extends Error {}

/// Throws, so the `finally` in `record` restores the sources before exiting.
function fail(message: string): never {
  throw new DemoError(message);
}

function replaceOnce(text: string, from: string, to: string, what: string): string {
  const count = text.split(from).length - 1;
  if (count !== 1) fail(`expected one match for ${what}, found ${count}; update the skill to match the code`);
  return text.replace(from, to);
}

async function run(cmd: string[]): Promise<{ code: number; stdout: string; stderr: string }> {
  const proc = Bun.spawn(cmd, { cwd: repo, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  return { code, stdout, stderr };
}

async function must(cmd: string[]): Promise<string> {
  const result = await run(cmd);
  if (result.code !== 0) fail(`${cmd.join(" ")} exited ${result.code}\n${result.stderr}`);
  return result.stdout;
}

/// The tuner run in progress, stopped if the script is interrupted.
let activeRun: ReturnType<typeof Bun.spawn> | undefined;

/// Runs the demo timeline, which exits on its own, and returns its stderr.
/// A run still alive after 90 s is killed.
async function runTimeline(env: Record<string, string>): Promise<string> {
  const proc = Bun.spawn([binary, "--debug"], {
    cwd: repo,
    env: { ...process.env, VIEW_PROBE: "1", ...env },
    stdout: "ignore",
    stderr: "pipe",
  });
  activeRun = proc;
  const killer = setTimeout(() => proc.kill(), 90_000);
  const stderr = await new Response(proc.stderr).text();
  const code = await proc.exited;
  clearTimeout(killer);
  activeRun = undefined;
  if (code !== 0) fail(`the demo run exited ${code}\n${stderr.slice(-2000)}`);
  return stderr;
}

function timing(stderr: string): { show: number; hide: number; popSeconds: number } {
  const show = stderr.match(/demo-timing show=(\S+)/);
  const hide = stderr.match(/demo-timing hide=(\S+) pop_seconds=(\S+)/);
  if (!show || !hide) fail("the demo run printed no show or hide time");
  return { show: Number(show[1]), hide: Number(hide[1]), popSeconds: Number(hide[2]) };
}

/// The smallest screen rect, in points from the top-left, holding every
/// visible panel frame from the moment the demo shows the overlay: the
/// overlay plus the height the correction grows into. Before that, the panel
/// can sit visible where the tuner first put it.
function panelRect(stderr: string, show: number): Rect {
  const frames = [...stderr.matchAll(/demo-panel t=(\S+) x=(\S+) top=(\S+) w=(\S+) h=(\S+) visible=true/g)]
    .filter((m) => Number(m[1]) >= show)
    .map((m) => ({ x: Number(m[2]), top: Number(m[3]), width: Number(m[4]), height: Number(m[5]) }));
  if (frames.length === 0) fail("the dry run printed no visible overlay frames");
  const left = Math.min(...frames.map((f) => f.x));
  const right = Math.max(...frames.map((f) => f.x + f.width));
  const top = Math.min(...frames.map((f) => f.top));
  const bottom = Math.max(...frames.map((f) => f.top + f.height));
  return { x: Math.round(left), top: Math.round(top), width: Math.round(right - left), height: Math.round(bottom - top) };
}

async function screencaptureRunning(): Promise<boolean> {
  return (await run(["pgrep", "-f", `screencapture.*${movPath}`])).code === 0;
}

/// Extracts frames of `video` with `filter` (after optional input `seek`
/// arguments) and tiles them six to a row into `<outDir>/<name>.png`.
async function contactSheet(video: string, name: string, seek: string[], filter: string): Promise<string> {
  const sheetDir = join(outDir, "sheet", name);
  rmSync(sheetDir, { recursive: true, force: true });
  mkdirSync(sheetDir, { recursive: true });
  await must(["ffmpeg", "-loglevel", "error", "-y", ...seek, "-i", video, "-vf", filter, join(sheetDir, "%03d.png")]);
  const frames = [...new Bun.Glob("*.png").scanSync(sheetDir)].sort().map((f) => join(sheetDir, f));
  const sheet = join(outDir, `${name}.png`);
  await must(["magick", "montage", ...frames, "-tile", "6x", "-geometry", "+3+3", "-font", MONTAGE_FONT, sheet]);
  return sheet;
}

async function record() {
  for (const tool of ["cargo", "ffmpeg", "ffprobe", "magick", "pgrep"]) {
    if (!Bun.which(tool)) fail(`${tool} is not installed`);
  }
  if (!existsSync(tunerPath)) fail("run from the simple-ptt repository root");
  if (existsSync(probePath)) fail(`${probePath} already exists; another probe is wired in, so stop and clear it first`);
  if (!existsSync(probeAsset)) fail(`missing ${probeAsset}`);
  mkdirSync(outDir, { recursive: true });

  const tunerOriginal = await Bun.file(tunerPath).text();
  const modOriginal = await Bun.file(modPath).text();
  let restored = false;
  const restore = () => {
    if (restored) return;
    restored = true;
    writeFileSync(tunerPath, tunerOriginal);
    writeFileSync(modPath, modOriginal);
    rmSync(probePath, { force: true });
    console.error("record-demo: restored src/overlay/dev/tuner.rs and mod.rs, removed view_probe.rs");
  };
  for (const signal of ["SIGINT", "SIGTERM"] as const) {
    process.on(signal, () => {
      activeRun?.kill();
      restore();
      process.exit(130);
    });
  }

  try {
    const block = await Bun.file(blockAsset).text();
    await Bun.write(tunerPath, replaceOnce(tunerOriginal, ANCHOR, block + ANCHOR, "the end of tuner::run in tuner.rs"));
    await Bun.write(modPath, replaceOnce(modOriginal, MOD_FROM, MOD_TO, "the tuner's mod line in mod.rs"));
    await Bun.write(probePath, Bun.file(probeAsset));

    console.error("record-demo: building");
    await must(["cargo", "build", "--locked"]);

    console.error("record-demo: dry run to find the overlay's rect (about 25 s)");
    const dryRun = await runTimeline({});
    await Bun.write(join(outDir, "dry-run.log"), dryRun);
    const rect = panelRect(dryRun, timing(dryRun).show);
    const rectArg = `${rect.x},${rect.top},${rect.width},${rect.height}`;

    console.error(`record-demo: recording ${rectArg} (about 25 s)`);
    rmSync(movPath, { force: true });
    const recordRun = await runTimeline({ DEMO_CAPTURE: movPath, DEMO_RECT: rectArg });
    await Bun.write(join(outDir, "record-run.log"), recordRun);
    const recording: Recording = { rect, ...timing(recordRun) };
    await Bun.write(metaPath, JSON.stringify(recording));
    for (let waited = 0; await screencaptureRunning(); waited += 250) {
      if (waited > 30_000) fail("screencapture is still running 30 s after the demo ended");
      await Bun.sleep(250);
    }
  } finally {
    restore();
  }

  if (!existsSync(movPath)) fail("screencapture wrote no video; grant the terminal Screen Recording permission and retry");
  const info = await must(["ffprobe", "-v", "error", "-show_entries", "stream=width,height:format=duration", "-of", "compact", movPath]);
  const overview = await contactSheet(movPath, "overview", [], "fps=1,scale=385:-1");
  console.log(`recording: ${movPath}\n${info.trim()}\noverview (1 frame a second): ${overview}`);
}

/// The recording at `FPS` as grey frames of `PROBE_WIDTH` pixels wide.
async function greyFrames(): Promise<Uint8Array[]> {
  const dims = (await must(["ffprobe", "-v", "error", "-show_entries", "stream=width,height", "-of", "csv=p=0", movPath]))
    .trim()
    .split(",")
    .map(Number);
  const height = 2 * Math.round((PROBE_WIDTH * dims[1]) / dims[0] / 2);
  const probe = Bun.spawn(
    ["ffmpeg", "-loglevel", "error", "-i", movPath, "-vf", `fps=${FPS},scale=${PROBE_WIDTH}:${height},format=gray`, "-f", "rawvideo", "-"],
    { cwd: repo, stdout: "pipe", stderr: "pipe" },
  );
  const bytes = new Uint8Array(await new Response(probe.stdout).arrayBuffer());
  if ((await probe.exited) !== 0) fail("ffmpeg could not decode the recording");
  const frameSize = PROBE_WIDTH * height;
  return Array.from({ length: Math.floor(bytes.length / frameSize) }, (_, i) => bytes.subarray(i * frameSize, (i + 1) * frameSize));
}

function differs(frame: Uint8Array, background: Uint8Array): boolean {
  let changed = 0;
  for (let i = 0; i < frame.length; i++) {
    if (Math.abs(frame[i] - background[i]) > PIXEL_CHANGE) changed++;
  }
  return changed > frame.length * CHANGED_SHARE;
}

/// The overlay's first and last frames in the recording, at `FPS`: the first
/// frame that differs from the background before the pop in, and the last
/// that differs from the background after the pop out.
async function overlayFrames(): Promise<{ first: number; last: number }> {
  const frames = await greyFrames();
  const before = frames[0];
  const after = frames[frames.length - 1];
  const first = frames.findIndex((frame) => differs(frame, before));
  const last = frames.findLastIndex((frame) => differs(frame, after));
  if (first < 0 || last < 0) fail("the overlay never appeared in the recording");
  return { first, last };
}

async function encode() {
  if (!existsSync(movPath) || !existsSync(metaPath)) fail("run `record` first");
  const recording: Recording = await Bun.file(metaPath).json();
  const { first, last } = await overlayFrames();
  const popIn = first / FPS;
  const [earliest, latest] = POP_IN_WINDOW_SECONDS;
  if (popIn < earliest || popIn > latest) {
    fail(
      `the first change in the recording is at ${popIn.toFixed(3)} s, outside ${earliest}–${latest} s: ` +
        "the background moved before the overlay appeared, so keep the screen still and record again",
    );
  }
  const measured = (last + 1 - first) / FPS;
  const expected = recording.hide - recording.show + recording.popSeconds;
  if (Math.abs(measured - expected) > DURATION_TOLERANCE_SECONDS) {
    fail(
      `the overlay shows for ${measured.toFixed(3)} s in the recording but ${expected.toFixed(3)} s in the timeline: ` +
        "the background moved before or after the overlay, so keep the screen still and record again",
    );
  }
  // From the last frame before the pop in to the first frame after the pop out.
  const start = (first - 1) / FPS;
  const end = (last + 2) / FPS;
  // Scale to the overlay's size in points, so a Retina capture plays at the
  // size the overlay has on screen.
  await must([
    "ffmpeg", "-loglevel", "error", "-y",
    "-ss", start.toFixed(3), "-t", (end - start).toFixed(3), "-i", movPath,
    "-vf", `fps=${FPS},scale=${recording.rect.width}:-2:flags=lanczos`,
    "-c:v", "libx264", "-preset", "slow", "-crf", "18", "-pix_fmt", "yuv420p", "-profile:v", "high",
    "-movflags", "+faststart", "-an", mp4Path,
  ]);
  const bytes = Bun.file(mp4Path).size;
  if (bytes > GITHUB_VIDEO_LIMIT_BYTES) fail(`${mp4Path} is ${bytes} bytes, over GitHub's 10 MB limit for videos on a free plan`);
  const info = await must(["ffprobe", "-v", "error", "-show_entries", "stream=codec_name,width,height,r_frame_rate:format=duration,size", "-of", "compact", mp4Path]);
  const duration = Number((await must(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", mp4Path])).trim());
  const firstSheet = await contactSheet(mp4Path, "first", [], "select=lt(n\\,18),scale=300:-1");
  const lastSheet = await contactSheet(mp4Path, "last", ["-ss", Math.max(0, duration - 0.6).toFixed(3)], "scale=300:-1");
  console.log(
    `trim: ${start.toFixed(3)}–${end.toFixed(3)} s of the recording (overlay ${measured.toFixed(3)} s, timeline ${expected.toFixed(3)} s)\n` +
      `video: ${mp4Path}\n${info.trim()}\n` +
      `first 18 frames: ${firstSheet}\nlast 0.6 s: ${lastSheet}`,
  );
}

const [command] = process.argv.slice(2);
try {
  if (command === "record") await record();
  else if (command === "encode") await encode();
  else fail("usage: record-demo.ts record | encode");
} catch (error) {
  if (!(error instanceof DemoError)) throw error;
  console.error(`record-demo: ${error.message}`);
  process.exit(1);
}
