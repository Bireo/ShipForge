import { API, classifyLogLevel, esc, formatDuration, getCurrentWindow, listen } from "./api.js";
import { attachLightDismissFallback, confirmDialog, copyText, hydrateIcons, toast, withBusy } from "./ui.js";
import { renderOverview } from "./pages/overview.js";
import { renderCompile } from "./pages/compile.js";
import { renderPackage } from "./pages/package.js";
import { renderRelease } from "./pages/release.js";
import { renderVersion } from "./pages/version.js";
import { showOneTimeHealthReport } from "./components/health-report.js";
import { closeManualDrawer, openManualDrawer } from "./components/manual.js";
import { openOnboardingWizard } from "./components/wizard.js";
import { closeCommandPalette, openCommandPalette } from "./components/command-palette.js";
import { closePreferences, openPreferences } from "./components/preferences.js";
import {
  closeBuiltInContextMenu,
  initBuiltInContextMenu,
  openProjectRowMenu,
} from "./components/context-menu.js";

const PAGES = {
  overview: renderOverview,
  compile: renderCompile,
  package: renderPackage,
  release: renderRelease,
  version: renderVersion,
};

const PAGE_LABELS = {
  overview: "发版总览",
  compile: "编译与环境",
  package: "分析与分包",
  release: "产物与仓库",
  version: "版本与源码",
};

const SIDEBAR_KEY = "sf-sidebar-collapsed";
const VER_CACHE_KEY = "sf-proj-ver-cache";

function loadCachedProjectMeta() {
  try {
    const raw = localStorage.getItem(VER_CACHE_KEY);
    if (!raw) return new Map();
    const obj = JSON.parse(raw);
    return new Map(Object.entries(obj || {}));
  } catch {
    return new Map();
  }
}

function saveCachedProjectMeta(map) {
  try {
    const obj = {};
    for (const [k, v] of map.entries()) obj[k] = v;
    localStorage.setItem(VER_CACHE_KEY, JSON.stringify(obj));
  } catch {}
}

function closeContextMenu() {
  closeBuiltInContextMenu();
  document.querySelectorAll('.sb-proj-more[aria-expanded="true"]').forEach((b) => {
    b.setAttribute("aria-expanded", "false");
  });
}

