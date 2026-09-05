#!/usr/bin/env node
// Rasterize the icon candidates and build a side-by-side comparison sheet.
//
// Uses headless Chromium via Playwright (the only rasterizer needed; no
// ImageMagick or librsvg). Run with a playwright install on the module path:
//
//   npx --yes playwright@1.56 install chromium   # once
//   node docs/icon-options/render.mjs
//
// Outputs, next to this script:
//   masters/<option>.png          1024x1024, the AppIcon master for that option
//   renders/<option>-<size>.png   512 / 256 / 128 / 64 / 32 / 16 (untracked)
//   comparison.html / .png        all options at Dock, Finder and sidebar sizes

import { chromium } from "playwright";
import { readFileSync, readdirSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, basename } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "renders");
const masters = join(here, "masters");
mkdirSync(out, { recursive: true });
mkdirSync(masters, { recursive: true });

const svgs = readdirSync(here)
  .filter((f) => f.endsWith(".svg"))
  .sort();
const sizes = [1024, 512, 256, 128, 64, 32, 16];

const browser = await chromium.launch();

for (const file of svgs) {
  const name = basename(file, ".svg");
  const svg = readFileSync(join(here, file), "utf8");
  for (const size of sizes) {
    const page = await browser.newPage({
      viewport: { width: size, height: size },
      deviceScaleFactor: 1,
    });
    await page.setContent(
      `<style>html,body{margin:0;background:transparent}svg{display:block;width:${size}px;height:${size}px}</style>${svg}`,
    );
    const target = size === 1024 ? join(masters, `${name}.png`) : join(out, `${name}-${size}.png`);
    await page.screenshot({ path: target, omitBackground: true });
    await page.close();
  }
  console.log(`rendered ${name}`);
}

// Comparison sheet: one row per option, on a light and a dark ground, at the
// sizes macOS actually shows an app icon (Dock 128, Finder 64/32, sidebar 16).
const sheetSizes = [128, 64, 32, 16];
const rows = svgs
  .map((file) => {
    const name = basename(file, ".svg");
    const cells = (bg) =>
      sheetSizes
        .map(
          (s) =>
            `<td style="background:${bg};padding:16px;vertical-align:middle;text-align:center">` +
            `<img src="renders/${name}-${s}.png" width="${s}" height="${s}" style="display:inline-block;image-rendering:auto"></td>`,
        )
        .join("");
    return `<tr><th style="text-align:left;padding:12px 16px;font:600 15px -apple-system,system-ui,sans-serif;color:#222">${name}</th>${cells("#ececec")}${cells("#2b2b2e")}</tr>`;
  })
  .join("");
const header =
  `<tr><th></th>` +
  [...sheetSizes, ...sheetSizes]
    .map((s) => `<th style="font:12px -apple-system,system-ui,sans-serif;color:#666;padding:6px">${s} px</th>`)
    .join("") +
  `</tr>`;
const sheet = `<style>html,body{margin:0;background:#fff}table{border-collapse:separate;border-spacing:0 8px;margin:16px}</style><table>${header}${rows}</table>`;

writeFileSync(join(here, "comparison.html"), sheet);
const page = await browser.newPage({ viewport: { width: 1600, height: 200 }, deviceScaleFactor: 1 });
await page.goto("file://" + join(here, "comparison.html"), { waitUntil: "load" });
await page.screenshot({ path: join(here, "comparison.png"), fullPage: true });
await browser.close();
console.log("wrote comparison.png");
