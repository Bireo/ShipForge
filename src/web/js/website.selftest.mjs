import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dir, "..", "..", "..");
const webDir = path.join(repoRoot, "website");
const docsAssets = path.join(repoRoot, "docs", "assets");

const html = fs.readFileSync(path.join(webDir, "index.html"), "utf8");
const css = fs.readFileSync(path.join(webDir, "styles.css"), "utf8");
const js = fs.readFileSync(path.join(webDir, "app.js"), "utf8");

// 1. Required landmarks, interactive controls, anchors & top-layer dialogs
for (const id of [
  "scroll-progress",
  "site-header",
  "lang-toggle",
  "theme-toggle",
  "header-star-btn",
  "header-star-count",
  "header-coffee-btn",
  "hero-dl-installer",
  "hero-dl-portable",
  "stage-frame",
  "stage-caption",
  "win-chrome-title",
  "showcase",
  "benchmarks",
  "bench-bars",
  "download",
  "dl-btn-exe",
  "dl-btn-zip",
  "dl-sha-link",
  "copy-dev-cmd",
  "sponsor-dialog",
  "media-lightbox",
  "lb-img",
  "lb-close-btn",
  "lb-zoom-btn",
]) {
  assert.ok(html.includes(`id="${id}"`), `website/index.html missing #${id}`);
}

// 2. Verify all local image assets referenced in index.html exist on disk
const srcMatches = [...html.matchAll(/(?:src|href)="(assets\/[^"?#]+)"/g)].map((m) => m[1]);
assert.ok(srcMatches.length >= 10, "Expected >=10 local asset references in website/index.html");
for (const rel of new Set(srcMatches)) {
  const full = path.join(webDir, rel);
  assert.ok(fs.existsSync(full), `Missing website asset: ${rel}`);
  assert.ok(fs.statSync(full).size > 200, `Empty/corrupt website asset: ${rel}`);
}
for (const f of ["logo.svg", "icon-256.png", "social-preview.png"]) {
  assert.ok(fs.existsSync(path.join(docsAssets, f)), `Missing docs/assets/${f}`);
}
for (const shot of [
  "01-overview-pipeline.png",
  "02-compile-prereq.png",
  "03-package-split.png",
  "04-release-repos.png",
  "05-version-changelog.png",
  "06-updater-prefs.png",
]) {
  assert.ok(fs.existsSync(path.join(docsAssets, "screenshots", shot)), `Missing docs/assets/screenshots/${shot}`);
}

// 3. Modern Web Guidance & Full Interactive Motion Parity Contracts
assert.ok(html.includes('<meta name="color-scheme" content="light dark">'), "Must declare color-scheme meta");
assert.ok(css.includes("light-dark(") && css.includes('meta[name="color-scheme"][content="dark"]'), "CSS must reflect color-scheme meta");
assert.ok(css.includes("scroll-padding-top:"), "Sticky shrinking header must set scroll-padding-top for WCAG 2.4.11");
assert.ok(css.includes("animation-timeline: scroll(root block)"), "CSS must use native scroll(root block) timeline");
assert.ok(css.includes("stage-perspective-unfold") && css.includes("header-compact"), "CSS must include 3D stage unfold and shrinking header keyframes");
assert.ok(js.includes("CSS.supports('(animation-timeline: scroll()) and (animation-range: 0% 100%)')"), "JS must include full scroll-timeline feature detection and fallback");
assert.ok(css.includes("anchor-name: --active-stage-tab") && css.includes("position-anchor: --active-stage-tab"), "Stage tabs must use CSS Anchor Positioning sliding underline");
assert.ok(css.includes("@supports not (position-anchor: --active-stage-tab)"), "Anchor tab underline must provide @supports fallback");
assert.ok(css.includes("view-timeline: --layered block") && css.includes("inset-counter-float"), "Story 04 must include layered counter-scroll inset parallax");
assert.ok(css.includes("js-float-ready") && css.includes(".from-top") && js.includes("js-float-ready"), "Must implement gradual bidirectional blur-to-crisp float-in reveal");
assert.ok(html.includes('class="count-up"') && js.includes("triggerCountUpsInside"), "Benchmarks stat quad must implement count-up animation");
assert.ok(html.includes('class="bar-fill') && html.includes('class="seg-switch"'), "Benchmarks bento must include interactive segmented bar chart");
assert.ok(html.includes('class="zoom-hint"') && css.includes(".zoom-trigger:hover .zoom-hint"), "Screenshots must provide hover scale + floating zoom-hint pill");
assert.ok(html.includes('id="sponsor-dialog" closedby="any"') && html.includes('id="media-lightbox" closedby="any"'), "Dialogs must declare closedby=any");
assert.ok(css.includes("transition-behavior: allow-discrete") && css.includes("@starting-style") && css.includes("overlay"), "Dialogs must animate to/from top layer cleanly");
assert.ok(js.includes("document.startViewTransition") && js.includes("shipforge-zoom-media") && js.includes("findReturnImageForCurrentSlide"), "Lightbox must use bidirectional shared-element View Transitions with return morph & WAAPI fallback");
assert.ok(js.includes("execCommand('copy')"), "Copy button must include legacy clipboard fallback");

// 4. Fresh Nordic Mineral Sage / Forged Emerald-Teal & Warm Brass Palette Contract
assert.ok(css.includes("#0d766e") && css.includes("#2dd4bf") && css.includes("#b45309"), "Website must use the Forged Emerald-Teal & Warm Brass palette");
assert.ok(!css.includes("#2563eb") && !css.includes("#d44b24"), "Website must not retain old cobalt or terracotta accents");

// 5. Bilingual i18n parity check (every data-i18n key in HTML must exist in COPY.zh & COPY.en)
const htmlKeys = [...html.matchAll(/data-i18n="([^"]+)"/g)].map((m) => m[1]);
for (const k of new Set(htmlKeys)) {
  assert.ok(js.includes(`'${k}':`) || js.includes(`${k}:`), `Missing i18n key "${k}" in website/app.js`);
}

// 6. Distinct identity check (must NOT leak unrelated product branding, must point to Bireo/ShipForge)
for (const [name, content] of [["index.html", html], ["styles.css", css], ["app.js", js]]) {
  assert.ok(!/QDisk|TidyAssistant|tidy-assistant/i.test(content), `${name} must not leak QDisk branding`);
}
assert.ok(html.includes("https://github.com/Bireo/ShipForge") && js.includes("Bireo/ShipForge"), "Website must point to Bireo/ShipForge");

// 7. Repo documentation presence
for (const doc of ["README.md", "README.en.md", "LICENSE", "THIRD_PARTY.md", "CHANGELOG.md", "docs/发包与在线更新指南.md"]) {
  assert.ok(fs.existsSync(path.join(repoRoot, doc)), `Missing repo doc: ${doc}`);
}

console.log("ShipForge website & repo contract selftest: 全部通过");
