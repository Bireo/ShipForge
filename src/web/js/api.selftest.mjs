import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  esc,
  formatBytes,
  formatDuration,
  formatDurationText,
  isValidSemver,
  parseCommandLine,
  classifyLogLevel,
  kindLabel,
  categoryLabel,
} from "./api.js";

const __dir = path.dirname(fileURLToPath(import.meta.url));
const webRoot = path.resolve(__dir, "..");

// 1. HTML escaping prevents XSS
assert.equal(esc('<script>"x"&\'y\'</script>'), "&lt;script&gt;&quot;x&quot;&amp;&#39;y&#39;&lt;/script&gt;");
assert.equal(esc(null), "");

// 2. Byte formatter
assert.equal(formatBytes(512), "512 B");
assert.equal(formatBytes(1536), "1.5 KB");
assert.equal(formatBytes(2 * 1024 * 1024), "2.00 MB");

// 2b. Progressive compact duration formatter (rollover at 60s -> 1m0s, no template padding)
assert.equal(formatDuration(0), "0.0s");
assert.equal(formatDuration(-50), "0.0s");
assert.equal(formatDuration(NaN), "0.0s");
assert.equal(formatDuration(950), "1.0s");
assert.equal(formatDuration(59949), "59.9s");
assert.equal(formatDuration(59950), "1m0s");
assert.equal(formatDuration(60000), "1m0s");
assert.equal(formatDuration(65000), "1m5s");
assert.equal(formatDuration(3599000), "59m59s");
assert.equal(formatDuration(3600000), "1h0m0s");
assert.equal(formatDuration(3661000), "1h1m1s");
assert.equal(formatDuration(86400000), "1d0h0m0s");
assert.equal(formatDuration(90061000), "1d1h1m1s");
assert.equal(formatDuration(45900, { subSecond: false }), "45s");
assert.equal(formatDuration(60000, { subSecond: false }), "1m0s");
assert.equal(formatDurationText("613.9s"), "10m13s");
assert.equal(formatDurationText("292.0s"), "4m52s");
assert.equal(formatDurationText("12.3s"), "12.3s");
assert.equal(formatDurationText("1h2m3s"), "1h2m3s");
assert.equal(formatDurationText(""), "-");

// 3. Semver validator
assert.equal(isValidSemver("1.46.0"), true);
assert.equal(isValidSemver("2.0.0-beta.1"), true);
assert.equal(isValidSemver("1.46.9-rc.1"), true);
assert.equal(isValidSemver("v1.0.0"), false);
assert.equal(isValidSemver("1.2"), false);
assert.equal(isValidSemver("1.2.3-"), false);
assert.equal(isValidSemver("1.2.3-beta..1"), false);
assert.equal(isValidSemver("1.2.3-.beta"), false);

// 4. Command line parser handles quotes
assert.deepEqual(parseCommandLine('pwsh -NoProfile -File "build.ps1" -Bump patch'), {
  command: "pwsh",
  args: ["-NoProfile", "-File", "build.ps1", "-Bump", "patch"],
});
assert.deepEqual(parseCommandLine("   "), { command: "", args: [] });

