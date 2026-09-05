#!/usr/bin/env node
// Build the icon assets in apple/App/Assets.xcassets from the SVG sources
// next to this script:
//
//   AppIcon.appiconset/     the app icon, one PNG per macOS slot. The 16 and
//                           32 pt slots take the reduced "-small" artwork;
//                           128 pt and up take the full artwork.
//   DocumentBadge.imageset/ the badge the system composes onto the .zarr and
//                           .icechunk document icons (UTTypeIconBadgeName in
//                           App/Info.plist), at 1x and 2x.
//
// Run after generate.py, with playwright on the module path (`mise run
// icons` does both):
//
//   npx --yes playwright@1.56 install chromium   # once
//   node docs/icon/assets.mjs

import { chromium } from "playwright";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const option = process.argv[2] ?? "gridlook";
const catalog = join(here, "..", "..", "apple", "App", "Assets.xcassets");
const set = join(catalog, "AppIcon.appiconset");
mkdirSync(set, { recursive: true });

const full = join(here, `${option}.svg`);
const smallPath = join(here, `${option}-small.svg`);
const small = existsSync(smallPath) ? smallPath : full;

// Every slot of a macOS app icon set. `pt` is the point size Xcode labels
// the slot with, `scale` its 1x/2x, `art` which SVG fills it.
const slots = [
  { pt: 16, scale: 1, art: small },
  { pt: 16, scale: 2, art: small },
  { pt: 32, scale: 1, art: small },
  { pt: 32, scale: 2, art: small },
  { pt: 128, scale: 1, art: full },
  { pt: 128, scale: 2, art: full },
  { pt: 256, scale: 1, art: full },
  { pt: 256, scale: 2, art: full },
  { pt: 512, scale: 1, art: full },
  { pt: 512, scale: 2, art: full },
];

const browser = await chromium.launch();

async function render(art, px, target) {
  const page = await browser.newPage({ viewport: { width: px, height: px }, deviceScaleFactor: 1 });
  await page.setContent(
    `<style>html,body{margin:0;background:transparent}svg{display:block;width:${px}px;height:${px}px}</style>${readFileSync(art, "utf8")}`,
  );
  await page.screenshot({ path: target, omitBackground: true });
  await page.close();
  console.log(`${basename(target)}  <- ${basename(art)}`);
}

const info = { author: "xcode", version: 1 };

const images = [];
for (const { pt, scale, art } of slots) {
  const filename = `icon_${pt}x${pt}@${scale}x.png`;
  await render(art, pt * scale, join(set, filename));
  images.push({ filename, idiom: "mac", scale: `${scale}x`, size: `${pt}x${pt}` });
}
writeFileSync(join(set, "Contents.json"), JSON.stringify({ images, info }, null, 2) + "\n");

// Document-icon badge: the system scales and composites it onto the document
// shape itself, so a 256 pt image at 1x and 2x is plenty.
const badgeSet = join(catalog, "DocumentBadge.imageset");
mkdirSync(badgeSet, { recursive: true });
const badgeArt = join(here, `${option}-badge.svg`);
const badgeImages = [];
for (const scale of [1, 2]) {
  const filename = `badge@${scale}x.png`;
  await render(badgeArt, 256 * scale, join(badgeSet, filename));
  badgeImages.push({ filename, idiom: "universal", scale: `${scale}x` });
}
writeFileSync(join(badgeSet, "Contents.json"), JSON.stringify({ images: badgeImages, info }, null, 2) + "\n");

await browser.close();
writeFileSync(join(catalog, "Contents.json"), JSON.stringify({ info }, null, 2) + "\n");
console.log(`wrote ${catalog}`);