export const App = {
  config: { activeProjectId: null, projects: [], defaultPort: 18765, autoWatch: true, onboardingCompleted: false },
  overview: null,
  serverStatus: null,
  appVersion: "",
  currentPage: "overview",
  subTabs: {
    compile: "presets",
    package: "analyze",
    release: "artifacts",
    version: "changelog",
  },
  projectMetaCache: loadCachedProjectMeta(),
  packageState: { lastAnalysisByProj: new Map(), extraExcludesByProj: new Map() },
  renderEpoch: 0,
  overviewReqSeq: 0,
  switchSeq: 0,
  unmountPage: null,

  // Strictly per-project isolated execution, terminal, pipeline & watch state
  activeTasksByProj: new Map(), // projectId -> { taskId, label, isPipeline, startedAt }
  lastProgressByProj: new Map(), // projectId -> { stage, detail, percent, indeterminate, state }
  finishedTaskIds: new Set(), // bounded recent completed task IDs (blocks fast-finish-before-return race)
  logsByProj: new Map(), // projectId -> LogEntry[]
  pipelineStepsByProj: new Map(), // projectId -> PipelineStep[]
  watchBannerByProj: new Map(), // projectId -> string
  watchInFlightByProj: new Set(), // projectId currently probing
  taskTimer: null,
  progressDismissTimer: null,

  // Adaptive 5-mode terminal state: hidden | docked | floating | minimized | fullscreen
  termMode: "hidden",
  termLastVisibleMode: "docked",
  termTone: "auto",

  // Auto-watch state
  watchTimer: null,

  get activeProject() {
    return (
      this.config.projects.find((p) => p.id === this.config.activeProjectId) ||
      this.config.projects[0] ||
      null
    );
  },

  get activeProjectIdKey() {
    return this.activeProject?.id || "__system__";
  },

  get activeTaskId() {
    const pid = this.activeProject?.id;
    return (
      (pid ? this.activeTasksByProj.get(pid)?.taskId : null) ||
      this.activeTasksByProj.get("__system__")?.taskId ||
      null
    );
  },

  get activeTaskLabel() {
    const pid = this.activeProject?.id;
    return (
      (pid ? this.activeTasksByProj.get(pid)?.label : "") ||
      this.activeTasksByProj.get("__system__")?.label ||
      ""
    );
  },

  get logs() {
    const key = this.activeProjectIdKey;
    if (!this.logsByProj.has(key)) this.logsByProj.set(key, []);
    const projLogs = this.logsByProj.get(key);
    if (key === "__system__") return projLogs;
    const sysLogs = this.logsByProj.get("__system__");
    if (!sysLogs || !sysLogs.length) return projLogs;
    return [...projLogs, ...sysLogs].sort((a, b) => (a._seq || 0) - (b._seq || 0));
  },

  get pipelineSteps() {
    const key = this.activeProjectIdKey;
    if (!this.pipelineStepsByProj.has(key)) this.pipelineStepsByProj.set(key, []);
    return this.pipelineStepsByProj.get(key);
  },

  hasAnyVisibleLogs() {
    const key = this.activeProjectIdKey;
    if ((this.logsByProj.get(key)?.length || 0) > 0) return true;
    return (this.logsByProj.get("__system__")?.length || 0) > 0;
  },

  findProjectIdByTaskId(taskId) {
    if (!taskId) return null;
    for (const [pid, info] of this.activeTasksByProj.entries()) {
      if (info.taskId === taskId) return pid;
    }
    return null;
  },

  async refreshOverview(silent = false) {
    const proj = this.activeProject;
    const reqSeq = ++this.overviewReqSeq;
    if (!proj) {
      this.overview = null;
      this.updateChrome();
      this.renderProjectList();
      this.renderPage();
      return;
    }
    try {
      const [ov, srv] = await Promise.all([
        API.getProjectOverview(proj.id),
        API.getLocalServerStatus(),
      ]);
      // Guard against stale project loads without letting tab navigation drop the active project's overview!
      if (reqSeq !== this.overviewReqSeq || this.activeProject?.id !== proj.id) return;
      this.overview = ov;
      this.serverStatus = srv;
      const effDirty = ov.changeReport?.dirtyCount ?? ov.git?.dirtyCount ?? 0;
      const status = !ov.rootExists
        ? "err"
        : ov.healthIssues?.length || !ov.versions?.synced || effDirty > 0
          ? "warn"
          : "ok";
      this.projectMetaCache.set(proj.id, {
        version: ov.versions?.current || "",
        status,
      });
      saveCachedProjectMeta(this.projectMetaCache);
      this.clearWatchBannerForProject(proj.id);
      this.updateChrome();
      this.renderProjectList();
      this.renderPage();
      if (!silent) toast("状态已同步", "ok", 1300);
    } catch (e) {
      if (reqSeq === this.overviewReqSeq) toast(`加载项目失败：${e}`, "err");
    }
  },

  async reloadConfig(keepPage = true) {
    this.config = await API.getAppState();
    const validIds = new Set((this.config.projects || []).map((p) => p.id));
    let pruned = false;
    for (const k of [...this.projectMetaCache.keys()]) {
      if (!validIds.has(k)) {
        this.projectMetaCache.delete(k);
        pruned = true;
      }
    }
    if (pruned) saveCachedProjectMeta(this.projectMetaCache);
    this.renderProjectList();
    await this.refreshOverview(true);
    if (!keepPage) this.navigate("overview");
  },

  clearTransientCrossProjectHints() {
    this._pendingChangelog = null;
    this._pendingChangelogVer = null;
    this._pendingChangelogDraft = null;
    this._pendingSelectRelease = null;
    this._pendingSelectReleaseTag = null;
  },

  syncTaskChromeForActiveProject() {
    const pid = this.activeProject?.id;
    const task =
      (pid ? this.activeTasksByProj.get(pid) : null) ||
      this.activeTasksByProj.get("__system__") ||
      null;
    const cancelBtn = document.getElementById("btnCancelTask");
    if (task) {
      cancelBtn?.classList.remove("hidden");
      this.updateTermDots("running");
      const dur = formatDuration(performance.now() - task.startedAt);
      this.setTermMeta(task.label, dur);
      const savedProg = this.lastProgressByProj.get(task.projectId);
      this.showProgressBar({
        stage: savedProg?.stage || task.label,
        detail: savedProg?.detail || "执行中…",
        percent: savedProg?.percent ?? 10,
        indeterminate: Boolean(savedProg?.indeterminate),
        state: "running",
        cancellable: true,
      });
    } else {
      cancelBtn?.classList.add("hidden");
      this.updateTermDots("");
      this.setTermMeta(this.activeProject ? `${this.activeProject.name} · 就绪` : "就绪", "");
      // Always hide progress capsule when switched to an idle project so Project A's bar NEVER bleeds into Project B!
      this.hideProgressBar();
    }
    const dot = document.getElementById("tbTermDot");
    if (dot) dot.classList.toggle("hidden", !this.hasAnyVisibleLogs() && !task);
  },

  async switchProject(pid) {
    if (!pid || pid === this.activeProject?.id) return;
    const seq = ++this.switchSeq;
    try {
      const nextCfg = await API.setActiveProject(pid);
      if (seq !== this.switchSeq) return;
      this.config = nextCfg;
      // Immediately clear stale overview & pending cross-project hints so ZERO old project state leaks into the new project!
      this.overview = null;
      this.clearTransientCrossProjectHints();
      this.syncWatchBannerForActiveProject();
      this.rerenderLogs();
      this.syncTaskChromeForActiveProject();
      this.renderProjectList();
      await this.refreshOverview(true);
    } catch (e) {
      if (seq === this.switchSeq) toast(`切换项目失败：${e}`, "err");
    }
  },

  navigate(page, subTab = null) {
    closeContextMenu();
    // Map legacy routes & preference tabs cleanly
    if (page === "settings" || page === "manage") {
      if (subTab === "config" || subTab === "project") {
        this.openPreferences("project");
        return;
      }
      if (subTab === "env" || subTab === "storage") {
        this.openPreferences("storage");
        return;
      }
      if (subTab === "toolchain") {
        this.openPreferences("toolchain");
        return;
      }
      if (subTab === "credentials") {
        this.openPreferences("credentials");
        return;
      }
      if (subTab === "guardrails") {
        this.openPreferences("guardrails");
        return;
      }
      if (subTab === "updates" || subTab === "about") {
        this.openPreferences("updates");
        return;
      }
      page = "version";
      if (subTab === "version") subTab = "sync";
    } else if (page === "build") {
      page = "compile";
    }

    if (!PAGES[page]) return;
    this.currentPage = page;
    if (subTab) this.subTabs[page] = subTab;
    document.querySelectorAll("#navList .nav-item").forEach((btn) => {
      btn.classList.toggle("active", btn.dataset.page === page);
    });
    this.updateChrome();
    this.renderPage();
  },

  getSubTabDefs(page) {
    const ov = this.overview;
    const proj = this.activeProject;
    if (!proj) return [];
    if (page === "compile") {
      return [
        { id: "presets", label: "框架模板" },
        { id: "prereq", label: "环境自检" },
        { id: "actions", label: "命令与工序", count: proj.actions?.length || 0 },
      ];
    }
    if (page === "package") {
      return [
        { id: "analyze", label: "体积与敏感分析" },
        { id: "pack", label: "分包与分卷工坊" },
      ];
    }
    if (page === "release") {
      const activeCount = (ov?.artifacts || []).filter((a) => !a.isArchived).length;
      return [
        { id: "artifacts", label: "产物分级与清单", count: activeCount },
        { id: "repos", label: "多平台发布" },
        { id: "local", label: "本机更新源" },
      ];
    }
    if (page === "version") {
      return [
        { id: "changelog", label: "变更日志" },
        { id: "sync", label: "版本同步" },
        { id: "git", label: "Git 源码" },
        { id: "journal", label: "构建台账" },
      ];
    }
    return [];
  },

  renderToolbarSubTabs() {
    const host = document.getElementById("tbSubTabs");
    if (!host) return;
    const defs = this.getSubTabDefs(this.currentPage);
    if (!defs.length) {
      host.classList.add("hidden");
      host.innerHTML = "";
      return;
    }
    const activeId = this.subTabs[this.currentPage] || defs[0].id;
    host.classList.remove("hidden");
    host.innerHTML = defs
      .map(
        (d) => `<button class="sf-seg-btn ${d.id === activeId ? "active" : ""}" data-seg="${esc(d.id)}" type="button" role="tab" aria-selected="${d.id === activeId}">
          <span>${esc(d.label)}</span>
          ${typeof d.count === "number" ? `<span class="sf-seg-count">${d.count}</span>` : ""}
        </button>`
      )
      .join("");
    host.querySelectorAll("[data-seg]").forEach((btn) => {
      btn.addEventListener("click", () => {
        if (this.subTabs[this.currentPage] === btn.dataset.seg) return;
        this.navigate(this.currentPage, btn.dataset.seg);
      });
    });
  },

  toggleSidebar(force = null) {
    const shell = document.getElementById("appShell");
    const btn = document.getElementById("btnToggleSidebar");
    if (!shell) return;
    const next = typeof force === "boolean" ? force : !shell.classList.contains("sidebar-collapsed");
    shell.classList.toggle("sidebar-collapsed", next);
    btn?.classList.toggle("active", !next);
    btn?.setAttribute("aria-expanded", String(!next));
    try {
      if (next) localStorage.setItem(SIDEBAR_KEY, "1");
      else localStorage.removeItem(SIDEBAR_KEY);
    } catch {}
  },

  openPreferences(pane = "general") {
    closeContextMenu();
    closeCommandPalette();
    openPreferences(this, pane);
  },

  openCommandPalette() {
    closeContextMenu();
    openCommandPalette(this);
  },

  openOnboardingWizard() {
    openOnboardingWizard(this);
  },

  openManual() {
    openManualDrawer(this.currentPage, this);
  },

  openTerminal() {
    if (this.termMode === "hidden" || this.termMode === "minimized") {
      this.setTermMode(this.termLastVisibleMode || "docked");
    }
  },

  async openDataDir() {
    try {
      const info = await API.getStorageInfo();
      await API.openInExplorer(info.dataRoot);
    } catch (err) {
      toast(`${err}`, "err");
    }
  },

  async removeProjectWithConfirm(project) {
    if (!project) return false;
    const ok = await confirmDialog({
      title: `移出项目：${project.name}`,
      message: "仅从 ShipForge 列表移除，绝不会删除磁盘上的任何文件。",
      confirmText: "移出列表",
      danger: true,
    });
    if (!ok) return false;
    try {
      const wasActive = this.activeProject?.id === project.id;
      this.switchSeq += 1;
      await API.removeProject(project.id);
      if (wasActive) {
        this.overview = null;
        this.clearTransientCrossProjectHints();
      }
      this.projectMetaCache.delete(project.id);
      this.logsByProj.delete(project.id);
      this.pipelineStepsByProj.delete(project.id);
      this.watchBannerByProj.delete(project.id);
      this.activeTasksByProj.delete(project.id);
      this.lastProgressByProj.delete(project.id);
      this.watchInFlightByProj.delete(project.id);
      this.packageState.lastAnalysisByProj.delete(project.id);
      this.packageState.extraExcludesByProj.delete(project.id);
      this.packageState.lastPackResultByProj?.delete(project.id);
      this._ovFormTouchedByProj?.delete(project.id);
      if (this._plNotesState) delete this._plNotesState[project.id];
      saveCachedProjectMeta(this.projectMetaCache);
      toast(`已移出「${project.name}」`, "ok");
      await this.reloadConfig(!wasActive);
      return true;
    } catch (err) {
      toast(`${err}`, "err");
      return false;
    }
  },

  async probeProjectChangesNow() {
    return this.probeProjectChanges(true);
  },

  renderPage() {
    const epoch = ++this.renderEpoch;
    if (typeof this.unmountPage === "function") {
      try {
        this.unmountPage();
      } catch {}
      this.unmountPage = null;
    }
    this.renderToolbarSubTabs();
    const root = document.getElementById("pageViewport");
    if (!root) return;
    if (!this.activeProject) {
      root.innerHTML = `
        <div class="page-wrap">
          <div class="empty-state" style="margin-top:52px">
            <h2 style="margin:0 0 8px">导入本地项目开始构建与发布</h2>
            <p class="muted" style="margin:0 0 20px;max-width:520px;margin-inline:auto">
              支持 Tauri、Electron、Wails、Rust、Node、Python、Go、.NET、Flutter、C/C++ 与 Java；自动检查前置依赖，直连 GitHub / Gitee / GitLab / Gitea 仓库发布。
            </p>
            <div class="field-row" style="justify-content:center;gap:8px">
              <button class="btn btn-accent" id="btnEmptyWizard" type="button">3 步快速配置向导</button>
              <button class="btn btn-primary" id="btnEmptyDiscover" type="button">扫描工作区目录</button>
              <button class="btn" id="btnEmptyAdd" type="button">手动选择项目目录</button>
            </div>
          </div>
        </div>
      `;
      document.getElementById("btnEmptyWizard")?.addEventListener("click", () => this.openOnboardingWizard());
      document.getElementById("btnEmptyDiscover")?.addEventListener("click", () => this.openDiscoverProjectsDialog());
      document.getElementById("btnEmptyAdd")?.addEventListener("click", () => this.importProject());
      return;
    }
    const fn = PAGES[this.currentPage];
    if (fn) {
      try {
        const cleanup = fn(root, this, epoch);
        if (typeof cleanup === "function") this.unmountPage = cleanup;
      } catch (err) {
        console.error("renderPage failed:", err);
        toast(`页面渲染异常：${err?.message || err}`, "err", 4500);
      }
    }
  },

  isProjectBusy(projectId = null) {
    if (!projectId) return this.activeTasksByProj.size > 0;
    return this.activeTasksByProj.has(projectId);
  },

  renderPipelineSteps() {
    const host = document.getElementById("ovPipeSteps");
    if (!host) return;
    const steps = this.pipelineSteps;
    if (!steps.length) {
      host.innerHTML = "";
      return;
    }
    host.innerHTML = steps
      .map(
        (s) => `<span class="pipe-step" data-status="${esc(s.status)}" title="${esc(s.message || "")}">
          <span class="dot ${s.status === "ok" ? "dot-ok" : s.status === "failed" ? "dot-err" : s.status === "running" ? "dot-warn" : ""}"></span>
          <span>${esc(s.name)}</span>
          ${s.message ? `<span class="dim" style="font-size:12px">${esc(s.message)}</span>` : ""}
        </span>`
      )
      .join("");
  },

  openProjectContextMenu(proj, anchorBtn) {
    openProjectRowMenu(proj, anchorBtn, this);
  },

  renderProjectList() {
    const list = document.getElementById("projectList");
    const countEl = document.getElementById("sbProjCount");
    const projects = this.config.projects || [];
    if (countEl) countEl.textContent = String(projects.length);
    if (!list) return;

    const activeId = this.activeProject?.id;
    if (!projects.length) {
      list.innerHTML = `<div class="dim" style="padding:10px 10px;font-size:12px">暂无项目，点击下方导入或扫描</div>`;
      return;
    }

    list.innerHTML = projects
      .map((p) => {
        const isActive = p.id === activeId;
        const cached = this.projectMetaCache.get(p.id);
        const liveVer = isActive && this.overview?.versions?.current ? this.overview.versions.current : cached?.version;
        const verBadge = liveVer && liveVer !== "0.0.0" ? `v${liveVer}` : p.kind.toUpperCase();
        const isRunning = this.isProjectBusy(p.id);
        const dotCls = isRunning ? "running" : cached?.status || "";
        return `
          <div class="sb-proj-row ${isActive ? "active" : ""}" data-pid="${esc(p.id)}" role="listitem">
            <button class="sb-proj-main" data-pick-pid="${esc(p.id)}" type="button" title="${esc(p.root)}">
              <span class="sb-proj-dot ${dotCls}"></span>
              <span class="sb-proj-name">${esc(p.name)}</span>
              <span class="sb-proj-ver">${esc(verBadge)}</span>
            </button>
            <button class="sb-proj-more" data-more-pid="${esc(p.id)}" type="button" aria-label="项目操作菜单" aria-expanded="false" title="项目操作">⋯</button>
          </div>
        `;
      })
      .join("");

    list.querySelectorAll("[data-pick-pid]").forEach((btn) => {
      btn.addEventListener("click", () => this.switchProject(btn.dataset.pickPid));
    });
    list.querySelectorAll("[data-more-pid]").forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const proj = projects.find((x) => x.id === btn.dataset.morePid);
        if (proj) this.openProjectContextMenu(proj, btn);
      });
    });
  },

  updateChrome() {
    const proj = this.activeProject;
    const crumbProj = document.getElementById("tbCrumbProj");
    const crumbPage = document.getElementById("tbCrumbPage");
    const adapterEl = document.getElementById("sbAdapterMeta");

    if (crumbProj) crumbProj.textContent = proj ? proj.name : "ShipForge";
    if (crumbPage) crumbPage.textContent = PAGE_LABELS[this.currentPage] || "工作台";
    if (adapterEl) adapterEl.textContent = proj ? proj.adapterLabel : "多框架构建";

    const srvBadge = document.getElementById("tbServerBadge");
    if (srvBadge) {
      const running = Boolean(this.serverStatus?.running);
      srvBadge.classList.toggle("hidden", !running);
      if (running) {
        const ownerId = this.serverStatus.projectId || "";
        const ownerName = this.serverStatus.projectName || ownerId;
        const ownedByOther = Boolean(ownerId && proj && ownerId !== proj.id);
        srvBadge.textContent = ownedByOther
          ? `${ownerName} :${this.serverStatus.port} (${this.serverStatus.hitCount})`
          : `:${this.serverStatus.port} (${this.serverStatus.hitCount})`;
        srvBadge.title = ownerName
          ? `本机更新源归属项目：${ownerName} (127.0.0.1:${this.serverStatus.port})`
          : `本机更新源 127.0.0.1:${this.serverStatus.port}`;
      }
    }
    this.renderToolbarSubTabs();
  },

  async openDiscoverProjectsDialog(initialDir = null) {
    const dlg = document.createElement("dialog");
    dlg.setAttribute("closedby", "any");
    dlg.style.width = "min(660px, calc(100vw - 32px))";
    dlg.innerHTML = `
      <div class="modal-card">
        <div class="modal-head">
          <h1>扫描工作区发现项目</h1>
          <button class="icon-btn" id="discCloseBtn" type="button" aria-label="关闭">✕</button>
        </div>
        <div class="modal-body scroller">
          <div class="field-row" style="align-items:stretch">
            <input class="input mono" id="discDirInput" style="flex:1;min-width:200px" value="${esc(initialDir || (this.config.scanRoots || [])[0] || "")}" placeholder="选择或输入工作区目录（留空则扫描已保存目录）">
            <button class="btn" id="discPickBtn" type="button">浏览…</button>
            <button class="btn btn-primary" id="discScanBtn" type="button">开始扫描</button>
          </div>
          <label class="sf-switch-label" style="font-size:12px">
            <input class="sf-switch" type="checkbox" switch id="discSaveRoot" checked>
            <span>记住此目录为自动扫描工作区</span>
          </label>
          <div id="discResults" class="muted" style="font-size:12.5px">正在探测项目…</div>
        </div>
        <div class="modal-foot">
          <button class="btn" id="discCancelBtn" type="button">取消</button>
          <button class="btn btn-accent" id="discImportBtn" type="button" disabled>导入选中项目</button>
        </div>
      </div>
    `;
    attachLightDismissFallback(dlg);
    document.body.appendChild(dlg);
    dlg.showModal();

    const close = () => {
      if (dlg.open) dlg.close("");
    };
    dlg.addEventListener("close", () => dlg.remove());
    dlg.querySelector("#discCloseBtn")?.addEventListener("click", close);
    dlg.querySelector("#discCancelBtn")?.addEventListener("click", close);

    let scanSeq = 0;
    const runScan = async () => {
      const seq = ++scanSeq;
      const dirVal = dlg.querySelector("#discDirInput").value.trim() || null;
      const saveRoot = Boolean(dlg.querySelector("#discSaveRoot").checked);
      const box = dlg.querySelector("#discResults");
      const impBtn = dlg.querySelector("#discImportBtn");
      box.innerHTML = `<span class="dim">正在扫描软件工程特征（跳过 node_modules / target / .git）…</span>`;
      impBtn.disabled = true;
      try {
        const items = await API.discoverWorkspaceProjects(dirVal, saveRoot);
        if (seq !== scanSeq || !dlg.isConnected) return;
        if (!items.length) {
          box.innerHTML = `<div class="muted">未发现可识别项目。请点击「浏览…」选择包含代码仓库的上级目录。</div>`;
          return;
        }
        const selectable = items.filter((it) => !it.alreadyImported);
        box.innerHTML = `<div class="row-list">${items
          .map(
            (it) => `<label class="list-row" style="cursor:${it.alreadyImported ? "default" : "pointer"};opacity:${it.alreadyImported ? 0.58 : 1}">
              <div style="display:flex;align-items:center;gap:10px;min-width:0;flex:1">
                <input type="checkbox" name="discProj" value="${esc(it.path)}" ${it.alreadyImported ? "disabled" : "checked"} style="accent-color:var(--accent-btn)">
                <div class="list-row-main">
                  <div class="list-row-title">
                    <span>${esc(it.name)}</span>
                    <span class="tag">${esc(it.kind.toUpperCase())}</span>
                    ${it.version && it.version !== "0.0.0" ? `<span class="tag mono">v${esc(it.version)}</span>` : ""}
                    ${it.alreadyImported ? `<span class="tag tag-ok">已导入</span>` : ""}
                  </div>
                  <div class="list-row-desc mono" style="font-size:12px">${esc(it.path)}${it.githubRepo ? ` · ${esc(it.githubRepo)}` : ""}</div>
                </div>
              </div>
            </label>`
          )
          .join("")}</div>`;
        impBtn.disabled = selectable.length === 0;
        impBtn.textContent = selectable.length ? `导入选中的 ${selectable.length} 个项目` : "全部已导入";
        box.querySelectorAll('input[name="discProj"]').forEach((cb) => {
          cb.addEventListener("change", () => {
            const cnt = box.querySelectorAll('input[name="discProj"]:checked').length;
            impBtn.disabled = cnt === 0;
            impBtn.textContent = cnt ? `导入选中的 ${cnt} 个项目` : "请选择项目";
          });
        });
      } catch (err) {
        if (seq === scanSeq && dlg.isConnected) {
          box.innerHTML = `<span class="err">扫描失败：${esc(err)}</span>`;
        }
      }
    };

    dlg.querySelector("#discPickBtn")?.addEventListener("click", async () => {
      const picked = await API.pickFolder("选择包含多个软件项目的工作区目录");
      if (picked) {
        dlg.querySelector("#discDirInput").value = picked;
        runScan();
      }
    });
    dlg.querySelector("#discScanBtn")?.addEventListener("click", (e) =>
      withBusy(e.currentTarget, "扫描中…", runScan)
    );
    dlg.querySelector("#discImportBtn")?.addEventListener("click", (e) =>
      withBusy(e.currentTarget, "导入中…", async () => {
        const paths = [...dlg.querySelectorAll('input[name="discProj"]:checked')].map((el) => el.value);
        if (!paths.length) return;
        try {
          this.config = await API.importProjectsBatch(paths);
          toast(`已导入 ${paths.length} 个项目`, "ok");
          close();
          await this.reloadConfig(true);
        } catch (err) {
          toast(`导入失败：${err}`, "err");
        }
      })
    );

    runScan();
  },

  async importProject() {
    if (this._importInFlight) return;
    this._importInFlight = true;
    try {
      const folder = await API.pickFolder("选择软件项目根目录");
      if (!folder) return;
      const added = await API.addProjectByPath(folder);
      toast(`已导入 ${added.name}（${added.adapterLabel}）`, "ok");
      await this.reloadConfig(true);
    } catch (e) {
      toast(`导入失败：${e}`, "err");
    } finally {
      this._importInFlight = false;
    }
  },

  // ==========================================================
  // Auto & Manual Change Detection (Strictly Project-Isolated)
  // ==========================================================
  showWatchBanner(text) {
    const pid = this.activeProject?.id;
    if (pid) this.showWatchBannerForProject(pid, text);
  },

  showWatchBannerForProject(pid, text) {
    if (!pid) return;
    this.watchBannerByProj.set(pid, text);
    this.syncWatchBannerForActiveProject();
  },

  clearWatchBannerForProject(pid) {
    if (!pid) return;
    this.watchBannerByProj.delete(pid);
    this.syncWatchBannerForActiveProject();
  },

  syncWatchBannerForActiveProject() {
    const bar = document.getElementById("watchBanner");
    const txt = document.getElementById("watchBannerText");
    if (!bar || !txt) return;
    const pid = this.activeProject?.id;
    const text = pid ? this.watchBannerByProj.get(pid) : null;
    if (text) {
      txt.textContent = text;
      bar.classList.remove("hidden");
    } else {
      txt.textContent = "";
      bar.classList.add("hidden");
    }
  },

  hideWatchBanner() {
    const pid = this.activeProject?.id;
    if (pid) this.watchBannerByProj.delete(pid);
    this.syncWatchBannerForActiveProject();
  },

  async probeProjectChanges(manual = false) {
    const proj = this.activeProject;
    if (!proj) {
      if (manual) toast("请先导入本地项目后再执行变动扫描", "warn", 2000);
      return;
    }
    const pid = proj.id;
    const projName = proj.name || pid;
    if (this.isProjectBusy(pid) || this.watchInFlightByProj.has(pid)) return;
    // Never compare auto-watch diff against a stale or different project's overview
    if (!manual && this.overview?.project?.id !== pid) return;

    const seq = this.switchSeq;
    const ovSeq = this.overviewReqSeq;
    const curOv = this.overview?.project?.id === pid ? this.overview : null;
    const prevDirty = curOv?.changeReport?.dirtyCount ?? curOv?.git?.dirtyCount ?? 0;
    const prevSha = curOv?.changeReport?.headSha || curOv?.git?.headSha || "";
    const prevVer = curOv?.changeReport?.currentVersion || curOv?.versions?.current || "";
    const prevSourceFp = curOv?.changeReport?.sourceFingerprint || "";
    const prevArtifactFp = curOv?.changeReport?.artifactFingerprint || "";
    const prevDeployStatus = curOv?.changeReport?.deployReadiness?.status || "";
    const prevArtifactUnix = curOv?.changeReport?.deployReadiness?.newestArtifactUnix || 0;

    this.watchInFlightByProj.add(pid);
    try {
      const rep = await API.detectProjectChanges(pid, manual);
      // Critical cross-project & concurrent-refresh race barrier:
      // discard result if user switched project OR explicitly refreshed/synced overview mid-scan!
      if (seq !== this.switchSeq || this.activeProject?.id !== pid) return;
      if (!manual && ovSeq !== this.overviewReqSeq) return;

      const isFsMode = rep.scanMode === "filesystem";
      const dr = rep.deployReadiness || null;
      const deployStatusChanged = Boolean(
        dr?.status &&
          (prevDeployStatus
            ? dr.status !== prevDeployStatus
            : dr.status === "ready_to_deploy")
      );
      const deployArtifactChanged = Boolean(
        prevArtifactUnix > 0 && dr?.newestArtifactUnix && dr.newestArtifactUnix !== prevArtifactUnix
      );
      const uiOutOfSync =
        rep.dirtyCount !== prevDirty ||
        Boolean(rep.headSha && rep.headSha !== prevSha) ||
        Boolean(rep.currentVersion && rep.currentVersion !== prevVer) ||
        Boolean(prevSourceFp && rep.sourceFingerprint && rep.sourceFingerprint !== prevSourceFp) ||
        Boolean(prevArtifactFp && rep.artifactFingerprint && rep.artifactFingerprint !== prevArtifactFp) ||
        deployStatusChanged ||
        deployArtifactChanged;

      const summaryTag = rep.changeSummary ? `（${rep.changeSummary}）` : "";
      const sample = (rep.dirtyFiles || []).slice(0, 2).join("、");

      if (manual) {
        if (rep.versionChanged || rep.hasExternalChange || uiOutOfSync) {
          await this.refreshOverview(true);
          if (seq !== this.switchSeq || this.activeProject?.id !== pid) return;
          const ahead = this.overview?.git?.aheadCount || 0;
          const drNow = this.overview?.changeReport?.deployReadiness || dr;
          if (drNow?.status === "ready_to_deploy") {
            toast(
              `[${projName}] ${drNow.headline}（${drNow.detail}）`,
              "ok",
              4200
            );
          } else if (!isFsMode && rep.dirtyCount === 0 && prevDirty > 0) {
            toast(
              ahead > 0
                ? `[${projName}] 已同步：工作区已提交干净（↑${ahead} 个提交待推送远端）`
                : `[${projName}] 已同步：v${rep.currentVersion} 工作区已提交干净`,
              "ok",
              2600
            );
          } else if (rep.dirtyCount > 0) {
            toast(
              isFsMode
                ? `[${projName}] 已同步：v${rep.currentVersion} 探测到 ${rep.dirtyCount} 项文件变动${summaryTag}${sample ? `：${sample}` : ""}`
                : `[${projName}] 状态已同步：v${rep.currentVersion}（${rep.dirtyCount} 个未提交${summaryTag}${sample ? `：${sample}` : ""}）`,
              "warn",
              3400
            );
          } else if (rep.artifactChanged) {
            toast(`[${projName}] 已同步：v${rep.currentVersion} 检测到构建产物变动`, "ok", 2600);
          } else {
            toast(
              ahead > 0
                ? `[${projName}] 已同步：v${rep.currentVersion} 工作区干净（↑${ahead} 个提交待推送）`
                : `[${projName}] 已同步：v${rep.currentVersion} 工作区干净`,
              "ok",
              2000
            );
          }
        } else if (dr?.status === "ready_to_deploy") {
          toast(`[${projName}] ${dr.headline}（${dr.detail}）`, "ok", 4200);
        } else if (rep.consecutiveWarning) {
          toast(`[${projName}] ${rep.consecutiveWarning}`, "warn", 4200);
          await this.refreshOverview(true);
        } else if (rep.dirtyCount > 0) {
          toast(
            isFsMode
              ? `[${projName}] v${rep.currentVersion} 扫描完成：无新增变化`
              : `[${projName}] v${rep.currentVersion} 无新增变化（Git 仍有 ${rep.dirtyCount} 个未提交${summaryTag}${sample ? `：${sample}` : ""}）`,
            "warn",
            3600
          );
        } else {
          const ahead = this.overview?.git?.aheadCount || 0;
          if (ahead > 0) {
            toast(`[${projName}] v${rep.currentVersion} 工作区已全部提交（本地有 ${ahead} 个提交待推送远端）`, "ok", 3000);
          } else {
            toast(`[${projName}] v${rep.currentVersion} 工作区干净，无新增改动`, "ok", 1800);
          }
        }
      } else if (rep.versionChanged || rep.hasExternalChange || uiOutOfSync) {
        const activeEl = document.activeElement;
        const isEditingForm = Boolean(
          activeEl && /^(INPUT|TEXTAREA|SELECT)$/.test(activeEl.tagName) && !activeEl.readOnly
        );
        const hasOpenDialog = Boolean(document.querySelector("dialog[open]"));
        const hasExpandedDetails = Boolean(document.querySelector("#pageViewport details[open]"));
        const hasUnsavedOverviewEdits =
          this.currentPage === "overview" &&
          (Boolean(this._plNotesState?.[pid]?.userTouched) || Boolean(this._ovFormTouchedByProj?.has(pid)));
        const onDraftSensitivePage =
          this.currentPage === "release" ||
          this.currentPage === "version" ||
          this.currentPage === "package";
        const canAutoRefreshSilently =
          !isEditingForm &&
          !hasOpenDialog &&
          !hasExpandedDetails &&
          !hasUnsavedOverviewEdits &&
          !onDraftSensitivePage;

        if (canAutoRefreshSilently) {
          // Reconcile baseline & UI automatically so banner count and KPI card NEVER contradict each other
          await API.detectProjectChanges(pid, true).catch(() => {});
          if (seq !== this.switchSeq || this.activeProject?.id !== pid) return;
          await this.refreshOverview(true);
          if (dr?.status === "ready_to_deploy" && (prevDeployStatus !== "ready_to_deploy" || rep.versionChanged || rep.artifactChanged)) {
            toast(`[${projName}] ${dr.headline}，可直接一键部署推送`, "ok", 4200);
          } else if (rep.versionChanged) {
            toast(`[${projName}] 自动探测到新版本 v${rep.currentVersion}，已自动隔离版本工作区`, "ok", 3200);
          }
        } else {
          const msg = rep.versionChanged
            ? `[${projName}] 自动探测到新版本 v${rep.currentVersion}（已自动隔离该版本工作区）`
            : !isFsMode && rep.dirtyCount === 0 && rep.headSha && rep.headSha !== prevSha
              ? `[${projName}] 自动探测到新 Git 提交 (${rep.headSha})，工作区已干净`
              : rep.dirtyCount > 0
                ? `[${projName}] 自动探测到${isFsMode ? "本地文件" : "源码或产物"}变动（v${rep.currentVersion} · ${rep.dirtyCount} 项${summaryTag}）`
                : `[${projName}] 自动探测到产物变动（v${rep.currentVersion}）`;
          this.showWatchBannerForProject(pid, msg);
        }
      }
    } catch {
      if (manual && seq === this.switchSeq && this.activeProject?.id === pid) {
        toast(`[${projName}] 探测改动失败`, "err");
      }
    } finally {
      this.watchInFlightByProj.delete(pid);
    }
  },

  // ==========================================================
  // Toolbar Dynamic Progress Capsule + 2px Top Hairline Progress
  // ==========================================================
  showProgressBar({ stage, detail = "", percent = 0, indeterminate = false, state = "running", cancellable = false }) {
    clearTimeout(this.progressDismissTimer);
    const wrap = document.getElementById("execProgressWrap");
    const bar = document.getElementById("execProgressBar");
    const hairline = document.getElementById("execHairline");
    const fill = document.getElementById("execHairlineFill");
    if (!wrap || !bar) return;

    const pct = Math.max(0, Math.min(100, Number(percent) || 0));
    wrap.dataset.state = state;
    wrap.classList.add("visible");
    wrap.setAttribute("aria-hidden", "false");

    document.getElementById("execStageLabel").textContent = stage || "执行中…";
    document.getElementById("execDetailText").textContent = detail || "";
    document.getElementById("execPctText").textContent = indeterminate ? "…" : `${Math.round(pct)}%`;
    document.getElementById("btnExecCancel")?.classList.toggle("hidden", !cancellable);
    document.getElementById("btnExecDismiss")?.classList.toggle("hidden", state === "running");

    if (indeterminate) bar.removeAttribute("value");
    else bar.value = pct;

    if (hairline && fill) {
      hairline.dataset.state = state;
      hairline.dataset.indeterminate = indeterminate ? "1" : "0";
      hairline.classList.add("visible");
      fill.style.width = indeterminate ? "36%" : `${pct}%`;
    }
  },

  hideProgressBar() {
    clearTimeout(this.progressDismissTimer);
    const wrap = document.getElementById("execProgressWrap");
    const hairline = document.getElementById("execHairline");
    if (wrap) {
      wrap.classList.remove("visible");
      wrap.setAttribute("aria-hidden", "true");
    }
    if (hairline) {
      hairline.classList.remove("visible");
      hairline.dataset.indeterminate = "0";
    }
  },

  // ==========================================================
  // Adaptive 5-Mode Terminal Controller
  // ==========================================================
  setTermMode(mode) {
    const shell = document.getElementById("termShell");
    const mini = document.getElementById("termMiniPill");
    const trigger = document.getElementById("btnToggleTerm");
    if (!shell) return;

    if (mode === "docked" || mode === "floating" || mode === "fullscreen") {
      this.termLastVisibleMode = mode;
    }
    this.termMode = mode;
    shell.dataset.mode = mode;

    mini?.classList.toggle("hidden", mode !== "minimized");
    trigger?.classList.toggle("active", mode !== "hidden" && mode !== "minimized");
    document.getElementById("btnTermFloat")?.classList.toggle("active", mode === "floating");
    document.getElementById("btnTermFull")?.classList.toggle("active", mode === "fullscreen");

    if (mode === "floating") this.clampFloatingTerm();
    if (mode === "docked" || mode === "floating" || mode === "fullscreen") {
      const body = document.getElementById("termBody");
      if (body) body.scrollTop = body.scrollHeight;
    }
  },

  clampFloatingTerm() {
    const shell = document.getElementById("termShell");
    if (!shell || this.termMode !== "floating") return;
    const rect = shell.getBoundingClientRect();
    const w = Math.max(420, Math.min(window.innerWidth - 24, rect.width || 720));
    const h = Math.max(180, Math.min(window.innerHeight - 64, rect.height || 360));
    const x = Math.max(8, Math.min(window.innerWidth - w - 8, rect.left || 240));
    const y = Math.max(48, Math.min(window.innerHeight - 54, rect.top || 120));
    shell.style.setProperty("--term-w", `${Math.round(w)}px`);
    shell.style.setProperty("--term-fh", `${Math.round(h)}px`);
    shell.style.setProperty("--term-x", `${Math.round(x)}px`);
    shell.style.setProperty("--term-y", `${Math.round(y)}px`);
  },

  toggleTerminal() {
    if (this.termMode === "hidden" || this.termMode === "minimized") {
      this.setTermMode(this.termLastVisibleMode || "docked");
    } else {
      this.setTermMode("hidden");
    }
  },

  toggleTermTone() {
    this.termTone = this.termTone === "auto" ? "dark" : "auto";
    const shell = document.getElementById("termShell");
    if (shell) shell.dataset.tone = this.termTone;
    try {
      localStorage.setItem("sf-term-tone", this.termTone);
    } catch {}
    toast(this.termTone === "dark" ? "终端已切换为深色高对比" : "终端已恢复跟随系统主题", "ok", 1400);
  },

  setTaskRunning(taskId, label, isPipeline = false, projectId = null) {
    if (taskId && this.finishedTaskIds.has(taskId)) return;
    const pid = projectId || this.activeProject?.id || "__system__";
    if (isPipeline) {
      // Preserve any early step events that already arrived for this exact pipelineId before invoke returned
      const existingSteps = this.pipelineStepsByProj.get(pid) || [];
      this.pipelineStepsByProj.set(
        pid,
        existingSteps.filter((s) => s.pipelineId === taskId)
      );
    }
    this.lastProgressByProj.set(pid, {
      stage: label,
      detail: "正在启动任务…",
      percent: 4,
      indeterminate: false,
    });
    const existingLogs = this.logsByProj.get(pid);
    if (existingLogs && existingLogs.length > 0) {
      const now = new Date();
      const ts = [now.getHours(), now.getMinutes(), now.getSeconds()]
        .map((n) => String(n).padStart(2, "0"))
        .join(":");
      this.appendLog({
        taskId,
        projectId: pid,
        stream: "system",
        ts,
        line: `──── 新任务开始：${label} ────`,
      });
    }
    this.activeTasksByProj.set(pid, {
      taskId,
      projectId: pid,
      label,
      isPipeline,
      startedAt: performance.now(),
    });
    this.renderProjectList();
    window.dispatchEvent(new CustomEvent("sf-task-state-changed"));

    if (pid === this.activeProjectIdKey || pid === "__system__") {
      document.getElementById("tbTermDot")?.classList.remove("hidden");
      document.getElementById("btnCancelTask")?.classList.remove("hidden");
      this.updateTermDots("running");
      this.showProgressBar({
        stage: label,
        detail: "正在启动任务…",
        percent: 4,
        indeterminate: false,
        state: "running",
        cancellable: true,
      });
    }

    clearInterval(this.taskTimer);
    const tick = () => {
      const curTask =
        this.activeTasksByProj.get(this.activeProjectIdKey) ||
        this.activeTasksByProj.get("__system__") ||
        null;
      if (!curTask) return;
      const dur = formatDuration(performance.now() - curTask.startedAt);
      const elapsed = document.getElementById("execElapsedText");
      const miniMeta = document.getElementById("termMiniMeta");
      if (elapsed) elapsed.textContent = dur;
      this.setTermMeta(curTask.label, dur);
      if (miniMeta) miniMeta.textContent = dur;
    };
    tick();
    this.taskTimer = setInterval(tick, 250);
  },

  setTermMeta(labelText, durText = "") {
    const meta = document.getElementById("termMeta");
    if (!meta) return;
    let lblEl = meta.querySelector(".term-meta-label");
    let durEl = meta.querySelector(".term-meta-dur");
    if (!lblEl || !durEl) {
      meta.textContent = "";
      lblEl = document.createElement("span");
      lblEl.className = "term-meta-label";
      durEl = document.createElement("span");
      durEl.className = "term-meta-dur";
      meta.append(lblEl, durEl);
    }
    lblEl.textContent = labelText || "";
    durEl.textContent = durText ? `· ${durText}` : "";
    durEl.hidden = !durText;
  },

  updateTermDots(cls) {
    for (const id of ["termDot", "termMiniDot"]) {
      const d = document.getElementById(id);
      if (d) d.className = `term-dot ${cls}`;
    }
  },

  completeRunChrome({ taskId, projectId = null, label, success, cancelled, durationMs, detail = "" }) {
    if (taskId) {
      this.finishedTaskIds.add(taskId);
      if (this.finishedTaskIds.size > 64) {
        const first = this.finishedTaskIds.values().next().value;
        if (first) this.finishedTaskIds.delete(first);
      }
    }
    const pid = projectId || this.findProjectIdByTaskId(taskId) || this.activeProjectIdKey;
    const tracked = this.activeTasksByProj.get(pid);
    if (tracked && taskId && tracked.taskId !== taskId) return;
    this.activeTasksByProj.delete(pid);
    this.lastProgressByProj.delete(pid);
    if (this.activeTasksByProj.size === 0) {
      clearInterval(this.taskTimer);
      this.taskTimer = null;
    }
    this.renderProjectList();
    window.dispatchEvent(new CustomEvent("sf-task-state-changed"));

    const isForActive = pid === this.activeProjectIdKey || pid === "__system__";
    const dur = formatDuration(durationMs || 0);
    const targetProj = (this.config.projects || []).find((p) => p.id === pid);
    const projPrefix = targetProj && !isForActive ? `[${targetProj.name}] ` : "";

    if (isForActive) {
      document.getElementById("btnCancelTask")?.classList.add("hidden");
      const miniMeta = document.getElementById("termMiniMeta");
      const elapsed = document.getElementById("execElapsedText");
      if (elapsed) elapsed.textContent = dur;

      if (cancelled) {
        this.updateTermDots("");
        this.setTermMeta(`${label} · 已取消`, dur);
        if (miniMeta) miniMeta.textContent = "已取消";
        this.showProgressBar({ stage: `${label} · 已取消`, detail, percent: 0, state: "err", cancellable: false });
        toast(`已取消：${label}`, "warn");
        this.progressDismissTimer = setTimeout(() => this.hideProgressBar(), 2600);
      } else if (success) {
        this.updateTermDots("ok");
        this.setTermMeta(`${label} · 完成`, dur);
        if (miniMeta) miniMeta.textContent = dur;
        this.showProgressBar({ stage: `${label} · 完成`, detail: detail || `耗时 ${dur}`, percent: 100, state: "ok", cancellable: false });
        toast(detail || `完成：${label}（${dur}）`, "ok");
        this.progressDismissTimer = setTimeout(() => this.hideProgressBar(), 3200);
      } else {
        this.updateTermDots("err");
        this.setTermMeta(`${label} · 失败`, dur);
        if (miniMeta) miniMeta.textContent = "失败";
        this.showProgressBar({ stage: `${label} · 失败`, detail, percent: 100, state: "err", cancellable: false });
        if (this.termMode === "hidden") this.setTermMode("docked");
        toast(detail || `失败：${label}`, "err", 4800);
      }
      if (pid === this.activeProject?.id) {
        this.refreshOverview(true);
      }
    } else {
      // Background project finished while user is viewing another project: notify cleanly without clobbering active UI
      if (cancelled) {
        toast(`${projPrefix}已取消：${label}`, "warn");
      } else if (success) {
        toast(`${projPrefix}${detail || `完成：${label}（${dur}）`}`, "ok", 3600);
      } else {
        toast(`${projPrefix}${detail || `失败：${label}`}`, "err", 5200);
      }
      if (targetProj) {
        API.getProjectOverview(targetProj.id)
          .then((ov) => {
            const effDirty = ov.changeReport?.dirtyCount ?? ov.git?.dirtyCount ?? 0;
            const status = !ov.rootExists
              ? "err"
              : ov.healthIssues?.length || !ov.versions?.synced || effDirty > 0
                ? "warn"
                : "ok";
            this.projectMetaCache.set(targetProj.id, { version: ov.versions?.current || "", status });
            saveCachedProjectMeta(this.projectMetaCache);
            this.renderProjectList();
          })
          .catch(() => {});
      }
    }
  },

  appendLog(entry) {
    const pid = entry.projectId || this.findProjectIdByTaskId(entry.taskId) || "__system__";
    let bucket = this.logsByProj.get(pid);
    if (!bucket) {
      bucket = [];
      this.logsByProj.set(pid, bucket);
    }
    this._logSeq = (this._logSeq || 0) + 1;
    bucket.push({ ...entry, _seq: this._logSeq });
    if (bucket.length > 1500) bucket.splice(0, bucket.length - 1500);
    if (pid !== this.activeProjectIdKey && pid !== "__system__") return;
    document.getElementById("tbTermDot")?.classList.remove("hidden");
    const filter = (document.getElementById("termFilter")?.value || "").trim().toLowerCase();
    if (filter && !entry.line.toLowerCase().includes(filter)) return;
    this.appendLogDom(entry);
  },

  appendLogDom(entry) {
    const body = document.getElementById("termBody");
    if (!body) return;
    const atBottom = body.scrollHeight - body.scrollTop - body.clientHeight < 44;
    const row = document.createElement("div");
    const lv = classifyLogLevel(entry.line, entry.stream);
    row.className = `log-line ${entry.stream || "stdout"} ${lv}`.trim();
    const ts = document.createElement("span");
    ts.className = "log-ts";
    ts.textContent = entry.ts || "";
    const txt = document.createElement("span");
    txt.className = "log-txt";
    txt.textContent = entry.line || "";
    row.append(ts, txt);
    body.appendChild(row);
    if (body.childElementCount > 1500) body.firstElementChild?.remove();
    if (atBottom) body.scrollTop = body.scrollHeight;
  },

  rerenderLogs() {
    const body = document.getElementById("termBody");
    if (!body) return;
    const filter = (document.getElementById("termFilter")?.value || "").trim().toLowerCase();
    const frag = document.createDocumentFragment();
    for (const entry of this.logs) {
      if (filter && !entry.line.toLowerCase().includes(filter)) continue;
      const row = document.createElement("div");
      const lv = classifyLogLevel(entry.line, entry.stream);
      row.className = `log-line ${entry.stream || "stdout"} ${lv}`.trim();
      const ts = document.createElement("span");
      ts.className = "log-ts";
      ts.textContent = entry.ts || "";
      const txt = document.createElement("span");
      txt.className = "log-txt";
      txt.textContent = entry.line || "";
      row.append(ts, txt);
      frag.appendChild(row);
    }
    body.innerHTML = "";
    body.appendChild(frag);
    body.scrollTop = body.scrollHeight;
  },

  async runAction(action, extraOpts = {}) {
    if (!this.activeProject) return;
    const pid = this.activeProject.id;
    if (this.isProjectBusy(pid)) {
      toast("当前项目正在执行任务，请等待完成或先点击「停止」", "warn", 3200);
      return;
    }
    try {
      const taskId = await API.runProjectAction({
        projectId: pid,
        actionId: action.id,
        bump: extraOpts.bump || null,
        explicitVersion: extraOpts.explicitVersion || null,
        channel: extraOpts.channel || null,
        skipTests: Boolean(extraOpts.skipTests),
      });
      this.setTaskRunning(taskId, action.label, false, pid);
    } catch (e) {
      toast(`${e}`, "err", 4200);
    }
  },
};

