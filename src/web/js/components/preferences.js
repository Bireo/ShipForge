import { API, esc, formatBytes, listen } from "../api.js";
import {
  attachLightDismissFallback,
  confirmDialog,
  getAccentPreference,
  getStoredTheme,
  getSystemScheme,
  hydrateIcons,
  setAccentPreference,
  setThemePreference,
  toast,
  withBusy,
} from "../ui.js";
import { renderConfigPane, renderStoragePane, renderToolchainPane } from "../pages/manage.js";
import { renderGuardrailsPane } from "../pages/settings.js";

const PANES = [
  { id: "general", label: "通用与外观", ico: "palette" },
  { id: "project", label: "当前项目配置", ico: "folder" },
  { id: "storage", label: "存储与扫描", ico: "drive" },
  { id: "toolchain", label: "工具链中心", ico: "wrench" },
  { id: "credentials", label: "DPAPI 凭据保险箱", ico: "key" },
  { id: "guardrails", label: "安全缓冲", ico: "shield" },
  { id: "updates", label: "版本与更新", ico: "download" },
];

let activePrefsDlg = null;
let activePaneCleanup = null;

export function closePreferences() {
  if (typeof activePaneCleanup === "function") {
    try {
      activePaneCleanup();
    } catch {}
    activePaneCleanup = null;
  }
  if (activePrefsDlg) {
    try {
      if (activePrefsDlg.open) activePrefsDlg.close("");
    } catch {}
    activePrefsDlg.remove();
    activePrefsDlg = null;
  }
}

export function openPreferences(app, initialPane = "general") {
  closePreferences();

  let currentPane = PANES.some((p) => p.id === initialPane) ? initialPane : "general";
  let paneEpoch = 0;

  const dlg = document.createElement("dialog");
  dlg.className = "prefs-dialog";
  dlg.setAttribute("closedby", "any");
  dlg.setAttribute("aria-labelledby", "prefsTitle");

  dlg.innerHTML = `
    <div class="prefs-shell">
      <nav class="prefs-nav scroller" aria-label="偏好设置分类">
        <div class="prefs-nav-title">偏好设置</div>
        ${PANES.map(
          (p) => `<button class="prefs-nav-item ${p.id === currentPane ? "active" : ""}" data-pref-pane="${p.id}" type="button">
            <span class="nav-ico" data-ico="${p.ico}"></span>
            <span>${esc(p.label)}</span>
          </button>`
        ).join("")}
      </nav>
      <div class="prefs-main">
        <div class="prefs-head">
          <h1 id="prefsTitle">通用与外观</h1>
          <button class="icon-btn" id="prefsCloseBtn" type="button" aria-label="关闭偏好设置">✕</button>
        </div>
        <div class="prefs-body scroller" id="prefsBody"></div>
      </div>
    </div>
  `;

  hydrateIcons(dlg);
  attachLightDismissFallback(dlg);

  const cleanupCurrent = () => {
    if (typeof activePaneCleanup === "function") {
      try {
        activePaneCleanup();
      } catch {}
      activePaneCleanup = null;
    }
  };

  dlg.addEventListener("close", () => {
    cleanupCurrent();
    dlg.remove();
    if (activePrefsDlg === dlg) activePrefsDlg = null;
  });
  dlg.querySelector("#prefsCloseBtn")?.addEventListener("click", () => dlg.close(""));

  const renderPane = () => {
    cleanupCurrent();
    const localEpoch = ++paneEpoch;
    const paneMeta = PANES.find((p) => p.id === currentPane) || PANES[0];
    const titleEl = dlg.querySelector("#prefsTitle");
    if (titleEl) titleEl.textContent = paneMeta.label;
    dlg.querySelectorAll("[data-pref-pane]").forEach((btn) => {
      btn.classList.toggle("active", btn.dataset.prefPane === currentPane);
    });

    const host = dlg.querySelector("#prefsBody");
    if (!host) return;
    const isAlive = () => dlg.isConnected && localEpoch === paneEpoch;

    if (currentPane === "general") {
      activePaneCleanup = renderGeneralPane(host, app, () => dlg.close(""));
    } else if (currentPane === "project") {
      if (!app.overview) {
        host.innerHTML = `<div class="empty-state">尚未选择活动项目。请先在左侧栏导入或选择一个项目。</div>`;
      } else {
        activePaneCleanup = renderConfigPane(host, app, app.overview, {
          inModal: true,
          onProjectRemoved: () => dlg.close(""),
        });
      }
    } else if (currentPane === "storage") {
      activePaneCleanup = renderStoragePane(host, app, isAlive);
    } else if (currentPane === "toolchain") {
      activePaneCleanup = renderToolchainPane(host, app, isAlive);
    } else if (currentPane === "credentials") {
      activePaneCleanup = renderCredentialsPane(host, isAlive);
    } else if (currentPane === "guardrails") {
      activePaneCleanup = renderGuardrailsPane(host, app, app.activeProject, isAlive);
    } else if (currentPane === "updates") {
      activePaneCleanup = renderUpdatesPane(host, app, isAlive);
    }
  };

  dlg.querySelectorAll("[data-pref-pane]").forEach((btn) => {
    btn.addEventListener("click", () => {
      currentPane = btn.dataset.prefPane;
      renderPane();
    });
  });

  document.body.appendChild(dlg);
  activePrefsDlg = dlg;
  dlg.showModal();
  renderPane();
}

