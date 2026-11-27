import { API, esc, formatBytes, kindLabel } from "../api.js";
import { choiceDialog, confirmDialog, copyText, showContentDialog, toast, withBusy } from "../ui.js";

const STAGE_ORDER = [
  { id: "all", label: "全部" },
  { id: "pending_verify", label: "待验证" },
  { id: "verified", label: "已验证" },
  { id: "ready", label: "可发布" },
  { id: "published", label: "已上架" },
  { id: "archived", label: "已归档" },
];

export function renderRelease(root, app, epoch) {
  const ov = app.overview;
  if (!ov) return;
  const { project, versions, artifacts } = ov;
  const activeTab = app.subTabs.release || "artifacts";

  root.innerHTML = `
    <div class="page-wrap page-wrap-wide">
      <div class="page-head">
        <div>
          <h1>产物与仓库</h1>
          <div class="page-head-sub">${
            activeTab === "repos"
              ? "原生直连 GitHub / Gitee / GitLab / Gitea，左栏浏览线上版本，右栏发布新版本或编辑、下架、撤回历史 Release"
              : activeTab === "local"
                ? "在 127.0.0.1 启动本机测试更新源，发版前验证客户端检测与下载流程"
                : "按生命周期核验产物 SHA-256、生成 latest.json 在线更新清单并归档旧包"
          }</div>
        </div>
        <div class="head-actions">
          <button class="btn" id="relOpenDirBtn" type="button">打开产物目录</button>
        </div>
      </div>

      <div id="relBody" class="page-body-fill"></div>
    </div>
  `;

  document.getElementById("relOpenDirBtn")?.addEventListener("click", () => {
    const relDir = project.releaseDir || "release";
    const dir = /^[a-zA-Z]:/.test(relDir)
      ? relDir
      : `${project.root}\\${relDir}`;
    API.openInExplorer(dir).catch((e) => toast(`${e}`, "err"));
  });

  const body = document.getElementById("relBody");
  if (activeTab === "repos") {
    return renderMultiRepoTab(body, app, project, versions, artifacts, epoch);
  }
  if (activeTab === "local") {
    return renderLocalServerTab(body, app, project, ov);
  }
  return renderClassifiedArtifactsTab(body, app, project, versions, artifacts, epoch);
}