function setupTerminalGestures() {
  const shell = document.getElementById("termShell");
  const head = document.getElementById("termHead");
  const topResizer = document.getElementById("termResizeTop");
  const cornerResizer = document.getElementById("termResizeCorner");
  if (!shell || !head) return;

  // 1. Docked top-edge height drag + double-click snap
  let dockDragging = false;
  let dockStartY = 0;
  let dockStartH = 240;
  topResizer?.addEventListener("pointerdown", (e) => {
    dockDragging = true;
    dockStartY = e.clientY;
    dockStartH = shell.getBoundingClientRect().height;
    topResizer.setPointerCapture(e.pointerId);
    e.preventDefault();
  });
  topResizer?.addEventListener("pointermove", (e) => {
    if (!dockDragging) return;
    const h = Math.max(130, Math.min(window.innerHeight * 0.72, dockStartH + (dockStartY - e.clientY)));
    shell.style.setProperty("--term-h", `${Math.round(h)}px`);
  });
  const endDockDrag = () => { dockDragging = false; };
  topResizer?.addEventListener("pointerup", endDockDrag);
  topResizer?.addEventListener("pointercancel", endDockDrag);
  topResizer?.addEventListener("dblclick", () => {
    const cur = shell.getBoundingClientRect().height;
    const half = Math.round(window.innerHeight * 0.48);
    shell.style.setProperty("--term-h", cur > half - 30 ? "240px" : `${half}px`);
  });

  // 2. Floating free-drag & docked-upward auto-detach
  let floatDragging = false;
  let dragOffsetX = 0;
  let dragOffsetY = 0;
  let detachStartY = 0;
  let maybeDetach = false;

  head.addEventListener("pointerdown", (e) => {
    if (e.target.closest("button, input")) return;
    if (App.termMode === "floating") {
      floatDragging = true;
      const rect = shell.getBoundingClientRect();
      dragOffsetX = e.clientX - rect.left;
      dragOffsetY = e.clientY - rect.top;
      shell.classList.add("dragging");
      head.setPointerCapture(e.pointerId);
      e.preventDefault();
    } else if (App.termMode === "docked") {
      maybeDetach = true;
      detachStartY = e.clientY;
      head.setPointerCapture(e.pointerId);
    }
  });

  head.addEventListener("pointermove", (e) => {
    if (maybeDetach && detachStartY - e.clientY > 24) {
      maybeDetach = false;
      const w = Math.min(760, window.innerWidth - 60);
      const h = Math.min(380, window.innerHeight - 120);
      const x = Math.max(20, Math.min(window.innerWidth - w - 20, e.clientX - w / 2));
      const y = Math.max(48, Math.min(window.innerHeight - h - 20, e.clientY - 16));
      shell.style.setProperty("--term-w", `${Math.round(w)}px`);
      shell.style.setProperty("--term-fh", `${Math.round(h)}px`);
      shell.style.setProperty("--term-x", `${Math.round(x)}px`);
      shell.style.setProperty("--term-y", `${Math.round(y)}px`);
      App.setTermMode("floating");
      floatDragging = true;
      dragOffsetX = e.clientX - x;
      dragOffsetY = e.clientY - y;
      shell.classList.add("dragging");
    }
    if (!floatDragging) return;
    const rect = shell.getBoundingClientRect();
    const x = Math.max(8, Math.min(window.innerWidth - rect.width - 8, e.clientX - dragOffsetX));
    const y = Math.max(48, Math.min(window.innerHeight - 44, e.clientY - dragOffsetY));
    shell.style.setProperty("--term-x", `${Math.round(x)}px`);
    shell.style.setProperty("--term-y", `${Math.round(y)}px`);
  });

  const endDrag = () => {
    floatDragging = false;
    maybeDetach = false;
    shell.classList.remove("dragging");
  };
  head.addEventListener("pointerup", endDrag);
  head.addEventListener("pointercancel", endDrag);
  window.addEventListener("resize", () => App.clampFloatingTerm());

  head.addEventListener("dblclick", (e) => {
    if (e.target.closest("button, input")) return;
    App.setTermMode(App.termMode === "fullscreen" ? "docked" : "fullscreen");
  });

  // 3. Floating corner free-resize
  let resizing = false;
  let rStartX = 0;
  let rStartY = 0;
  let rStartW = 720;
  let rStartH = 360;
  cornerResizer?.addEventListener("pointerdown", (e) => {
    resizing = true;
    const rect = shell.getBoundingClientRect();
    rStartX = e.clientX;
    rStartY = e.clientY;
    rStartW = rect.width;
    rStartH = rect.height;
    shell.classList.add("resizing");
    cornerResizer.setPointerCapture(e.pointerId);
    e.preventDefault();
  });
  cornerResizer?.addEventListener("pointermove", (e) => {
    if (!resizing) return;
    const w = Math.max(420, Math.min(window.innerWidth - 24, rStartW + (e.clientX - rStartX)));
    const h = Math.max(180, Math.min(window.innerHeight - 64, rStartH + (e.clientY - rStartY)));
    shell.style.setProperty("--term-w", `${Math.round(w)}px`);
    shell.style.setProperty("--term-fh", `${Math.round(h)}px`);
  });
  const endCornerResize = () => {
    resizing = false;
    shell.classList.remove("resizing");
  };
  cornerResizer?.addEventListener("pointerup", endCornerResize);
  cornerResizer?.addEventListener("pointercancel", endCornerResize);
}