function renderGeneralPane(container, app, closeModal) {
  const renderControls = () => {
    const stored = getStoredTheme() || "system";
    const sysText = getSystemScheme() === "dark" ? "当前深色" : "当前浅色";
    const accent = getAccentPreference();

    container.innerHTML = `
      <div>
        <div class="sf-section-title">外观与主题</div>
        <div class="sf-group">
          <div class="sf-row">
            <div class="sf-row-main">
              <div class="sf-row-title">外观模式</div>
              <div class="sf-row-desc">浅色采用柔和灰侧栏 + 纯白画布；深色对齐 macOS Sequoia 层次</div>
            </div>
            <div class="sf-row-ctrl">
              <div class="sf-segmented" role="radiogroup" aria-label="外观模式">
                <button class="sf-seg-btn ${stored === "system" ? "active" : ""}" data-pref-theme="system" type="button">系统 (${sysText})</button>
                <button class="sf-seg-btn ${stored === "light" ? "active" : ""}" data-pref-theme="light" type="button">浅色</button>
                <button class="sf-seg-btn ${stored === "dark" ? "active" : ""}" data-pref-theme="dark" type="button">深色</button>
              </div>
            </div>
          </div>

          <div class="sf-row">
            <div class="sf-row-main">
              <div class="sf-row-title">强调色</div>
              <div class="sf-row-desc">默认 Apple Notes 暖金琥珀，支持一键切换 macOS 系统蓝</div>
            </div>
            <div class="sf-row-ctrl accent-swatches">
              <button class="accent-swatch ${accent === "amber" ? "active" : ""}" data-pref-accent="amber" type="button">
                <span class="accent-swatch-dot" style="background:#D98A00"></span>
                <span>暖金琥珀</span>
              </button>
              <button class="accent-swatch ${accent === "blue" ? "active" : ""}" data-pref-accent="blue" type="button">
                <span class="accent-swatch-dot" style="background:#007AFF"></span>
                <span>macOS 系统蓝</span>
              </button>
            </div>
          </div>

          <div class="sf-row">
            <div class="sf-row-main">
              <div class="sf-row-title">终端深色高对比模式</div>
              <div class="sf-row-desc">在浅色主题下也将执行终端锁定为高对比深色背景</div>
            </div>
            <div class="sf-row-ctrl">
              <input class="sf-switch" type="checkbox" switch id="prefTermDark" ${app.termTone === "dark" ? "checked" : ""}>
            </div>
          </div>
        </div>
      </div>

      <div>
        <div class="sf-section-title">工作台行为</div>
        <div class="sf-group">
          <div class="sf-row">
            <div class="sf-row-main">
              <div class="sf-row-title">后台自动改动监测</div>
              <div class="sf-row-desc">窗口可见且空闲时每 8 秒检测版本号、未提交文件与产物变动</div>
            </div>
            <div class="sf-row-ctrl">
              <input class="sf-switch" type="checkbox" switch id="prefAutoWatch" ${app.config.autoWatch !== false ? "checked" : ""}>
            </div>
          </div>
          <div class="sf-row">
            <div class="sf-row-main">
              <div class="sf-row-title">3 步快速配置向导</div>
              <div class="sf-row-desc">随时重新运行项目导入、环境补全与仓库绑定向导</div>
            </div>
            <div class="sf-row-ctrl">
              <button class="btn btn-xs" id="prefOpenWizardBtn" type="button">启动向导</button>
            </div>
          </div>
        </div>
      </div>
    `;

    container.querySelectorAll("[data-pref-theme]").forEach((btn) => {
      btn.addEventListener("click", () => setThemePreference(btn.dataset.prefTheme));
    });
    container.querySelectorAll("[data-pref-accent]").forEach((btn) => {
      btn.addEventListener("click", () => setAccentPreference(btn.dataset.prefAccent));
    });
    container.querySelector("#prefTermDark")?.addEventListener("change", (e) => {
      const nextTone = e.target.checked ? "dark" : "auto";
      if (app.termTone !== nextTone) app.toggleTermTone();
    });
    container.querySelector("#prefAutoWatch")?.addEventListener("change", async (e) => {
      const cb = e.currentTarget;
      const desired = cb.checked;
      try {
        app.config = await API.setAutoWatch(desired);
        toast(desired ? "已开启后台改动监测" : "已关闭自动监测（可按 F5 手动扫描）", "ok", 1800);
      } catch (err) {
        cb.checked = !desired;
        toast(`${err}`, "err");
      }
    });
    container.querySelector("#prefOpenWizardBtn")?.addEventListener("click", () => {
      closeModal();
      app.openOnboardingWizard();
    });
  };

  renderControls();
  const onSync = () => renderControls();
  window.addEventListener("sf-theme-changed", onSync);
  window.addEventListener("sf-accent-changed", onSync);
  return () => {
    window.removeEventListener("sf-theme-changed", onSync);
    window.removeEventListener("sf-accent-changed", onSync);
  };
}