function renderClassifiedArtifactsTab(container, app, project, versions, artifacts, epoch) {
  const active = artifacts.filter((a) => !a.isArchived);
  const installers = active.filter((a) => a.kind === "installer");
  const portables = active.filter((a) => a.kind === "portable");
  const curVer = versions.current || "";
  // Strictly pre-select ONLY artifacts matching current project version so stale older builds
  // are NEVER silently paired with a newer bumped version in latest.json!
  const defaultInst = installers.find((a) => a.version === curVer)?.name || "";
  const defaultPort = portables.find((a) => a.version === curVer)?.name || "";
  const repoProv = project.repoBinding?.provider || "github";
  const repoBase = (project.repoBinding?.baseUrl || "").trim().replace(/\/+$/, "");
  const repoSlug = project.repoBinding?.repoSlug || project.githubRepo || "";
  const defaultChannel = curVer.includes("-") ? "beta" : "stable";
  const defaultUrlMode = repoSlug && repoProv === "github" ? "github" : "custom";
  const defaultCustomBase = (() => {
    if (!repoSlug || repoProv === "github") return "";
    const host =
      repoBase ||
      ({ gitee: "https://gitee.com", gitlab: "https://gitlab.com", gitea: "https://gitea.com" }[
        repoProv
      ] || "https://github.com");
    if (repoProv === "gitlab") {
      return `${host}/api/v4/projects/${encodeURIComponent(repoSlug)}/packages/generic/release/${curVer}`;
    }
    return `${host}/${repoSlug}/releases/download/v${curVer}`;
  })();
  let stageFilter = "all";

  container.innerHTML = `
    <div class="guide-bar">
      <div class="guide-steps">
        <span class="guide-step current">1. 冒烟运行</span>
        <span class="dim">→</span>
        <span class="guide-step">2. 核验 SHA-256</span>
        <span class="dim">→</span>
        <span class="guide-step">3. 生成 latest.json</span>
        <span class="dim">→</span>
        <span class="guide-step">4. 多平台上传发布</span>
      </div>
      <div class="field-row" style="gap:6px">
        <button class="btn btn-xs btn-soft hidden" id="relVerifyPendingBtn" type="button">一键验证待检</button>
        <button class="btn btn-xs" id="relVerifySumsBtn" type="button">核验 SHA-256</button>
        <button class="btn btn-xs" id="relPruneBtn" type="button">归档旧版本</button>
        <button class="btn btn-xs btn-accent" id="relGoPublishTabBtn" type="button">去上传发布 →</button>
      </div>
    </div>

    <section class="card">
      <div class="card-head">
        <h2>产物生命周期分类</h2>
        <span class="dim mono" id="relStageSummary">加载分类状态…</span>
      </div>
      <div class="stage-strip" id="relStageStrip"></div>
      <div id="relArtifactTableHost"><span class="dim">正在读取产物列表…</span></div>
    </section>

    <section class="card card-soft">
      <details class="disclosure" style="border-top:none;padding-top:0">
        <summary style="font-size:13.5px;font-weight:600;color:var(--text-1)">生成在线更新清单（latest.json 与 SHA256SUMS.txt）</summary>
        <div style="margin-top:12px;display:flex;flex-direction:column;gap:10px">
          <div class="grid-3">
            <div class="field">
              <label for="mfVer">版本号</label>
              <input class="input mono" id="mfVer" value="${esc(versions.current)}">
            </div>
            <div class="field">
              <label for="mfChannel">通道</label>
              <select class="select" id="mfChannel">
                <option value="stable" ${defaultChannel === "stable" ? "selected" : ""}>正式版 (stable)</option>
                <option value="beta" ${defaultChannel === "beta" ? "selected" : ""}>测试版 (beta)</option>
              </select>
            </div>
            <div class="field">
              <label for="mfUrlMode">下载地址模式</label>
              <select class="select" id="mfUrlMode">
                <option value="github" ${defaultUrlMode === "github" ? "selected" : ""}>GitHub Releases</option>
                <option value="custom" ${defaultUrlMode === "custom" ? "selected" : ""}>自定义镜像 / CDN / 非 GitHub 平台</option>
                <option value="local">本机测试源 (127.0.0.1)</option>
              </select>
            </div>
          </div>

          <div class="grid-2">
            <div class="field">
              <label for="mfInst">安装包</label>
              <select class="select mono" id="mfInst">
                <option value="">（无）</option>
                ${installers.map((a) => `<option value="${esc(a.name)}" ${a.name === defaultInst ? "selected" : ""}>${esc(a.name)}</option>`).join("")}
              </select>
            </div>
            <div class="field">
              <label for="mfPort">便携包</label>
              <select class="select mono" id="mfPort">
                <option value="">（无）</option>
                ${portables.map((a) => `<option value="${esc(a.name)}" ${a.name === defaultPort ? "selected" : ""}>${esc(a.name)}</option>`).join("")}
              </select>
            </div>
          </div>

          <div class="grid-2">
            <div class="field">
              <label for="mfRepo">仓库标识 (owner/repo)</label>
              <input class="input mono" id="mfRepo" value="${esc(repoSlug)}" placeholder="owner/repo">
            </div>
            <div class="field">
              <label for="mfBaseUrl">自定义下载基地址（仅自定义模式）</label>
              <input class="input mono" id="mfBaseUrl" value="${esc(defaultCustomBase)}" placeholder="https://dl.example.com/releases/v1.0.0">
            </div>
          </div>

          <div class="field">
            <div class="field-row" style="justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap">
              <div class="field-row" style="gap:8px;align-items:center">
                <label for="mfNotes" style="margin:0">更新说明（自动读取，可清空自写）</label>
                <span class="tag" id="mfNotesSource">自动读取中…</span>
              </div>
              <div class="field-row" style="gap:4px">
                <button class="btn btn-xs btn-ghost" id="mfNotesSaveClBtn" type="button">写入变更记录</button>
                <button class="btn btn-xs btn-ghost" id="mfNotesReloadBtn" type="button">重新读取</button>
                <button class="btn btn-xs btn-ghost" id="mfNotesClearBtn" type="button">清空</button>
              </div>
            </div>
            <textarea class="textarea mono" id="mfNotes" style="min-height:84px" placeholder="已自动读取本版本更新说明；也可点「清空」自行填写，留空生成时将自动提取"></textarea>
          </div>
          <div class="field-row" style="justify-content:flex-end">
            <button class="btn btn-primary" id="mfGenBtn" type="button">生成并写入清单</button>
          </div>
        </div>
      </details>
    </section>
  `;

  document.getElementById("relGoPublishTabBtn")?.addEventListener("click", () => app.navigate("release", "repos"));

  let classifiedList = [];
  let loadSeq = 0;

  const paintClassified = () => {
    const strip = container.querySelector("#relStageStrip");
    const host = container.querySelector("#relArtifactTableHost");
    const sum = container.querySelector("#relStageSummary");
    const pendingBtn = container.querySelector("#relVerifyPendingBtn");
    if (!strip || !host || !host.isConnected) return;

    const counts = { all: classifiedList.length };
    STAGE_ORDER.forEach((s) => {
      if (s.id !== "all") counts[s.id] = 0;
    });
    classifiedList.forEach((c) => {
      counts[c.stage] = (counts[c.stage] || 0) + 1;
    });
    if (sum) sum.textContent = `共 ${classifiedList.length} 项`;
    if (pendingBtn) {
      const pCnt = counts.pending_verify || 0;
      pendingBtn.classList.toggle("hidden", pCnt === 0);
      pendingBtn.textContent = `一键验证待检 (${pCnt})`;
    }

    strip.innerHTML = STAGE_ORDER.map(
      (s) => `<button class="stage-pill ${stageFilter === s.id ? "active" : ""}" data-st="${s.id}" type="button">
        <span>${s.label}</span><strong>${counts[s.id] || 0}</strong>
      </button>`
    ).join("");
    strip.querySelectorAll("[data-st]").forEach((btn) => {
      btn.addEventListener("click", () => {
        stageFilter = btn.dataset.st;
        paintClassified();
      });
    });

    const filtered =
      stageFilter === "all" ? classifiedList : classifiedList.filter((c) => c.stage === stageFilter);

    if (!filtered.length) {
      host.innerHTML = `<div class="dim" style="padding:18px 0;text-align:center">当前分类下暂无产物</div>`;
      return;
    }

    host.innerHTML = `
      <div class="table-wrap">
        <table class="data-table">
          <thead>
            <tr><th>文件名</th><th>阶段</th><th>类型</th><th>体积</th><th>操作</th></tr>
          </thead>
          <tbody>
            ${filtered
              .map((c) => {
                const a = c.artifact;
                const stageCls =
                  c.stage === "published" || c.stage === "ready" || c.stage === "verified"
                    ? "tag-ok"
                    : c.stage === "pending_verify"
                      ? "tag-warn"
                      : "";
                return `<tr>
                  <td class="mono" style="word-break:break-all">
                    ${esc(a.name)}
                    ${c.sha256Short ? `<span class="dim" style="font-size:10.5px"> (${esc(c.sha256Short)})</span>` : ""}
                  </td>
                  <td><span class="tag ${stageCls}">${esc(c.stageLabel)}</span></td>
                  <td><span class="tag">${esc(kindLabel(a.kind))}</span></td>
                  <td class="mono">${formatBytes(a.size)}</td>
                  <td>
                    <div class="field-row" style="gap:4px">
                      <button class="btn btn-xs" data-sha="${esc(a.path)}" type="button">SHA256</button>
                      ${
                        c.stage === "pending_verify"
                          ? `<button class="btn btn-xs btn-primary" data-mark-ok="${esc(a.name)}" type="button">标记已验证</button>`
                          : ""
                      }
                      ${
                        c.stage === "verified"
                          ? `<button class="btn btn-xs btn-soft" data-mark-ready="${esc(a.name)}" type="button">标为可发布</button>`
                          : ""
                      }
                      ${
                        c.publishedUrl
                          ? `<button class="btn btn-xs" data-open-url="${esc(c.publishedUrl)}" type="button">查看线上</button>`
                          : ""
                      }
                      ${
                        !a.isArchived
                          ? `<button class="btn btn-xs btn-ghost" data-arch="${esc(a.name)}" type="button">归档</button>`
                          : ""
                      }
                    </div>
                  </td>
                </tr>`;
              })
              .join("")}
          </tbody>
        </table>
      </div>
    `;

    host.querySelectorAll("[data-sha]").forEach((btn) => {
      btn.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "计算…", async () => {
          try {
            const hash = await API.computeArtifactSha256(btn.dataset.sha);
            await copyText(hash, `已复制 SHA-256：${hash.slice(0, 14)}…`);
          } catch (err) {
            toast(`${err}`, "err");
          }
        })
      );
    });
    host.querySelectorAll("[data-mark-ok]").forEach((btn) => {
      btn.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "核验中…", async () => {
          const fname = btn.dataset.markOk;
          try {
            const updated = await API.markArtifactStage(project.id, [fname], "verified");
            if (Array.isArray(updated)) {
              classifiedList = updated;
              paintClassified();
            } else {
              await loadClassified();
            }
            toast(`已计算 SHA-256 并标记 ${fname} 为「已验证」`, "ok", 2200);
          } catch (err) {
            toast(`${err}`, "err");
          }
        })
      );
    });
    host.querySelectorAll("[data-mark-ready]").forEach((btn) => {
      btn.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "更新中…", async () => {
          const fname = btn.dataset.markReady;
          try {
            const updated = await API.markArtifactStage(project.id, [fname], "ready");
            if (Array.isArray(updated)) {
              classifiedList = updated;
              paintClassified();
            } else {
              await loadClassified();
            }
            toast(`已标记 ${fname} 为「可发布」`, "ok", 1800);
          } catch (err) {
            toast(`${err}`, "err");
          }
        })
      );
    });
    host.querySelectorAll("[data-open-url]").forEach((btn) => {
      btn.addEventListener("click", () => API.openExternalUrl(btn.dataset.openUrl).catch(() => {}));
    });
    host.querySelectorAll("[data-arch]").forEach((btn) => {
      btn.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "归档中…", async () => {
          try {
            toast(await API.archiveReleaseArtifact(project.id, btn.dataset.arch), "ok");
            await app.refreshOverview(true);
          } catch (err) {
            toast(`${err}`, "err");
          }
        })
      );
    });
  };

  const loadClassified = async () => {
    const seq = ++loadSeq;
    try {
      const list = await API.listClassifiedArtifacts(project.id);
      if (seq !== loadSeq || !container.isConnected) return;
      classifiedList = list || [];
      paintClassified();
    } catch (err) {
      const host = container.querySelector("#relArtifactTableHost");
      if (host) host.innerHTML = `<div class="notice notice-err">${esc(err)}</div>`;
    }
  };
  loadClassified();

  container.querySelector("#relVerifyPendingBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "核验中…", async () => {
      const pendingFiles = classifiedList
        .filter((c) => c.stage === "pending_verify" && !c.artifact.isArchived)
        .map((c) => c.artifact.name);
      if (!pendingFiles.length) return toast("当前无待验证产物", "ok");
      try {
        const updated = await API.markArtifactStage(project.id, pendingFiles, "verified");
        if (Array.isArray(updated)) {
          classifiedList = updated;
          paintClassified();
        } else {
          await loadClassified();
        }
        toast(`已为 ${pendingFiles.length} 个待检产物计算 SHA-256 并标记为「已验证」`, "ok", 2600);
      } catch (err) {
        toast(`${err}`, "err", 4000);
      }
    })
  );

  container.querySelector("#relVerifySumsBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "校验中…", async () => {
      try {
        const items = await API.verifyReleaseChecksums(project.id);
        const listed = items.filter((i) => i.status !== "unlisted");
        const okItems = listed.filter((i) => i.status === "ok");
        const unlistedItems = items.filter((i) => i.status === "unlisted");
        const okFiles = okItems.map((i) => i.file);
        if (okFiles.length) {
          const updated = await API.markArtifactStage(project.id, okFiles, "verified");
          if (Array.isArray(updated)) {
            classifiedList = updated;
            paintClassified();
          } else {
            await loadClassified();
          }
        }

        const statusBadge = (st) => {
          if (st === "ok") return `<span class="tag tag-ok">通过</span>`;
          if (st === "unlisted") return `<span class="tag tag-warn">未入清单</span>`;
          if (st === "invalid") return `<span class="tag tag-err">非法越界路径</span>`;
          if (st === "mismatch") return `<span class="tag tag-err">哈希不匹配</span>`;
          return `<span class="tag tag-err">缺失</span>`;
        };
        const title = listed.length
          ? `SHA-256 核验结果（清单 ${okItems.length}/${listed.length} 通过${unlistedItems.length ? ` · 另有 ${unlistedItems.length} 项未入表` : ""}）`
          : `SHA-256 计算完成（${unlistedItems.length} 项未生成校验表）`;

        const unlistedNotice = unlistedItems.length
          ? `<div class="notice notice-warn" style="display:block;font-size:12px;line-height:1.6">检测到 <strong>${unlistedItems.length}</strong> 个活跃产物未收录在当前 <code>SHA256SUMS.txt</code> 中（通常为历史旧版本产物或尚未重新生成更新清单），因此它们在列表中仍为「待验证」。你可以直接点下方按钮为其标记已验证，或归档历史旧版本。</div>`
          : "";

        const dialogActions = [];
        if (unlistedItems.length) {
          dialogActions.push({
            label: `标记这 ${unlistedItems.length} 项为已验证`,
            cls: "btn-accent",
            onClick: async (dlg, btn) => {
              await withBusy(btn, "标记中…", async () => {
                try {
                  const updated = await API.markArtifactStage(
                    project.id,
                    unlistedItems.map((u) => u.file),
                    "verified"
                  );
                  if (Array.isArray(updated)) {
                    classifiedList = updated;
                    paintClassified();
                  } else {
                    await loadClassified();
                  }
                  dlg.close("");
                  toast(`已将 ${unlistedItems.length} 个未入清单产物标记为「已验证」`, "ok", 2600);
                } catch (err) {
                  toast(`${err}`, "err", 4000);
                }
              });
            },
          });
        }

        showContentDialog({
          title,
          htmlBody: `${unlistedNotice}<div class="table-wrap"><table class="data-table"><thead><tr><th>文件</th><th>状态</th><th>SHA-256 摘要</th></tr></thead><tbody>${items
            .map(
              (i) => `<tr>
                <td class="mono">${esc(i.file)}</td>
                <td>${statusBadge(i.status)}</td>
                <td class="mono dim">${esc((i.actual || i.expected || "").slice(0, 16))}${i.actual || i.expected ? "…" : "-"}</td>
              </tr>`
            )
            .join("")}</tbody></table></div>`,
          actions: dialogActions,
        });
      } catch (err) {
        toast(`${err}`, "err", 4000);
      }
    })
  );

  document.getElementById("relPruneBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "归档中…", async () => {
      const keep = project.keepReleases || 2;
      const ok = await confirmDialog({
        title: "归档旧版本产物",
        message: `仅保留最近 ${keep} 个版本的活跃产物，其余移至 archive/ 目录？`,
        confirmText: "确认归档",
      });
      if (!ok) return;
      try {
        const moved = await API.pruneOldReleases(project.id, keep);
        toast(moved.length ? `已归档 ${moved.length} 个旧产物` : "无需归档", "ok");
        await app.refreshOverview(true);
      } catch (err) {
        toast(`${err}`, "err");
      }
    })
  );

  // Auto-fill manifest release notes (primary: auto-extract; secondary: clear & custom write)
  const mfNotesEl = document.getElementById("mfNotes");
  const mfNotesSourceEl = document.getElementById("mfNotesSource");
  const mfVerEl = document.getElementById("mfVer");
  let mfNotesSeq = 0;
  let mfNotesUserTouched = false;
  let mfNotesIsFallback = false;

  const setMfNotesBadge = (text, cls = "") => {
    if (!mfNotesSourceEl) return;
    mfNotesSourceEl.className = `tag ${cls}`.trim();
    mfNotesSourceEl.textContent = text;
  };
  const loadMfNotes = async (ver, force = false) => {
    if (!mfNotesEl || !ver) return;
    if (!force && mfNotesUserTouched) return;
    const seq = ++mfNotesSeq;
    setMfNotesBadge("自动读取中…");
    try {
      const res = await API.resolveReleaseNotesDetail(project.id, ver);
      if (seq !== mfNotesSeq || epoch !== app.renderEpoch || !mfNotesEl.isConnected) return;
      if (!force && mfNotesUserTouched) return;
      mfNotesEl.value = res?.notes || "";
      mfNotesIsFallback = Boolean(res?.isFallback);
      setMfNotesBadge(res?.sourceLabel || "自动读取", res?.isFallback ? "tag-warn" : "tag-ok");
    } catch {
      if (seq === mfNotesSeq && mfNotesEl.isConnected) {
        setMfNotesBadge("读取失败，可手动填写", "tag-warn");
      }
    }
  };
  mfNotesEl?.addEventListener("input", () => {
    mfNotesUserTouched = true;
    mfNotesIsFallback = false;
    const hasText = !!mfNotesEl.value.trim();
    setMfNotesBadge(hasText ? "手动编辑" : "已清空（可自行填写）", hasText ? "tag-accent" : "");
  });
  document.getElementById("mfNotesClearBtn")?.addEventListener("click", () => {
    if (!mfNotesEl) return;
    mfNotesUserTouched = true;
    mfNotesIsFallback = false;
    mfNotesEl.value = "";
    setMfNotesBadge("已清空（可自行填写）");
    mfNotesEl.focus();
  });
  document.getElementById("mfNotesSaveClBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "写入中…", async () => {
      const body = mfNotesEl?.value.trim() || "";
      const targetV = mfVerEl?.value.trim() || versions.current;
      if (!body || (!mfNotesUserTouched && mfNotesIsFallback)) {
        toast("当前仅为默认兜底文案，请先补充实质更新内容后再写入变更记录", "warn", 3400);
        mfNotesEl?.focus();
        return;
      }
      try {
        const writtenPath = await API.upsertChangelogEntry(project.id, targetV, body);
        mfNotesIsFallback = false;
        mfNotesUserTouched = false;
        setMfNotesBadge(`已同步写入 (${writtenPath})`, "tag-ok");
        toast(`已将 v${targetV} 更新内容同步写入 ${writtenPath}`, "ok");
      } catch (err) {
        toast(`${err}`, "err", 4200);
      }
    })
  );
  document.getElementById("mfNotesReloadBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "读取中…", async () => {
      mfNotesUserTouched = false;
      mfNotesIsFallback = false;
      await loadMfNotes(mfVerEl?.value.trim() || versions.current, true);
    })
  );
  mfVerEl?.addEventListener("change", () => {
    const v = mfVerEl.value.trim();
    if (!v) return;
    const instSel = document.getElementById("mfInst");
    const portSel = document.getElementById("mfPort");
    if (instSel) {
      const hitInst = installers.find((a) => a.version === v);
      instSel.value = hitInst ? hitInst.name : "";
    }
    if (portSel) {
      const hitPort = portables.find((a) => a.version === v);
      portSel.value = hitPort ? hitPort.name : "";
    }
    const chSel = document.getElementById("mfChannel");
    if (chSel) chSel.value = v.includes("-") ? "beta" : "stable";
    loadMfNotes(v, false);
  });
  loadMfNotes(mfVerEl?.value.trim() || versions.current, false);

  document.getElementById("mfGenBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "生成中…", async () => {
      const targetVer = document.getElementById("mfVer").value.trim();
      const inst = document.getElementById("mfInst").value || null;
      const port = document.getElementById("mfPort").value || null;
      const mismatched = [inst, port]
        .filter(Boolean)
        .map((name) => artifacts.find((a) => a.name === name))
        .filter((a) => a && a.version && targetVer && a.version !== targetVer);
      if (mismatched.length) {
        const okMismatch = await confirmDialog({
          title: "检测到产物版本与清单版本不一致",
          message: `清单声明版本为 v${targetVer}，但所选产物属于旧/不同版本（${mismatched.map((a) => `${a.name} → v${a.version}`).join("、")}）。继续写入将导致客户端无限更新或发版被拦截！`,
          confirmText: "仍要写入（不推荐）",
          danger: true,
        });
        if (!okMismatch) return;
      }
      const activeLocalPort =
        app.serverStatus?.running && app.serverStatus?.port
          ? app.serverStatus.port
          : app.config.defaultPort || 18765;
      try {
        const res = await API.generateReleaseManifest({
          projectId: project.id,
          version: targetVer,
          channel: document.getElementById("mfChannel").value,
          urlMode: document.getElementById("mfUrlMode").value,
          githubRepo: document.getElementById("mfRepo").value.trim(),
          baseUrl: document.getElementById("mfBaseUrl").value.trim(),
          localPort: activeLocalPort,
          installerFile: inst,
          portableFile: port,
          extraFiles: [],
          minVersion: "",
          notes:
            !mfNotesUserTouched && mfNotesIsFallback
              ? ""
              : document.getElementById("mfNotes").value.trim(),
        });
        const readyFiles = [inst, port, "latest.json", "SHA256SUMS.txt"].filter(Boolean);
        await API.markArtifactStage(project.id, readyFiles, "ready");
        toast("已生成 latest.json 与 SHA256SUMS.txt", "ok");
        showContentDialog({
          title: "已写入更新清单 latest.json",
          htmlBody: `<pre class="code-box" style="margin:0">${esc(res.latestJson)}</pre>`,
        });
        await app.refreshOverview(true);
      } catch (err) {
        toast(`${err}`, "err", 4200);
      }
    })
  );
}

