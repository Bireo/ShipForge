// Typed Tauri IPC bridge + pure helpers

const getTauri = () => window.__TAURI__ || null;

export async function invoke(cmd, args = {}) {
  const t = getTauri();
  if (!t?.core?.invoke) {
    throw new Error(`Tauri 运行时不可用（命令：${cmd}）`);
  }
  return t.core.invoke(cmd, args);
}

export async function listen(event, handler) {
  const t = getTauri();
  if (!t?.event?.listen) return () => {};
  return t.event.listen(event, handler);
}

export function getCurrentWindow() {
  return getTauri()?.window?.getCurrentWindow?.() || null;
}

export const API = {
  getAppState: () => invoke("get_app_state"),
  setAutoWatch: (enabled) => invoke("set_auto_watch", { enabled }),
  setScanRoots: (scanRoots) => invoke("set_scan_roots", { scanRoots }),
  discoverWorkspaceProjects: (scanDir = null, saveRoot = false) =>
    invoke("discover_workspace_projects", { scanDir, saveRoot }),
  importProjectsBatch: (paths) => invoke("import_projects_batch", { paths }),
  detectProjectGithubRepo: (projectId) => invoke("detect_project_github_repo", { projectId }),
  launchGhAuth: (mode = "login") => invoke("launch_gh_auth", { mode }),
  getAppInfo: () => invoke("get_app_info"),
  setActiveProject: (projectId) => invoke("set_active_project", { projectId }),
  pickFolder: (title = null) => invoke("pick_folder", { title }),
  addProjectByPath: (path) => invoke("add_project_by_path", { path }),
  rescanProject: (projectId) => invoke("rescan_project", { projectId }),
  updateProjectConfig: (project) => invoke("update_project_config", { project }),
  removeProject: (projectId) => invoke("remove_project", { projectId }),
  getProjectOverview: (projectId) => invoke("get_project_overview", { projectId }),

  detectProjectChanges: (projectId, acknowledge = false) =>
    invoke("detect_project_changes", { projectId, acknowledge }),
  getVersionBuckets: (projectId) => invoke("get_version_buckets", { projectId }),
  saveVersionDraft: (projectId, versionStr, notesDraft) =>
    invoke("save_version_draft", { projectId, versionStr, notesDraft }),
  sealVersionBucket: (projectId, versionStr, sealed) =>
    invoke("seal_version_bucket", { projectId, versionStr, sealed }),

  getStorageInfo: () => invoke("get_storage_info"),
  migrateStorageRoot: (newRoot) => invoke("migrate_storage_root", { newRoot }),
  consolidateToolsAndCaches: () => invoke("consolidate_tools_and_caches"),
  scanToolchains: () => invoke("scan_toolchains"),
  repairToolchain: (toolId) => invoke("repair_toolchain", { toolId }),
  repairAllEssentialToolchains: () => invoke("repair_all_essential_toolchains"),

  previewBump: (current, kind) => invoke("preview_bump", { current, kind }),
  applyVersion: (projectId, newVersion) => invoke("apply_version", { projectId, newVersion }),
  getChangelog: (projectId) => invoke("get_changelog", { projectId }),
  addChangelogEntry: (projectId, versionStr, summary, body) =>
    invoke("add_changelog_entry", { projectId, versionStr, summary, body }),
  upsertChangelogEntry: (projectId, versionStr, body, summary = null) =>
    invoke("upsert_changelog_entry", { projectId, versionStr, body, summary }),
  resolveReleaseNotesDetail: (projectId, versionStr) =>
    invoke("resolve_release_notes_detail", { projectId, versionStr }),
  extractReleaseNotes: (projectId, versionStr) =>
    invoke("extract_release_notes", { projectId, versionStr }),
  draftChangelogFromGit: (projectId, versionStr = null) =>
    invoke("draft_changelog_from_git", { projectId, versionStr }),

  runProjectAction: (opts) => invoke("run_project_action", opts),
  runCustomCommand: (opts) => invoke("run_custom_command", opts),
  runPipeline: (req) => invoke("run_pipeline", { req }),
  deployExistingRelease: (req) => invoke("deploy_existing_release", { req }),
  cancelTask: (taskId) => invoke("cancel_task", { taskId }),
  getActiveTasks: () => invoke("get_active_tasks"),

  listProjectEntries: (projectId) => invoke("list_project_entries", { projectId }),
  packPortableArchive: (req) => invoke("pack_portable_archive", { req }),
  computeArtifactSha256: (path) => invoke("compute_artifact_sha256", { path }),
  generateReleaseManifest: (req) => invoke("generate_release_manifest", { req }),
  verifyReleaseChecksums: (projectId) => invoke("verify_release_checksums", { projectId }),
  archiveReleaseArtifact: (projectId, fileName) =>
    invoke("archive_release_artifact", { projectId, fileName }),
  pruneOldReleases: (projectId, keepVersions) =>
    invoke("prune_old_release_artifacts", { projectId, keepVersions }),

  getLocalServerStatus: () => invoke("get_local_server_status"),
  startLocalServer: (projectId, port, rewriteManifest = true) =>
    invoke("start_local_server", { projectId, port, rewriteManifest }),
  stopLocalServer: () => invoke("stop_local_server"),
  launchProjectExe: (projectId, updateFeedUrl = null) =>
    invoke("launch_project_exe", { projectId, updateFeedUrl }),
  openInExplorer: (path) => invoke("open_in_explorer", { path }),
  openExternalUrl: (url) => invoke("open_external_url", { url }),

  gitCommit: (projectId, message) => invoke("git_commit_changes", { projectId, message }),
  gitTag: (projectId, tag, message = "") => invoke("git_tag_release", { projectId, tag, message }),
  gitPush: (projectId, pushTags = false) => invoke("git_push_remote", { projectId, pushTags }),
  gitUndoLastCommit: (projectId) => invoke("git_undo_last_commit", { projectId }),
  gitDeleteTag: (projectId, tag, deleteRemote = false) =>
    invoke("git_delete_tag", { projectId, tag, deleteRemote }),
  cleanStaleLock: (projectId) => invoke("clean_stale_build_lock", { projectId }),

  getGhStatus: () => invoke("get_gh_status"),
  getGhReleases: (repo) => invoke("get_gh_releases", { repo }),
  publishToGithub: (req) => invoke("publish_to_github", { req }),

  // ShipForge v2.0 APIs
  setOnboardingCompleted: (completed) => invoke("set_onboarding_completed", { completed }),
  updateGuardrailSettings: (settings) => invoke("update_guardrail_settings", { settings }),
  getSystemHeadroom: (projectId = null) => invoke("get_system_headroom", { projectId }),
  checkCrashRecovery: () => invoke("check_crash_recovery"),
  getProjectProfileMatrix: (projectId) => invoke("get_project_profile_matrix", { projectId }),
  listFrameworkPresets: (projectId) => invoke("list_framework_presets", { projectId }),
  applyFrameworkPreset: (projectId, presetId) =>
    invoke("apply_framework_preset", { projectId, presetId }),
  applyProjectProfile: (projectId, language, shell) =>
    invoke("apply_project_profile", { projectId, language, shell }),
  inspectProjectPrerequisites: (projectId) =>
    invoke("inspect_project_prerequisites", { projectId }),
  planPrerequisiteRepair: (projectId, selectedIds = []) =>
    invoke("plan_prerequisite_repair", { projectId, selectedIds }),
  executePrerequisiteRepair: (projectId, approvedIds = []) =>
    invoke("execute_prerequisite_repair", { projectId, approvedIds }),
  analyzeProjectFiles: (projectId) => invoke("analyze_project_files", { projectId }),
  savePackageProfiles: (projectId, profiles) =>
    invoke("save_package_profiles", { projectId, profiles }),
  listClassifiedArtifacts: (projectId) => invoke("list_classified_artifacts", { projectId }),
  markArtifactStage: (projectId, fileNames, stage) =>
    invoke("mark_artifact_stage", { projectId, fileNames, stage }),
  listRepoCredentials: () => invoke("list_repo_credentials"),
  verifyAndSaveRepoCredential: ({ id = null, provider, baseUrl = "", label = "", token }) =>
    invoke("verify_and_save_repo_credential", { id, provider, baseUrl, label, token }),
  deleteRepoCredential: (credId) => invoke("delete_repo_credential", { credId }),
  listRemoteRepos: (provider, baseUrl = "", credentialId = "") =>
    invoke("list_remote_repos", { provider, baseUrl, credentialId }),
  bindProjectRepo: ({
    projectId,
    provider,
    baseUrl = "",
    repoSlug,
    credentialId = "",
    cloneUrl = null,
    initGitIfNeeded = true,
  }) =>
    invoke("bind_project_repo", {
      projectId,
      provider,
      baseUrl,
      repoSlug,
      credentialId,
      cloneUrl,
      initGitIfNeeded,
    }),
  listReleasesMulti: (provider, baseUrl = "", credentialId = "", repo) =>
    invoke("list_releases_multi", { provider, baseUrl, credentialId, repo }),
  publishReleaseMulti: (req) => invoke("publish_release_multi", { req }),
  updateReleaseMulti: (req) => invoke("update_release_multi", { req }),
  syncChangelogToOnlineRelease: (req) => invoke("sync_changelog_to_online_release", { req }),
  deleteReleaseMulti: (req) => invoke("delete_release_multi", { req }),
  deleteReleaseAssetMulti: (req) => invoke("delete_release_asset_multi", { req }),
  updaterStatus: () => invoke("updater_status"),
  updaterConfigGet: () => invoke("updater_config_get"),
  updaterConfigSet: (patch) => invoke("updater_config_set", { patch }),
  updaterCheck: (channel = null) => invoke("updater_check", { channel }),
  updaterDownload: (opts) =>
    invoke("updater_download", {
      url: opts.url,
      sha256: opts.sha256,
      size: opts.size || 0,
      version: opts.version,
      fileName: opts.fileName,
      packageKind: opts.packageKind,
    }),
  updaterCancel: () => invoke("updater_cancel"),
  updaterClearCache: () => invoke("updater_clear_cache"),
  updaterAutoUpdate: (opts) =>
    invoke("updater_auto_update", {
      url: opts.url,
      sha256: opts.sha256,
      size: opts.size || 0,
      version: opts.version,
      fileName: opts.fileName,
      packageKind: opts.packageKind,
      theme: opts.theme || null,
    }),
  updaterInstall: (silent = false) => invoke("updater_install", { silent: !!silent }),
  updaterChangelog: (includeRemote = false) => invoke("updater_changelog", { includeRemote: !!includeRemote }),
};