function renderCredentialsPane(container, isAlive) {
  container.innerHTML = `
    <div>
      <div class="sf-section-title">添加仓库访问令牌 (DPAPI 系统加密存储)</div>
      <section class="card card-soft" style="gap:10px">
        <div class="grid-3">
          <div class="field">
            <label for="vcProv">托管平台</label>
            <select class="select" id="vcProv">
              <option value="github">GitHub</option>
              <option value="gitee">Gitee (码云)</option>
              <option value="gitlab">GitLab (官方/自建)</option>
              <option value="gitea">Gitea / Forgejo</option>
            </select>
          </div>
          <div class="field">
            <label for="vcBaseUrl">自建地址（官方留空）</label>
            <input class="input mono" id="vcBaseUrl" placeholder="默认官方接口">
          </div>
          <div class="field">
            <label for="vcLabel">备注名称（可选）</label>
            <input class="input" id="vcLabel" placeholder="例如：主账号 / 公司内网">
          </div>
        </div>
        <div class="field-row">
          <input class="input mono" id="vcToken" type="password" style="flex:1;min-width:220px" placeholder="粘贴 Personal Access Token (需具备 repo / release 权限)">
          <button class="btn btn-primary" id="vcSaveBtn" type="button">验证并加密保存</button>
        </div>
      </section>
    </div>

    <div>
      <div class="sf-section-title">
        <span>已保存加密凭据</span>
        <button class="btn btn-xs btn-ghost" id="vcReloadBtn" type="button">刷新</button>
      </div>
      <div id="vcListHost"><div class="dim" style="padding:10px">正在读取凭据保险箱…</div></div>
    </div>

    <div>
      <div class="sf-section-title">GitHub CLI (gh) 兼容通道</div>
      <div class="sf-group">
        <div class="sf-row">
          <div class="sf-row-main">
            <div class="sf-row-title">本机 gh 登录状态</div>
            <div class="sf-row-desc" id="vcGhStatus">正在检测…</div>
          </div>
          <div class="sf-row-ctrl">
            <button class="btn btn-xs" id="vcGhLoginBtn" type="button">启动 gh 登录</button>
          </div>
        </div>
      </div>
    </div>
  `;

  const loadList = async () => {
    const host = container.querySelector("#vcListHost");
    if (!host) return;
    try {
      const creds = await API.listRepoCredentials();
      if (!isAlive()) return;
      if (!creds.length) {
        host.innerHTML = `<div class="sf-group"><div class="sf-row"><span class="muted">暂无已保存凭据。在上方输入令牌后会自动联网校验并经 Windows DPAPI 加密落盘。</span></div></div>`;
        return;
      }
      host.innerHTML = `<div class="sf-group">${creds
        .map(
          (c) => `<div class="sf-row">
            <div class="sf-row-main">
              <div class="sf-row-title">
                <span class="tag tag-accent">${esc(c.provider.toUpperCase())}</span>
                <strong>${esc(c.username || c.label || "已认证账号")}</strong>
                <span class="mono dim" style="font-size:11.5px">${esc(c.maskedToken)}</span>
              </div>
              <div class="sf-row-desc mono">${esc(c.baseUrl || "官方默认接口")} · 创建于 ${esc(c.createdAt)}</div>
            </div>
            <div class="sf-row-ctrl">
              <button class="btn btn-xs btn-danger" data-vc-del="${esc(c.id)}" type="button">删除</button>
            </div>
          </div>`
        )
        .join("")}</div>`;

      host.querySelectorAll("[data-vc-del]").forEach((btn) => {
        btn.addEventListener("click", (ev) =>
          withBusy(ev.currentTarget, "删除中…", async () => {
            const cred = creds.find((x) => x.id === btn.dataset.vcDel);
            const ok = await confirmDialog({
              title: "删除已保存的仓库访问令牌",
              message: `确认从 DPAPI 凭据保险箱永久移除 ${cred ? `${cred.provider.toUpperCase()} · ${cred.username || cred.label || cred.maskedToken}` : "该凭据"}？`,
              confirmText: "确认删除",
              danger: true,
            });
            if (!ok) return;
            try {
              await API.deleteRepoCredential(btn.dataset.vcDel);
              toast("已删除凭据", "ok");
              await loadList();
            } catch (err) {
              toast(`${err}`, "err");
            }
          })
        );
      });
    } catch (err) {
      host.innerHTML = `<div class="notice notice-err">${esc(err)}</div>`;
    }
  };

  const loadGh = async () => {
    const el = container.querySelector("#vcGhStatus");
    if (!el) return;
    try {
      const st = await API.getGhStatus();
      if (!isAlive()) return;
      el.textContent = st.authenticated
        ? `已认证：${st.account || "GitHub"} (${st.version || "gh"})`
        : st.installed
          ? `已安装 gh (${st.version || ""})，但尚未登录`
          : "未安装 gh CLI（不影响上方原生 PAT 令牌发布）";
    } catch {
      if (isAlive()) el.textContent = "未检测到 gh CLI";
    }
  };

  loadList();
  loadGh();

  container.querySelector("#vcReloadBtn")?.addEventListener("click", loadList);
  container.querySelector("#vcSaveBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "验证中…", async () => {
      const provider = container.querySelector("#vcProv").value;
      const baseUrl = container.querySelector("#vcBaseUrl").value.trim();
      const label = container.querySelector("#vcLabel").value.trim();
      const token = container.querySelector("#vcToken").value.trim();
      if (!token) return toast("请粘贴访问令牌", "warn");
      try {
        const meta = await API.verifyAndSaveRepoCredential({ provider, baseUrl, label, token });
        container.querySelector("#vcToken").value = "";
        toast(`凭据已验证并加密保存：${meta.username}`, "ok", 3000);
        loadList();
      } catch (err) {
        toast(`${err}`, "err", 4200);
      }
    })
  );
  container.querySelector("#vcGhLoginBtn")?.addEventListener("click", async () => {
    try {
      toast(await API.launchGhAuth("login"), "ok", 3200);
    } catch (err) {
      toast(`${err}`, "err");
    }
  });
}

