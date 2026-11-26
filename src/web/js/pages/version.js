import { API, esc, isValidSemver } from "../api.js";
import { confirmDialog, copyText, toast, withBusy } from "../ui.js";
import { renderGitPane, renderJournalPane } from "./manage.js";

export function renderVersion(root, app, epoch) {
  const ov = app.overview;
  if (!ov) return;
  const { project, rootExists, versions, changeReport, lock } = ov;
  const activeTab = app.subTabs.version || "changelog";

  const subDesc =
    activeTab === "sync"
      ? "多配置文件原子同步 + 按版本号独立隔离草稿与构建指纹"
      : activeTab === "git"
        ? "工作区变更暂存、提交、打版本标签与推送远端仓库"
        : activeTab === "journal"
          ? "构建锁存活监控与项目历史构建台账"
          : "Apple Notes 双栏排版：左侧浏览历史版本记录，右侧纯白画布撰写或阅读变更日志";

  root.innerHTML = `
    <div class="page-wrap page-wrap-wide">
      <div class="page-head">
        <div>
          <h1>版本与源码</h1>
          <div class="page-head-sub">${esc(subDesc)}</div>
        </div>
        <div class="head-actions">
          <span class="tag mono ${versions.synced ? "tag-ok" : "tag-warn"}">v${esc(versions.current)} · ${versions.synced ? "已对齐" : "不一致"}</span>
        </div>
      </div>

      <div id="versionBody" class="page-body-fill"></div>
    </div>
  `;

  const body = document.getElementById("versionBody");
  if (activeTab === "sync") {
    return renderVersionSyncTab(body, app, ov, epoch);
  }
  if (activeTab === "git") {
    return renderGitPane(body, app, ov);
  }
  if (activeTab === "journal") {
    return renderJournalPane(body, app, ov);
  }
  return renderChangelogNotesTab(body, app, ov, epoch);
}

function normalizeEntriesNewestFirst(rawList) {
  const list = Array.isArray(rawList) ? [...rawList] : [];
  if (list.length < 2) return list;
  const parseSemverKey = (v) => {
    const m = String(v || "")
      .trim()
      .replace(/^v/i, "")
      .match(/^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?/);
    if (!m) return null;
    return [Number(m[1]), Number(m[2]), Number(m[3]), m[4] ? 0 : 1, (m[4] || "").toLowerCase()];
  };
  const first = parseSemverKey(list[0].version);
  const last = parseSemverKey(list[list.length - 1].version);
  if (first && last) {
    for (let i = 0; i < 4; i++) {
      if (first[i] !== last[i]) {
        return first[i] < last[i] ? list.reverse() : list;
      }
    }
    if (first[4] !== last[4]) {
      return first[4] < last[4] ? list.reverse() : list;
    }
  }
  return list;
}

