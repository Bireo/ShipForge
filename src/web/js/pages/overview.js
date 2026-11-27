import { API, esc, formatBytes, formatDurationText, isValidSemver } from "../api.js";
import { confirmDialog, toast, withBusy } from "../ui.js";

const PROVIDER_LABELS = {
  github: "GitHub",
  gitee: "Gitee",
  gitlab: "GitLab",
  gitea: "Gitea",
};

export function renderOverview(root, app, epoch) {
  const ov = app.overview;
  if (!ov) return;
  const { project, rootExists, versions, git, lock, artifacts, recentBuilds, healthIssues, changeReport } = ov;

  const buildActions = project.actions.filter((a) => a.category === "build" || a.category === "package");
  const defaultBuildId = buildActions[0]?.id || "";
  const activeArtifacts = artifacts.filter((a) => !a.isArchived);
  const bucket = changeReport?.bucket;
  const deployInfo = changeReport?.deployReadiness || null;

  const repoProv = project.repoBinding?.provider || "github";
  const repoSlug = project.repoBinding?.repoSlug || project.githubRepo || "";
  const repoBound = repoSlug.includes("/");
  const hasGitRemote = Boolean(git.isRepo && git.remoteUrl);
  const defaultDeployGitMode = hasGitRemote
    ? (deployInfo?.gitDirtyCount || 0) > 0
      ? "commit_tag_push"
      : "tag_push"
    : "none";

  root.innerHTML = `
    <div class="page-wrap">
      <div class="page-head">
        <div>
          <h1>${esc(project.name)}</h1>
          <div class="page-head-sub mono">${esc(project.root)}</div>
        </div>
        <div class="head-actions">
          ${
            ov.launchExeExists
              ? `<button class="btn" id="ovLaunchBtn" type="button" title="启动当前程序 (${formatBytes(ov.launchExeSize)})">启动程序</button>`
              : ""
          }
          <button class="btn" id="ovOpenDirBtn" type="button">打开目录</button>
        </div>
      </div>

      <!-- 4-Cell Hairline Status Strip -->
      <div class="sf-status-strip">
        <button class="sf-status-cell" id="ovKpiVer" type="button">
          <span class="sf-status-label"><span class="dot ${versions.synced ? "dot-ok" : "dot-warn"}"></span>当前版本</span>
          <span class="sf-status-val mono">v${esc(versions.current)}</span>
          <span class="sf-status-sub">${bucket?.sealed ? "已封版" : versions.synced ? "配置文件已同步" : "版本号未对齐 · 点击修复"}</span>
        </button>

        <button class="sf-status-cell" id="ovKpiPreset" type="button">
          <span class="sf-status-label"><span class="dot dot-ok"></span>编译与环境</span>
          <span class="sf-status-val">${esc(project.adapterLabel || project.kind)}</span>
          <span class="sf-status-sub">点击套用模板或自检补全依赖</span>
        </button>

        <button class="sf-status-cell" id="ovKpiRepo" type="button">
          <span class="sf-status-label"><span class="dot ${repoBound ? "dot-ok" : "dot-warn"}"></span>发布目标仓库</span>
          <span class="sf-status-val">${repoBound ? esc(repoSlug) : "尚未绑定"}</span>
          <span class="sf-status-sub">${repoBound ? `${esc(PROVIDER_LABELS[repoProv] || repoProv)} · 点击管理或上传` : "点击快速绑定我的仓库"}</span>
        </button>

        <button class="sf-status-cell" id="ovKpiGit" type="button" title="${(() => {
          const items = changeReport?.classifiedChanges || [];
          if (items.length) {
            return esc(items.map((c) => `[${c.category}] ${c.status} ${c.path}`).join("\n"));
          }
          if (git.dirtyCount) {
            return esc((git.changes || []).map((c) => `${c.status} ${c.path}`).join("\n"));
          }
          if (git.aheadCount) return `本地已有 ${git.aheadCount} 个提交等待推送到远端`;
          return changeReport?.scanMode === "filesystem" ? "无 Git 仓库 · 已启用本地文件指纹扫描" : "";
        })()}">
          <span class="sf-status-label"><span class="dot ${(() => {
            const effDirty = changeReport?.dirtyCount ?? git.dirtyCount ?? 0;
            if (effDirty > 0) return "dot-warn";
            if (deployInfo?.status === "ready_to_deploy") return "dot-info";
            if (!git.isRepo) return "dot-ok";
            if (git.aheadCount) return "dot-info";
            return "dot-ok";
          })()}"></span>${git.isRepo ? "Git 与产物" : "文件扫描与产物"}</span>
          <span class="sf-status-val">${git.isRepo ? `${esc(git.branch || "初始分支")} · ${activeArtifacts.length} 个产物` : `${activeArtifacts.length} 个本地产物`}</span>
          <span class="sf-status-sub">${(() => {
            const effDirty = changeReport?.dirtyCount ?? git.dirtyCount ?? 0;
            const summary = changeReport?.changeSummary || "";
            if (deployInfo?.status === "ready_to_deploy" && effDirty === 0) {
              return `v${esc(deployInfo.version || versions.current)} 已打包就绪 · 待部署推送`;
            }
            if (!git.isRepo) {
              if (effDirty > 0) {
                return `探测 ${effDirty} 项变动${summary ? `（${esc(summary)}）` : ""}`;
              }
              return "本地文件指纹监控中 · 未启用 Git";
            }
            if (effDirty > 0) {
              const firstPath = changeReport?.dirtyFiles?.[0] || git.changes?.[0]?.path || "";
              const leaf = firstPath.split(/[\\/]/).filter(Boolean).pop() || firstPath;
              const hint = summary
                ? ` (${esc(summary)})`
                : leaf
                  ? ` (${esc(leaf)}${effDirty > 1 ? " 等" : ""})`
                  : "";
              return `${effDirty} 个未提交${hint}${git.aheadCount ? ` · ↑${git.aheadCount}` : ""}`;
            }
            if (git.aheadCount) return `已全部提交 · ↑${git.aheadCount} 待推送`;
            return "工作区已同步";
          })()}</span>
        </button>
      </div>

      ${
        healthIssues.length
          ? `<div class="notice ${rootExists ? "notice-warn" : "notice-err"}">
              <div>${healthIssues.map(esc).join("；")}</div>
              <div class="field-row">
                ${changeReport?.consecutiveWarning ? `<button class="btn btn-xs btn-primary" id="ovIsolateBumpBtn" type="button">递增小版本</button>` : ""}
                ${lock?.exists && !lock.alive ? `<button class="btn btn-xs" id="ovCleanLockBtn" type="button">清理死锁</button>` : ""}
                ${!versions.synced && rootExists ? `<button class="btn btn-xs" id="ovFixVerBtn" type="button">去对齐版本</button>` : ""}
              </div>
            </div>`
          : ""
      }

      <!-- Fullscreen-Adaptive Workbench Grid (Stacked < 1320px, 2-Column Workbench >= 1320px) -->
      <div class="ov-workbench-grid">
      <!-- Grouped-List One-Click Release with Live previewBump Preview -->
      <div class="ov-pipeline-col">
        ${
          deployInfo && (deployInfo.status === "ready_to_deploy" || deployInfo.status === "partial_artifacts")
            ? `
        <div class="sf-section-title" id="ovDeployHead">
          <span>部署与推送（免重编）</span>
          <span class="tag mono ${deployInfo.status === "ready_to_deploy" ? "tag-ok" : "tag-warn"}">${
            deployInfo.status === "ready_to_deploy"
              ? `v${esc(deployInfo.version)} 成品就绪 · ${formatBytes(deployInfo.artifactTotalBytes)}`
              : `v${esc(deployInfo.version)} 存在待处理项`
          }</span>
        </div>
        <div class="sf-group" id="ovDeployCard" style="margin-bottom:14px">
          <div class="sf-row" style="flex-direction:column;align-items:stretch;gap:8px">
            <div class="field-row" style="justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap">
              <div class="sf-row-title">${esc(deployInfo.headline)}</div>
              <div class="field-row" style="gap:6px;flex-wrap:wrap">
                ${(deployInfo.artifactFiles || [])
                  .map((f) => `<span class="tag mono">${esc(f)}</span>`)
                  .join("")}
              </div>
            </div>
            <div class="sf-row-desc">${esc(deployInfo.detail)}</div>
            ${
              deployInfo.pendingActions?.length
                ? `<div class="field-row" style="gap:6px;flex-wrap:wrap">${deployInfo.pendingActions
                    .map((a) => `<span class="tag tag-accent">${esc(a)}</span>`)
                    .join("")}</div>`
                : ""
            }
            ${
              deployInfo.blockers?.length
                ? `<div class="field-row" style="gap:6px;flex-wrap:wrap">${deployInfo.blockers
                    .map((b) => `<span class="tag tag-warn">⚠ ${esc(b)}</span>`)
                    .join("")}</div>`
                : ""
            }
            <div class="field-row" style="gap:6px;align-items:center;font-size:12px" id="dpNotesPreviewRow">
              <span class="tag" id="dpNotesSource">读取 v${esc(deployInfo.version)} 日志…</span>
              <span class="dim" id="dpNotesSnippet" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1"></span>
            </div>
          </div>

          <div class="sf-row">
            <div class="sf-row-main">
              <div class="sf-row-title">部署推送方式</div>
              <div class="sf-row-desc">直接核验现有成品哈希与清单，无需重新编译；自动复用下方更新内容</div>
            </div>
            <div class="sf-row-ctrl" style="display:flex;gap:8px;flex-wrap:wrap;justify-content:flex-end">
              <select class="select" id="dpGitMode" style="min-width:170px" ${git.isRepo ? "" : "disabled"}>
                <option value="none" ${defaultDeployGitMode === "none" ? "selected" : ""}>不操作 Git</option>
                <option value="tag_push" ${defaultDeployGitMode === "tag_push" ? "selected" : ""}>打标 v${esc(versions.current)} 并推送远端</option>
                <option value="commit_tag_push" ${defaultDeployGitMode === "commit_tag_push" ? "selected" : ""}>先提交改动 + 打标并推送</option>
              </select>
              <select class="select" id="dpChannel" style="min-width:132px" title="本次免重编部署的发布通道（独立于下方一键构建通道，避免误推测试版）">
                <option value="stable" ${String(versions.current || "").split("+")[0].includes("-") ? "" : "selected"}>正式版 (Stable)</option>
                <option value="beta" ${String(versions.current || "").split("+")[0].includes("-") ? "selected" : ""}>测试版 (Beta)</option>
              </select>
              <select class="select" id="dpReleaseMode" style="min-width:185px">
                <option value="release" ${repoBound || !git.isRepo ? "selected" : ""}>${
                  repoBound
                    ? `发布至 ${esc(PROVIDER_LABELS[repoProv] || repoProv)} (${esc(repoSlug)})`
                    : "发布至目标仓库"
                }</option>
                <option value="draft">上传并存为草稿 (Draft)</option>
                ${
                  git.isRepo
                    ? `<option value="git_only" ${repoBound ? "" : "selected"}>仅推送 Git（不上传 Release）</option>`
                    : ""
                }
              </select>
            </div>
          </div>

          <div class="sf-row" style="background:var(--canvas);justify-content:space-between;align-items:center;gap:10px;flex-wrap:wrap">
            <label class="sf-switch-label" style="font-size:12px">
              <input class="sf-switch" type="checkbox" switch id="dpEnsureManifest" checked>
              <span>自动补齐/校验 latest.json 与 SHA256SUMS.txt</span>
            </label>
            <div class="field-row" style="gap:8px">
              ${
                !repoBound && !hasGitRemote
                  ? `<button class="btn btn-soft" id="ovDeployBindRepoBtn" type="button">去绑定发布仓库 →</button>`
                  : ""
              }
              ${
                deployInfo.sourceNewerThanArtifacts
                  ? `<label class="sf-switch-label" style="font-size:11.5px"><input class="sf-switch" type="checkbox" switch id="dpForceStale"><span>忽略源码较新强推</span></label>`
                  : ""
              }
              <button class="btn btn-primary" id="ovQuickDeployBtn" type="button" ${
                !rootExists ||
                (!repoBound && !hasGitRemote) ||
                (deployInfo.status !== "ready_to_deploy" && !deployInfo.artifactsReady)
                  ? "disabled"
                  : ""
              }>立即部署推送（免重编）</button>
            </div>
          </div>
        </div>`
            : ""
        }

        <div class="sf-section-title">
          <span>一键构建并发布</span>
          <span class="tag mono tag-accent" id="plVerPreview">v${esc(versions.current)} → 计算中…</span>
        </div>

        <div class="sf-group">
          <div class="sf-row">
            <div class="sf-row-main">
              <div class="sf-row-title">版本号变化</div>
              <div class="sf-row-desc">原子更新所有已识别配置文件；中断或失败时自动回滚并弹出健康报告</div>
            </div>
            <div class="sf-row-ctrl">
              ${(() => {
                // Only default to keeping current version when ALL hold:
                // 1) changeReport.bucket exists, 2) buildCount === 0, 3) !sealed,
                // 4) current version is valid (!= "0.0.0") AND no active artifact already exists for it!
                // Prevents both accidental double-bump after "递增小版本" AND accidental overwrite of newly imported releases.
                const hasCurrentVerArtifact = (ov.artifacts || []).some(
                  (a) => !a.isArchived && a.version && a.version === versions.current
                );
                const isFreshUnbuilt =
                  Boolean(changeReport?.bucket) &&
                  Boolean(versions.current) &&
                  versions.current !== "0.0.0" &&
                  (changeReport.bucket.buildCount ?? 0) === 0 &&
                  !changeReport.bucket.sealed &&
                  !hasCurrentVerArtifact;
                return `<select class="select ov-ctrl-select" id="plBump">
                  <option value="none" ${isFreshUnbuilt ? "selected" : ""}>保持当前 v${esc(versions.current)}${isFreshUnbuilt ? "（尚未构建）" : ""}</option>
                  <option value="patch" ${isFreshUnbuilt ? "" : "selected"}>自动 +0.0.1（修订修复推荐）</option>
                  <option value="minor">自动 +0.1.0（新功能发布）</option>
                  <option value="major">自动 +1.0.0（重大升级）</option>
                  <option value="beta">生成测试版 (-beta.N)</option>
                  <option value="custom">手动指定版本号…</option>
                </select>`;
              })()}
            </div>
          </div>

          <div class="sf-row hidden" id="plCustomVerRow">
            <div class="sf-row-main">
              <div class="sf-row-title">手动目标版本号</div>
              <div class="sf-row-desc">遵循语义化版本规范，例如 1.4.0 或 1.4.0-beta.1</div>
            </div>
            <div class="sf-row-ctrl">
              <input class="input mono ov-ctrl-select" id="plCustomVer" placeholder="例如 1.4.0" value="${esc(versions.current)}">
            </div>
          </div>

          <div class="sf-row">
            <div class="sf-row-main">
              <div class="sf-row-title">产物发布去向</div>
              <div class="sf-row-desc">选择仅保留在本地 release/ 目录或直接流式上传至已绑定仓库</div>
            </div>
            <div class="sf-row-ctrl">
              <select class="select ov-ctrl-select" id="plTarget">
                <option value="local" selected>仅保存在本地 release/ 目录</option>
                <option value="remote">${
                  repoBound
                    ? `直接发布至 ${esc(PROVIDER_LABELS[repoProv] || repoProv)} (${esc(repoSlug)})`
                    : "直接发布至已绑定仓库"
                }</option>
                <option value="remote-draft">上传并存为草稿 (Draft)</option>
              </select>
            </div>
          </div>

          <div class="sf-row">
            <div class="sf-row-main">
              <div class="sf-row-title">源码 Git 动作</div>
              <div class="sf-row-desc">构建成功后自动提交源码、创建版本 Tag 并按需推送远端</div>
            </div>
            <div class="sf-row-ctrl">
              <select class="select ov-ctrl-select" id="plGitMode" ${git.isRepo ? "" : "disabled"}>
                <option value="none" ${git.isRepo ? "" : "selected"}>不操作 Git</option>
                <option value="commit_tag" ${git.isRepo ? "selected" : ""}>本地提交源码并打版本号标签</option>
                <option value="commit_tag_push">提交源码 + 打标签 + 推送远端</option>
              </select>
            </div>
          </div>

          <div class="sf-row" style="flex-direction:column;align-items:stretch;gap:8px">
            <div class="field-row" style="justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap">
              <div class="field-row" style="gap:8px;align-items:center">
                <span class="sf-row-title">版本更新内容</span>
                <span class="tag" id="plNotesSource">自动读取中…</span>
              </div>
              <div class="field-row" style="gap:6px">
                <button class="btn btn-xs btn-ghost hidden" id="plNotesUseLatestBtn" type="button" title="载入变更记录中最近一条有效版本的说明作为参考并编辑">载入最近日志</button>
                <button class="btn btn-xs btn-ghost" id="plNotesSaveClBtn" type="button" title="将当前更新内容同步写入项目的变更记录文档（自动适配升序/降序，不重复标题）">写入变更记录</button>
                <button class="btn btn-xs btn-ghost" id="plNotesReloadBtn" type="button" title="从变更记录 / README / Git 提交重新自动读取">重新读取</button>
                <button class="btn btn-xs btn-ghost" id="plNotesClearBtn" type="button" title="清空内容以便自行填写">清空</button>
              </div>
            </div>
            <textarea class="textarea mono" id="plNotes" style="min-height:88px;max-height:220px" placeholder="已自动读取本版本更新日志；也可点「清空」自行撰写，留空将在发布时自动提取"></textarea>
          </div>

          <div class="sf-row" style="flex-direction:column;align-items:stretch;gap:10px">
            <details class="disclosure" style="border-top:none;padding-top:0">
              <summary>更多发版选项（发布通道、指定构建工序、测试与清单开关、提交说明）</summary>
              <div class="grid-3" style="margin-top:12px">
                <div class="field">
                  <label for="plChannel">发布通道</label>
                  <select class="select" id="plChannel">
                    <option value="stable">正式版 (Stable)</option>
                    <option value="beta">预发布测试版 (Beta)</option>
                  </select>
                </div>
                <div class="field">
                  <label for="plBuildAct">构建模板工序</label>
                  <select class="select" id="plBuildAct">
                    ${
                      buildActions.length
                        ? buildActions
                            .map(
                              (a) =>
                                `<option value="${esc(a.id)}" ${a.id === defaultBuildId ? "selected" : ""}>${esc(a.label)}</option>`
                            )
                            .join("")
                        : `<option value="">（请先在「编译与环境」套用框架模板）</option>`
                    }
                  </select>
                </div>
                <div class="field">
                  <label for="plCommitMsg">Git 提交说明（留空自动生成）</label>
                  <input class="input" id="plCommitMsg" placeholder="chore(release): vX.Y.Z">
                </div>
              </div>
              <div class="field-row" style="margin-top:12px;gap:16px">
                <label class="sf-switch-label"><input class="sf-switch" type="checkbox" switch id="plRunTests" checked><span>构建前运行测试</span></label>
                <label class="sf-switch-label"><input class="sf-switch" type="checkbox" switch id="plRunBuild" checked><span>执行编译打包</span></label>
                <label class="sf-switch-label"><input class="sf-switch" type="checkbox" switch id="plGenManifest" checked><span>生成更新清单与校验文件</span></label>
              </div>
            </details>
          </div>

          <div class="sf-row" style="background:var(--canvas);justify-content:flex-end">
            <div class="pipe-steps" id="ovPipeSteps" style="flex:1"></div>
            <div class="sf-row-ctrl">
              <button class="btn btn-accent" id="ovRunPipelineBtn" type="button" ${rootExists ? "" : "disabled"}>开始构建并发布</button>
            </div>
          </div>
        </div>
      </div>

      <!-- Common Actions + Recent Records -->
      <div class="ov-side-col">
        <section class="card">
          <div class="card-head">
            <h2>常用工序</h2>
            <button class="btn btn-xs btn-ghost" id="ovGoBuildBtn" type="button">编译与环境 →</button>
          </div>
          <div class="chip-grid">
            ${
              project.actions.length
                ? project.actions
                    .slice(0, 8)
                    .map(
                      (a) =>
                        `<button class="btn ${a.dangerous ? "btn-danger" : ""}" data-run-action="${esc(a.id)}" type="button" title="${esc(a.description)}" ${rootExists ? "" : "disabled"}>${esc(a.label)}</button>`
                    )
                    .join("")
                : `<div class="muted" style="font-size:12.5px">尚未套用构建模板，前往「编译与环境」一键适配。</div>`
            }
          </div>
        </section>

        <section class="card">
          <div class="card-head">
            <h2>近期记录</h2>
            <div class="field-row" style="gap:4px">
              <button class="btn btn-xs btn-ghost" id="ovGoReleaseBtn" type="button">产物 (${activeArtifacts.length})</button>
              <button class="btn btn-xs btn-ghost" id="ovGoJournalBtn" type="button">全部台账 →</button>
            </div>
          </div>
          ${
            recentBuilds.length
              ? `<div class="row-list">
                  ${recentBuilds
                    .slice(0, 8)
                    .map(
                      (b) => `<div class="list-row">
                        <div class="list-row-main">
                          <div class="list-row-title">
                            <span class="tag ${b.result === "ok" ? "tag-ok" : b.result === "cancelled" ? "tag-warn" : "tag-err"}">${esc(b.result)}</span>
                            <span class="mono">v${esc(b.version)}</span>
                            <span class="muted">${esc(b.action)}</span>
                          </div>
                        </div>
                        <div class="dim mono" style="font-size:11.5px;white-space:nowrap">${esc(formatDurationText(b.duration))}</div>
                      </div>`
                    )
                    .join("")}
                </div>`
              : `<div class="dim" style="padding:18px 0;text-align:center;font-size:12.5px">暂无构建记录</div>`
          }
        </section>
      </div>
      </div>
    </div>
  `;

  app.renderPipelineSteps();

  // Live next-version preview using API.previewBump
  const plBump = document.getElementById("plBump");
  const customRow = document.getElementById("plCustomVerRow");
  const customInp = document.getElementById("plCustomVer");
  const previewEl = document.getElementById("plVerPreview");
  const plNotesEl = document.getElementById("plNotes");
  const plNotesSourceEl = document.getElementById("plNotesSource");

  if (!app._plNotesState) app._plNotesState = {};
  if (!app._ovFormTouchedByProj) app._ovFormTouchedByProj = new Set();
  app._ovFormTouchedByProj.delete(project.id);
  const markOvFormTouched = () => app._ovFormTouchedByProj.add(project.id);
  for (const id of [
    "plBump",
    "plCustomVer",
    "plTarget",
    "plGitMode",
    "plChannel",
    "plBuildAct",
    "plCommitMsg",
    "plRunTests",
    "plRunBuild",
    "plGenManifest",
    "dpGitMode",
    "dpChannel",
    "dpReleaseMode",
    "dpEnsureManifest",
    "dpForceStale",
  ]) {
    const el = document.getElementById(id);
    if (el) {
      el.addEventListener("change", markOvFormTouched);
      if (el.tagName === "INPUT" && el.type !== "checkbox") {
        el.addEventListener("input", markOvFormTouched);
      }
    }
  }
  const savedPlNotes = app._plNotesState[project.id] || null;

  let previewSeq = 0;
  let notesSeq = 0;
  let plNotesUserTouched = Boolean(savedPlNotes?.userTouched);
  let plNotesIsFallback = Boolean(savedPlNotes?.isFallback);
  let effectiveTargetVer = versions.current;

  const setPlNotesBadge = (text, cls = "") => {
    if (!plNotesSourceEl) return;
    plNotesSourceEl.className = `tag ${cls}`.trim();
    plNotesSourceEl.textContent = text;
  };

  const persistPlNotesMemory = () => {
    app._plNotesState[project.id] = {
      text: plNotesEl?.value || "",
      userTouched: plNotesUserTouched,
      isFallback: plNotesIsFallback,
    };
  };

  if (plNotesUserTouched && plNotesEl) {
    plNotesEl.value = savedPlNotes.text || "";
    const hasText = !!plNotesEl.value.trim();
    setPlNotesBadge(hasText ? "手动编辑" : "已清空（可自行填写）", hasText ? "tag-accent" : "");
  }

  const plNotesUseLatestBtn = document.getElementById("plNotesUseLatestBtn");
  let latestChangelogRef = null;
  const syncLatestClBtn = (ver, notes) => {
    latestChangelogRef = ver && notes ? { ver, notes } : null;
    if (!plNotesUseLatestBtn) return;
    if (latestChangelogRef) {
      plNotesUseLatestBtn.textContent = `载入 v${latestChangelogRef.ver} 日志`;
      plNotesUseLatestBtn.classList.remove("hidden");
    } else {
      plNotesUseLatestBtn.classList.add("hidden");
    }
  };

  const loadPipelineNotes = async (ver, force = false) => {
    if (!plNotesEl || !ver) return;
    if (!force && plNotesUserTouched) return;
    const seq = ++notesSeq;
    setPlNotesBadge("自动读取中…");
    syncLatestClBtn(null, null);
    try {
      const res = await API.resolveReleaseNotesDetail(project.id, ver);
      if (seq !== notesSeq || epoch !== app.renderEpoch || !plNotesEl.isConnected) return;
      if (!force && plNotesUserTouched) return;
      plNotesEl.value = res?.notes || "";
      plNotesIsFallback = Boolean(res?.isFallback);
      setPlNotesBadge(
        res?.sourceLabel || "自动读取",
        res?.isFallback ? "tag-warn" : "tag-ok"
      );
      if (res?.isFallback && res?.latestChangelogVersion && res?.latestChangelogNotes) {
        syncLatestClBtn(res.latestChangelogVersion, res.latestChangelogNotes);
      }
      persistPlNotesMemory();
    } catch {
      if (seq === notesSeq && plNotesEl.isConnected) {
        setPlNotesBadge("读取失败，可手动填写", "tag-warn");
      }
    }
  };

  const plChannelEl = document.getElementById("plChannel");
  let plChannelUserTouched = false;
  let plChannelTouchedVer = null;
  plChannelEl?.addEventListener("change", () => {
    // 手动选择只对「当前这个目标版本」有效，换版本后必须重新跟随版本号，防止旧的 beta 选择残留把正式版推成测试版
    plChannelUserTouched = true;
    plChannelTouchedVer = String(effectiveTargetVer || "");
  });
  const channelRiskOk = async (ver, ch) => {
    const pre = String(ver || "").split("+")[0].includes("-");
    if (ch === "beta" && !pre) {
      return confirmDialog({
        title: "通道与版本号不一致",
        message: `v${ver} 是正式版本号，但发布通道选了「测试版」。将作为预发布推送，正式版用户不会收到这次更新。`,
        confirmText: "仍按测试版发布",
        danger: true,
      });
    }
    return true;
  };

  const syncDeployNotesPreviewFromEditor = () => {
    if (!deployInfo || effectiveTargetVer !== versions.current) return;
    const dpSrcEl = document.getElementById("dpNotesSource");
    const dpSnipEl = document.getElementById("dpNotesSnippet");
    if (!dpSrcEl) return;
    const txt = (plNotesEl?.value || "").trim();
    if (plNotesUserTouched) {
      dpSrcEl.className = `tag ${txt ? "tag-accent" : ""}`.trim();
      dpSrcEl.textContent = txt ? "日志来源：下方已编辑内容" : "日志来源：留空（部署时自动提取）";
      if (dpSnipEl) {
        const firstLine = txt.split(/\r?\n/).map((l) => l.trim()).find(Boolean) || "";
        dpSnipEl.textContent = firstLine.slice(0, 90);
        dpSnipEl.title = txt;
      }
    }
  };

  plNotesEl?.addEventListener("input", () => {
    plNotesUserTouched = true;
    plNotesIsFallback = false;
    const hasText = !!plNotesEl.value.trim();
    setPlNotesBadge(hasText ? "手动编辑" : "已清空（可自行填写）", hasText ? "tag-accent" : "");
    persistPlNotesMemory();
    syncDeployNotesPreviewFromEditor();
  });
  document.getElementById("plNotesClearBtn")?.addEventListener("click", () => {
    if (!plNotesEl) return;
    plNotesUserTouched = true;
    plNotesIsFallback = false;
    plNotesEl.value = "";
    setPlNotesBadge("已清空（可自行填写）");
    persistPlNotesMemory();
    syncDeployNotesPreviewFromEditor();
    plNotesEl.focus();
  });
  plNotesUseLatestBtn?.addEventListener("click", () => {
    if (!plNotesEl || !latestChangelogRef) return;
    plNotesUserTouched = true;
    plNotesIsFallback = false;
    plNotesEl.value = latestChangelogRef.notes;
    setPlNotesBadge(`已载入 v${latestChangelogRef.ver}（请按需修改）`, "tag-accent");
    syncLatestClBtn(null, null);
    persistPlNotesMemory();
    syncDeployNotesPreviewFromEditor();
    plNotesEl.focus();
  });
  document.getElementById("plNotesSaveClBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "写入中…", async () => {
      const body = plNotesEl?.value.trim() || "";
      if (!body || (!plNotesUserTouched && plNotesIsFallback)) {
        toast("当前仅为默认兜底文案，请先补充实质更新内容后再写入变更记录", "warn", 3400);
        plNotesEl?.focus();
        return;
      }
      try {
        const writtenPath = await API.upsertChangelogEntry(project.id, effectiveTargetVer, body);
        plNotesIsFallback = false;
        plNotesUserTouched = false;
        delete app._plNotesState[project.id];
        syncLatestClBtn(null, null);
        setPlNotesBadge(`已同步写入 (${writtenPath})`, "tag-ok");
        toast(`已将 v${effectiveTargetVer} 更新内容同步写入 ${writtenPath}`, "ok");
      } catch (err) {
        toast(`${err}`, "err", 4200);
      }
    })
  );
  document.getElementById("plNotesReloadBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "读取中…", async () => {
      plNotesUserTouched = false;
      plNotesIsFallback = false;
      delete app._plNotesState[project.id];
      await loadPipelineNotes(effectiveTargetVer, true);
    })
  );

  const syncChannelForVersion = (ver, bumpKind) => {
    if (!plChannelEl) return;
    if (plChannelUserTouched && plChannelTouchedVer === String(ver || "")) return;
    plChannelUserTouched = false;
    const isPre = bumpKind === "beta" || String(ver || "").includes("-");
    plChannelEl.value = isPre ? "beta" : "stable";
  };

  const updateVersionPreview = async () => {
    if (!plBump || !previewEl) return;
    const seq = ++previewSeq;
    const kind = plBump.value;
    customRow?.classList.toggle("hidden", kind !== "custom");

    if (kind === "none") {
      previewEl.className = "tag mono";
      previewEl.textContent = `保持 v${versions.current}`;
      effectiveTargetVer = versions.current;
      syncChannelForVersion(effectiveTargetVer, kind);
      loadPipelineNotes(effectiveTargetVer, false);
      return;
    }
    if (kind === "custom") {
      const val = (customInp?.value || "").trim();
      const valid = isValidSemver(val);
      previewEl.className = `tag mono ${valid ? "tag-accent" : "tag-err"}`;
      previewEl.textContent = valid ? `v${versions.current} → v${val}` : "版本号格式无效";
      if (valid) {
        effectiveTargetVer = val;
        syncChannelForVersion(effectiveTargetVer, kind);
        loadPipelineNotes(effectiveTargetVer, false);
      }
      return;
    }
    try {
      const next = await API.previewBump(versions.current, kind);
      if (seq !== previewSeq || epoch !== app.renderEpoch) return;
      previewEl.className = "tag mono tag-accent";
      previewEl.textContent = `v${versions.current} → v${next}`;
      effectiveTargetVer = next;
      syncChannelForVersion(effectiveTargetVer, kind);
      loadPipelineNotes(effectiveTargetVer, false);
    } catch {
      if (seq === previewSeq) {
        previewEl.className = "tag mono";
        previewEl.textContent = `v${versions.current}`;
        effectiveTargetVer = versions.current;
        syncChannelForVersion(effectiveTargetVer, kind);
        loadPipelineNotes(effectiveTargetVer, false);
      }
    }
  };
  plBump?.addEventListener("change", updateVersionPreview);
  customInp?.addEventListener("input", updateVersionPreview);
  updateVersionPreview();

  document.getElementById("ovOpenDirBtn")?.addEventListener("click", () =>
    API.openInExplorer(project.root).catch((e) => toast(`${e}`, "err"))
  );
  document.getElementById("ovLaunchBtn")?.addEventListener("click", async () => {
    try {
      toast(await API.launchProjectExe(project.id), "ok");
    } catch (e) {
      toast(`${e}`, "err");
    }
  });

  // Status strip navigation
  document.getElementById("ovKpiVer")?.addEventListener("click", () => app.navigate("version", "sync"));
  document.getElementById("ovKpiPreset")?.addEventListener("click", () => app.navigate("compile", "presets"));
  document.getElementById("ovKpiRepo")?.addEventListener("click", () => app.navigate("release", "repos"));
  document.getElementById("ovKpiGit")?.addEventListener("click", () => app.navigate("version", "git"));
  document.getElementById("ovGoBuildBtn")?.addEventListener("click", () => app.navigate("compile", "actions"));
  document.getElementById("ovGoReleaseBtn")?.addEventListener("click", () => app.navigate("release", "artifacts"));
  document.getElementById("ovGoJournalBtn")?.addEventListener("click", () => app.navigate("version", "journal"));
  document.getElementById("ovFixVerBtn")?.addEventListener("click", () => app.navigate("version", "sync"));

  document.getElementById("ovIsolateBumpBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "递增中…", async () => {
      if (app.isProjectBusy(project.id)) {
        toast("当前项目正在执行任务，请等待完成后再修改版本", "warn", 3200);
        return;
      }
      try {
        const nextVer = await API.previewBump(versions.current, "patch");
        const ok = await confirmDialog({
          title: "递增版本号",
          message: `将版本号从 v${versions.current} 更新为 v${nextVer}？`,
          confirmText: "确认更新",
        });
        if (!ok) return;
        await API.applyVersion(project.id, nextVer);
        toast(`已更新至 v${nextVer}`, "ok");
        await app.refreshOverview(true);
      } catch (err) {
        toast(`${err}`, "err");
      }
    })
  );

  document.getElementById("ovCleanLockBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "清理中…", async () => {
      try {
        toast(await API.cleanStaleLock(project.id), "ok");
        await app.refreshOverview(true);
      } catch (err) {
        toast(`${err}`, "err");
      }
    })
  );

  root.querySelectorAll("[data-run-action]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const act = project.actions.find((a) => a.id === btn.dataset.runAction);
      if (!act) return;
      if (act.dangerous) {
        const ok = await confirmDialog({
          title: `确认执行：${act.label}`,
          message: act.description || "该操作会清理缓存或修改已有产物，确认继续？",
          detail: `${act.command} ${act.args.join(" ")}`,
          confirmText: "继续执行",
          danger: true,
        });
        if (!ok) return;
      }
      app.runAction(act, {});
    });
  });

  const syncBusyButtons = () => {
    const busy = app.isProjectBusy(project.id);
    const runBtn = document.getElementById("ovRunPipelineBtn");
    if (runBtn && !runBtn.dataset.busyLock) runBtn.disabled = !rootExists || busy;
    const depBtn = document.getElementById("ovQuickDeployBtn");
    if (depBtn && !depBtn.dataset.busyLock) {
      depBtn.disabled = !rootExists || busy || (!repoBound && !hasGitRemote) || !deployInfo?.artifactsReady;
    }
    const isoBtn = document.getElementById("ovIsolateBumpBtn");
    if (isoBtn && !isoBtn.dataset.busyLock) isoBtn.disabled = busy;
    root.querySelectorAll("[data-run-action]").forEach((btn) => {
      if (!btn.dataset.busyLock) btn.disabled = !rootExists || busy;
    });
  };
  syncBusyButtons();
  const onTaskState = () => syncBusyButtons();
  window.addEventListener("sf-task-state-changed", onTaskState);

  if (deployInfo && (deployInfo.status === "ready_to_deploy" || deployInfo.status === "partial_artifacts")) {
    const dpSrcEl = document.getElementById("dpNotesSource");
    const dpSnipEl = document.getElementById("dpNotesSnippet");
    API.resolveReleaseNotesDetail(project.id, deployInfo.version || versions.current)
      .then((res) => {
        if (epoch !== app.renderEpoch || !dpSrcEl?.isConnected) return;
        dpSrcEl.className = `tag ${res?.isFallback ? "tag-warn" : "tag-ok"}`;
        dpSrcEl.textContent = `日志来源：${res?.sourceLabel || "自动读取"}`;
        if (dpSnipEl) {
          const firstLine = (res?.notes || "")
            .split(/\r?\n/)
            .map((l) => l.trim())
            .find((l) => l.length > 0) || "";
          dpSnipEl.textContent = firstLine.slice(0, 90);
          dpSnipEl.title = res?.notes || "";
        }
      })
      .catch(() => {});
  }

  document.getElementById("ovDeployBindRepoBtn")?.addEventListener("click", () => {
    app.navigate("release", "repos");
  });

  document.getElementById("ovQuickDeployBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "部署中…", async () => {
      if (app.isProjectBusy(project.id)) {
        toast("当前项目已有任务运行中", "warn");
        return;
      }
      const dpGitMode = document.getElementById("dpGitMode")?.value || "none";
      const dpRelMode = document.getElementById("dpReleaseMode")?.value || "release";
      const uploadRelease = dpRelMode === "release" || dpRelMode === "draft";
      const ensureManifest = Boolean(document.getElementById("dpEnsureManifest")?.checked);
      const forceStaleSource = Boolean(document.getElementById("dpForceStale")?.checked);

      if (uploadRelease && !repoBound) {
        toast("请先绑定目标发布仓库后再上传", "warn", 3200);
        app.navigate("release", "repos");
        return;
      }
      if (dpGitMode !== "none" && !git.remoteUrl) {
        toast("本地尚未绑定远端仓库，请先去「产物与仓库」一键绑定", "warn", 3800);
        return;
      }
      if (deployInfo?.sourceNewerThanArtifacts && !forceStaleSource) {
        toast("检测到源码/子引擎比已有产物更新，请先构建或勾选「忽略源码较新强推」", "warn", 4200);
        return;
      }
      if (!uploadRelease && dpGitMode === "none" && !ensureManifest) {
        toast("请至少选择一项部署动作（推送 Git、上传 Release 或更新清单）", "warn");
        return;
      }

      const targetVer = versions.current;
      const isPreTarget = String(targetVer || "").split("+")[0].includes("-");
      const dpChannel = isPreTarget
        ? "beta"
        : (document.getElementById("dpChannel")?.value || "stable");
      if (uploadRelease && !(await channelRiskOk(targetVer, dpChannel))) return;
      // Strictly reuse #plNotes ONLY when the pipeline form targets the SAME `versions.current`
      // so notes drafted for a bumped next version (e.g. v1.47.2) NEVER leak into v1.47.1 deploy!
      const plNotesMatchesCurrentVer = effectiveTargetVer === targetVer;
      const notesVal =
        !plNotesMatchesCurrentVer || (!plNotesUserTouched && plNotesIsFallback)
          ? ""
          : document.getElementById("plNotes")?.value.trim() || "";
      const summaryLines = [
        `版本：v${targetVer}`,
        `通道：${dpChannel === "beta" ? "预发布测试版 (Beta · 仅测试通道可见)" : "正式版 (Stable · 全员可收到更新)"}`,
        `成品：${(deployInfo?.artifactFiles || []).join("、")}`,
        `Git：${dpGitMode === "none" ? "不操作" : dpGitMode === "commit_tag_push" ? "提交改动 + 打标推送" : "打标并推送远端"}`,
        `发布：${uploadRelease ? `${PROVIDER_LABELS[repoProv] || repoProv} (${repoSlug})${dpRelMode === "draft" ? " [草稿 · 不会推送给用户]" : ""}` : "仅推 Git，不上传"}`,
      ];
      if (git.isRepo && git.aheadCount > 0 && dpGitMode === "none" && uploadRelease) {
        summaryLines.push(
          `⚠ 注意：本地 Git 有 ${git.aheadCount} 个未推送提交，不推 Git 直接发布将使远端 Tag 指向旧提交；建议改选「打标并推送远端」`
        );
      }
      const ok = await confirmDialog({
        title: `确认直接部署推送 v${targetVer}（免重编）`,
        message: "将核验现有成品 SHA-256 哈希与更新清单，并执行部署推送：",
        detail: summaryLines.join("\n"),
        confirmText: "立即部署推送",
      });
      if (!ok) return;

      try {
        const pid = await API.deployExistingRelease({
          projectId: project.id,
          version: targetVer,
          channel: dpChannel,
          ensureManifest,
          verifyChecksums: true,
          gitMode: dpGitMode,
          gitCommitMessage: document.getElementById("plCommitMsg")?.value.trim() || "",
          uploadRelease,
          draft: dpRelMode === "draft",
          overwriteAssets: true,
          releaseNotes: notesVal,
          artifactFiles: deployInfo?.artifactFiles || [],
          forceStaleSource,
        });
        delete app._plNotesState[project.id];
        app.setTaskRunning(pid, "部署推送流程", true, project.id);
      } catch (err) {
        toast(`${err}`, "err", 4800);
      }
    }).finally(syncBusyButtons)
  );

  document.getElementById("ovRunPipelineBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "启动中…", async () => {
      if (app.isProjectBusy(project.id)) {
        toast("当前项目已有任务运行中", "warn");
        return;
      }
      const bumpKind = document.getElementById("plBump").value;
      const customVersion = bumpKind === "custom" ? document.getElementById("plCustomVer").value.trim() : null;
      if (bumpKind === "custom" && !isValidSemver(customVersion)) {
        toast("请输入合法语义版本号，例如 1.4.0 或 1.4.0-beta.1", "warn");
        return;
      }
      const target = document.getElementById("plTarget").value;
      const uploadRemote = target === "remote" || target === "remote-draft";
      const gitMode = document.getElementById("plGitMode")?.value || "none";
      const gitCommitMessage = document.getElementById("plCommitMsg")?.value.trim() || "";
      const runTests = document.getElementById("plRunTests").checked;
      const runBuild = document.getElementById("plRunBuild").checked;
      const generateManifest = document.getElementById("plGenManifest").checked;
      if (
        bumpKind === "none" &&
        !runTests &&
        !runBuild &&
        !generateManifest &&
        !uploadRemote &&
        gitMode === "none"
      ) {
        toast("请至少选择一项发版步骤（版本递增、测试、编译、清单、Git 或上传）", "warn", 3400);
        return;
      }
      if (uploadRemote && !repoBound) {
        toast("请先绑定目标仓库后再上传", "warn", 3200);
        app.navigate("release", "repos");
        return;
      }
      if (gitMode === "commit_tag_push" && !git.remoteUrl) {
        toast("本地尚未绑定远端仓库，请先去「产物与仓库」一键绑定", "warn", 3800);
        return;
      }

      if (!(await channelRiskOk(effectiveTargetVer, document.getElementById("plChannel").value))) return;
      const req = {
        projectId: project.id,
        bumpKind,
        customVersion,
        channel: document.getElementById("plChannel").value,
        runTests,
        runBuild,
        buildActionId: document.getElementById("plBuildAct").value,
        generateManifest,
        uploadGithub: uploadRemote,
        githubRepo: repoSlug,
        repoProvider: repoProv,
        repoBaseUrl: project.repoBinding?.baseUrl || "",
        credentialId: project.repoBinding?.credentialId || "",
        releaseNotes:
          !plNotesUserTouched && plNotesIsFallback
            ? ""
            : document.getElementById("plNotes")?.value.trim() || "",
        draft: target === "remote-draft",
        gitMode,
        gitCommitMessage,
      };
      try {
        const pid = await API.runPipeline(req);
        delete app._plNotesState[project.id];
        app.setTaskRunning(pid, "构建发布流程", true, project.id);
      } catch (err) {
        toast(`${err}`, "err", 4200);
      }
    }).finally(syncBusyButtons)
  );

  return () => {
    window.removeEventListener("sf-task-state-changed", onTaskState);
  };
}
