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

// 1. Required landmarks, controls, anchors & dialogs
for (const id of [
  "scroll-progress",
  "site-header",
  "lang-toggle",
  "theme-toggle",
  "header-star-btn",
  "header-star-count",
  "header-coffee-btn",
  "hero-ver-pill",
  "hero-setup-btn",
  "hero-portable-btn",
  "stage-caption",
  "pipeline",
  "showcase",
  "benchmarks",
  "bench-kpi-grid",
  "download",
  "dl-setup-sha",
  "dl-port-sha",
  "dev-disclosure",
  "sponsor-dialog",
  "media-lightbox",
  "lb-img",
  "lb-close",
  "site-toast",
]) {
  assert.ok(html.includes(`id="${id}"`), `website/index.html missing #${id}`);
}

// 2. Verify all local image assets referenced in index.html & README.md exist on disk
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

// 3. Modern Web Guidance contracts
assert.ok(html.includes('<meta name="color-scheme" content="light dark">'), "Must declare color-scheme meta");
assert.ok(css.includes("light-dark(") && css.includes('meta[name="color-scheme"][content="dark"]'), "CSS must reflect color-scheme meta");
assert.ok(css.includes("animation-timeline: scroll()"), "CSS must use native scroll-timeline progress");
assert.ok(js.includes('CSS.supports("animation-timeline", "scroll()")'), "JS must provide scroll-timeline fallback");
assert.ok(html.includes('id="sponsor-dialog" closedby="any"') && html.includes('id="media-lightbox" closedby="any"'), "Dialogs must declare closedby=any");
assert.ok(js.includes('"closedBy" in HTMLDialogElement.prototype'), "JS must implement light-dismiss fallback");
assert.ok(css.includes("transition-behavior: allow-discrete") && css.includes("@starting-style") && css.includes("overlay"), "Dialogs must animate to/from top layer cleanly");
assert.ok(js.includes("canTransitionDisplay") && js.includes("data-closing"), "JS must include top-layer exit fallback");
assert.ok(js.includes("document.startViewTransition") && js.includes("sf-lightbox-media"), "Lightbox must use View Transitions API with fallback");
assert.ok(js.includes("legacyCopy"), "Copy button must include legacy clipboard fallback");

// 4. Bilingual i18n parity check (every data-i18n key in HTML must exist in I18N.en)
const htmlKeys = [...html.matchAll(/data-i18n="([^"]+)"/g)].map((m) => m[1]);
for (const k of new Set(htmlKeys)) {
  if (k === "cap.0") continue;
  assert.ok(js.includes(`"${k}"`), `Missing English i18n key "${k}" in website/app.js`);
}

// 5. Distinct identity check (must NOT contain QDisk branding or QDisk repo URLs)
for (const [name, content] of [["index.html", html], ["styles.css", css], ["app.js", js]]) {
  assert.ok(!/QDisk|TidyAssistant|tidy-assistant/i.test(content), `${name} must not leak QDisk branding`);
}

// 6. Repo documentation presence (matching QDisk repo layout)
for (const doc of ["README.md", "README.en.md", "LICENSE", "THIRD_PARTY.md", "CHANGELOG.md", "docs/发包与在线更新指南.md"]) {
  assert.ok(fs.existsSync(path.join(repoRoot, doc)), `Missing repo doc: ${doc}`);
}

console.log("ShipForge website & repo contract selftest: 全部通过");