function renderChangelogNotesTab(container, app, ov, epoch) {
  const { project, rootExists, versions, changeReport } = ov;
  const activeBucket = changeReport?.bucket;

  let entries = [];
  let cachedBuckets = [];
  let selectedMode = "compose"; // "compose" | entry index
  const pendingForThisProj =
    app._pendingChangelog && app._pendingChangelog.projectId === project.id
      ? app._pendingChangelog
      : null;
  let draftVer = pendingForThisProj?.ver || versions.current;
  let draftSummary = "";
  let draftBody = pendingForThisProj?.draft ?? activeBucket?.notesDraft ?? "";
  let bodyUserEdited = false;
  if (pendingForThisProj) {
    app._pendingChangelog = null;
  }

  container.innerHTML = `
    <div class="notes-split">
      <!-- Left: Apple Notes History List -->
      <div class="notes-list-pane">
        <div class="notes-list-head">
          <strong style="font-size:12.5px">变更历史 <span class="dim" id="clCountMeta" style="font-weight:400"></span></strong>
          <button class="btn btn-xs btn-primary" id="clNewEntryBtn" type="button">+ 新建条目</button>
        </div>
        <div class="notes-list-items scroller" id="clHistoryList">
          <div class="dim" style="padding:14px 8px;font-size:12px">正在解析 ${esc(project.changelogPath || "CHANGELOG.md")}…</div>
        </div>
      </div>

      <!-- Right: Pure White Apple Notes Editor / Reader -->
      <div class="notes-editor-pane" id="clEditorPane"></div>
    </div>
  `;

  const syncDraftFromBuckets = (ver) => {
    if (bodyUserEdited && draftBody.trim()) return;
    const hit = cachedBuckets.find((b) => b.version === ver);
    if (hit && typeof hit.notesDraft === "string") {
      draftBody = hit.notesDraft;
      const bodyEl = document.getElementById("clBody");
      if (bodyEl) bodyEl.value = draftBody;
    }
  };

  const renderSidebarList = () => {
    const host = document.getElementById("clHistoryList");
    const meta = document.getElementById("clCountMeta");
    if (!host) return;
    if (meta) meta.textContent = entries.length ? `(${entries.length})` : "";

    const composeRow = `
      <button class="notes-item ${selectedMode === "compose" ? "active" : ""}" data-note-select="compose" type="button">
        <div class="notes-item-top">
          <span class="notes-item-title">撰写 v${esc(draftVer)}</span>
          <span class="tag tag-accent">编辑</span>
        </div>
        <div class="notes-item-snippet">${esc(draftSummary || (draftBody ? draftBody.split("\n")[0] : "隔离草稿 · 智能写入变更记录"))}</div>
      </button>
    `;

    const historyRows = entries.length
      ? entries
          .map((item, idx) => {
            const title = item.heading.replace(/^##\s*/, "");
            const firstLine = (item.body || "")
              .split(/\r?\n/)
              .map((s) => s.trim())
              .find(Boolean) || "无详细条目";
            return `<button class="notes-item ${selectedMode === idx ? "active" : ""}" data-note-select="${idx}" type="button">
              <div class="notes-item-top">
                <span class="notes-item-title mono">v${esc(item.version)}</span>
              </div>
              <div class="notes-item-snippet">${esc(title)}</div>
              <div class="notes-item-snippet dim" style="font-size:11px">${esc(firstLine)}</div>
            </button>`;
          })
          .join("")
      : `<div class="dim" style="padding:14px 10px;font-size:12px">尚无历史版本记录</div>`;

    host.innerHTML = composeRow + historyRows;
    host.querySelectorAll("[data-note-select]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const val = btn.dataset.noteSelect;
        selectedMode = val === "compose" ? "compose" : Number(val);
        renderSidebarList();
        renderRightPane();
      });
    });
  };

  const renderRightPane = () => {
    const pane = document.getElementById("clEditorPane");
    if (!pane) return;

    if (selectedMode === "compose") {
      pane.innerHTML = `
        <div class="card-head">
          <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap">
            <h2 style="margin:0;font-size:16px">撰写变更记录</h2>
            <span class="tag mono tag-accent">v${esc(draftVer)} 隔离草稿</span>
          </div>
          <div class="field-row" style="gap:6px">
            <button class="btn btn-xs" id="clSaveDraftBtn" type="button" title="仅保存到当前版本隔离草稿，不修改源码文件">暂存草稿</button>
            <button class="btn btn-xs" id="clDraftGitBtn" type="button" ${rootExists ? "" : "disabled"}>从 Git 提交草拟</button>
            <button class="btn btn-xs btn-primary" id="clSaveBtn" type="button" ${rootExists ? "" : "disabled"}>写入变更记录</button>
          </div>
        </div>

        <div class="grid-2">
          <div class="field">
            <label for="clVer">版本号</label>
            <input class="input mono" id="clVer" value="${esc(draftVer)}">
          </div>
          <div class="field">
            <label for="clSummary">摘要标题</label>
            <input class="input" id="clSummary" value="${esc(draftSummary)}" placeholder="例如：优化打包引擎与 Apple Notes 界面升级">
          </div>
        </div>

        <div class="field" style="flex:1">
          <label for="clBody">变更详情 (Markdown)</label>
          <textarea class="textarea mono" id="clBody" style="min-height:260px;flex:1" placeholder="- 新增 …&#10;- 优化 …&#10;- 修复 …">${esc(draftBody)}</textarea>
        </div>

        <div class="field-row" style="justify-content:space-between;font-size:11.5px">
          <span class="dim mono">目标文件：${esc(project.changelogPath || "CHANGELOG.md")}</span>
          <span class="dim">左侧点击任意历史版本可切换纯白阅读视图</span>
        </div>
      `;

      const verInp = document.getElementById("clVer");
      const sumInp = document.getElementById("clSummary");
      const bodyInp = document.getElementById("clBody");

      verInp?.addEventListener("input", () => {
        draftVer = verInp.value.trim() || versions.current;
      });
      verInp?.addEventListener("change", () => {
        draftVer = verInp.value.trim() || versions.current;
        syncDraftFromBuckets(draftVer);
        renderSidebarList();
      });
      sumInp?.addEventListener("input", () => {
        draftSummary = sumInp.value;
      });
      bodyInp?.addEventListener("input", () => {
        draftBody = bodyInp.value;
        bodyUserEdited = true;
      });

      document.getElementById("clSaveDraftBtn")?.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "保存…", async () => {
          const ver = verInp.value.trim() || versions.current;
          const body = bodyInp.value;
          try {
            await API.saveVersionDraft(project.id, ver, body);
            draftVer = ver;
            draftBody = body;
            toast(`已保存 v${ver} 隔离草稿`, "ok", 1800);
            API.getVersionBuckets(project.id).then((b) => (cachedBuckets = b || [])).catch(() => {});
          } catch (err) {
            toast(`${err}`, "err");
          }
        })
      );

      document.getElementById("clDraftGitBtn")?.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "读取 Git…", async () => {
          try {
            draftBody = await API.draftChangelogFromGit(project.id);
            bodyInp.value = draftBody;
            toast("已从 Git 提交生成变更草稿", "ok");
          } catch (err) {
            toast(`${err}`, "warn");
          }
        })
      );

      document.getElementById("clSaveBtn")?.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "写入中…", async () => {
          const ver = verInp.value.trim();
          const summary = sumInp.value.trim();
          const body = bodyInp.value.trim();
          if (!isValidSemver(ver)) {
            toast("请填写有效的语义版本号", "warn");
            return;
          }
          try {
            const data = await API.addChangelogEntry(project.id, ver, summary, body);
            await API.saveVersionDraft(project.id, ver, body).catch(() => {});
            toast(`已同步写入 v${ver} 到变更记录`, "ok");
            entries = normalizeEntriesNewestFirst(data?.entries || []);
            draftSummary = "";
            bodyUserEdited = false;
            const hitIdx = entries.findIndex(
              (item) => (item.version || "").toLowerCase() === ver.toLowerCase()
            );
            selectedMode = hitIdx >= 0 ? hitIdx : 0;
            renderSidebarList();
            renderRightPane();
          } catch (err) {
            toast(`写入失败：${err}`, "err");
          }
        })
      );
    } else {
      const item = entries[selectedMode];
      if (!item) {
        selectedMode = "compose";
        renderRightPane();
        return;
      }
      const cleanHeading = item.heading.replace(/^##\s*/, "");
      pane.innerHTML = `
        <div class="card-head" style="padding-bottom:10px;border-bottom:1px solid var(--line)">
          <div style="min-width:0">
            <div style="display:flex;align-items:center;gap:8px;margin-bottom:4px">
              <span class="tag mono tag-accent">v${esc(item.version)}</span>
              <span class="dim mono" style="font-size:11.5px">${esc(project.changelogPath || "CHANGELOG.md")}</span>
            </div>
            <h2 class="notes-doc-title">${esc(cleanHeading)}</h2>
          </div>
          <div class="field-row" style="gap:6px">
            <button class="btn btn-xs" id="clCopyReadBtn" type="button">复制内容</button>
            <button class="btn btn-xs btn-soft" id="clCloneToEditBtn" type="button">载入到编辑器</button>
            <button class="btn btn-xs btn-primary" id="clBackComposeBtn" type="button">撰写新条目</button>
          </div>
        </div>
        <div class="scroller" style="flex:1;overflow-y:auto;padding-top:4px">
          <pre class="notes-doc-body">${esc(item.body || "（该版本暂无正文描述）")}</pre>
        </div>
      `;

      document.getElementById("clCopyReadBtn")?.addEventListener("click", () => {
        const bodyText = (item.body || "").trim();
        const headingText = (item.heading || "").trim();
        const fullText = bodyText.startsWith(headingText)
          ? bodyText
          : `${headingText}\n\n${bodyText}`.trim();
        copyText(fullText, `已复制 v${item.version} 变更说明`);
      });
      document.getElementById("clCloneToEditBtn")?.addEventListener("click", () => {
        draftVer = item.version || versions.current;
        draftBody = item.body || "";
        bodyUserEdited = true;
        selectedMode = "compose";
        renderSidebarList();
        renderRightPane();
        toast(`已将 v${item.version} 内容载入编辑器`, "ok", 1400);
      });
      document.getElementById("clBackComposeBtn")?.addEventListener("click", () => {
        selectedMode = "compose";
        renderSidebarList();
        renderRightPane();
      });
    }
  };

  document.getElementById("clNewEntryBtn")?.addEventListener("click", () => {
    selectedMode = "compose";
    renderSidebarList();
    renderRightPane();
    document.getElementById("clSummary")?.focus();
  });

  renderSidebarList();
  renderRightPane();

  API.getVersionBuckets(project.id)
    .then((b) => {
      if (epoch === app.renderEpoch) cachedBuckets = b || [];
    })
    .catch(() => {});

  API.getChangelog(project.id)
    .then((data) => {
      if (epoch !== app.renderEpoch) return;
      entries = data?.exists && Array.isArray(data.entries) ? normalizeEntriesNewestFirst(data.entries) : [];
      renderSidebarList();
    })
    .catch(() => {});
}