async function init() {
  hydrateIcons(document);

  // Restore sidebar state & terminal tone
  try {
    if (localStorage.getItem(SIDEBAR_KEY) === "1") {
      App.toggleSidebar(true);
    } else {
      document.getElementById("btnToggleSidebar")?.classList.add("active");
    }
    const savedTone = localStorage.getItem("sf-term-tone");
    if (savedTone === "dark" || savedTone === "auto") {
      App.termTone = savedTone;
      const shell = document.getElementById("termShell");
      if (shell) shell.dataset.tone = savedTone;
    }
  } catch {}

  const win = getCurrentWindow();
  document.getElementById("btnWinMin")?.addEventListener("click", () => win?.minimize());
  document.getElementById("btnWinMax")?.addEventListener("click", () => win?.toggleMaximize());
  document.getElementById("btnWinClose")?.addEventListener("click", () => win?.close());
  document.getElementById("titlebar")?.addEventListener("dblclick", (e) => {
    if (e.target.closest("button, input")) return;
    win?.toggleMaximize();
  });

  document.querySelectorAll("[data-resize]").forEach((el) => {
    el.addEventListener("mousedown", (e) => {
      if (e.button !== 0) return;
      win?.startResizeDragging?.(el.dataset.resize);
    });
  });

  initBuiltInContextMenu(App);
  setupTerminalGestures();

  // Toolbar & sidebar chrome controls
  document.getElementById("btnToggleSidebar")?.addEventListener("click", () => App.toggleSidebar());
  document.getElementById("tbServerBadge")?.addEventListener("click", () => App.navigate("release", "local"));
  document.getElementById("btnOpenPalette")?.addEventListener("click", () => App.openCommandPalette());
  document.getElementById("btnDetectChanges")?.addEventListener("click", () => App.probeProjectChanges(true));
  document.getElementById("btnToggleTerm")?.addEventListener("click", () => App.toggleTerminal());
  document.getElementById("btnOpenManual")?.addEventListener("click", () => App.openManual());
  document.getElementById("btnOpenPrefs")?.addEventListener("click", () => App.openPreferences("general"));
  document.getElementById("btnDiscoverProjects")?.addEventListener("click", () => App.openDiscoverProjectsDialog());
  document.getElementById("btnAddProject")?.addEventListener("click", () => App.importProject());

  document.getElementById("btnExecViewTerm")?.addEventListener("click", () => App.openTerminal());
  document.getElementById("btnExecDismiss")?.addEventListener("click", () => App.hideProgressBar());

  const cancelRunning = async () => {
    if (!App.activeTaskId) return;
    try {
      await API.cancelTask(App.activeTaskId);
      toast("已发送停止指令", "warn");
    } catch (e) {
      toast(`停止失败：${e}`, "err");
    }
  };
  document.getElementById("btnExecCancel")?.addEventListener("click", cancelRunning);
  document.getElementById("btnCancelTask")?.addEventListener("click", cancelRunning);

  // Watch banner
  document.getElementById("btnWatchSync")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "同步中…", async () => {
      const targetPid = App.activeProject?.id;
      const seq = App.switchSeq;
      App.hideWatchBanner();
      if (targetPid) {
        await API.detectProjectChanges(targetPid, true).catch(() => {});
        if (seq !== App.switchSeq || App.activeProject?.id !== targetPid) return;
      }
      await App.refreshOverview(false);
    })
  );
  document.getElementById("btnWatchDismiss")?.addEventListener("click", async () => {
    const targetPid = App.activeProject?.id;
    const seq = App.switchSeq;
    App.hideWatchBanner();
    if (targetPid) {
      const rep = await API.detectProjectChanges(targetPid, true).catch(() => null);
      if (seq !== App.switchSeq || App.activeProject?.id !== targetPid) return;
      // Sync in-memory overview baseline without re-rendering DOM or interrupting unsaved forms,
      // so the next 8s auto-watch tick will NOT re-pop the banner unless a NEW disk change occurs.
      if (rep && App.overview?.project?.id === targetPid) {
        App.overview.changeReport = rep;
        if (App.overview.git && rep.scanMode === "git") {
          App.overview.git.dirtyCount = rep.dirtyCount;
          if (rep.headSha) App.overview.git.headSha = rep.headSha;
        }
      }
    }
  });

  // Terminal controls
  document.getElementById("btnTermTone")?.addEventListener("click", () => App.toggleTermTone());
  document.getElementById("btnTermFloat")?.addEventListener("click", () => {
    App.setTermMode(App.termMode === "floating" ? "docked" : "floating");
  });
  document.getElementById("btnTermMin")?.addEventListener("click", () => App.setTermMode("minimized"));
  document.getElementById("btnTermFull")?.addEventListener("click", () => {
    App.setTermMode(App.termMode === "fullscreen" ? "docked" : "fullscreen");
  });
  document.getElementById("btnTermHide")?.addEventListener("click", () => App.setTermMode("hidden"));
  document.getElementById("termMiniPill")?.addEventListener("click", () => {
    App.setTermMode(App.termLastVisibleMode || "docked");
  });

  document.getElementById("termFilter")?.addEventListener("input", () => App.rerenderLogs());
  document.getElementById("btnClearLog")?.addEventListener("click", () => {
    App.logsByProj.set(App.activeProjectIdKey, []);
    App.logsByProj.set("__system__", []);
    document.getElementById("termBody").innerHTML = "";
    if (!App.activeTaskId) document.getElementById("tbTermDot")?.classList.add("hidden");
  });
  document.getElementById("btnCopyLog")?.addEventListener("click", () => {
    const text = App.logs.map((l) => `[${l.ts}] ${l.line}`).join("\n");
    copyText(text || "(无日志)", "已复制终端日志");
  });

  document.querySelectorAll("#navList .nav-item").forEach((btn) => {
    btn.addEventListener("click", () => App.navigate(btn.dataset.page));
  });

  // Global macOS-style shortcuts
  window.addEventListener("keydown", (e) => {
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === "k") {
      e.preventDefault();
      App.openCommandPalette();
    } else if (mod && e.key === ",") {
      e.preventDefault();
      App.openPreferences("general");
    } else if (mod && e.key.toLowerCase() === "b") {
      e.preventDefault();
      App.toggleSidebar();
    } else if (mod && e.key === "`") {
      e.preventDefault();
      App.toggleTerminal();
    } else if (e.key === "F1") {
      e.preventDefault();
      App.openManual();
    } else if (e.key === "F5") {
      e.preventDefault();
      App.probeProjectChanges(true);
    } else if (e.key === "Escape") {
      closeContextMenu();
      closeManualDrawer();
      if (App.termMode === "fullscreen" && !document.querySelector("dialog[open]")) {
        e.preventDefault();
        App.setTermMode("docked");
      }
    }
  });

  // Backend event subscriptions (strictly routed by projectId / taskId)
  await listen("task-log", (ev) => App.appendLog(ev.payload));
  await listen("task-progress", (ev) => {
    const p = ev.payload;
    const pid = p.projectId || App.findProjectIdByTaskId(p.taskId) || "__system__";
    const progObj = {
      stage: p.stage || p.label,
      detail: p.detail || "",
      percent: p.percent,
      indeterminate: p.indeterminate,
    };
    if (App.activeTasksByProj.get(pid)) {
      App.lastProgressByProj.set(pid, progObj);
    }
    if (pid !== App.activeProjectIdKey && pid !== "__system__") return;
    const isActive = Boolean(App.activeTasksByProj.get(pid));
    if (!isActive) {
      const elapsed = document.getElementById("execElapsedText");
      if (elapsed) elapsed.textContent = "";
    }
    const done = !isActive && p.percent >= 100;
    const isErr = (p.stage || "").includes("失败") || (p.stage || "").includes("中止");
    App.showProgressBar({
      stage: p.stage || p.label,
      detail: p.detail || "",
      percent: p.percent,
      indeterminate: p.indeterminate,
      state: isErr ? "err" : done ? "ok" : "running",
      cancellable: isActive,
    });
    if (done && !isErr) {
      App.progressDismissTimer = setTimeout(() => App.hideProgressBar(), 2600);
    }
  });
  await listen("task-finished", (ev) => {
    const d = ev.payload;
    if (d.isPipelineSubstep) return;
    App.completeRunChrome({
      taskId: d.taskId,
      projectId: d.projectId || null,
      label: d.label,
      success: d.success,
      cancelled: d.cancelled,
      durationMs: d.durationMs,
      detail: d.success ? `完成：${d.label}` : `失败：${d.label}（exit ${d.exitCode}）`,
    });
  });
  await listen("pipeline-step", (ev) => {
    const step = ev.payload;
    const pid = step.projectId || App.findProjectIdByTaskId(step.pipelineId) || App.activeProjectIdKey;
    let steps = App.pipelineStepsByProj.get(pid);
    if (!steps || (steps.length > 0 && steps[0].pipelineId !== step.pipelineId)) {
      steps = [];
      App.pipelineStepsByProj.set(pid, steps);
    }
    const idx = steps.findIndex(
      (s) => s.pipelineId === step.pipelineId && s.index === step.index
    );
    if (idx >= 0) steps[idx] = step;
    else steps.push(step);
    if (pid === App.activeProjectIdKey) {
      App.renderPipelineSteps();
      window.dispatchEvent(new CustomEvent("sf-pipeline-updated"));
    }
  });
  await listen("pipeline-finished", (ev) => {
    const d = ev.payload;
    const pid = d.projectId || App.findProjectIdByTaskId(d.pipelineId);
    const trackedLabel = pid ? App.activeTasksByProj.get(pid)?.label : "";
    App.completeRunChrome({
      taskId: d.pipelineId,
      projectId: d.projectId || null,
      label: trackedLabel || (String(d.pipelineId || "").startsWith("deploy-") ? "部署推送流程" : "构建发布流程"),
      success: d.success,
      cancelled: d.cancelled,
      durationMs: d.durationMs,
      detail: d.summary,
    });
  });
  await listen("health-report-once", (ev) => {
    if (ev.payload) showOneTimeHealthReport(ev.payload, App);
  });
  await listen("local-server-log", async () => {
    try {
      App.serverStatus = await API.getLocalServerStatus();
      App.updateChrome();
      window.dispatchEvent(new CustomEvent("sf-server-updated"));
    } catch {}
  });

  try {
    const info = await API.getAppInfo();
    App.appVersion = info.version || "";
    const el = document.getElementById("sbAppVer");
    if (el && App.appVersion) el.textContent = `v${App.appVersion}`;
  } catch {}

  await App.reloadConfig(true);

  // Gently refresh sibling project versions & baselines in sidebar (localStorage is only for instant initial paint)
  (async () => {
    const others = (App.config.projects || [])
      .filter((p) => p.id !== App.activeProject?.id)
      .slice(0, 8);
    for (const p of others) {
      try {
        const ov = await API.getProjectOverview(p.id);
        const effDirty = ov.changeReport?.dirtyCount ?? ov.git?.dirtyCount ?? 0;
        const status = !ov.rootExists
          ? "err"
          : ov.healthIssues?.length || !ov.versions?.synced || effDirty > 0
            ? "warn"
            : "ok";
        App.projectMetaCache.set(p.id, { version: ov.versions?.current || "", status });
      } catch {}
    }
    if (others.length) {
      saveCachedProjectMeta(App.projectMetaCache);
      App.renderProjectList();
    }
  })();

  API.checkCrashRecovery()
    .then((rec) => {
      if (rec && rec.recoveredCount > 0) {
        toast(`检测到上次异常中断，已自动安全回滚 ${rec.recoveredCount} 项未完结事务`, "warn", 5500);
        App.refreshOverview(true);
      }
    })
    .catch(() => {});

  if (App.config.onboardingCompleted === false && !App.activeProject) {
    App.openOnboardingWizard();
  }

  App.watchTimer = setInterval(() => {
    if (document.visibilityState === "visible" && App.config.autoWatch !== false) {
      App.probeProjectChanges(false);
    }
  }, 8000);

  // Silent official GitHub Releases update check 6 seconds after startup
  App.updaterTimer = setTimeout(() => {
    if (document.visibilityState !== "visible") return;
    API.updaterStatus()
      .then((st) => {
        if (!st?.config?.autoCheck) return null;
        return API.updaterCheck(st.config.channel);
      })
      .then((res) => {
        if (!res || !res.hasUpdate || res.isIgnored) return;
        App.pendingUpdate = res;
        document.getElementById("tbPrefsUpdDot")?.classList.remove("hidden");
        toast(`发现 ShipForge 新版本 v${res.latestVersion}，可在「偏好设置 → 版本与更新」升级`, "ok", 6000);
      })
      .catch(() => {});
  }, 6000);
}

init();