// 5. Semantic log classifier
assert.equal(classifyLogLevel("=== 流水线 [1/3] 构建 ===", "system"), "lv-section");
assert.equal(classifyLogLevel("──── 新任务开始：一键发版流水线 ────", "system"), "lv-section");
assert.equal(classifyLogLevel("[绿] cargo test: 19 passed", "stdout"), "lv-ok");
assert.equal(classifyLogLevel("[红] 编译失败", "stdout"), "lv-err");
assert.equal(classifyLogLevel("warning: unused var", "stdout"), "lv-warn");
// Colour spec: red ONLY for real errors; stderr alone is NEVER an error
assert.equal(classifyLogLevel("   Compiling tidy-assistant v1.47.4 (D:\\Tools\\x)", "stderr"), "");
assert.equal(classifyLogLevel("        Info Looking up installed tauri packages to check mismatched versions...", "stderr"), "");
assert.equal(classifyLogLevel("warning: unused variable: `x`", "stderr"), "lv-warn");
assert.equal(classifyLogLevel("error[E0425]: cannot find value `y` in this scope", "stderr"), "lv-err");
assert.equal(classifyLogLevel("error: could not compile `shipforge`", "stderr"), "lv-err");
assert.equal(classifyLogLevel("test result: FAILED. 104 passed; 2 failed", "stdout"), "lv-err");
assert.equal(classifyLogLevel("构建失败：exit 1", "stdout"), "lv-err");
assert.equal(classifyLogLevel("actions.selftest.mjs: 54/54 通过，失败 0", "stdout"), "");
assert.equal(classifyLogLevel("test result: ok. 106 passed; 0 failed; 1 ignored", "stdout"), "lv-ok");
assert.equal(classifyLogLevel("Finished `release` profile [optimized] target(s) in 47s", "stderr"), "");

// 6. Labels
assert.equal(kindLabel("installer"), "安装包");
assert.equal(categoryLabel("package"), "打包");

// 7. Static contract check: Apple Notes x macOS Sequoia shell IDs & no hardcoded v2.0.0
const html = fs.readFileSync(path.join(webRoot, "index.html"), "utf8");
assert.ok(!html.includes('id="tbRunningBadge"'), "Old awkward top-left tbRunningBadge must be removed");
assert.ok(!html.includes("v2.0.0"), "Hardcoded v2.0.0 in index.html must be removed");
for (const id of [
  "appShell",
  "titlebar",
  "btnToggleSidebar",
  "tbBreadcrumb",
  "tbCrumbProj",
  "tbCrumbPage",
  "tbServerBadge",
  "tbSubTabs",
  "execHairline",
  "execHairlineFill",
  "execProgressWrap",
  "execProgressBar",
  "execStageLabel",
  "execDetailText",
  "execPctText",
  "execElapsedText",
  "btnExecViewTerm",
  "btnExecCancel",
  "btnExecDismiss",
  "btnOpenPalette",
  "btnDetectChanges",
  "btnToggleTerm",
  "tbTermDot",
  "btnOpenManual",
  "btnOpenPrefs",
  "btnWinMin",
  "btnWinMax",
  "btnWinClose",
  "sidebar",
  "navList",
  "sbProjCount",
  "projectList",
  "btnAddProject",
  "btnDiscoverProjects",
  "sbAdapterMeta",
  "sbAppVer",
  "watchBanner",
  "watchBannerText",
  "btnWatchSync",
  "btnWatchDismiss",
  "pageViewport",
  "termShell",
  "termResizeTop",
  "termResizeCorner",
  "termHead",
  "termDot",
  "termMeta",
  "termFilter",
  "btnCancelTask",
  "btnTermTone",
  "btnCopyLog",
  "btnClearLog",
  "btnTermFloat",
  "btnTermMin",
  "btnTermFull",
  "btnTermHide",
  "termBody",
  "termMiniPill",
  "termMiniDot",
  "termMiniLabel",
  "termMiniMeta",
  "modalDialog",
  "modalTitle",
  "modalCloseBtn",
  "modalBody",
  "modalFoot",
  "toastStack",
]) {
  assert.ok(html.includes(`id="${id}"`), `index.html missing required element #${id}`);
}

// 8. Static contract check: modules, dead file cleanup, defect elimination & new features
const appSrc = fs.readFileSync(path.join(__dir, "app.js"), "utf8");
for (const m of appSrc.matchAll(/from\s+"(\.\/(?:pages|components)\/[^"]+)"/g)) {
  const full = path.join(__dir, m[1]);
  assert.ok(fs.existsSync(full), `Missing imported module ${m[1]}`);
}
assert.ok(!fs.existsSync(path.join(__dir, "pages", "build.js")), "Dead file pages/build.js must be deleted");
assert.ok(fs.existsSync(path.join(__dir, "components", "command-palette.js")), "Missing command-palette.js");
assert.ok(fs.existsSync(path.join(__dir, "components", "preferences.js")), "Missing preferences.js");

