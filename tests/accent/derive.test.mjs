// Guards the accent ink derivation, the load-bearing accessibility logic
// behind the accent picker. The single implementation lives in
// assets/prepaint.js (exposed to app.js as window.deriveWarningInk); the
// ACCENT-DERIVE markers fence the block. This slices it, runs it, and asserts
// the derived ink clears WCAG AA against the rendered pill, for the presets
// plus adversarial picks.
//
// Run: node tests/accent/derive.test.mjs (mise run test:accent).

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

const AA = 4.5;
// Mirror the per-theme surfaces the derivation targets: --color-base-100, which
// is the surface .badge-warning mixes the pill into.
const SURFACE = { light: "#ffffff", dark: "#1d232a" };
const THEMES = ["light", "dark"];

// The default amber, the three shipped presets, and adversarial picks: extremes
// of lightness, saturation, and hue that stress the lightness scan.
const BASES = {
  "default amber": "#f5a524",
  "teal preset": "#06b6d4",
  "rust preset": "#c2410c",
  "magenta preset": "#a21caf",
  white: "#ffffff",
  black: "#000000",
  "near-white": "#fefefe",
  "pale yellow": "#fffbe6",
  "pure red": "#ff0000",
  "pure green": "#00ff00",
  "pure blue": "#0000ff",
  "mid grey": "#808080",
  "near-black": "#101010",
};

// Slice the JS between the ACCENT-DERIVE markers: from the line after BEGIN to
// the line before END.
function sliceBlock(src, label) {
  const begin = src.indexOf("ACCENT-DERIVE:BEGIN");
  const end = src.indexOf("ACCENT-DERIVE:END");
  if (begin < 0 || end < 0 || end < begin) {
    throw new Error(`${label}: ACCENT-DERIVE markers missing or out of order`);
  }
  const from = src.indexOf("\n", begin) + 1;
  const to = src.lastIndexOf("\n", end) + 1;
  return src.slice(from, to);
}

// Evaluate the sliced block in isolation and hand back the functions the test needs.
function load(block, label) {
  try {
    return new Function(
      `${block}\nreturn { deriveWarningInk, contrastRatio, mixColors };`,
    )();
  } catch (e) {
    throw new Error(`${label}: block did not evaluate: ${e.message}`);
  }
}

const src = readFileSync(join(root, "assets", "prepaint.js"), "utf8");
const derive = load(sliceBlock(src, "prepaint.js"), "prepaint.js");

const failures = [];
for (const [name, base] of Object.entries(BASES)) {
  for (const theme of THEMES) {
    const ink = derive.deriveWarningInk(base, theme);
    const pill = derive.mixColors(base, 16, SURFACE[theme]);
    const ratio = derive.contrastRatio(ink, pill);
    if (ratio < AA) {
      failures.push(`${name} ${theme}: ink ${ink} on pill ${pill} is ${ratio.toFixed(2)}, below AA ${AA}`);
    }
  }
}

if (failures.length) {
  console.error("accent derivation FAILED:");
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}

const cases = Object.keys(BASES).length * THEMES.length;
console.log(`accent derivation OK: ${cases} cases clear AA ${AA}`);