function renderMultiRepoTab(container, app, project, versions, artifacts, epoch) {
  const curVer = versions.current;
  const active = artifacts.filter((a) => !a.isArchived);
  const curProv = project.repoBinding?.provider || "github";
  const curBase = project.repoBinding?.baseUrl || "";
  const curSlug = project.repoBinding?.repoSlug || project.githubRepo || "";
  const curCredId = project.repoBinding?.credentialId || "";

  let showAllAssets = false;
  let releasesList = [];
  let selectedReleaseKey = null; // null => compose new release; otherwise `${r.id || r.tagName}`
  let relFilter = "";
  let releaseLoadSeq = 0;

  // Draft state preserved when switching between compose and detail view
  const composeState = {
    tag: `v${curVer}`,
    title: `${project.productName || project.name} v${curVer}`,
    notes: "",
    notesSourceLabel: "自动读取中…",
    notesSourceCls: "",
    notesIsFallback: false,
    prerelease: curVer.includes("-"),
    draft: false,
    overwriteAssets: true,
    notesLoaded: false,
    userTouched: false,
    uncheckedAssets: new Set(),
  };

  const relKey = (r) => `${r.id || ""}::${r.tagName || ""}`;
  const providerDisplay = (p) =>
    ({ github: "GitHub", gitee: "Gitee", gitlab: "GitLab", gitea: "Gitea" }[p] || p);

  const getConnParams = () => ({
    provider: container.querySelector("#mpProvider")?.value || curProv,
    baseUrl: container.querySelector("#mpBaseUrl")?.value.trim() || "",
    credentialId: container.querySelector("#mpCredSelect")?.value || "",
    repo: container.querySelector("#mpRepo")?.value.trim() || "",
  });

  const renderAssetPills = (tagVal) => {
    const verClean = (tagVal || curVer).replace(/^v/i, "").trim();
    const list = showAllAssets
      ? active
      : active.filter(
          (a) =>
            !a.version ||
            a.version === verClean ||
            a.kind === "manifest" ||
            a.kind === "checksum" ||
            a.name.endsWith(".split.json") ||
            /\.\d{3}$/.test(a.name)
        );
    if (!list.length) {
      return `<span class="dim" style="font-size:12px">暂无匹配 v${esc(verClean)} 的产物（可开启「显示全部产物」）</span>`;
    }
    return list
      .map(
        (a) =>
          `<label class="check-pill"><input type="checkbox" name="mpAsset" value="${esc(a.name)}" ${composeState.uncheckedAssets.has(a.name) ? "" : "checked"}><span class="mono">${esc(a.name)}</span></label>`
      )
      .join("");
  };

  container.innerHTML = `
    <!-- Streamlined Single-Row Repo Binding Strip -->
    <div class="repo-bind-bar">
      <div class="repo-bind-row">
        <select class="select" id="mpProvider" style="width:128px" aria-label="目标平台">
          <option value="github" ${curProv === "github" ? "selected" : ""}>GitHub</option>
          <option value="gitee" ${curProv === "gitee" ? "selected" : ""}>Gitee (码云)</option>
          <option value="gitlab" ${curProv === "gitlab" ? "selected" : ""}>GitLab</option>
          <option value="gitea" ${curProv === "gitea" ? "selected" : ""}>Gitea / Forgejo</option>
        </select>
        <input class="input mono" id="mpRepo" style="flex:1;min-width:190px" value="${esc(curSlug)}" placeholder="owner/repo">
        <button class="btn btn-xs" id="mpPullReposBtn" type="button">拉取仓库</button>
        <button class="btn btn-xs btn-primary" id="mpBindRepoBtn" type="button">保存绑定</button>
        <button class="btn btn-xs btn-ghost" id="mpToggleCredBtn" type="button">自建与凭据 ▾</button>
        <button class="btn btn-xs btn-soft" id="mpOpenVaultBtn" type="button">凭据保险箱</button>
      </div>

      <select class="select hidden" id="mpRepoPicker"></select>

      <div id="mpCredPanel" class="hidden" style="padding-top:8px;border-top:1px solid var(--line);display:flex;flex-direction:column;gap:8px">
        <div class="grid-2">
          <div class="field">
            <label for="mpBaseUrl">自建服务地址（官方留空）</label>
            <input class="input mono" id="mpBaseUrl" value="${esc(curBase)}" placeholder="默认官方接口">
          </div>
          <div class="field">
            <label for="mpCredSelect">认证凭据（DPAPI 加密保存）</label>
            <select class="select" id="mpCredSelect"><option value="">自动匹配（兼容 gh CLI）</option></select>
          </div>
        </div>
        <div class="field-row" style="gap:6px">
          <input class="input" id="mpNewLabel" style="width:140px" placeholder="备注（可选）">
          <input class="input mono" id="mpNewToken" type="password" style="flex:1;min-width:200px" placeholder="快速粘贴新 Personal Access Token">
          <button class="btn btn-xs btn-accent" id="mpVerifySaveCredBtn" type="button">验证并加密保存</button>
        </div>
        <div id="mpCredListHost" class="row-list"></div>
      </div>
    </div>

    <!-- Apple Notes 2-Column Release & Version Recall Workspace -->
    <div class="notes-split">
      <!-- Left: Apple Notes Soft History & Version Management Sidebar -->
      <div class="notes-list-pane">
        <div class="notes-list-head">
          <div class="field-row" style="gap:6px">
            <strong style="font-size:12.5px">线上版本</strong>
            <span class="tag" id="mpRelCount">0</span>
          </div>
          <div class="field-row" style="gap:4px">
            <button class="btn btn-xs btn-accent" id="mpNewReleaseBtn" type="button">+ 新发布</button>
            <button class="btn btn-xs btn-ghost" id="mpLoadListBtn" type="button" title="刷新线上 Release 列表">刷新</button>
          </div>
        </div>
        <div class="notes-list-filter hidden" id="mpRelFilterWrap">
          <input class="input input-xs mono" id="mpRelFilter" type="search" placeholder="筛选版本或标题…" aria-label="筛选线上版本">
        </div>
        <div class="notes-list-items scroller" id="mpReleasesList">
          <div class="dim" style="padding:14px 8px;font-size:12px">绑定仓库或点击刷新加载…</div>
        </div>
      </div>

      <!-- Right: Pure White Canvas (Compose New Release OR Manage / Recall Existing Release) -->
      <div class="notes-editor-pane" id="mpRightPane"></div>
    </div>
  `;

  const saveComposeDraftFromDom = () => {
    const tagEl = container.querySelector("#mpTag");
    if (!tagEl) return;
    composeState.tag = tagEl.value;
    composeState.title = container.querySelector("#mpTitle")?.value ?? composeState.title;
    composeState.notes = container.querySelector("#mpNotes")?.value ?? composeState.notes;
    composeState.prerelease = !!container.querySelector("#mpPre")?.checked;
    composeState.draft = !!container.querySelector("#mpDraft")?.checked;
    composeState.overwriteAssets = !!container.querySelector("#mpOverwrite")?.checked;
    container.querySelectorAll('input[name="mpAsset"]').forEach((cb) => {
      if (cb.checked) composeState.uncheckedAssets.delete(cb.value);
      else composeState.uncheckedAssets.add(cb.value);
    });
  };

  let mpNotesSeq = 0;
  const syncMpNotesBadge = () => {
    const el = container.querySelector("#mpNotesSource");
    if (!el) return;
    el.className = `tag ${composeState.notesSourceCls || ""}`.trim();
    el.textContent = composeState.notesSourceLabel || "自动读取";
  };

  const loadNotesFromChangelog = async (force = false) => {
    if (!force && composeState.userTouched) return;
    const tagVal = container.querySelector("#mpTag")?.value || composeState.tag || curVer;
    const tagVer = tagVal.replace(/^v/i, "").trim();
    const seq = ++mpNotesSeq;
    composeState.notesSourceLabel = "自动读取中…";
    composeState.notesSourceCls = "";
    syncMpNotesBadge();
    try {
      const res = await API.resolveReleaseNotesDetail(project.id, tagVer);
      if (!container.isConnected || seq !== mpNotesSeq) return;
      if (!force && composeState.userTouched) return;
      if (!force && composeState.notesLoaded && composeState.notes.trim()) return;
      composeState.notes = res?.notes || "";
      composeState.notesIsFallback = Boolean(res?.isFallback);
      composeState.notesSourceLabel = res?.sourceLabel || "自动读取";
      composeState.notesSourceCls = res?.isFallback ? "tag-warn" : "tag-ok";
      composeState.notesLoaded = true;
      const el = container.querySelector("#mpNotes");
      if (el) el.value = composeState.notes;
      syncMpNotesBadge();
    } catch {
      if (!container.isConnected || seq !== mpNotesSeq) return;
      composeState.notesSourceLabel = "读取失败，可手动填写";
      composeState.notesSourceCls = "tag-warn";
      syncMpNotesBadge();
    }
  };

  const paintReleasesSidebar = () => {
    const host = container.querySelector("#mpReleasesList");
    const countEl = container.querySelector("#mpRelCount");
    const filterWrap = container.querySelector("#mpRelFilterWrap");
    if (!host) return;
    if (countEl) countEl.textContent = String(releasesList.length);
    if (filterWrap) filterWrap.classList.toggle("hidden", releasesList.length <= 5);

    const q = relFilter.trim().toLowerCase();
    const filtered = q
      ? releasesList.filter(
          (r) =>
            (r.tagName || "").toLowerCase().includes(q) ||
            (r.name || "").toLowerCase().includes(q)
        )
      : releasesList;

    const composeEntry = `
      <button class="notes-item ${selectedReleaseKey === null ? "active" : ""}" data-rel-compose="1" type="button">
        <div class="notes-item-top">
          <span class="notes-item-title">+ 发布新版本</span>
          <span class="tag mono">v${esc(curVer)}</span>
        </div>
        <div class="notes-item-snippet">上传本地构建产物并创建或更新 Release</div>
      </button>
    `;

    const itemsHtml = filtered.length
      ? filtered
          .map((r) => {
            const key = relKey(r);
            const isAct = selectedReleaseKey === key;
            const assetCount = (r.assets || []).length;
            const badge = r.isDraft
              ? `<span class="tag">草稿</span>`
              : r.isPrerelease
                ? `<span class="tag tag-warn">Pre</span>`
                : `<span class="tag tag-ok">正式版</span>`;
            const dateStr = (r.publishedAt || "").slice(0, 10) || "未发布";
            return `<button class="notes-item ${isAct ? "active" : ""}" data-rel-key="${esc(key)}" type="button">
              <div class="notes-item-top">
                <span class="notes-item-title mono">${esc(r.tagName)}</span>
                ${badge}
              </div>
              <div class="notes-item-snippet">${esc(r.name || r.tagName)} · ${esc(dateStr)}${assetCount ? ` · ${assetCount} 资产` : ""}</div>
            </button>`;
          })
          .join("")
      : `<div class="dim" style="padding:12px 10px;font-size:12px">${
          releasesList.length ? "未匹配到符合条件的版本" : "该仓库暂无线上 Release"
        }</div>`;

    host.innerHTML = `${composeEntry}<div class="notes-list-sep"></div>${itemsHtml}`;

    host.querySelector("[data-rel-compose]")?.addEventListener("click", () => {
      if (selectedReleaseKey !== null) {
        selectedReleaseKey = null;
        paintReleasesSidebar();
        paintRightPane();
      }
    });
    host.querySelectorAll("[data-rel-key]").forEach((btn) => {
      btn.addEventListener("click", () => {
        saveComposeDraftFromDom();
        selectedReleaseKey = btn.dataset.relKey;
        paintReleasesSidebar();
        paintRightPane();
      });
    });
  };

  const paintComposePane = (pane) => {
    const matchingExisting = releasesList.find(
      (r) => (r.tagName || "").trim().toLowerCase() === (composeState.tag || "").trim().toLowerCase()
    );
    pane.innerHTML = `
      <div class="card-head">
        <h2>创建 / 更新 Release 并上传产物</h2>
        <span class="tag tag-ok">原生流式上传</span>
      </div>

      ${
        matchingExisting
          ? `<div class="notice notice-warn" style="display:flex;align-items:center;justify-content:space-between;gap:10px;font-size:12px">
              <span>线上已存在 <strong>${esc(matchingExisting.tagName)}</strong>（${matchingExisting.isDraft ? "草稿" : matchingExisting.isPrerelease ? "预发布" : "正式版"}，${(matchingExisting.assets || []).length} 个资产）</span>
              <button class="btn btn-xs" id="mpJumpExistingBtn" type="button">直接管理 / 撤回此版本 →</button>
            </div>`
          : ""
      }

      <div class="grid-2">
        <div class="field">
          <label for="mpTag">Release Tag</label>
          <input class="input mono" id="mpTag" value="${esc(composeState.tag)}">
        </div>
        <div class="field">
          <label for="mpTitle">发布标题</label>
          <input class="input" id="mpTitle" value="${esc(composeState.title)}">
        </div>
      </div>

      <div class="field">
        <div class="field-row" style="justify-content:space-between">
          <label>待上传产物（上传前自动校验 latest.json 防泄漏与哈希）</label>
          <label class="sf-switch-label" style="font-size:11.5px">
            <input class="sf-switch" type="checkbox" switch role="switch" id="mpShowAllAssets" ${showAllAssets ? "checked" : ""}>
            <span>显示全部 (${active.length})</span>
          </label>
        </div>
        <div class="chip-grid" id="mpAssetHost">${renderAssetPills(composeState.tag)}</div>
      </div>

      <div class="field" style="flex:1">
        <div class="field-row" style="justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap">
          <div class="field-row" style="gap:8px;align-items:center">
            <label for="mpNotes" style="margin:0">发布说明 (Markdown)</label>
            <span class="tag ${esc(composeState.notesSourceCls)}" id="mpNotesSource">${esc(composeState.notesSourceLabel)}</span>
          </div>
          <div class="field-row" style="gap:4px">
            <button class="btn btn-xs btn-ghost" id="mpNotesSaveClBtn" type="button">写入变更记录</button>
            <button class="btn btn-xs btn-ghost" id="mpNotesChangelogBtn" type="button">重新读取</button>
            <button class="btn btn-xs btn-ghost" id="mpNotesGitBtn" type="button">从 Git 草拟</button>
            <button class="btn btn-xs btn-ghost" id="mpNotesClearBtn" type="button">清空</button>
          </div>
        </div>
        <textarea class="textarea mono" id="mpNotes" style="min-height:132px;flex:1" placeholder="已自动读取本版本发布说明；可点「清空」自行撰写，留空上传时将自动提取">${esc(composeState.notes)}</textarea>
      </div>

      <div class="field-row" style="justify-content:space-between;padding-top:8px;border-top:1px solid var(--line)">
        <div class="field-row" style="gap:14px">
          <label class="sf-switch-label" style="font-size:12px"><input class="sf-switch" type="checkbox" switch role="switch" id="mpPre" ${composeState.prerelease ? "checked" : ""}><span>Prerelease</span></label>
          <label class="sf-switch-label" style="font-size:12px"><input class="sf-switch" type="checkbox" switch role="switch" id="mpDraft" ${composeState.draft ? "checked" : ""}><span>Draft 草稿</span></label>
          <label class="sf-switch-label" style="font-size:12px"><input class="sf-switch" type="checkbox" switch role="switch" id="mpOverwrite" ${composeState.overwriteAssets ? "checked" : ""}><span>覆盖同名资产</span></label>
        </div>
        <button class="btn btn-accent" id="mpPublishBtn" type="button">校验并上传发布</button>
      </div>
    `;

    pane.querySelector("#mpJumpExistingBtn")?.addEventListener("click", () => {
      if (matchingExisting) {
        saveComposeDraftFromDom();
        selectedReleaseKey = relKey(matchingExisting);
        paintReleasesSidebar();
        paintRightPane();
      }
    });

    pane.querySelector("#mpTag")?.addEventListener("change", (e) => {
      composeState.tag = e.target.value;
      const isPre = composeState.tag.includes("-");
      composeState.prerelease = isPre;
      const preEl = pane.querySelector("#mpPre");
      if (preEl) preEl.checked = isPre;
      const host = pane.querySelector("#mpAssetHost");
      if (host) host.innerHTML = renderAssetPills(composeState.tag);
      if (!composeState.userTouched || !composeState.notes.trim()) {
        composeState.userTouched = false;
        loadNotesFromChangelog(true);
      }
    });
    pane.querySelector("#mpShowAllAssets")?.addEventListener("change", (e) => {
      showAllAssets = e.target.checked;
      const host = pane.querySelector("#mpAssetHost");
      if (host) host.innerHTML = renderAssetPills(pane.querySelector("#mpTag")?.value || composeState.tag);
    });
    pane.querySelector("#mpNotes")?.addEventListener("input", (e) => {
      composeState.userTouched = true;
      composeState.notesIsFallback = false;
      composeState.notes = e.target.value;
      const hasText = !!composeState.notes.trim();
      composeState.notesSourceLabel = hasText ? "手动编辑" : "已清空（可自行填写）";
      composeState.notesSourceCls = hasText ? "tag-accent" : "";
      syncMpNotesBadge();
    });
    pane.querySelector("#mpNotesClearBtn")?.addEventListener("click", () => {
      composeState.userTouched = true;
      composeState.notesIsFallback = false;
      composeState.notes = "";
      composeState.notesSourceLabel = "已清空（可自行填写）";
      composeState.notesSourceCls = "";
      const el = pane.querySelector("#mpNotes");
      if (el) {
        el.value = "";
        el.focus();
      }
      syncMpNotesBadge();
    });
    pane.querySelector("#mpNotesSaveClBtn")?.addEventListener("click", (e) =>
      withBusy(e.currentTarget, "写入中…", async () => {
        saveComposeDraftFromDom();
        const body = composeState.notes.trim();
        const tagVer = (composeState.tag || curVer).replace(/^v/i, "").trim();
        if (!body || (!composeState.userTouched && composeState.notesIsFallback)) {
          toast("当前仅为默认兜底文案，请先补充实质更新内容后再写入变更记录", "warn", 3400);
          pane.querySelector("#mpNotes")?.focus();
          return;
        }
        try {
          const writtenPath = await API.upsertChangelogEntry(project.id, tagVer, body);
          composeState.userTouched = false;
          composeState.notesIsFallback = false;
          composeState.notesSourceLabel = `已同步写入 (${writtenPath})`;
          composeState.notesSourceCls = "tag-ok";
          syncMpNotesBadge();
          toast(`已将 v${tagVer} 更新内容同步写入 ${writtenPath}`, "ok");
        } catch (err) {
          toast(`${err}`, "err", 4200);
        }
      })
    );
    pane.querySelector("#mpNotesChangelogBtn")?.addEventListener("click", (e) =>
      withBusy(e.currentTarget, "读取中…", async () => {
        composeState.userTouched = false;
        composeState.notesIsFallback = false;
        await loadNotesFromChangelog(true);
      })
    );
    pane.querySelector("#mpNotesGitBtn")?.addEventListener("click", (e) =>
      withBusy(e.currentTarget, "草拟中…", async () => {
        try {
          const drafted = await API.draftChangelogFromGit(project.id);
          composeState.notes = drafted;
          composeState.notesLoaded = true;
          composeState.userTouched = true;
          composeState.notesIsFallback = false;
          composeState.notesSourceLabel = "自动提取（Git 草拟）";
          composeState.notesSourceCls = "tag-ok";
          const el = pane.querySelector("#mpNotes");
          if (el) el.value = drafted;
          syncMpNotesBadge();
          toast("已从 Git 提交生成发布说明", "ok");
        } catch (err) {
          toast(`${err}`, "err");
        }
      })
    );

    pane.querySelector("#mpPublishBtn")?.addEventListener("click", (e) =>
      withBusy(e.currentTarget, "上传中…", async () => {
        if (app.isProjectBusy(project.id)) {
          toast("当前项目正在执行构建或发布任务，请等待完成后再上传", "warn", 3200);
          return;
        }
        saveComposeDraftFromDom();
        const { provider, baseUrl, credentialId, repo } = getConnParams();
        const tag = composeState.tag.trim();
        const title = composeState.title.trim();
        const notes =
          !composeState.userTouched && composeState.notesIsFallback
            ? ""
            : composeState.notes.trim();
        const files = [...pane.querySelectorAll('input[name="mpAsset"]:checked')].map((el) => el.value);
        if (!tag || /\s/.test(tag)) {
          toast("请填写合法的 Release Tag（例如 v1.0.0，不能含空格）", "warn");
          pane.querySelector("#mpTag")?.focus();
          return;
        }
        if (!repo.includes("/")) {
          toast("请先绑定目标仓库 (owner/repo)", "warn");
          return;
        }
        if (!files.length) {
          toast("请至少勾选一个待上传产物", "warn");
          return;
        }
        const ok = await confirmDialog({
          title: `确认发布至 ${providerDisplay(provider)}`,
          message: `将 ${files.length} 个文件上传到 ${repo} (${tag})？`,
          detail: files.join("\n"),
          confirmText: "确认上传",
        });
        if (!ok) return;
        try {
          const url = await API.publishReleaseMulti({
            projectId: project.id,
            provider,
            baseUrl,
            credentialId,
            repo,
            tag,
            title,
            notes,
            files,
            prerelease: composeState.prerelease,
            draft: composeState.draft,
            overwriteAssets: composeState.overwriteAssets,
          });
          toast(`发布成功：${url}`, "ok", 4500);
          app._pendingSelectRelease = { projectId: project.id, tag };
          await app.refreshOverview(true);
        } catch (err) {
          toast(`${err}`, "err", 5200);
        }
      })
    );
  };

  const paintDetailPane = (pane, rel) => {
    const { provider, baseUrl, credentialId, repo } = getConnParams();
    const supportsDraft = provider === "github" || provider === "gitea";
    const supportsPre = provider !== "gitlab";
    const statusBadge = rel.isDraft
      ? `<span class="tag">草稿隐藏中</span>`
      : rel.isPrerelease
        ? `<span class="tag tag-warn">预发布测试版</span>`
        : `<span class="tag tag-ok">已公开正式版</span>`;
    const dateText = (rel.publishedAt || "").replace("T", " ").replace("Z", "").slice(0, 16) || "未公开";
    const assets = rel.assets || [];

    pane.innerHTML = `
      <div class="rel-manage-head">
        <div class="rel-manage-title-wrap">
          <div class="field-row" style="gap:8px;flex-wrap:wrap">
            <h2 class="notes-doc-title mono">${esc(rel.tagName)}</h2>
            ${statusBadge}
          </div>
          <div class="rel-meta-strip">
            <span>${esc(providerDisplay(provider))} · <span class="mono">${esc(repo)}</span></span>
            <span>·</span>
            <span class="mono">${esc(dateText)}</span>
            <span>·</span>
            <span>${assets.length} 个线上资产</span>
          </div>
        </div>
        <div class="field-row" style="gap:6px;flex-wrap:wrap">
          ${rel.url ? `<button class="btn btn-xs" id="mpDetailOpenBtn" type="button">打开线上</button>` : ""}
          ${
            supportsDraft
              ? `<button class="btn btn-xs ${rel.isDraft ? "btn-primary" : "btn-soft"}" id="mpDetailToggleDraftBtn" type="button" title="${
                  rel.isDraft ? "将草稿重新公开为正式 Release" : "临时下架为草稿（不删除资产与标签，可随时恢复）"
                }">${rel.isDraft ? "重新公开上线" : "下架转为草稿"}</button>`
              : ""
          }
          <button class="btn btn-xs btn-danger" id="mpDetailRecallBtn" type="button">撤回发布</button>
        </div>
      </div>

      <div class="rel-detail-grid">
      <div class="sf-group">
        <div class="sf-row">
          <div class="sf-row-meta">
            <div class="sf-row-title">发布标题</div>
            <div class="sf-row-desc">修改线上显示的 Release 标题</div>
          </div>
          <div class="sf-row-ctrl" style="flex:1;max-width:440px">
            <input class="input" id="mpEditTitle" value="${esc(rel.name || rel.tagName)}" style="width:100%">
          </div>
        </div>

        <div class="sf-row">
          <div class="sf-row-meta">
            <div class="sf-row-title">预发布测试版 (Prerelease)</div>
            <div class="sf-row-desc">${supportsPre ? "开启后标记为测试/预览版，不影响正式版更新通道" : "GitLab Releases 不区分 Prerelease 标记"}</div>
          </div>
          <div class="sf-row-ctrl">
            <input class="sf-switch" type="checkbox" switch role="switch" id="mpEditPre" ${rel.isPrerelease ? "checked" : ""} ${supportsPre ? "" : "disabled"} aria-label="预发布测试版">
          </div>
        </div>

        <div class="sf-row">
          <div class="sf-row-meta">
            <div class="sf-row-title">草稿隐藏状态 (Draft)</div>
            <div class="sf-row-desc">${supportsDraft ? "开启后对外隐藏该版本（软下架，资产仍完整保留）" : `${providerDisplay(provider)} 暂不支持草稿模式，如需下架请点右上角「撤回发布」`}</div>
          </div>
          <div class="sf-row-ctrl">
            <input class="sf-switch" type="checkbox" switch role="switch" id="mpEditDraft" ${rel.isDraft ? "checked" : ""} ${supportsDraft ? "" : "disabled"} aria-label="草稿隐藏状态">
          </div>
        </div>

        <div class="sf-row">
          <div class="sf-row-meta">
            <div class="sf-row-title">同步覆盖线上 latest.json</div>
            <div class="sf-row-desc">仅同步更新本地并覆盖线上 latest.json 的说明文案，不重新上传安装包与便携包（版本不一致时自动跳过防误覆盖）</div>
          </div>
          <div class="sf-row-ctrl">
            <input class="sf-switch" type="checkbox" switch role="switch" id="mpEditSyncManifest" ${
              rel.tagName.replace(/^v/i, "").toLowerCase() === String(versions?.current || "").replace(/^v/i, "").toLowerCase()
                ? "checked"
                : ""
            } aria-label="同步覆盖线上 latest.json">
          </div>
        </div>

        <div class="sf-row sf-row-stack">
          <div class="field" style="width:100%">
            <label for="mpEditNotes">发布说明 (Markdown)</label>
            <textarea class="textarea mono" id="mpEditNotes" style="min-height:112px" placeholder="本版本暂无发布说明…">${esc(rel.body || "")}</textarea>
          </div>
          <div class="field-row" style="justify-content:space-between;width:100%">
            <button class="btn btn-xs btn-soft" id="mpCloneToComposeBtn" type="button">载入到新发布模板</button>
            <button class="btn btn-primary" id="mpDetailSaveBtn" type="button">保存修改</button>
          </div>
        </div>
      </div>

      <div class="rel-assets-section">
        <div class="card-head" style="margin-bottom:6px">
          <h3 style="margin:0;font-size:13px;font-weight:600">线上产物资产 <span class="tag">${assets.length}</span></h3>
          <span class="dim" style="font-size:11.5px">可复制直链或单独移除错误上传的文件</span>
        </div>
        ${
          assets.length
            ? `<div class="sf-group">
                ${assets
                  .map(
                    (a) => `<div class="sf-row">
                      <div class="sf-row-meta">
                        <div class="sf-row-title mono" style="word-break:break-all">${esc(a.name)}</div>
                        <div class="sf-row-desc mono">${a.size ? formatBytes(a.size) : "外链 / 包注册表资产"}</div>
                      </div>
                      <div class="sf-row-ctrl" style="gap:6px">
                        ${
                          a.downloadUrl
                            ? `<button class="btn btn-xs" data-copy-asset="${esc(a.downloadUrl)}" type="button">复制链接</button>`
                            : ""
                        }
                        ${
                          a.id
                            ? `<button class="btn btn-xs btn-danger" data-del-asset-id="${esc(a.id)}" data-del-asset-name="${esc(a.name)}" type="button">删除</button>`
                            : ""
                        }
                      </div>
                    </div>`
                  )
                  .join("")}
              </div>`
            : `<div class="sf-group"><div class="sf-row"><div class="dim" style="font-size:12px">该 Release 暂无可管理的线上产物资产。</div></div></div>`
        }
      </div>
      </div>
    `;

    pane.querySelector("#mpDetailOpenBtn")?.addEventListener("click", () => {
      if (rel.url) API.openExternalUrl(rel.url).catch((e) => toast(`${e}`, "err"));
    });

    const applyUpdate = async (btn, overrideDraft = null) => {
      const title = pane.querySelector("#mpEditTitle")?.value.trim() || rel.name || rel.tagName;
      const notes = pane.querySelector("#mpEditNotes")?.value ?? rel.body ?? "";
      const prerelease = !!pane.querySelector("#mpEditPre")?.checked;
      const draft = overrideDraft !== null ? overrideDraft : !!pane.querySelector("#mpEditDraft")?.checked;
      const syncLatestJson =
        overrideDraft === null && !!pane.querySelector("#mpEditSyncManifest")?.checked;
      await withBusy(btn, syncLatestJson ? "保存并同步清单…" : "保存中…", async () => {
        try {
          const updated = await API.updateReleaseMulti({
            projectId: project.id,
            provider,
            baseUrl,
            credentialId,
            repo,
            releaseId: rel.id || "",
            tagName: rel.tagName,
            title,
            notes,
            prerelease,
            draft,
            syncLatestJson,
          });
          // Merge assets safely: if provider PATCH omits existing binaries and only returns updated latest.json, preserve existing binaries
          const prevAssets = Array.isArray(rel.assets) ? rel.assets : [];
          const nextAssets = Array.isArray(updated.assets) ? updated.assets : [];
          let mergedAssets = nextAssets.length ? nextAssets : prevAssets;
          if (
            nextAssets.length === 1 &&
            nextAssets[0].name?.toLowerCase() === "latest.json" &&
            prevAssets.length > 1
          ) {
            mergedAssets = [
              ...prevAssets.filter((a) => a.name?.toLowerCase() !== "latest.json"),
              nextAssets[0],
            ];
          }
          // Replace in local cache & re-render immediately
          const idx = releasesList.findIndex((r) => relKey(r) === relKey(rel));
          if (idx >= 0) {
            releasesList[idx] = {
              ...updated,
              assets: mergedAssets,
            };
            selectedReleaseKey = relKey(releasesList[idx]);
          }
          const msg =
            overrideDraft === true
              ? `已将 ${rel.tagName} 下架转为草稿（资产已保留）`
              : overrideDraft === false
                ? `已将 ${rel.tagName} 重新公开上线`
                : updated.manifestSynced
                  ? `已更新 Release ${rel.tagName} 并同步覆盖线上 latest.json`
                  : updated.manifestSyncMessage
                    ? updated.manifestSyncMessage
                    : `已更新 Release ${rel.tagName}`;
          toast(
            msg,
            syncLatestJson && !updated.manifestSynced && updated.manifestSyncMessage ? "warn" : "ok",
            syncLatestJson ? 4600 : 3200
          );
          paintReleasesSidebar();
          paintRightPane();
        } catch (err) {
          toast(`${err}`, "err", 5200);
        }
      });
    };

    pane.querySelector("#mpDetailSaveBtn")?.addEventListener("click", (e) => applyUpdate(e.currentTarget, null));
    pane.querySelector("#mpDetailToggleDraftBtn")?.addEventListener("click", async (e) => {
      const triggerBtn = e.currentTarget;
      const targetDraft = !rel.isDraft;
      const ok = await confirmDialog({
        title: targetDraft ? `下架 ${rel.tagName} 并转为草稿` : `重新公开 ${rel.tagName}`,
        message: targetDraft
          ? `确认将 ${rel.tagName} 转为草稿？转化后外部用户将不可见，但已上传的 ${assets.length} 个资产与 Git Tag 均会保留，随时可一键重新公开。`
          : `确认将草稿 ${rel.tagName} 重新公开发布上线？`,
        confirmText: targetDraft ? "确认下架为草稿" : "确认公开上线",
      });
      if (!ok) return;
      await applyUpdate(triggerBtn, targetDraft);
    });

    pane.querySelector("#mpDetailRecallBtn")?.addEventListener("click", async (e) => {
      if (app.isProjectBusy(project.id)) {
        toast("当前项目正在执行构建或发布任务，请等待完成后再撤回", "warn", 3200);
        return;
      }
      const triggerBtn = e.currentTarget;
      const { action, options } = await choiceDialog({
        title: `撤回发布 ${rel.tagName}`,
        message: `确认从 ${providerDisplay(provider)} (${repo}) 永久删除 Release「${rel.name || rel.tagName}」？`,
        notice: `删除后，该线上 Release 页面及其 ${assets.length} 个线上资产将被移除；本地产物阶段将自动回退为「可发布」，方便修正后重新上传。`,
        noticeKind: "err",
        danger: true,
        options: [
          {
            key: "deleteRemoteTag",
            label: `同时删除远端 Git Tag (${rel.tagName})`,
            desc: "开启后将同步删除远端仓库上的同名版本标签（否则保留 Tag 仅撤回 Release）",
            checked: false,
          },
          {
            key: "deleteLocalTag",
            label: `同时删除本地 Git Tag (${rel.tagName})`,
            desc: "开启后将同步移除本机 Git 仓库中的同名标签（不影响任何代码提交记录）",
            checked: false,
          },
        ],
        buttons: [
          { id: "cancel", label: "取消", cls: "", autofocus: true },
          { id: "confirm", label: "确认撤回并删除", cls: "btn-danger" },
        ],
      });
      if (action !== "confirm") return;

      await withBusy(triggerBtn, "撤回中…", async () => {
        try {
          const res = await API.deleteReleaseMulti({
            projectId: project.id,
            provider,
            baseUrl,
            credentialId,
            repo,
            releaseId: rel.id || "",
            tagName: rel.tagName,
            deleteRemoteTag: !!options.deleteRemoteTag,
            deleteLocalTag: !!options.deleteLocalTag,
          });
          toast(res.summary, res.warnings?.length ? "warn" : "ok", 4500);
          if (res.warnings?.length) {
            toast(res.warnings.join("；"), "warn", 5200);
          }
          selectedReleaseKey = null;
          await app.refreshOverview(true);
        } catch (err) {
          toast(`${err}`, "err", 5200);
        }
      });
    });

    pane.querySelector("#mpCloneToComposeBtn")?.addEventListener("click", () => {
      composeState.tag = rel.tagName;
      composeState.title = rel.name || rel.tagName;
      composeState.notes = rel.body || "";
      composeState.notesLoaded = true;
      composeState.userTouched = true;
      composeState.notesIsFallback = false;
      composeState.notesSourceLabel = `已载入线上 ${rel.tagName}`;
      composeState.notesSourceCls = "tag-accent";
      composeState.prerelease = !!rel.isPrerelease;
      composeState.draft = !!rel.isDraft;
      selectedReleaseKey = null;
      paintReleasesSidebar();
      paintRightPane();
      toast(`已将 ${rel.tagName} 内容载入发布表单`, "ok", 1800);
    });

    pane.querySelectorAll("[data-copy-asset]").forEach((btn) => {
      btn.addEventListener("click", () => copyText(btn.dataset.copyAsset, "已复制资产下载链接"));
    });

    pane.querySelectorAll("[data-del-asset-id]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const assetId = btn.dataset.delAssetId;
        const assetName = btn.dataset.delAssetName;
        const ok = await confirmDialog({
          title: "删除线上资产文件",
          message: `确认从 ${rel.tagName} 中永久删除线上资产「${assetName}」？`,
          confirmText: "确认删除资产",
          danger: true,
        });
        if (!ok) return;
        await withBusy(btn, "删除中…", async () => {
          try {
            const msg = await API.deleteReleaseAssetMulti({
              provider,
              baseUrl,
              credentialId,
              repo,
              releaseId: rel.id || "",
              tagName: rel.tagName,
              assetId,
              assetName,
            });
            toast(msg, "ok", 3000);
            rel.assets = (rel.assets || []).filter((a) => String(a.id) !== String(assetId));
            paintReleasesSidebar();
            paintRightPane();
          } catch (err) {
            toast(`${err}`, "err", 4800);
          }
        });
      });
    });
  };

  const paintRightPane = () => {
    const pane = container.querySelector("#mpRightPane");
    if (!pane) return;
    if (selectedReleaseKey !== null) {
      const found = releasesList.find((r) => relKey(r) === selectedReleaseKey);
      if (found) {
        paintDetailPane(pane, found);
        return;
      }
      selectedReleaseKey = null;
    }
    paintComposePane(pane);
  };

  paintRightPane();
  loadNotesFromChangelog(false);

  const loadCreds = async (preferCredId = null) => {
    try {
      const creds = await API.listRepoCredentials();
      if (!container.isConnected) return;
      const sel = container.querySelector("#mpCredSelect");
      if (sel) {
        const chosen = preferCredId ?? (sel.value || curCredId);
        sel.innerHTML =
          `<option value="">自动匹配（兼容 gh CLI）</option>` +
          creds
            .map(
              (c) =>
                `<option value="${esc(c.id)}" ${c.id === chosen ? "selected" : ""}>${esc(c.provider)} · ${esc(c.username || c.label)} (${esc(c.maskedToken)})</option>`
            )
            .join("");
      }
      const listHost = container.querySelector("#mpCredListHost");
      if (listHost) {
        listHost.innerHTML = creds.length
          ? creds
              .map(
                (c) => `<div class="list-row">
                  <div class="list-row-main">
                    <div class="list-row-title">
                      <span class="tag">${esc(c.provider)}</span>
                      <strong>${esc(c.username || c.label)}</strong>
                      <span class="mono dim">${esc(c.maskedToken)}</span>
                    </div>
                    <div class="list-row-desc mono">${esc(c.baseUrl || "官方默认接口")} · ${esc(c.createdAt)}</div>
                  </div>
                  <button class="btn btn-xs btn-danger" data-del-cred="${esc(c.id)}" type="button">删除</button>
                </div>`
              )
              .join("")
          : `<div class="dim" style="padding:8px 12px;font-size:12px">暂无已保存令牌</div>`;
        listHost.querySelectorAll("[data-del-cred]").forEach((btn) => {
          btn.addEventListener("click", (ev) =>
            withBusy(ev.currentTarget, "删除中…", async () => {
              const c = creds.find((x) => x.id === btn.dataset.delCred);
              const ok = await confirmDialog({
                title: "删除已保存的访问令牌",
                message: `确认永久移除 ${c ? `${c.provider} · ${c.username || c.label || c.maskedToken}` : "该令牌"}？`,
                confirmText: "确认删除",
                danger: true,
              });
              if (!ok) return;
              try {
                await API.deleteRepoCredential(btn.dataset.delCred);
                toast("已删除令牌", "ok");
                await loadCreds();
              } catch (err) {
                toast(`${err}`, "err");
              }
            })
          );
        });
      }
    } catch {}
  };
  loadCreds();

  const loadReleases = async (selectTagAfter = null) => {
    const seq = ++releaseLoadSeq;
    const host = container.querySelector("#mpReleasesList");
    const { provider, baseUrl, credentialId, repo } = getConnParams();
    if (!host) return;
    if (!repo.includes("/")) {
      releasesList = [];
      paintReleasesSidebar();
      host.insertAdjacentHTML(
        "beforeend",
        `<div class="dim" style="padding:10px 10px;font-size:11.5px">请先填写或拉取选择仓库 (owner/repo)</div>`
      );
      return;
    }
    host.innerHTML = `<div class="dim" style="padding:14px 10px;font-size:12px">正在拉取线上 Releases…</div>`;
    try {
      const list = await API.listReleasesMulti(provider, baseUrl, credentialId, repo);
      if (seq !== releaseLoadSeq || !container.isConnected) return;
      // Preserve in-progress compose inputs, asset selections & right-pane focus ONLY (never steal focus from #mpRepo/#termFilter)
      const rightPaneEl = container.querySelector("#mpRightPane");
      if (selectedReleaseKey === null) saveComposeDraftFromDom();
      const focusInsideRightPane = Boolean(rightPaneEl && rightPaneEl.contains(document.activeElement));
      const activeIdBefore = focusInsideRightPane ? document.activeElement?.id || "" : "";
      const activeSelStart =
        focusInsideRightPane && typeof document.activeElement.selectionStart === "number"
          ? document.activeElement.selectionStart
          : null;
      const activeSelEnd =
        focusInsideRightPane && typeof document.activeElement.selectionEnd === "number"
          ? document.activeElement.selectionEnd
          : null;
      releasesList = list || [];
      if (selectTagAfter) {
        const hit = releasesList.find((r) => r.tagName === selectTagAfter);
        selectedReleaseKey = hit ? relKey(hit) : null;
      } else if (selectedReleaseKey !== null && !releasesList.some((r) => relKey(r) === selectedReleaseKey)) {
        selectedReleaseKey = null;
      }
      paintReleasesSidebar();
      paintRightPane();
      if (activeIdBefore && selectedReleaseKey === null && rightPaneEl) {
        const restored = rightPaneEl.querySelector(`#${CSS.escape(activeIdBefore)}`);
        if (restored && typeof restored.focus === "function") {
          restored.focus();
          if (activeSelStart !== null && typeof restored.setSelectionRange === "function") {
            try {
              restored.setSelectionRange(activeSelStart, activeSelEnd ?? activeSelStart);
            } catch {}
          }
        }
      }
    } catch (err) {
      if (seq !== releaseLoadSeq || !container.isConnected) return;
      releasesList = [];
      paintReleasesSidebar();
      host.insertAdjacentHTML(
        "beforeend",
        `<div class="muted" style="padding:10px 10px;font-size:12px">${esc(err)}</div>`
      );
    }
  };
  const pendingTag =
    app._pendingSelectRelease && app._pendingSelectRelease.projectId === project.id
      ? app._pendingSelectRelease.tag
      : null;
  if (app._pendingSelectRelease?.projectId === project.id) {
    app._pendingSelectRelease = null;
  }
  if (curSlug.includes("/")) {
    loadReleases(pendingTag);
  } else {
    paintReleasesSidebar();
  }

  container.querySelector("#mpNewReleaseBtn")?.addEventListener("click", () => {
    selectedReleaseKey = null;
    paintReleasesSidebar();
    paintRightPane();
  });
  container.querySelector("#mpLoadListBtn")?.addEventListener("click", () => loadReleases());
  container.querySelector("#mpRelFilter")?.addEventListener("input", (e) => {
    relFilter = e.target.value;
    paintReleasesSidebar();
  });
  container.querySelector("#mpProvider")?.addEventListener("change", () => {
    const picker = container.querySelector("#mpRepoPicker");
    if (picker) {
      picker.classList.add("hidden");
      picker.innerHTML = "";
    }
  });
  container.querySelector("#mpOpenVaultBtn")?.addEventListener("click", () => app.openPreferences("credentials"));
  container.querySelector("#mpToggleCredBtn")?.addEventListener("click", () => {
    container.querySelector("#mpCredPanel")?.classList.toggle("hidden");
  });

  container.querySelector("#mpVerifySaveCredBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "验证中…", async () => {
      const provider = container.querySelector("#mpProvider").value;
      const baseUrl = container.querySelector("#mpBaseUrl").value.trim();
      const label = container.querySelector("#mpNewLabel").value.trim();
      const token = container.querySelector("#mpNewToken").value.trim();
      if (!token) {
        toast("请先粘贴 Personal Access Token", "warn");
        container.querySelector("#mpNewToken")?.focus();
        return;
      }
      try {
        const meta = await API.verifyAndSaveRepoCredential({ provider, baseUrl, label, token });
        container.querySelector("#mpNewToken").value = "";
        toast(`令牌验证通过并已加密保存：${meta.username}`, "ok", 3000);
        await loadCreds(meta.id || "");
      } catch (err) {
        toast(`${err}`, "err", 4200);
      }
    })
  );

  container.querySelector("#mpPullReposBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "拉取中…", async () => {
      const { provider, baseUrl, credentialId } = getConnParams();
      try {
        const repos = await API.listRemoteRepos(provider, baseUrl, credentialId);
        const sel = container.querySelector("#mpRepoPicker");
        if (!repos.length) {
          toast("未查询到有写权限的仓库", "warn");
          return;
        }
        sel.innerHTML =
          `<option value="">— 选择要绑定的仓库 (${repos.length}) —</option>` +
          repos
            .map(
              (r) =>
                `<option value="${esc(r.fullName)}" data-clone="${esc(r.cloneUrl)}">${esc(r.fullName)}${r.private ? " [私有]" : ""}</option>`
            )
            .join("");
        sel.classList.remove("hidden");
        sel.onchange = () => {
          if (sel.value) container.querySelector("#mpRepo").value = sel.value;
        };
      } catch (err) {
        toast(`${err}`, "err", 4200);
      }
    })
  );

  container.querySelector("#mpBindRepoBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "绑定中…", async () => {
      const { provider, baseUrl, credentialId, repo: repoSlug } = getConnParams();
      if (!repoSlug || !repoSlug.includes("/")) {
        toast("请填写或拉取选择有效的仓库标识（owner/repo）", "warn");
        container.querySelector("#mpRepo")?.focus();
        return;
      }
      const picker = container.querySelector("#mpRepoPicker");
      const cloneUrl = picker?.selectedOptions?.[0]?.dataset?.clone || null;
      try {
        await API.bindProjectRepo({
          projectId: project.id,
          provider,
          baseUrl,
          repoSlug,
          credentialId,
          cloneUrl,
          initGitIfNeeded: true,
        });
        await app.reloadConfig(true);
        toast(`已绑定 ${providerDisplay(provider)} 仓库：${repoSlug}`, "ok");
      } catch (err) {
        toast(`${err}`, "err", 4200);
      }
    })
  );
}