export function esc(str) {
  return String(str ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

export function formatBytes(bytes) {
  const n = Number(bytes) || 0;
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(2)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

/**
 * Progressive compact duration formatter:
 * - < 60s (subSecond=true): 0.0s..59.9s (rolls cleanly at 59.95s -> 1m0s, never displays "60.0s")
 * - < 60s (subSecond=false): 0s..59s
 * - >= 60s: 1m0s, 4m52s, 1h0m0s, 1d1h1m1s (only displays units reached so far)
 */
export function formatDuration(ms, { subSecond = true } = {}) {
  const n = Number(ms);
  if (!Number.isFinite(n) || n <= 0) return subSecond ? "0.0s" : "0s";
  let totalSec;
  if (subSecond) {
    const tenths = Math.round(n / 100);
    if (tenths < 600) return `${(tenths / 10).toFixed(1)}s`;
    totalSec = Math.floor(tenths / 10);
  } else {
    totalSec = Math.floor(n / 1000);
    if (totalSec < 60) return `${totalSec}s`;
  }
  const days = Math.floor(totalSec / 86400);
  const remDay = totalSec % 86400;
  const hours = Math.floor(remDay / 3600);
  const remHr = remDay % 3600;
  const mins = Math.floor(remHr / 60);
  const secs = remHr % 60;
  if (days > 0) return `${days}d${hours}h${mins}m${secs}s`;
  if (hours > 0) return `${hours}h${mins}m${secs}s`;
  return `${mins}m${secs}s`;
}

/** Normalizes legacy duration strings like "613.9s" or "292.0s" from build journals into progressive format */
export function formatDurationText(raw) {
  const s = String(raw ?? "").trim();
  if (!s || s === "-") return "-";
  const secMatch = /^(\d+(?:\.\d+)?)s$/i.exec(s);
  if (secMatch) {
    return formatDuration(parseFloat(secMatch[1]) * 1000, { subSecond: true });
  }
  const msMatch = /^(\d+(?:\.\d+)?)ms$/i.exec(s);
  if (msMatch) {
    return formatDuration(parseFloat(msMatch[1]), { subSecond: true });
  }
  return s;
}

export function isValidSemver(v) {
  return /^\d+\.\d+\.\d+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.test(String(v || "").trim());
}

export function parseCommandLine(raw) {
  const s = String(raw || "").trim();
  if (!s) return { command: "", args: [] };
  const tokens = [];
  let cur = "";
  let quote = null;
  for (let i = 0; i < s.length; i++) {
    const ch = s[i];
    if (quote) {
      if (ch === quote) quote = null;
      else cur += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (/\s/.test(ch)) {
      if (cur) {
        tokens.push(cur);
        cur = "";
      }
    } else {
      cur += ch;
    }
  }
  if (cur) tokens.push(cur);
  return { command: tokens[0] || "", args: tokens.slice(1) };
}

/**
 * Terminal log colour spec (single source of truth):
 *  - RED (`lv-err`)   ONLY for real errors: explicit ✗ / [红] marks, `error:` / `error[E0000]:` / `fatal:` /
 *                     `panicked at` / `Traceback` / `FAILED` / non-zero "N failed" / "失败：…" / non-zero exit.
 *  - AMBER (`lv-warn`) for warnings (`warning:` / 警告 / [黄]).
 *  - GREEN (`lv-ok`)   for explicit success marks (✓ / [绿] / 全部通过 / 构建完成 / passed / OK).
 *  - ACCENT (`lv-section`) for section headers; everything else stays neutral.
 *  The output STREAM alone (stderr) NEVER implies an error: cargo, git, npm, pwsh etc. write normal progress
 *  ("Compiling…", "Info …") to stderr.
 */
export function classifyLogLevel(line, stream) {
  const t = String(line || "").trim();
  if (t.startsWith("===") || t.startsWith("▶ ") || t.startsWith("──── ")) return "lv-section";
  if (t.startsWith("✗") || t.includes("[红]")) return "lv-err";
  if (t.startsWith("✓") || t.includes("[绿]")) return "lv-ok";
  if (t.includes("[黄]")) return "lv-warn";

  const zeroFail = /(?:\b0\s+(?:failed|failures?|errors?)\b|失败\s*[:：=]?\s*0\b|\bfailures?\s*[:=]\s*0\b|\berrors?\s*[:=]\s*0\b)/i.test(t);
  const isErr =
    /^(?:error(?:\[[A-Za-z0-9]+\])?\s*:|fatal\s*:|panicked at\b|traceback \(most recent call last\))/i.test(t) ||
    /\bthread '.*' panicked\b/.test(t) ||
    /\btest result:\s*FAILED\b/.test(t) ||
    /\b[1-9]\d*\s+(?:failed|failures?)\b/i.test(t) ||
    /(?:^|[\s:：（(\[])(?:构建|编译|测试|发布|上传|打包|执行)?失败(?:[:：（(]|$)/.test(t) && !zeroFail ||
    /\bexit(?:ed)?(?:\s+code)?\s*[:=]?\s*[1-9]\d*\b/i.test(t) ||
    /^(?:FAILED|FAIL)\b/.test(t);
  if (isErr && !zeroFail) return "lv-err";

  if (/^(?:warning(?:\[[A-Za-z0-9_]+\])?\s*:)/i.test(t) || t.includes("警告")) return "lv-warn";
  if (t.includes("全部通过") || t.includes("构建完成") || /\b(?:passed|tests? OK)\b/.test(t) || /\bOK\b\s*(?:\(|$)/.test(t)) {
    return "lv-ok";
  }
  return stream === "system" ? "system" : "";
}

export function kindLabel(kind) {
  const map = {
    installer: "安装包",
    portable: "便携包",
    manifest: "更新清单",
    checksum: "校验表",
    archive: "归档",
    other: "产物",
  };
  return map[kind] || kind;
}

export function categoryLabel(cat) {
  const map = {
    test: "测试",
    build: "构建",
    package: "打包",
    publish: "发布",
    maintenance: "维护",
    custom: "自定义",
  };
  return map[cat] || cat;
}