function renderUpdatesPane(container, app, isAlive) {
  let status = null;
  let checkRes = app.pendingUpdate || null;
  let checking = false;
  let downloading = false;
  let cancelling = false;
  let silentInstall = false;
  let changelogData = null;
  let unlistenProg = null;
  let unlistenChk = null;
  let disposed = false;

  container.innerHTML = `
    <div>
      <div class="sf-section-title">
        <span>ShipForge 版本与官方在线更新</span>
        <div class="field-row" style="gap:6px">
          <button class="btn btn-xs btn-ghost" id="updOpenReleasesBtn" type="button">查看发布页</button>
          <button class="btn btn-xs btn-primary" id="updCheckBtn" type="button">检查更新</button>
        </div>
      </div>
      <div class="sf-group" id="updSummaryHost">
        <div class="sf-row"><span class="dim">正在读取版本与更新状态…</span></div>
      </div>
    </div>

    <div class="upd-prog-box hidden" id="updCheckProgBox">
      <div class="upd-prog-row">
        <span id="updCheckProgStage">正在连接官方更新源…</span>
        <span class="mono" id="updCheckProgPct">10%</span>
      </div>
      <div class="upd-prog-track"><div class="upd-prog-fill" id="updCheckProgFill"></div></div>
    </div>

    <div class="upd-prog-box hidden" id="updDlProgBox">
      <div class="upd-prog-row">
        <span id="updDlProgMeta">准备下载…</span>
        <button class="btn btn-xs btn-ghost" id="updCancelDlBtn" type="button">取消下载</button>
      </div>
      <div class="upd-prog-track"><div class="upd-prog-fill" id="updDlProgFill"></div></div>
    </div>

    <div id="updResultHost"></div>

    <div>
      <div class="sf-section-title">更新策略与缓存</div>
      <div class="sf-group" id="updSettingsHost"></div>
    </div>

    <div>
      <div class="sf-section-title">
        <span>版本变更记录</span>
        <button class="btn btn-xs btn-ghost" id="updSyncClBtn" type="button">同步 GitHub 记录</button>
      </div>
      <div id="updChangelogHost"><div class="sf-group"><div class="sf-row"><span class="dim">正在加载变更记录…</span></div></div></div>
    </div>
  `;

  const syncCheckBar = (stage, pct) => {
    const box = container.querySelector("#updCheckProgBox");
    if (!box) return;
    box.classList.toggle("hidden", !checking);
    if (!checking) return;
    const p = Math.max(0, Math.min(100, Number(pct) || 0));
    const fill = container.querySelector("#updCheckProgFill");
    const stEl = container.querySelector("#updCheckProgStage");
    const pctEl = container.querySelector("#updCheckProgPct");
    if (fill) fill.style.width = `${p}%`;
    if (stEl && stage) stEl.textContent = stage;
    if (pctEl) pctEl.textContent = `${p}%`;
  };

  const syncDlBar = (p) => {
    const box = container.querySelector("#updDlProgBox");
    if (!box) return;
    const show = downloading || (p && (p.phase === "downloading" || p.phase === "verifying"));
    box.classList.toggle("hidden", !show);
    if (!show) return;
    const pct = Math.max(0, Math.min(100, Number(p?.percent) || 0));
    const fill = container.querySelector("#updDlProgFill");
    const meta = container.querySelector("#updDlProgMeta");
    const cancelBtn = container.querySelector("#updCancelDlBtn");
    if (fill) fill.style.width = `${pct}%`;
    if (meta) {
      if (cancelling) {
        meta.textContent = "正在取消下载并清理临时文件…";
      } else if (p?.phase === "verifying") {
        meta.textContent = "正在校验 SHA-256 与文件头完整性…";
      } else {
        const dl = formatBytes(p?.downloaded || 0);
        const tot = p?.total > 0 ? formatBytes(p.total) : "计算中";
        const spd = p?.speedBps > 0 ? ` · ${formatBytes(p.speedBps)}/s` : "";
        meta.textContent = `${pct}% · ${dl} / ${tot}${spd}`;
      }
    }
    if (cancelBtn) {
      cancelBtn.disabled = cancelling;
      cancelBtn.textContent = cancelling ? "取消中…" : "取消下载";
    }
  };

  const paintSummaryAndSettings = () => {
    if (!isAlive() || !status) return;
    const kindLabel = status.installKind === "installer" ? "安装版 (NSIS)" : "便携版 (ZIP)";
    const curVer = status.currentVersion || app.appVersion || "-";
    const lastCheck = status.config?.lastCheckAt || "尚未检查";
    const summaryHost = container.querySelector("#updSummaryHost");
    if (summaryHost) {
      summaryHost.innerHTML = `
        <div class="sf-row">
          <div class="sf-row-main">
            <div class="sf-row-title">ShipForge · 轻筑 <span class="tag mono">v${esc(curVer)}</span> <span class="tag">${esc(kindLabel)}</span></div>
            <div class="sf-row-desc">官方源：GitHub Releases (3347567186/ShipForge) · 上次检查：${esc(lastCheck)}</div>
          </div>
        </div>
      `;
    }

    const ch = status.config?.channel === "beta" ? "beta" : "stable";
    const autoChk = status.config?.autoCheck !== false;
    const cacheBytes = Number(status.cacheBytes) || 0;
    const settingsHost = container.querySelector("#updSettingsHost");
    if (settingsHost) {
      settingsHost.innerHTML = `
        <div class="sf-row">
          <div class="sf-row-main">
            <div class="sf-row-title">更新通道</div>
            <div class="sf-row-desc">正式版仅推送稳定版本；测试版包含 Beta 预发布版本</div>
          </div>
          <div class="sf-row-ctrl">
            <div class="sf-segmented" role="radiogroup" aria-label="更新通道">
              <button class="sf-seg-btn ${ch === "stable" ? "active" : ""}" data-upd-ch="stable" type="button" ${downloading ? "disabled" : ""}>正式版</button>
              <button class="sf-seg-btn ${ch === "beta" ? "active" : ""}" data-upd-ch="beta" type="button" ${downloading ? "disabled" : ""}>测试版</button>
            </div>
          </div>
        </div>
        <div class="sf-row">
          <div class="sf-row-main">
            <div class="sf-row-title">启动时静默检查更新</div>
            <div class="sf-row-desc">打开软件后自动联网检查是否有新版，离线时不打扰</div>
          </div>
          <div class="sf-row-ctrl">
            <input class="sf-switch" type="checkbox" switch id="updAutoCheckSw" ${autoChk ? "checked" : ""}>
          </div>
        </div>
        <div class="sf-row">
          <div class="sf-row-main">
            <div class="sf-row-title">本地更新包缓存</div>
            <div class="sf-row-desc mono">${esc(status.updatesDir || "-")}（${formatBytes(cacheBytes)}）</div>
          </div>
          <div class="sf-row-ctrl">
            <button class="btn btn-xs" id="updClearCacheBtn" type="button" ${cacheBytes > 0 && !downloading ? "" : "disabled"}>清理缓存</button>
          </div>
        </div>
      `;

      settingsHost.querySelectorAll("[data-upd-ch]").forEach((btn) => {
        btn.addEventListener("click", async () => {
          const nextCh = btn.dataset.updCh;
          if (nextCh === ch) return;
          try {
            await API.updaterConfigSet({ channel: nextCh });
            // Clear stale cross-channel check result so Beta/Stable cards never bleed across channels
            checkRes = null;
            app.pendingUpdate = null;
            document.getElementById("tbPrefsUpdDot")?.classList.add("hidden");
            await refreshStatus();
            paintResult();
            toast(`已切换至${nextCh === "beta" ? "测试版" : "正式版"}通道，请点击「检查更新」获取最新清单`, "ok", 2200);
          } catch (err) {
            toast(`${err}`, "err");
          }
        });
      });

      settingsHost.querySelector("#updAutoCheckSw")?.addEventListener("change", async (e) => {
        const cb = e.currentTarget;
        const desired = cb.checked;
        try {
          await API.updaterConfigSet({ autoCheck: desired });
          await refreshStatus();
          toast(desired ? "已开启启动自动检查更新" : "已关闭启动自动检查更新", "ok", 1800);
        } catch (err) {
          cb.checked = !desired;
          toast(`${err}`, "err");
        }
      });

      settingsHost.querySelector("#updClearCacheBtn")?.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "清理中…", async () => {
          try {
            await API.updaterClearCache();
            if (checkRes) checkRes = { ...checkRes, staged: null };
            await refreshStatus();
            paintResult();
            toast("已清理本地更新缓存", "ok");
          } catch (err) {
            toast(`${err}`, "err");
          }
        })
      );
    }

    const checkBtn = container.querySelector("#updCheckBtn");
    if (checkBtn) checkBtn.disabled = Boolean(downloading || checking);
  };

  const paintResult = () => {
    const host = container.querySelector("#updResultHost");
    if (!host || !isAlive()) return;
    if (!checkRes) {
      host.innerHTML = "";
      return;
    }
    const staged = checkRes.staged || (status?.staged && status.staged.version === checkRes.latestVersion ? status.staged : null);
    if (!checkRes.hasUpdate) {
      document.getElementById("tbPrefsUpdDot")?.classList.add("hidden");
      host.innerHTML = `
        <div class="notice notice-ok">
          <div>当前已是最新版本 <strong class="mono">v${esc(checkRes.currentVersion)}</strong>（远端最新 v${esc(checkRes.latestVersion)}）。</div>
        </div>
      `;
      return;
    }
    const pkgLabel = checkRes.packageKind === "installer" ? "安装程序 (.exe)" : "便携压缩包 (.zip)";
    host.innerHTML = `
      <section class="card">
        <div class="card-head">
          <div>
            <h2>发现新版本 <span class="mono">v${esc(checkRes.latestVersion)}</span> ${checkRes.isPrerelease ? `<span class="tag tag-warn">Beta</span>` : `<span class="tag tag-ok">正式版</span>`}</h2>
            <div class="card-sub">
              ${esc(pkgLabel)} · ${checkRes.size ? formatBytes(checkRes.size) : "未知大小"}${checkRes.pubDate ? ` · ${esc(String(checkRes.pubDate).slice(0, 10))}` : ""}
              ${checkRes.sha256 ? ` · SHA-256 ${esc(checkRes.sha256.slice(0, 12))}…` : ""}
            </div>
          </div>
          <div class="field-row" style="gap:6px">
            ${
              staged
                ? `<button class="btn btn-xs btn-accent" id="updInstallBtn" type="button">立即安装并重启</button>`
                : `<button class="btn btn-xs btn-accent" id="updDownloadBtn" type="button" ${downloading ? "disabled" : ""}>下载更新</button>`
            }
            <button class="btn btn-xs btn-ghost" id="updSkipVerBtn" type="button">${checkRes.isIgnored ? "已跳过" : "跳过此版本"}</button>
          </div>
        </div>
        ${
          staged && checkRes.packageKind === "installer"
            ? `<label class="sf-switch-label" style="margin-bottom:8px"><input class="sf-switch" type="checkbox" switch id="updSilentSw" ${silentInstall ? "checked" : ""}><span>静默安装（后台自动升级并重启）</span></label>`
            : ""
        }
        ${checkRes.notes ? `<pre class="upd-notes-pre">${esc(checkRes.notes)}</pre>` : ""}
      </section>
    `;

    host.querySelector("#updSilentSw")?.addEventListener("change", (e) => {
      silentInstall = e.currentTarget.checked;
    });

    host.querySelector("#updDownloadBtn")?.addEventListener("click", async () => {
      if (downloading) return;
      downloading = true;
      cancelling = false;
      syncDlBar({ phase: "downloading", downloaded: 0, total: checkRes.size || 0, percent: 0, speedBps: 0 });
      paintResult();
      try {
        const info = await API.updaterDownload({
          url: checkRes.downloadUrl,
          sha256: checkRes.sha256,
          size: checkRes.size,
          version: checkRes.latestVersion,
          fileName: checkRes.fileName,
          packageKind: checkRes.packageKind,
        });
        downloading = false;
        cancelling = false;
        checkRes = { ...checkRes, staged: info };
        app.pendingUpdate = checkRes;
        await refreshStatus();
        syncDlBar(null);
        paintResult();
        toast(`v${info.version} 已下载并通过 SHA-256 校验，可立即安装`, "ok", 3500);
      } catch (err) {
        downloading = false;
        cancelling = false;
        syncDlBar(null);
        paintResult();
        const msg = String(err || "");
        toast(msg.includes("取消") ? "已取消更新下载" : `下载更新失败：${msg}`, msg.includes("取消") ? "warn" : "err", 4200);
      }
    });

    host.querySelector("#updInstallBtn")?.addEventListener("click", async (e) => {
      const ok = await confirmDialog({
        title: `安装 ShipForge v${checkRes.latestVersion}`,
        message:
          checkRes.packageKind === "portable"
            ? "将校验并原子替换当前便携版程序，完成后自动重启 ShipForge（失败会自动回滚旧版）。"
            : "将退出当前 ShipForge 并启动已校验的官方安装程序完成升级。",
        confirmText: "立即升级",
      });
      if (!ok) return;
      withBusy(e.currentTarget, "准备安装…", async () => {
        try {
          await API.updaterInstall(silentInstall);
          toast("已启动升级引导，程序即将退出并完成更新…", "ok", 4000);
        } catch (err) {
          toast(`${err}`, "err", 4500);
        }
      });
    });

    host.querySelector("#updSkipVerBtn")?.addEventListener("click", async () => {
      try {
        const nextIgnored = checkRes.isIgnored ? "" : checkRes.latestVersion;
        await API.updaterConfigSet({ ignoredVersion: nextIgnored });
        checkRes = { ...checkRes, isIgnored: Boolean(nextIgnored) };
        app.pendingUpdate = checkRes.hasUpdate ? checkRes : null;
        document.getElementById("tbPrefsUpdDot")?.classList.toggle("hidden", !(checkRes.hasUpdate && !checkRes.isIgnored));
        await refreshStatus();
        paintResult();
        toast(nextIgnored ? `已跳过 v${nextIgnored} 提醒` : "已恢复该版本提醒", "ok", 2000);
      } catch (err) {
        toast(`${err}`, "err");
      }
    });
  };

  const paintChangelog = () => {
    const host = container.querySelector("#updChangelogHost");
    if (!host || !isAlive()) return;
    const items = changelogData?.items || [];
    if (!items.length) {
      host.innerHTML = `<div class="sf-group"><div class="sf-row"><span class="dim">暂无版本变更记录。</span></div></div>`;
      return;
    }
    host.innerHTML = `<div class="upd-cl-list">${items
      .slice(0, 12)
      .map(
        (it, idx) => `
      <details class="upd-cl-item" ${idx === 0 ? "open" : ""}>
        <summary>
          <span>
            <span class="mono">v${esc(it.version)}</span>
            ${it.isCurrent ? `<span class="tag tag-ok">当前版本</span>` : ""}
            ${it.isPrerelease ? `<span class="tag tag-warn">Beta</span>` : ""}
            <span style="margin-left:6px">${esc(it.title || "")}</span>
          </span>
          <span class="dim mono" style="font-size:11px">${esc(it.date || "")}</span>
        </summary>
        ${it.body ? `<div class="upd-cl-body">${esc(it.body)}</div>` : ""}
      </details>
    `
      )
      .join("")}</div>`;
  };

  const refreshStatus = async () => {
    try {
      status = await API.updaterStatus();
      if (status?.busy) {
        downloading = true;
        syncDlBar(app._updaterProg || { phase: "downloading", downloaded: 0, total: 0, percent: 0, speedBps: 0 });
      } else if (downloading && app._updaterProg?.phase === "ready") {
        downloading = false;
        syncDlBar(null);
      }
      paintSummaryAndSettings();
      paintResult();
    } catch (err) {
      const host = container.querySelector("#updSummaryHost");
      if (host && isAlive()) host.innerHTML = `<div class="sf-row"><span class="tag tag-err">${esc(err)}</span></div>`;
    }
  };

  const loadChangelog = async (includeRemote = false) => {
    try {
      changelogData = await API.updaterChangelog(includeRemote);
      paintChangelog();
      if (includeRemote) {
        if (changelogData.remoteError) {
          toast(`远端同步失败（已显示内置记录）：${changelogData.remoteError}`, "warn", 3600);
        } else {
          toast("已同步 GitHub Releases 变更记录", "ok", 2000);
        }
      }
    } catch (err) {
      if (includeRemote) toast(`${err}`, "err");
    }
  };

  container.querySelector("#updOpenReleasesBtn")?.addEventListener("click", () => {
    const url = status?.releasesPage || "https://github.com/3347567186/ShipForge/releases";
    API.openExternalUrl(url).catch((err) => toast(`${err}`, "err"));
  });

  container.querySelector("#updCheckBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "检查中…", async () => {
      checking = true;
      syncCheckBar("正在连接官方更新源…", 12);
      try {
        const ch = status?.config?.channel || "stable";
        checkRes = await API.updaterCheck(ch);
        app.pendingUpdate = checkRes.hasUpdate ? checkRes : null;
        document.getElementById("tbPrefsUpdDot")?.classList.toggle("hidden", !(checkRes.hasUpdate && !checkRes.isIgnored));
        checking = false;
        syncCheckBar("", 0);
        await refreshStatus();
        paintResult();
        if (checkRes.hasUpdate) {
          toast(`发现新版本 v${checkRes.latestVersion}`, "ok", 2800);
        } else {
          toast(`当前 v${checkRes.currentVersion} 已是最新版本`, "ok", 2200);
        }
      } catch (err) {
        checking = false;
        syncCheckBar("", 0);
        toast(`${err}`, "err", 4500);
      }
    })
  );

  container.querySelector("#updCancelDlBtn")?.addEventListener("click", async () => {
    cancelling = true;
    syncDlBar({ phase: "downloading", percent: 0 });
    try {
      await API.updaterCancel();
    } catch {}
  });

  container.querySelector("#updSyncClBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "同步中…", () => loadChangelog(true))
  );

  listen("updater-progress", (ev) => {
    if (!ev.payload) return;
    app._updaterProg = ev.payload;
    if (!disposed && isAlive()) {
      if (ev.payload.phase === "ready" || ev.payload.phase === "error") {
        downloading = false;
        cancelling = false;
        syncDlBar(null);
        if (ev.payload.phase === "ready") {
          refreshStatus();
        } else {
          paintSummaryAndSettings();
          paintResult();
        }
      } else {
        downloading = true;
        syncDlBar(ev.payload);
      }
    }
  })
    .then((u) => {
      if (disposed) u();
      else unlistenProg = u;
    })
    .catch(() => {});

  listen("updater-check-progress", (ev) => {
    if (!disposed && isAlive() && ev.payload && checking) {
      syncCheckBar(ev.payload.stage, ev.payload.percent);
    }
  })
    .then((u) => {
      if (disposed) u();
      else unlistenChk = u;
    })
    .catch(() => {});

  refreshStatus();
  loadChangelog(false);

  return () => {
    disposed = true;
    try {
      if (typeof unlistenProg === "function") unlistenProg();
    } catch {}
    try {
      if (typeof unlistenChk === "function") unlistenChk();
    } catch {}
  };
}
