#!/usr/bin/env node
// Build apple/App/Assets.xcassets/AppIcon.appiconset: renders the icon SVGs
// into every macOS app-icon slot and writes the asset catalog's
// Contents.json files. The 16 and 32 pt slots take the reduced "-small"
// artwork; 128 pt and up take the full artwork. Run after generate.py, with
// playwright on the module path (`mise run icons` does both):
//
//   npx --yes playwright@1.56 install chromium   # once
//   node docs/icon/appiconset.mjs

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
const images = [];
for (const { pt, scale, art } of slots) {
  const px = pt * scale;
  const filename = `icon_${pt}x${pt}@${scale}x.png`;
  const page = await browser.newPage({ viewport: { width: px, height: px }, deviceScaleFactor: 1 });
  await page.setContent(
    `<style>html,body{margin:0;background:transparent}svg{display:block;width:${px}px;height:${px}px}</style>${readFileSync(art, "utf8")}`,
  );
  await page.screenshot({ path: join(set, filename), omitBackground: true });
  await page.close();
  images.push({ filename, idiom: "mac", scale: `${scale}x`, size: `${pt}x${pt}` });
  console.log(`${filename}  <- ${basename(art)}`);
}
await browser.close();

const info = { author: "xcode", version: 1 };
writeFileSync(join(set, "Contents.json"), JSON.stringify({ images, info }, null, 2) + "\n");
writeFileSync(join(catalog, "Contents.json"), JSON.stringify({ info }, null, 2) + "\n");
console.log(`wrote ${set}`);