function renderLocalServerTab(container, app, project, ov) {
  const draw = () => {
    const srv = app.serverStatus || { running: false, port: app.config.defaultPort || 18765, logs: [] };
    const ownedByOther = Boolean(srv.running && srv.projectId && srv.projectId !== project.id);
    const ownedByCurrent = Boolean(srv.running && (!srv.projectId || srv.projectId === project.id));
    const ownerLabel = srv.projectName || srv.projectId || "其他项目";
    const visibleLogs = ownedByOther ? [] : srv.logs || [];

    container.innerHTML = `
      <div class="grid-2">
        <section class="card">
          <div class="card-head">
            <h2>本机更新测试源 (127.0.0.1)</h2>
            <span class="tag ${ownedByCurrent ? "tag-ok" : ownedByOther ? "tag-warn" : ""}">${
              ownedByCurrent ? "运行中 · 当前项目" : ownedByOther ? `占用中 · ${esc(ownerLabel)}` : "已停止"
            }</span>
          </div>
          ${
            ownedByOther
              ? `<div class="notice notice-warn" style="margin-bottom:10px;font-size:12px">
                  本机更新测试源当前正在为项目 <strong>${esc(ownerLabel)}</strong> 提供服务（端口 <span class="mono">${srv.port}</span>）。
                  为防止将其他项目的 <span class="mono">latest.json</span> 误注入当前项目，已屏蔽跨项目联调启动；如需测试 <strong>${esc(project.name)}</strong>，请点击下方切换并重启服务。
                </div>`
              : ""
          }
          <div class="sf-group">
            <div class="sf-row">
              <div class="sf-row-main">
                <div class="sf-row-title">监听端口</div>
                <div class="sf-row-desc">仅绑定本机回环地址 127.0.0.1</div>
              </div>
              <div class="sf-row-ctrl">
                <input class="input mono" id="lsPort" type="number" min="1024" max="65535" style="width:100px" value="${srv.port || 18765}">
              </div>
            </div>
            <div class="sf-row">
              <div class="sf-row-main">
                <div class="sf-row-title">内存动态重写 latest.json</div>
                <div class="sf-row-desc">自动将下载链接指向本机端口，不污染磁盘文件</div>
              </div>
              <div class="sf-row-ctrl">
                <input class="sf-switch" type="checkbox" switch id="lsRewrite" ${srv.rewriteManifest !== false ? "checked" : ""}>
              </div>
            </div>
          </div>

          <div class="field-row">
            <button class="btn btn-primary" id="lsStartBtn" type="button">${
              ownedByOther ? "切换为当前项目重启服务" : ownedByCurrent ? "重启服务" : "启动服务"
            }</button>
            <button class="btn btn-danger" id="lsStopBtn" type="button" ${srv.running ? "" : "disabled"}>停止服务</button>
            ${
              ownedByCurrent && ov.launchExeExists
                ? `<button class="btn" id="lsLaunchTestBtn" type="button">带测试源启动程序</button>`
                : ""
            }
          </div>

          ${
            ownedByCurrent
              ? `<div class="status-ribbon">
                  <span class="ribbon-item mono">${esc(srv.feedUrl)}</span>
                  <button class="btn btn-xs" id="lsCopyFeedBtn" type="button">复制地址</button>
                  <span class="dim">命中 ${srv.hitCount || 0} 次</span>
                </div>`
              : ownedByOther
                ? `<div class="muted" style="font-size:12px">当前端口归属项目 <strong>${esc(ownerLabel)}</strong>，未向 <strong>${esc(project.name)}</strong> 暴露其清单地址与访问日志。</div>`
                : `<div class="muted" style="font-size:12px">用于在线上发布前，本地验证客户端的在线更新检测与安装包下载流程。</div>`
          }
        </section>

        <section class="card">
          <div class="card-head">
            <h2>访问日志</h2>
            <span class="dim mono">${visibleLogs.length} 条</span>
          </div>
          <div class="row-list">
            ${
              visibleLogs.length
                ? visibleLogs
                    .slice(-12)
                    .reverse()
                    .map(
                      (l) => `<div class="list-row">
                        <span class="mono" style="font-size:11.5px">${esc(l.method)} ${esc(l.path)}</span>
                        <span class="mono muted" style="font-size:11.5px">${l.status} · ${formatBytes(l.bytes)}</span>
                      </div>`
                    )
                    .join("")
                : `<div class="dim" style="padding:18px 0;text-align:center;font-size:12px">${
                    ownedByOther ? `当前服务属于 ${esc(ownerLabel)}，已隔离其访问日志` : "暂无请求"
                  }</div>`
            }
          </div>
        </section>
      </div>
    `;

    document.getElementById("lsStartBtn")?.addEventListener("click", (e) =>
      withBusy(e.currentTarget, "启动中…", async () => {
        try {
          const rawPort = Number(document.getElementById("lsPort").value);
          if (!Number.isInteger(rawPort) || rawPort < 1024 || rawPort > 65535) {
            toast("本地端口必须为 1024 ~ 65535 之间的整数", "warn", 3000);
            return;
          }
          const port = rawPort;
          const rewrite = document.getElementById("lsRewrite").checked;
          app.serverStatus = await API.startLocalServer(project.id, port, rewrite);
          app.updateChrome();
          toast(`本地更新源已启动：127.0.0.1:${port}`, "ok");
          draw();
        } catch (err) {
          toast(`${err}`, "err");
        }
      })
    );
    document.getElementById("lsStopBtn")?.addEventListener("click", (e) =>
      withBusy(e.currentTarget, "停止中…", async () => {
        try {
          app.serverStatus = await API.stopLocalServer();
          app.updateChrome();
          toast("本地更新源已停止", "ok");
          draw();
        } catch (err) {
          toast(`${err}`, "err");
        }
      })
    );
    document.getElementById("lsCopyFeedBtn")?.addEventListener("click", () => copyText(srv.feedUrl));
    document.getElementById("lsLaunchTestBtn")?.addEventListener("click", (e) =>
      withBusy(e.currentTarget, "启动中…", async () => {
        try {
          toast(await API.launchProjectExe(project.id, srv.feedUrl), "ok");
        } catch (err) {
          toast(`${err}`, "err");
        }
      })
    );
  };

  draw();
  const onSrv = () => {
    // Avoid clobbering port or rewrite switch while user is interacting with controls
    const activeId = document.activeElement?.id;
    if (activeId === "lsPort" || activeId === "lsRewrite") return;
    draw();
  };
  window.addEventListener("sf-server-updated", onSrv);
  return () => window.removeEventListener("sf-server-updated", onSrv);
}