const versionSrc = fs.readFileSync(path.join(__dir, "pages", "version.js"), "utf8");
assert.ok(!versionSrc.includes('id="mgOpenCfgBtn"'), "Duplicate mgOpenCfgBtn must be removed from version.js");
assert.ok(!versionSrc.includes('id="mgRemoveProjBtn"'), "Duplicate mgRemoveProjBtn must be removed from version.js");

const settingsSrc = fs.readFileSync(path.join(__dir, "pages", "settings.js"), "utf8");
assert.ok(!settingsSrc.includes("dupHead.style.display"), "settings.js must not hide manage.js page head");

const pkgSrc = fs.readFileSync(path.join(__dir, "pages", "package.js"), "utf8");
assert.ok(pkgSrc.includes("API.savePackageProfiles"), "package.js must wire API.savePackageProfiles");

const ovSrc = fs.readFileSync(path.join(__dir, "pages", "overview.js"), "utf8");
assert.ok(ovSrc.includes("API.previewBump"), "overview.js must call API.previewBump for live next-version preview");

// Verify old copy "编译与检测" is completely eradicated across src/web
const walk = (dir) => {
  const out = [];
  for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, ent.name);
    if (ent.isDirectory()) out.push(...walk(p));
    else if (/\.(html|css|js|mjs)$/i.test(ent.name) && ent.name !== "api.selftest.mjs") out.push(p);
  }
  return out;
};
for (const file of walk(webRoot)) {
  const text = fs.readFileSync(file, "utf8");
  assert.ok(!text.includes("编译与检测"), `Stale copy "编译与检测" found in ${file}`);
}

for (const pageFile of ["overview.js", "compile.js", "package.js", "release.js", "version.js", "manage.js"]) {
  const src = fs.readFileSync(path.join(__dir, "pages", pageFile), "utf8");
  assert.ok(src.includes("app.navigate("), `${pageFile} should route navigation through app.navigate()`);
}

// 9. Static contract check: Release Management/Recall + Git Commit/Push clarity + Verification fix
const apiSrc = fs.readFileSync(path.join(__dir, "api.js"), "utf8");
for (const fn of [
  "gitUndoLastCommit",
  "gitDeleteTag",
  "updateReleaseMulti",
  "deleteReleaseMulti",
  "deleteReleaseAssetMulti",
]) {
  assert.ok(apiSrc.includes(fn), `api.js must export ${fn}`);
}
const manageSrc = fs.readFileSync(path.join(__dir, "pages", "manage.js"), "utf8");
assert.ok(!manageSrc.includes("Push 提交与 Tags"), "Misleading label 'Push 提交与 Tags' must be removed");
assert.ok(manageSrc.includes('id="gitCommitPushBtn"'), "manage.js must provide one-click commit & push button");
assert.ok(manageSrc.includes('id="gitUndoCommitBtn"'), "manage.js must support undoing unpushed local commit");
assert.ok(manageSrc.includes("API.gitDeleteTag"), "manage.js must support local/remote tag deletion");

const releaseSrc = fs.readFileSync(path.join(__dir, "pages", "release.js"), "utf8");
assert.ok(releaseSrc.includes("API.updateReleaseMulti"), "release.js must wire online release edit/draft toggle");
assert.ok(releaseSrc.includes("API.deleteReleaseMulti"), "release.js must wire online release recall/delete");
assert.ok(releaseSrc.includes("API.deleteReleaseAssetMulti"), "release.js must wire single asset deletion");
assert.ok(releaseSrc.includes('id="relVerifyPendingBtn"'), "release.js must provide quick verify pending button");