function renderVersionSyncTab(container, app, ov, epoch) {
  const { project, rootExists, versions, changeReport, lock } = ov;
  const isBuildBusy = () =>
    Boolean(
      app.isProjectBusy(project.id) ||
        app.isProjectBusy("__system__") ||
        (lock?.exists && lock?.alive)
    );
  const busyLockTip = "当前项目正在构建或运行流水线，禁止中途修改版本号";

  container.innerHTML = `
    ${
      changeReport?.consecutiveWarning
        ? `<div class="notice notice-warn">
            <div>${esc(changeReport.consecutiveWarning)}</div>
            <button class="btn btn-xs btn-primary" id="verQuickBumpNext" type="button">递增 Patch 隔离</button>
          </div>`
        : ""
    }

    <div class="split-40-60">
      <section class="card">
        <div class="card-head">
          <h2>版本源原子同步</h2>
          <span class="dim" style="font-size:11.5px">${versions.readings.length} 个文件</span>
        </div>

        <div class="sf-group sf-group-plain">
          ${
            versions.readings.length
              ? versions.readings
                  .map(
                    (r) => `<div class="sf-row" style="min-height:40px;padding:8px 12px">
                      <div class="sf-row-main">
                        <div class="mono" style="font-size:12px;font-weight:600">${esc(r.path)}</div>
                        <div class="dim" style="font-size:11px">${esc(r.format)}</div>
                      </div>
                      <span class="tag mono ${r.error ? "tag-err" : r.version === versions.current ? "tag-ok" : "tag-warn"}">
                        ${esc(r.error ? r.error : r.version)}
                      </span>
                    </div>`
                  )
                  .join("")
              : `<div class="sf-row"><span class="muted">未配置版本文件，可在偏好设置 → 当前项目配置中添加</span></div>`
          }
        </div>

        <div class="field">
          <label>快捷计算下一版本</label>
          <div class="field-row" style="gap:6px">
            <button class="btn btn-xs" data-bump="patch" type="button">Patch</button>
            <button class="btn btn-xs" data-bump="minor" type="button">Minor</button>
            <button class="btn btn-xs" data-bump="major" type="button">Major</button>
            <button class="btn btn-xs" data-bump="beta" type="button">Beta</button>
          </div>
        </div>

        <div class="field">
          <label for="verTargetInput">目标语义版本号</label>
          <input class="input mono" id="verTargetInput" value="${esc(versions.current)}" placeholder="例如 1.47.0">
        </div>

        <button class="btn btn-primary" id="verApplyBtn" type="button" style="width:100%" ${rootExists ? "" : "disabled"}>原子同步全部版本文件</button>
      </section>

      <section class="card card-soft">
        <div class="card-head">
          <h2>版本隔离工作区</h2>
          <span class="dim" style="font-size:11.5px">按版本号独立保存草稿与构建指纹</span>
        </div>
        <div id="verBucketsList"><span class="dim">正在加载隔离记录…</span></div>
      </section>
    </div>
  `;

  document.getElementById("verQuickBumpNext")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "递增中…", async () => {
      try {
        const next = await API.previewBump(versions.current, "patch");
        await API.applyVersion(project.id, next);
        toast(`已递增并隔离新版本 v${next}`, "ok");
        await app.refreshOverview(true);
      } catch (err) {
        toast(`${err}`, "err");
      }
    }).finally(() => syncVersionMutationGuard())
  );

  container.querySelectorAll("[data-bump]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      try {
        const next = await API.previewBump(versions.current, btn.dataset.bump);
        document.getElementById("verTargetInput").value = next;
      } catch (err) {
        toast(`${err}`, "err");
      }
    });
  });

  document.getElementById("verApplyBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "写入中…", async () => {
      const target = document.getElementById("verTargetInput").value.trim();
      if (!isValidSemver(target)) {
        toast("版本号格式无效，应为 X.Y.Z 或 X.Y.Z-beta.N", "warn");
        return;
      }
      const ok = await confirmDialog({
        title: `原子同步版本至 v${target}`,
        message: `将统一更新以下 ${versions.readings.length} 个配置文件（任一失败自动回滚）：`,
        detail: versions.readings.map((r) => `${r.path} (${r.version || "?"} → ${target})`).join("\n"),
        confirmText: "确认同步",
      });
      if (!ok) return;
      try {
        await API.applyVersion(project.id, target);
        toast(`已同步版本至 v${target} 并建立独立隔离桶`, "ok");
        await app.refreshOverview(true);
      } catch (err) {
        toast(`同步失败：${err}`, "err");
      }
    }).finally(() => syncVersionMutationGuard())
  );

  const loadBuckets = () => {
    API.getVersionBuckets(project.id)
      .then((buckets) => {
        if (epoch !== app.renderEpoch) return;
        const host = document.getElementById("verBucketsList");
        if (!host) return;
        if (!buckets || !buckets.length) {
          host.innerHTML = `<div class="muted" style="font-size:12px">暂无隔离版本桶</div>`;
          return;
        }
        host.innerHTML = `<div class="sf-group sf-group-plain">${buckets
          .slice(0, 10)
          .map(
            (b) => `<div class="sf-row" style="padding:9px 12px">
              <div class="sf-row-main">
                <div class="sf-row-title">
                  <span class="mono">v${esc(b.version)}</span>
                  ${b.sealed ? `<span class="tag tag-ok">已封版</span>` : `<span class="tag">构建 ${b.buildCount} 次</span>`}
                </div>
                <div class="sf-row-desc mono" style="font-size:11px">基线 ${esc(b.baseGitSha || "-")} · ${b.changedFiles.length} 改动${b.notesDraft ? " · 有草稿" : ""}</div>
              </div>
              <div class="sf-row-ctrl" style="gap:4px">
                <button class="btn btn-xs btn-ghost" data-load-ver="${esc(b.version)}" type="button">打开草稿</button>
                <button class="btn btn-xs btn-ghost" data-seal-ver="${esc(b.version)}" data-sealed="${b.sealed ? "1" : "0"}" type="button">${b.sealed ? "解封" : "封版"}</button>
              </div>
            </div>`
          )
          .join("")}</div>`;

        host.querySelectorAll("[data-load-ver]").forEach((btn) => {
          btn.addEventListener("click", () => {
            const ver = btn.dataset.loadVer;
            const hit = buckets.find((x) => x.version === ver);
            app._pendingChangelog = {
              projectId: project.id,
              ver,
              draft: hit?.notesDraft || "",
            };
            app.navigate("version", "changelog");
            toast(`已载入 v${ver} 隔离草稿`, "ok", 1400);
          });
        });

        host.querySelectorAll("[data-seal-ver]").forEach((btn) => {
          btn.addEventListener("click", async () => {
            const ver = btn.dataset.sealVer;
            const nextSealed = btn.dataset.sealed !== "1";
            try {
              await API.sealVersionBucket(project.id, ver, nextSealed);
              toast(nextSealed ? `已封版 v${ver}（后续修改将自动预警）` : `已解封 v${ver}`, "ok", 2200);
              await app.refreshOverview(true);
            } catch (err) {
              toast(`${err}`, "err");
            }
          });
        });
      })
      .catch(() => {});
  };
  loadBuckets();

  function syncVersionMutationGuard() {
    const blocked = !rootExists || isBuildBusy();
    for (const id of ["verApplyBtn", "verQuickBumpNext"]) {
      const btn = document.getElementById(id);
      if (!btn) continue;
      btn.disabled = blocked;
      btn.title = isBuildBusy() ? busyLockTip : "";
    }
  }
  syncVersionMutationGuard();
  window.addEventListener("sf-task-state-changed", syncVersionMutationGuard);
  return () => window.removeEventListener("sf-task-state-changed", syncVersionMutationGuard);
}
