// WCAG contrast between the overlay's text and what is behind it, measured
// on `--overlay-snapshot` captures of the recording state (no correction).
// Usage: bun <skill>/scripts/contrast.ts <dir> [prefix] [halo_margin]
// `halo_margin` is the `GlassTuning::halo_margin` the captures were taken
// with (default: the current default, 90.71).
// Needs ImageMagick (`magick`). A value under 4.5 (WCAG AA) is marked "!".
//
// Two methods:
// - Paired (preferred): for `<name>.png` with a `<name>-notext.png` taken the
//   same way with an empty transcript, the pixels that differ are the
//   transcript's glyphs, and each is compared with the same pixel without
//   the text: what is behind that glyph, backdrop text included. The body
//   reports the 90th percentile (glyph cores) and the median of those
//   per-pixel ratios.
// - Median (flat backdrops only): the background is the region's median
//   luminance and the text the 2% tail farther from it. Backdrop text behind
//   the glass breaks this, so it is marked "~" and used for the footer, which
//   the paired captures keep.
import { $ } from "bun";

const [dir, prefix = "tint-", marginArg = "90.71"] = process.argv.slice(2);
// Captures are 2x and padded by 40pt around the window, and the window adds
// the halo margin around the glass, so the glass's top-left corner is at
// (40 + margin) * 2 pixels. The regions are measured from that corner.
const glass = Math.round((40 + Number(marginArg)) * 2);
const region = (width: number, height: number, x: number, y: number) =>
  `${width}x${height}+${glass + x}+${glass + y}`;
const regions = {
  body: region(980, 90, 40, 35),
  footer: region(1048, 40, 36, 312),
};
// A glyph pixel differs from the same pixel without text by more than this
// luminance.
const GLYPH_DELTA = 0.03;

function luminance(r: number, g: number, b: number): number {
  const channel = (value: number) => {
    const c = value / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

const ratio = (a: number, b: number) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);

async function luminances(path: string, crop: string): Promise<number[]> {
  const raw = new Uint8Array(await $`magick ${path} -crop ${crop} +repage -depth 8 rgb:-`.arrayBuffer());
  const values: number[] = [];
  for (let i = 0; i + 2 < raw.length; i += 3) values.push(luminance(raw[i], raw[i + 1], raw[i + 2]));
  return values;
}

const quantile = (sorted: number[], q: number) =>
  sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];

async function paired(withText: string, withoutText: string, crop: string): Promise<string> {
  const [text, behind] = await Promise.all([luminances(withText, crop), luminances(withoutText, crop)]);
  const ratios: number[] = [];
  for (let i = 0; i < text.length; i++) {
    if (Math.abs(text[i] - behind[i]) > GLYPH_DELTA) ratios.push(ratio(text[i], behind[i]));
  }
  if (ratios.length === 0) return "no glyphs found";
  ratios.sort((a, b) => a - b);
  const cores = quantile(ratios, 0.9);
  return `${cores.toFixed(2)}${cores < 4.5 ? "!" : " "}(median ${quantile(ratios, 0.5).toFixed(2)}, ${ratios.length} px)`;
}

async function median(path: string, crop: string): Promise<string> {
  const sorted = (await luminances(path, crop)).sort((a, b) => a - b);
  const bg = quantile(sorted, 0.5);
  const low = quantile(sorted, 0.02);
  const high = quantile(sorted, 0.98);
  const text = Math.abs(high - bg) > Math.abs(bg - low) ? high : low;
  const value = ratio(bg, text);
  return `${value.toFixed(2)}${value < 4.5 ? "!" : " "}~(bg ${bg.toFixed(3)} text ${text.toFixed(3)})`;
}

const files = (await $`ls ${dir}`.text())
  .split("\n")
  .filter((f) => f.startsWith(prefix) && f.endsWith(".png") && !f.endsWith("-notext.png"))
  .sort();
for (const file of files) {
  const path = `${dir}/${file}`;
  const blank = `${dir}/${file.replace(".png", "-notext.png")}`;
  const hasPair = await Bun.file(blank).exists();
  const body = hasPair ? await paired(path, blank, regions.body) : await median(path, regions.body);
  const footer = await median(path, regions.footer);
  console.log(`${file.replace(".png", "").padEnd(30)} body=${body}  footer=${footer}`);
}