// 10. Static contract check: Single Preferences entry (no duplicate sidebar prefs) + Updater + Context Menu
assert.ok(!html.includes('id="btnSidebarPrefs"'), "Duplicate sidebar #btnSidebarPrefs must be removed (keep single #btnOpenPrefs)");
assert.ok(html.includes('id="tbPrefsUpdDot"'), "Toolbar Preferences update badge #tbPrefsUpdDot must exist");
for (const fn of [
  "updaterStatus",
  "updaterConfigSet",
  "updaterCheck",
  "updaterDownload",
  "updaterCancel",
  "updaterClearCache",
  "updaterInstall",
  "updaterChangelog",
]) {
  assert.ok(apiSrc.includes(fn), `api.js must export ${fn}`);
}
const prefsSrc = fs.readFileSync(path.join(__dir, "components", "preferences.js"), "utf8");
assert.ok(prefsSrc.includes('id: "updates"'), "preferences.js must expose dedicated updates pane");
assert.ok(!prefsSrc.includes('id="prefOpenDataDirBtn"'), "preferences.js general pane must not duplicate Open Data Dir button");
assert.ok(!ovSrc.includes('id="ovProbeBtn"'), "overview.js must not duplicate toolbar Change Scan button");
assert.ok(!ovSrc.includes('id="ovWizardBtn"'), "overview.js must not duplicate onboarding wizard button in header");
assert.ok(!pkgSrc.includes('id="anGoPackBtn"'), "package.js must not duplicate top sub-tab switcher buttons");
assert.ok(html.includes('id="sfMenuAnchor"'), "Built-in context menu cursor anchor #sfMenuAnchor must exist");
assert.ok(html.includes('id="sfContextMenu"'), "Built-in context menu popover #sfContextMenu must exist");
assert.ok(html.includes('id="sfContextMenuGroup"'), "Built-in context menu group #sfContextMenuGroup must exist");
assert.ok(fs.existsSync(path.join(__dir, "components", "context-menu.js")), "Missing components/context-menu.js");

const cssSrc = fs.readFileSync(path.join(webRoot, "styles.css"), "utf8");
assert.ok(
  /\*\s*\{[^}]*-webkit-user-select:\s*none;[^}]*-moz-user-select:\s*none;[^}]*-ms-user-select:\s*none;[^}]*user-select:\s*none;/s.test(
    cssSrc
  ),
  "styles.css must set * { user-select: none } with -webkit-, -moz-, -ms- prefixes"
);
assert.ok(cssSrc.includes("-webkit-touch-callout: none"), "styles.css must set body { -webkit-touch-callout: none }");
assert.ok(
  cssSrc.includes("input,") &&
    cssSrc.includes("textarea,") &&
    cssSrc.includes("[contenteditable]") &&
    cssSrc.includes("user-select: text;"),
  "styles.css must keep input, textarea, [contenteditable] selectable with user-select: text"
);

// 11. Static contract check: Fullscreen / 2K / Ultrawide Fluid Desktop Adaptation
assert.ok(
  /\.page-wrap\s*\{[^}]*max-width:\s*none;/s.test(cssSrc),
  ".page-wrap must be full-width fluid (max-width: none) for fullscreen/2K adaptation"
);
assert.ok(cssSrc.includes("container-name: page"), ".page-viewport must expose named container 'page'");
assert.ok(cssSrc.includes("@container page (min-width: 1320px)"), "Overview fullscreen workbench query missing");
assert.ok(cssSrc.includes("@container page (min-width: 1400px)"), "Apple Notes fullscreen split query missing");
assert.ok(!releaseSrc.includes('class="notes-split" style="grid-template-columns:'), "release.js must not hardcode inline grid-template-columns on .notes-split");

// 12. Round 1 Red-Team Frontend Hardening Contracts
assert.ok(!ovSrc.includes("git.hasRemote"), "overview.js must use git.remoteUrl (GitStatus has no hasRemote field)");
assert.ok(ovSrc.includes("git.remoteUrl"), "overview.js must guard commit_tag_push with git.remoteUrl");
const uiSrc = fs.readFileSync(path.join(__dir, "ui.js"), "utf8");
assert.ok(uiSrc.includes("prepareModalForReopen"), "ui.js must guard #modalDialog against re-entrant showModal InvalidStateError");
const ctxSrc = fs.readFileSync(path.join(__dir, "components", "context-menu.js"), "utf8");
assert.ok(ctxSrc.includes("supportsSelectionRange"), "context-menu.js must safely handle <input type=number> without setRangeText crash");
assert.ok(appSrc.includes("overviewReqSeq") && appSrc.includes("switchSeq"), "app.js must isolate overviewReqSeq & switchSeq from renderEpoch");
assert.ok(settingsSrc.includes("Math.trunc"), "settings.js must truncate numeric inputs to integers for Rust u64/usize");
assert.ok(releaseSrc.includes("Number.isInteger(rawPort)"), "release.js must validate local server port as integer 1024..=65535");
assert.ok(releaseSrc.includes('st === "invalid"'), "release.js must render invalid traversal checksum badge");

// 13. Cross-Project Data Isolation & Change Classification Contracts (Red-Team Round 1 & 2)
for (const sym of [
  "activeTasksByProj",
  "lastProgressByProj",
  "finishedTaskIds",
  "logsByProj",
  "pipelineStepsByProj",
  "watchBannerByProj",
  "watchInFlightByProj",
  "clearTransientCrossProjectHints",
  "syncWatchBannerForActiveProject",
  "syncTaskChromeForActiveProject",
]) {
  assert.ok(appSrc.includes(sym), `app.js must implement per-project isolated state: ${sym}`);
}
assert.ok(
  !appSrc.includes("App.logs = []"),
  "app.js must clear logsByProj per project instead of assigning to readonly logs getter"
);
assert.ok(
  appSrc.includes("seq !== this.switchSeq || this.activeProject?.id !== pid"),
  "app.js probeProjectChanges must guard post-await updates with switchSeq + activeProject.id barrier"
);
assert.ok(
  releaseSrc.includes("ownedByOther") && releaseSrc.includes("切换为当前项目重启服务"),
  "release.js must isolate local update server ownership across projects"
);
const hrSrc = fs.readFileSync(path.join(__dir, "components", "health-report.js"), "utf8");
assert.ok(
  hrSrc.includes("reportProjectId") && hrSrc.includes("app.switchProject(reportProjectId)"),
  "health-report.js must display project ownership and switch project before launching/navigating"
);
assert.ok(
  ovSrc.includes("changeReport?.classifiedChanges") && ovSrc.includes("changeReport?.changeSummary"),
  "overview.js must display classified change summary consistently for both Git and non-Git projects"
);

// 14. Whole-App Accuracy & False-Data Prevention Contracts
const compileSrc = fs.readFileSync(path.join(__dir, "pages", "compile.js"), "utf8");
assert.ok(
  compileSrc.includes("请至少勾选一项要补全的前置依赖"),
  "compile.js must block empty checkbox repair instead of silently repairing everything"
);
assert.ok(
  compileSrc.includes("目标磁盘可用空间不足") && compileSrc.includes("已损坏"),
  "compile.js must distinguish disk headroom failure and corrupted tools accurately"
);
assert.ok(
  pkgSrc.includes("rep.truncated"),
  "package.js must warn when directory analysis hits the 80,000-file cap"
);
assert.ok(
  releaseSrc.includes("检测到产物版本与清单版本不一致"),
  "release.js must guard against generating version-mismatched latest.json manifests"
);

// 15. Auto Release Notes Extraction + Clear/Manual Edit Contracts
for (const sym of [
  "plNotes",
  "plNotesSource",
  "plNotesSaveClBtn",
  "plNotesClearBtn",
  "plNotesReloadBtn",
  "_plNotesState",
  "plNotesIsFallback",
  "resolveReleaseNotesDetail",
  "upsertChangelogEntry",
]) {
  assert.ok(ovSrc.includes(sym), `overview.js must support auto-filled/clearable pipeline notes: ${sym}`);
}
for (const sym of [
  "mfNotesSource",
  "mfNotesSaveClBtn",
  "mfNotesClearBtn",
  "mfNotesReloadBtn",
  "mfNotesIsFallback",
  "mpNotesSource",
  "mpNotesSaveClBtn",
  "mpNotesClearBtn",
  "resolveReleaseNotesDetail",
  "upsertChangelogEntry",
]) {
  assert.ok(releaseSrc.includes(sym), `release.js must support auto-filled/clearable manifest & publish notes: ${sym}`);
}

// 16. Ready-to-Deploy Detection & Standalone Zero-Rebuild Deploy/Push Contracts
for (const sym of [
  "deployReadiness",
  "ovDeployCard",
  "ovQuickDeployBtn",
  "dpGitMode",
  "dpReleaseMode",
  "dpEnsureManifest",
  "dpNotesSource",
  "deployExistingRelease",
  "plNotesMatchesCurrentVer",
]) {
  assert.ok(ovSrc.includes(sym), `overview.js must support ready-to-deploy detection & zero-rebuild push: ${sym}`);
}
assert.ok(
  !ovSrc.includes("!a.archived"),
  "overview.js must check !a.isArchived (camelCase) instead of !a.archived"
);
assert.ok(
  appSrc.includes("prevDeployStatus") && appSrc.includes("ready_to_deploy"),
  "app.js probeProjectChanges must detect ready_to_deploy state transitions on external builds"
);

// 17. Deep Full-App Audit & Red-Team Round 1 Hardening Contracts
assert.ok(
  !appSrc.includes("#pageHost"),
  "app.js must query #pageViewport (NOT non-existent #pageHost) so autoWatch never collapses expanded <details>"
);
assert.ok(
  appSrc.includes("#pageViewport details[open]") && appSrc.includes("_ovFormTouchedByProj"),
  "app.js autoWatch must protect expanded <details> and touched overview form controls"
);
assert.ok(
  appSrc.includes('this.activeTasksByProj.get("__system__")?.taskId') && appSrc.includes("hasAnyVisibleLogs"),
  "app.js must allow cancelling __system__ tasks and preserve __system__ logs on rerender/filter"
);
const wizardSrc = fs.readFileSync(path.join(__dir, "components", "wizard.js"), "utf8");
assert.ok(
  wizardSrc.includes("credentialId:") && wizardSrc.includes('dlg.addEventListener("close"'),
  "wizard.js must pass credentialId to bindProjectRepo and remove dialog on native Esc/backdrop close"
);
assert.ok(
  prefsSrc.includes("confirmDialog") && releaseSrc.includes("删除已保存的访问令牌"),
  "Both preferences.js and release.js must require confirmation before deleting DPAPI credentials"
);
assert.ok(
  releaseSrc.includes("saveComposeDraftFromDom()") &&
    releaseSrc.includes("uncheckedAssets") &&
    releaseSrc.includes("notesIsFallback"),
  "release.js must preserve user compose inputs & asset selections when loadReleases resolves asynchronously"
);
assert.ok(
  versionSrc.includes("normalizeEntriesNewestFirst") &&
    versionSrc.includes("app._pendingChangelog?.") === false &&
    versionSrc.includes("app._pendingChangelog.projectId === project.id"),
  "version.js must display changelogs newest-first and isolate pending changelog drafts per project"
);
assert.ok(
  compileSrc.includes("syncActionBusyState") && manageSrc.includes("syncGitBusyButtons"),
  "compile.js actions tab and manage.js Git tab must lock buttons via sf-task-state-changed while project is busy"
);
assert.ok(
  pkgSrc.includes("pendingPresetSources") &&
    pkgSrc.includes("lastPackResultByProj") &&
    pkgSrc.includes("engineTopDir"),
  "package.js must guard preset-before-entries race, persist pack result summary, and include engine dir in coreDefaults"
);
assert.ok(
  manageSrc.includes("isNonGhBinding") && manageSrc.includes("project.repoBinding.repoSlug"),
  "manage.js Project Settings must synchronize repoBinding.repoSlug without split-brain"
);
assert.ok(
  ovSrc.includes("ovDeployBindRepoBtn") && ovSrc.includes("syncChannelForVersion"),
  "overview.js must provide bind-repo CTA on unbound deploy cards and auto-sync channel for -beta versions"
);

// 18. Red-Team Round 2 Hardening Contracts
assert.ok(
  appSrc.includes("lastPackResultByProj?.delete(project.id)") &&
    appSrc.includes("_ovFormTouchedByProj?.delete(project.id)"),
  "app.js removeProjectWithConfirm must clean up all per-project transient caches"
);
assert.ok(
  releaseSrc.includes("focusInsideRightPane") &&
    releaseSrc.includes("app.isProjectBusy(project.id)"),
  "release.js must scope focus restoration to #mpRightPane and fail fast on busy project before dialogs"
);
assert.ok(
  compileSrc.includes("prereqSeq") && compileSrc.includes("当前项目正在执行任务，请等待完成后再补全依赖"),
  "compile.js prereq tab must guard concurrent refresh races and check busy state before repair"
);

// 19. Watch-Banner Zero-Loop & Release-Notes Placeholder Guard Contracts
assert.ok(
  appSrc.includes("ovSeq !== this.overviewReqSeq") &&
    appSrc.includes("App.overview.changeReport = rep") &&
    appSrc.includes("deployStatusChanged"),
  "app.js must prevent in-flight watch race after sync and update in-memory baseline on dismiss"
);
assert.ok(
  ovSrc.includes("plNotesUseLatestBtn") &&
    ovSrc.includes("(!plNotesUserTouched && plNotesIsFallback)") &&
    releaseSrc.includes("(!mfNotesUserTouched && mfNotesIsFallback)") &&
    releaseSrc.includes("(!composeState.userTouched && composeState.notesIsFallback)"),
  "overview.js & release.js must block writing bare fallback placeholders into changelog and offer latest-entry load"
);

// 20. Red-only-for-abnormal colour spec
assert.ok(
  !/\.log-line\.stderr[^{]*\{[^}]*var\(--err\)/.test(cssSrc),
  "stderr stream must never be styled red; only .lv-err (real errors) may be red in the terminal"
);
const btnDangerBlock = (cssSrc.match(/\.btn-danger\s*\{[^}]*\}/) || [""])[0];
assert.ok(
  btnDangerBlock && !btnDangerBlock.includes("--err"),
  ".btn-danger (stop/delete/remove/recall) must be neutral, red is reserved for abnormal states"
);
assert.ok(
  !/\.sf-menu-item\.danger[^{]*\{[^}]*var\(--err\)/.test(cssSrc) &&
    !/\.exec-capsule-btn\.stop:hover\s*\{[^}]*--err/.test(cssSrc),
  "context-menu danger items and stop-capsule hover must not turn red"
);

// 21. Region-scoped context menu: no universal dump of every global control
assert.ok(
  !ctxSrc.includes("Universal Workspace Controls") &&
    !ctxSrc.includes("深色外观模式") &&
    !ctxSrc.includes("使用说明书") &&
    ctxSrc.includes('el?.closest("#sidebar")') &&
    ctxSrc.includes("Region-scoped menu"),
  "context-menu.js must show only the clicked region's actions, never a global dump of theme/prefs/manual/terminal toggles"
);

console.log("ShipForge frontend selftest: 全部通过 (21/21)");
