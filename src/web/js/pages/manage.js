import { API, esc, formatBytes, formatDurationText, parseCommandLine } from "../api.js";
import { choiceDialog, confirmDialog, toast, withBusy } from "../ui.js";

function renderCommitRefPills(refsRaw) {
  if (!refsRaw || !refsRaw.trim()) return "";
  const tokens = refsRaw
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s && s !== "origin/HEAD" && !s.startsWith("origin/"));
  if (!tokens.length) return "";
  const shown = tokens.slice(0, 3);
  const rest = tokens.slice(3);
  const pills = shown.map((tok) => {
    if (tok.startsWith("HEAD -> ")) {
      const br = tok.slice(8).trim();
      return `<span class="tag mono tag-ok">${esc(br)}</span>`;
    }
    if (tok.startsWith("tag: ")) {
      const t = tok.slice(5).trim();
      return `<span class="tag mono tag-accent">${esc(t)}</span>`;
    }
    return `<span class="tag mono">${esc(tok)}</span>`;
  });
  if (rest.length) {
    pills.push(`<span class="tag mono" title="${esc(rest.join(", "))}">+${rest.length}</span>`);
  }
  return `<span class="ref-pills">${pills.join("")}</span>`;
}

export function renderGitPane(container, app, ov) {
  const { project, versions, git } = ov;
  if (!git.isRepo) {
    container.innerHTML = `<div class="empty-state">
      <div style="margin-bottom:10px">当前目录尚未初始化 Git 仓库。</div>
      <button class="btn btn-primary btn-xs" id="gitGoBindInitBtn" type="button">前往产物与仓库一键绑定并初始化 →</button>
    </div>`;
    container.querySelector("#gitGoBindInitBtn")?.addEventListener("click", () => app.navigate("release", "repos"));
    return;
  }

  let showAllCommits = false;
  let showAllTags = false;
  let tagFilter = "";
  const commits = git.recentCommits || [];
  const tags = git.tags || [];
  const canUndoHead = commits.length > 1 && (git.aheadCount > 0 || !git.hasUpstream);

  const suggestCommitMsg = () => {
    const names = (git.changes || [])
      .slice(0, 2)
      .map((c) => (c.path || "").split(/[\\/]/).pop())
      .filter(Boolean);
    if (names.length && git.changes.length <= 2) {
      return `chore: update ${names.join(", ")}`;
    }
    return `chore: update workspace (v${versions.current})`;
  };

  const renderCommitsHtml = () => {
    if (!commits.length) return `<div class="muted" style="font-size:12px">暂无提交历史。</div>`;
    const visible = showAllCommits ? commits : commits.slice(0, 8);
    const rows = visible
      .map((c, idx) => {
        const showUndo = idx === 0 && canUndoHead;
        return `<div class="commit-row">
          <div class="commit-main">
            <span class="commit-subject">${esc(c.subject)}</span>
            ${renderCommitRefPills(c.refs)}
          </div>
          <div class="commit-meta">
            ${
              showUndo
                ? `<button class="btn btn-xs btn-soft" id="gitUndoCommitBtn" type="button" title="安全撤回最新未推送的本地提交（所有代码修改保留在暂存区）">撤回此提交</button>`
                : ""
            }
            <span class="commit-sha">${esc(c.sha)}</span>
            <span class="mono">${esc(c.date)}</span>
          </div>
        </div>`;
      })
      .join("");
    const footer =
      commits.length > 8
        ? `<div class="commit-foot">
            <button class="btn btn-xs btn-ghost" id="gitToggleCommitsBtn" type="button">
              ${showAllCommits ? "收起（仅显示最近 8 条）" : `展开全部提交（其余 ${commits.length - 8} 条）`}
            </button>
          </div>`
        : "";
    return `<div class="commit-list">${rows}${footer}</div>`;
  };

  const renderTagsHtml = () => {
    if (!tags.length) return `<div class="muted" style="font-size:12px">暂无本地版本标签。</div>`;
    const q = tagFilter.trim().toLowerCase();
    const matched = q ? tags.filter((t) => t.toLowerCase().includes(q)) : tags;
    if (!matched.length) return `<div class="muted" style="font-size:12px">未匹配到包含「${esc(tagFilter)}」的标签。</div>`;
    const visible = showAllTags || q ? matched : matched.slice(0, 8);
    const chips = visible
      .map(
        (t) => `<div class="git-tag-chip">
          <span class="mono git-tag-name">${esc(t)}</span>
          <button class="git-tag-del" data-del-tag="${esc(t)}" type="button" title="删除标签 ${esc(t)}" aria-label="删除标签 ${esc(t)}">✕</button>
        </div>`
      )
      .join("");
    const more =
      !q && matched.length > 8
        ? `<div class="commit-foot">
            <button class="btn btn-xs btn-ghost" id="gitToggleTagsBtn" type="button">
              ${showAllTags ? "收起（仅显示最近 8 个）" : `展开全部标签（其余 ${matched.length - 8} 个）`}
            </button>
          </div>`
        : "";
    return `<div class="git-tags-grid">${chips}</div>${more}`;
  };

  const defaultHint = suggestCommitMsg();

  const branchHeadLabel = git.headSha
    ? `${git.branch || "main"} @ ${git.headSha}`
    : `${git.branch || "main"}（尚无提交）`;
  const hasRemote = Boolean(git.remoteUrl && git.remoteUrl.trim());

  container.innerHTML = `
    <div class="status-ribbon" style="justify-content:space-between">
      <div class="field-row">
        <span class="ribbon-item"><span class="dot ${git.dirtyCount ? "dot-warn" : git.aheadCount || git.behindCount ? "dot-info" : "dot-ok"}"></span>分支 <strong class="mono">${esc(branchHeadLabel)}</strong></span>
        <span class="ribbon-item">${
          git.dirtyCount
            ? `<span class="tag tag-warn">未提交 ${git.dirtyCount} 个</span>`
            : `<span class="tag tag-ok">已全部提交</span>`
        }</span>
        ${
          git.hasUpstream
            ? `<span class="ribbon-item">远端同步 ${
                git.aheadCount || git.behindCount
                  ? `<span class="tag tag-info mono">${git.aheadCount ? `↑ ${git.aheadCount} 待推送` : ""}${git.aheadCount && git.behindCount ? " · " : ""}${git.behindCount ? `↓ ${git.behindCount} 落后` : ""}</span>`
                  : `<strong class="mono">已同步</strong>`
              }</span>`
            : `<span class="ribbon-item">远端同步 <strong class="dim">${hasRemote ? "未绑定上游分支" : "未配置远端"}</strong></span>`
        }
        ${git.lastTag ? `<span class="ribbon-item">最新标签 <strong class="mono">${esc(git.lastTag)}</strong></span>` : ""}
      </div>
      <div class="field-row" style="gap:6px">
        <button class="btn btn-xs" id="gitPushBtn" type="button" title="${hasRemote ? "仅推送已经 commit 的分支历史（不会提交未暂存改动）" : "尚未配置 origin 远端仓库"}">推送分支</button>
        <button class="btn btn-xs ${git.dirtyCount ? "" : "btn-primary"}" id="gitPushTagsBtn" type="button" title="${hasRemote ? "推送已提交的分支历史并同步本地新增标签" : "尚未配置 origin 远端仓库"}">推送分支与标签</button>
      </div>
    </div>

    <div class="split-40-60">
      <section class="card">
        <div class="card-head">
          <h2>暂存提交与打标</h2>
          ${git.dirtyCount ? `<span class="tag tag-warn">${git.dirtyCount} 个待提交</span>` : `<span class="tag tag-ok">干净</span>`}
        </div>
        <div class="field-row" style="gap:6px">
          <input class="input" id="gitCommitMsg" style="flex:1;min-width:140px" placeholder="提交说明（留空默认：${esc(defaultHint)}）">
          <button class="btn ${git.dirtyCount ? "" : "btn-primary"}" id="gitCommitBtn" type="button" ${git.dirtyCount ? "" : "disabled"}>提交全部</button>
          ${
            git.dirtyCount
              ? `<button class="btn btn-primary" id="gitCommitPushBtn" type="button" title="先提交全部本地改动，再直接推送到远端仓库">提交并推送</button>`
              : ""
          }
        </div>
        <div class="field-row" style="gap:6px">
          <input class="input mono" id="gitTagName" style="width:128px" value="v${esc(versions.current)}">
          <input class="input" id="gitTagMsg" style="flex:1;min-width:110px" placeholder="标签说明（可选）">
          <button class="btn" id="gitTagBtn" type="button">打 Tag</button>
        </div>
        ${
          git.changes.length
            ? `<div class="table-wrap"><table class="dt">
                <thead><tr><th style="width:52px">状态</th><th>变更文件</th></tr></thead>
                <tbody>${git.changes
                  .slice(0, 60)
                  .map((c) => `<tr><td><span class="tag mono">${esc(c.status)}</span></td><td class="mono cell-wrap">${esc(c.path)}</td></tr>`)
                  .join("")}</tbody>
              </table></div>`
            : `<div class="muted" style="font-size:12px">工作区干净，无未提交变更。</div>`
        }
      </section>

      <div style="display:flex;flex-direction:column;gap:14px;min-width:0">
        <section class="card">
          <div class="card-head">
            <h2>最近提交历史 <span class="tag">${commits.length} 条</span></h2>
            ${canUndoHead ? `<span class="dim" style="font-size:11.5px">未推送提交可安全撤回</span>` : ""}
          </div>
          <div id="gitCommitsHost">${renderCommitsHtml()}</div>
        </section>

        <section class="card">
          <div class="card-head">
            <h2>本地版本标签 <span class="tag">${tags.length} 个</span></h2>
            ${
              tags.length > 8
                ? `<input class="input input-xs mono" id="gitTagFilter" style="width:148px" type="search" placeholder="筛选标签…" aria-label="筛选本地标签">`
                : ""
            }
          </div>
          <div id="gitTagsHost">${renderTagsHtml()}</div>
        </section>
      </div>
    </div>
  `;

  const bindCommitControls = () => {
    container.querySelector("#gitToggleCommitsBtn")?.addEventListener("click", () => {
      showAllCommits = !showAllCommits;
      const host = container.querySelector("#gitCommitsHost");
      if (host) {
        host.innerHTML = renderCommitsHtml();
        bindCommitControls();
      }
    });
    container.querySelector("#gitUndoCommitBtn")?.addEventListener("click", (e) =>
      withBusy(e.currentTarget, "撤回中…", async () => {
        const head = commits[0];
        const ok = await confirmDialog({
          title: "安全撤回最新本地提交",
          message: `确认撤回尚未推送的提交 ${head.sha}（${head.subject}）？`,
          detail: "将执行 git reset --soft HEAD~1：提交记录会被撤回，但所有代码修改均安全保留在暂存区与工作区，不会丢失任何文件。",
          confirmText: "确认撤回提交",
          danger: true,
        });
        if (!ok) return;
        try {
          toast(await API.gitUndoLastCommit(project.id), "ok", 4200);
          await app.refreshOverview(true);
        } catch (err) {
          toast(`${err}`, "err", 4800);
        }
      })
    );
  };
  bindCommitControls();

  const bindTagControls = () => {
    container.querySelector("#gitToggleTagsBtn")?.addEventListener("click", () => {
      showAllTags = !showAllTags;
      const host = container.querySelector("#gitTagsHost");
      if (host) {
        host.innerHTML = renderTagsHtml();
        bindTagControls();
      }
    });
    container.querySelectorAll("[data-del-tag]").forEach((btn) => {
      btn.addEventListener("click", (ev) =>
        withBusy(ev.currentTarget, "删除中…", async () => {
          if (app.isProjectBusy(project.id)) {
            toast("当前项目正在执行任务，请等待完成后再删除标签", "warn", 3000);
            return;
          }
          const tag = btn.dataset.delTag;
          const { action, options } = await choiceDialog({
            title: `删除版本标签 ${tag}`,
            message: `确认删除本地 Git 标签 ${tag}？删除标签不会影响已有代码提交记录。`,
            danger: true,
            options: [
              {
                key: "deleteRemote",
                label: `同时删除远端 Tag (${tag})`,
                desc: git.remoteUrl
                  ? `向 ${git.remoteUrl} 发送删除标签指令（若该标签已关联线上 Release，建议先在「多平台发布」撤回 Release）`
                  : "当前未检测到 origin 远端仓库",
                checked: false,
                disabled: !git.remoteUrl,
              },
            ],
            buttons: [
              { id: "cancel", label: "取消", cls: "", autofocus: true },
              { id: "confirm", label: "确认删除标签", cls: "btn-danger" },
            ],
          });
          if (action !== "confirm") return;
          try {
            toast(await API.gitDeleteTag(project.id, tag, !!options.deleteRemote), "ok", 3800);
            await app.refreshOverview(true);
          } catch (err) {
            toast(`${err}`, "err", 4800);
          }
        })
      );
    });
  };
  bindTagControls();

  container.querySelector("#gitTagFilter")?.addEventListener("input", (e) => {
    tagFilter = e.target.value;
    const host = container.querySelector("#gitTagsHost");
    if (host) {
      host.innerHTML = renderTagsHtml();
      bindTagControls();
    }
  });

  const resolveCommitMessage = () => {
    const inp = container.querySelector("#gitCommitMsg");
    const raw = (inp?.value || "").trim();
    if (raw) return raw;
    const fallback = suggestCommitMsg();
    if (inp) inp.value = fallback;
    return fallback;
  };

  container.querySelector("#gitCommitBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "提交中…", async () => {
      const msg = resolveCommitMessage();
      try {
        toast(await API.gitCommit(project.id, msg), "ok");
        await app.refreshOverview(true);
      } catch (err) {
        toast(`${err}`, "err");
      }
    })
  );

  container.querySelector("#gitCommitPushBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "提交并推送…", async () => {
      if (!hasRemote) {
        toast("当前仓库尚未配置远端 (origin)，请先在「产物与仓库 → 多平台发布」一键绑定", "warn", 3800);
        return;
      }
      const msg = resolveCommitMessage();
      try {
        const cRes = await API.gitCommit(project.id, msg);
        toast(`${cRes}，正在推送远端…`, "ok", 2000);
        const pRes = await API.gitPush(project.id, true);
        toast(`${cRes} · ${pRes}`, "ok", 4200);
        await app.refreshOverview(true);
      } catch (err) {
        if (app.termMode === "hidden") app.setTermMode("docked");
        toast(`${err}`, "err", 5500);
        await app.refreshOverview(true);
      }
    })
  );

  container.querySelector("#gitTagBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "打标中…", async () => {
      if (!commits.length) {
        toast("当前仓库尚无提交记录，请先提交至少一次代码后再打标签", "warn", 3200);
        return;
      }
      const tag = container.querySelector("#gitTagName").value.trim();
      const msg = container.querySelector("#gitTagMsg").value.trim();
      if (!tag) return toast("请输入标签名", "warn");
      try {
        toast(await API.gitTag(project.id, tag, msg), "ok");
        await app.refreshOverview(true);
      } catch (err) {
        toast(`${err}`, "err");
      }
    })
  );

  const doPush = (btn, withTags) =>
    withBusy(btn, "推送中…", async () => {
      if (!hasRemote) {
        toast("当前仓库尚未配置远端 (origin)，请先在「产物与仓库 → 多平台发布」一键绑定", "warn", 3800);
        return;
      }
      if (!commits.length && git.dirtyCount === 0) {
        toast("当前仓库尚无提交记录可推送", "warn", 2800);
        return;
      }
      if (git.dirtyCount > 0) {
        const previewFiles = (git.changes || [])
          .slice(0, 8)
          .map((c) => `${c.status}  ${c.path}`)
          .join("\n");
        const { action } = await choiceDialog({
          title: `工作区还有 ${git.dirtyCount} 个未提交变更`,
          message: `「推送」只会把已经 commit 的历史快照推到 ${git.remoteUrl || "origin"}，不会包含下面这些尚未提交的本地修改：`,
          detail: previewFiles + (git.dirtyCount > 8 ? `\n… 另有 ${git.dirtyCount - 8} 个文件` : ""),
          notice: "若希望把这些修改一并提交并推送，请选择「先提交再推送」。",
          noticeKind: "warn",
          buttons: [
            { id: "cancel", label: "取消", cls: "" },
            { id: "push-only", label: "仅推送已提交历史", cls: "" },
            {
              id: "commit-and-push",
              label: `先提交这 ${git.dirtyCount} 个文件再推送`,
              cls: "btn-primary",
              autofocus: true,
            },
          ],
        });
        if (!action) return;
        try {
          if (action === "commit-and-push") {
            const msg = resolveCommitMessage();
            const cRes = await API.gitCommit(project.id, msg);
            const pRes = await API.gitPush(project.id, withTags);
            toast(`${cRes} · ${pRes}`, "ok", 4200);
          } else {
            const pRes = await API.gitPush(project.id, withTags);
            toast(`${pRes}（工作区仍有 ${git.dirtyCount} 个未提交变更）`, "warn", 4500);
          }
          await app.refreshOverview(true);
        } catch (err) {
          if (app.termMode === "hidden") app.setTermMode("docked");
          toast(`${err}`, "err", 5500);
          await app.refreshOverview(true);
        }
        return;
      }

      const ok = await confirmDialog({
        title: withTags ? "推送分支与新增标签" : "推送当前分支到远端",
        message: `确认向 ${git.remoteUrl || "origin"} 推送 ${git.branch} 分支${withTags ? "并同步未推送的本地 Tags" : ""}？`,
        confirmText: "确认推送",
      });
      if (!ok) return;
      try {
        const res = await API.gitPush(project.id, withTags);
        toast(res, "ok", 3800);
        await app.refreshOverview(true);
      } catch (err) {
        if (app.termMode === "hidden") app.setTermMode("docked");
        toast(`${err}`, "err", 5500);
      }
    });

  container.querySelector("#gitPushBtn")?.addEventListener("click", (e) => doPush(e.currentTarget, false));
  container.querySelector("#gitPushTagsBtn")?.addEventListener("click", (e) => doPush(e.currentTarget, true));

  const syncGitBusyButtons = () => {
    const busy = app.isProjectBusy(project.id);
    for (const sel of [
      "#gitCommitBtn",
      "#gitCommitPushBtn",
      "#gitTagBtn",
      "#gitPushBtn",
      "#gitPushTagsBtn",
      "#gitUndoCommitBtn",
    ]) {
      const b = container.querySelector(sel);
      if (!b || b.dataset.busyLock) continue;
      if (sel === "#gitCommitBtn") b.disabled = busy || !git.dirtyCount;
      else b.disabled = busy;
    }
    container.querySelectorAll("[data-del-tag]").forEach((b) => {
      if (!b.dataset.busyLock) b.disabled = busy;
    });
  };
  syncGitBusyButtons();
  window.addEventListener("sf-task-state-changed", syncGitBusyButtons);
  return () => window.removeEventListener("sf-task-state-changed", syncGitBusyButtons);
}

export function renderJournalPane(container, app, ov) {
  const { project, lock, recentBuilds } = ov;
  container.innerHTML = `
    <div class="sf-group">
      <div class="sf-row">
        <div class="sf-row-main">
          <div class="sf-row-title">构建锁监控</div>
          <div class="sf-row-desc mono">${esc(lock?.path || "该项目未配置外部构建锁")}</div>
          ${
            lock?.exists
              ? `<div class="sf-row-desc mono" style="margin-top:2px">会话=${esc(lock.session)} · 版本=${esc(lock.version)} · 模式=${esc(lock.mode)} · 开始于 ${esc(lock.startedAt)}</div>`
              : ""
          }
        </div>
        <div class="sf-row-ctrl">
          <span class="tag ${lock?.exists ? (lock.alive ? "tag-warn" : "tag-err") : "tag-ok"}">
            ${lock ? (lock.exists ? (lock.alive ? `活跃持锁 PID ${lock.pid}` : `死锁残留 PID ${lock.pid}`) : "空闲") : "未启用"}
          </span>
          ${lock?.exists && !lock.alive ? `<button class="btn btn-xs" id="jnCleanLockBtn" type="button">归档死锁</button>` : ""}
        </div>
      </div>
    </div>

    <section class="card">
      <div class="card-head">
        <h2>历史构建台账 <span class="tag">${recentBuilds.length} 条</span></h2>
      </div>
      ${
        recentBuilds.length
          ? `<div class="table-wrap"><table class="dt">
              <thead><tr><th>时间</th><th>状态</th><th>版本</th><th>模式/工序</th><th>耗时</th><th class="col-hide-md">Git</th><th class="col-hide-sm">备注</th><th class="col-hide-md">来源</th></tr></thead>
              <tbody>${recentBuilds
                .map(
                  (b) => `<tr>
                    <td class="mono dim" style="white-space:nowrap">${esc(b.ts)}</td>
                    <td><span class="tag ${b.result === "ok" ? "tag-ok" : b.result === "cancelled" ? "tag-warn" : "tag-err"}">${esc(b.result)}</span></td>
                    <td class="mono">${esc(b.version)}</td>
                    <td class="mono cell-wrap">${esc(b.mode)}</td>
                    <td class="mono muted dur-cell">${esc(formatDurationText(b.duration))}</td>
                    <td class="mono dim col-hide-md">${esc(b.git || "-")}</td>
                    <td class="muted cell-wrap col-hide-sm">${esc(b.note || "-")}</td>
                    <td class="col-hide-md"><span class="tag">${esc(b.source)}</span></td>
                  </tr>`
                )
                .join("")}</tbody>
            </table></div>`
          : `<div class="muted" style="font-size:12px">暂无台账记录</div>`
      }
    </section>
  `;

  container.querySelector("#jnCleanLockBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "清理中…", async () => {
      try {
        toast(await API.cleanStaleLock(project.id), "ok");
        await app.refreshOverview(true);
      } catch (err) {
        toast(`${err}`, "err");
      }
    })
  );
}

export function renderConfigPane(container, app, ov, { inModal = false, onProjectRemoved = null } = {}) {
  const project = structuredClone(ov.project);
  const envText = Object.entries(project.env || {})
    .map(([k, v]) => `${k}=${v}`)
    .join("\n");
  const bindProv = (project.repoBinding?.provider || "").trim().toLowerCase();
  const isNonGhBinding = Boolean(bindProv && bindProv !== "github");
  const displayRepoSlug = project.repoBinding?.repoSlug || project.githubRepo || "";
  const repoLabelText = isNonGhBinding
    ? `${bindProv.toUpperCase()} 仓库 (owner/repo)`
    : "GitHub 仓库 (owner/repo)";

  container.innerHTML = `
    <div class="sf-group">
      <div class="sf-row">
        <div class="sf-row-main">
          <div class="sf-row-title">${esc(project.name)} <span class="tag">${esc(project.adapterLabel || project.kind)}</span></div>
          <div class="sf-row-desc mono">${esc(project.root)}</div>
        </div>
        <div class="sf-row-ctrl">
          <button class="btn btn-xs btn-danger" id="${inModal ? "prefRemoveProjBtn" : "mgRemoveProjBtn"}" type="button">移出此项目</button>
          <button class="btn btn-xs btn-primary" id="cfgSaveBtn" type="button">保存配置</button>
        </div>
      </div>
    </div>

    <section class="card">
      <div class="card-head">
        <h2>基础参数与路径</h2>
      </div>
      <div class="grid-3">
        <div class="field"><label for="cfgName">显示名称</label><input class="input" id="cfgName" value="${esc(project.name)}"></div>
        <div class="field"><label for="cfgProduct">产品名称</label><input class="input" id="cfgProduct" value="${esc(project.productName)}"></div>
        <div class="field"><label for="cfgPrefix">产物前缀</label><input class="input mono" id="cfgPrefix" value="${esc(project.artifactPrefix)}"></div>
      </div>
      <div class="grid-3">
        <div class="field"><label for="cfgRel">发布目录</label><input class="input mono" id="cfgRel" value="${esc(project.releaseDir)}"></div>
        <div class="field"><label for="cfgCl">变更记录路径</label><input class="input mono" id="cfgCl" value="${esc(project.changelogPath)}"></div>
        <div class="field">
          <label for="cfgGh">${esc(repoLabelText)}</label>
          <div class="field-row" style="gap:4px;flex-wrap:nowrap">
            <input class="input mono" id="cfgGh" style="flex:1;min-width:0" value="${esc(displayRepoSlug)}">
            ${isNonGhBinding ? "" : `<button class="btn btn-xs" id="cfgDetectGhBtn" type="button" title="从 Git remote 自动识别">识别</button>`}
          </div>
        </div>
      </div>
      <div class="grid-3">
        <div class="field"><label for="cfgExe">运行副本路径</label><input class="input mono" id="cfgExe" value="${esc(project.launchExe)}"></div>
        <div class="field"><label for="cfgLock">外部构建锁路径</label><input class="input mono" id="cfgLock" value="${esc(project.lockFile)}"></div>
        <div class="field"><label for="cfgKeep">旧包留存版本数</label><input class="input mono" id="cfgKeep" type="number" min="1" max="20" value="${project.keepReleases || 2}"></div>
      </div>
      <div class="field">
        <label for="cfgEnv">项目环境变量（每行 KEY=VALUE，支持 PATH_PREPEND）</label>
        <textarea class="textarea mono" id="cfgEnv" style="min-height:68px">${esc(envText)}</textarea>
      </div>
    </section>

    <section class="card">
      <div class="card-head"><h2>版本追踪文件（原子同步来源）</h2></div>
      <div class="field-row">
        <input class="input mono" id="newVfPath" style="flex:1;min-width:170px" placeholder="相对路径，如 package.json">
        <select class="select" id="newVfFmt" style="width:140px">
          <option value="tauri_json">tauri.conf.json</option>
          <option value="cargo_toml">Cargo.toml</option>
          <option value="package_json">package.json</option>
          <option value="pyproject_toml">pyproject.toml</option>
          <option value="csproj">*.csproj</option>
          <option value="pubspec_yaml">pubspec.yaml</option>
          <option value="regex">自定义正则</option>
        </select>
        <input class="input mono" id="newVfPat" style="width:150px" placeholder="仅 regex 必填">
        <button class="btn btn-xs" id="btnAddVf" type="button">+ 添加</button>
      </div>
      <div id="cfgVfTable"></div>
    </section>

    <section class="card">
      <div class="card-head"><h2>自定义工序列表</h2></div>
      <div class="field-row">
        <input class="input" id="newActLabel" style="width:140px" placeholder="工序名称">
        <select class="select" id="newActCat" style="width:94px">
          <option value="custom">自定义</option>
          <option value="test">测试</option>
          <option value="build">构建</option>
          <option value="package">打包</option>
          <option value="publish">发布</option>
          <option value="maintenance">维护</option>
        </select>
        <input class="input mono" id="newActCmd" style="flex:1;min-width:180px" placeholder="完整命令，如 python tools\\smoke.py">
        <button class="btn btn-xs" id="btnAddAct" type="button">+ 添加</button>
      </div>
      <div id="cfgActTable"></div>
    </section>
  `;

  container.querySelector(`#${inModal ? "prefRemoveProjBtn" : "mgRemoveProjBtn"}`)?.addEventListener("click", async () => {
    const removed = await app.removeProjectWithConfirm(ov.project);
    if (removed && typeof onProjectRemoved === "function") onProjectRemoved();
  });

  container.querySelector("#cfgDetectGhBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "…", async () => {
      try {
        const slug = await API.detectProjectGithubRepo(project.id);
        if (slug) {
          container.querySelector("#cfgGh").value = slug;
          toast(`已识别仓库：${slug}`, "ok");
        } else {
          toast("未从本地 Git remote 识别到 GitHub 仓库", "warn");
        }
      } catch (err) {
        toast(`${err}`, "err");
      }
    })
  );

  const paintVersionFiles = () => {
    const host = container.querySelector("#cfgVfTable");
    if (!host) return;
    const list = project.versionFiles || [];
    if (!list.length) {
      host.innerHTML = `<div class="muted" style="font-size:12px">暂无版本文件，可在上方手动添加。</div>`;
      return;
    }
    host.innerHTML = `<div class="row-list">${list
      .map(
        (vf, i) => `<div class="list-row" style="padding:7px 11px">
          <div class="list-row-main">
            <div class="list-row-title"><span class="mono">${esc(vf.path)}</span><span class="tag">${esc(vf.format)}</span></div>
            ${vf.pattern ? `<div class="list-row-desc mono">${esc(vf.pattern)}</div>` : ""}
          </div>
          <button class="btn btn-xs btn-ghost" data-del-vf="${i}" type="button">删除</button>
        </div>`
      )
      .join("")}</div>`;
    host.querySelectorAll("[data-del-vf]").forEach((b) => {
      b.addEventListener("click", () => {
        project.versionFiles.splice(Number(b.dataset.delVf), 1);
        paintVersionFiles();
      });
    });
  };
  paintVersionFiles();

  container.querySelector("#btnAddVf")?.addEventListener("click", () => {
    const path = container.querySelector("#newVfPath").value.trim();
    const format = container.querySelector("#newVfFmt").value;
    const pattern = container.querySelector("#newVfPat").value.trim();
    if (!path) return toast("请填写版本文件相对路径", "warn");
    if (format === "regex" && !pattern) return toast("regex 模式请提供正则表达式", "warn");
    project.versionFiles = project.versionFiles || [];
    project.versionFiles.push({ path, format, pattern });
    container.querySelector("#newVfPath").value = "";
    container.querySelector("#newVfPat").value = "";
    paintVersionFiles();
    toast("已添加版本文件，点击「保存配置」生效", "ok", 2200);
  });

  const paintActions = () => {
    const host = container.querySelector("#cfgActTable");
    if (!host) return;
    host.innerHTML = `<div class="row-list">${project.actions
      .map(
        (a, i) => `<div class="list-row" style="padding:7px 11px">
          <div class="list-row-main">
            <div class="list-row-title"><span>${esc(a.label)}</span><span class="tag">${esc(a.category)}</span></div>
            <div class="list-row-desc mono">${esc(a.command)} ${esc(a.args.join(" "))}</div>
          </div>
          <button class="btn btn-xs btn-ghost" data-del="${i}" type="button">删除</button>
        </div>`
      )
      .join("")}</div>`;
    host.querySelectorAll("[data-del]").forEach((b) => {
      b.addEventListener("click", () => {
        project.actions.splice(Number(b.dataset.del), 1);
        paintActions();
      });
    });
  };
  paintActions();

  container.querySelector("#btnAddAct")?.addEventListener("click", () => {
    const label = container.querySelector("#newActLabel").value.trim();
    const category = container.querySelector("#newActCat").value;
    const raw = container.querySelector("#newActCmd").value.trim();
    const parsed = parseCommandLine(raw);
    if (!label || !parsed.command) return toast("请填写工序名称和命令", "warn");
    project.actions.push({
      id: `custom-${Date.now()}`,
      label,
      category,
      command: parsed.command,
      args: parsed.args,
      cwd: "",
      description: raw,
      dangerous: false,
      supportsBump: false,
      supportsChannel: false,
      supportsSkipTests: false,
    });
    container.querySelector("#newActLabel").value = "";
    container.querySelector("#newActCmd").value = "";
    paintActions();
    toast("已加入列表，点击「保存配置」生效", "ok", 2200);
  });

  container.querySelector("#cfgSaveBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "保存中…", async () => {
      project.name = container.querySelector("#cfgName").value.trim() || project.name;
      project.productName = container.querySelector("#cfgProduct").value.trim();
      project.artifactPrefix = container.querySelector("#cfgPrefix").value.trim();
      project.releaseDir = container.querySelector("#cfgRel").value.trim() || "release";
      project.changelogPath = container.querySelector("#cfgCl").value.trim();
      const nextRepoSlug = container.querySelector("#cfgGh").value.trim();
      if (isNonGhBinding) {
        project.repoBinding = {
          ...(project.repoBinding || { provider: bindProv, baseUrl: "", credentialId: "" }),
          repoSlug: nextRepoSlug,
        };
        project.githubRepo = "";
      } else {
        project.githubRepo = nextRepoSlug;
        if (!project.repoBinding) {
          project.repoBinding = { provider: "github", baseUrl: "", repoSlug: nextRepoSlug, credentialId: "" };
        } else {
          project.repoBinding.provider = "github";
          project.repoBinding.repoSlug = nextRepoSlug;
        }
      }
      project.launchExe = container.querySelector("#cfgExe").value.trim();
      project.lockFile = container.querySelector("#cfgLock").value.trim();
      const rawKeep = Number(container.querySelector("#cfgKeep").value);
      project.keepReleases = Math.min(50, Math.max(1, Math.trunc(Number.isFinite(rawKeep) ? rawKeep : 2)));

      const envMap = {};
      for (const line of container.querySelector("#cfgEnv").value.split(/\r?\n/)) {
        const t = line.trim();
        if (!t || t.startsWith("#")) continue;
        const eq = t.indexOf("=");
        if (eq > 0) envMap[t.slice(0, eq).trim()] = t.slice(eq + 1).trim();
      }
      project.env = envMap;

      try {
        await API.updateProjectConfig(project);
        toast("项目配置已保存", "ok");
        await app.reloadConfig(true);
      } catch (err) {
        toast(`保存失败：${err}`, "err");
      }
    })
  );
}

export function renderStoragePane(container, app, isAlive = () => true) {
  container.innerHTML = `
    <section class="card">
      <div class="card-head">
        <div>
          <h2>工作区扫描目录</h2>
          <div class="card-sub">管理常用代码根目录，便于快速发现并批量导入本地项目</div>
        </div>
        <div class="field-row" style="gap:6px">
          <button class="btn btn-xs" id="btnAddScanRoot" type="button">+ 添加目录</button>
          <button class="btn btn-xs btn-primary" id="btnRunDiscoverNow" type="button">扫描项目</button>
        </div>
      </div>
      <div id="scanRootsHost"></div>
    </section>

    <section class="card">
      <div class="card-head">
        <div>
          <h2>数据与缓存存储目录</h2>
          <div class="card-sub">将配置、构建缓存与托管便携工具从 C 盘安全迁移至其他盘符（SHA-256 逐文件校验）</div>
        </div>
        <div class="field-row" style="gap:6px">
          <button class="btn btn-xs" id="btnOpenDataDir" type="button">打开数据目录</button>
          <button class="btn btn-xs" id="btnOpenToolsDir" type="button">打开工具目录</button>
          <button class="btn btn-xs" id="btnConsolidateTools" type="button">收归便携工具</button>
          <button class="btn btn-xs btn-primary" id="btnMigrateDir" type="button">迁移存储目录</button>
        </div>
      </div>
      <div id="storageInfoHost"><span class="dim">正在读取存储信息…</span></div>
    </section>
  `;

  const renderScanRoots = () => {
    const host = container.querySelector("#scanRootsHost");
    if (!host) return;
    const roots = app.config.scanRoots || [];
    if (!roots.length) {
      host.innerHTML = `<div class="muted" style="font-size:12px">暂未保存固定扫描目录。</div>`;
      return;
    }
    host.innerHTML = `<div class="chip-grid">${roots
      .map(
        (r) => `<span class="check-pill" style="cursor:default">
          <span class="mono">${esc(r)}</span>
          <button class="btn btn-xs btn-ghost" data-rm-scan-root="${esc(r)}" type="button" style="height:18px;padding:0 5px">✕</button>
        </span>`
      )
      .join("")}</div>`;
    host.querySelectorAll("[data-rm-scan-root]").forEach((btn) => {
      btn.addEventListener("click", (ev) =>
        withBusy(ev.currentTarget, "…", async () => {
          const next = (app.config.scanRoots || []).filter((x) => x !== btn.dataset.rmScanRoot);
          try {
            app.config = await API.setScanRoots(next);
            renderScanRoots();
            toast("已移除扫描目录", "ok", 1400);
          } catch (err) {
            toast(`${err}`, "err");
          }
        })
      );
    });
  };
  renderScanRoots();

  container.querySelector("#btnAddScanRoot")?.addEventListener("click", (ev) =>
    withBusy(ev.currentTarget, "添加中…", async () => {
      const picked = await API.pickFolder("选择包含软件项目的工作区根目录");
      if (!picked) return;
      const next = [...(app.config.scanRoots || []), picked];
      try {
        app.config = await API.setScanRoots(next);
        renderScanRoots();
        toast("已添加扫描目录", "ok", 1600);
      } catch (err) {
        toast(`${err}`, "err");
      }
    })
  );
  container.querySelector("#btnRunDiscoverNow")?.addEventListener("click", () => app.openDiscoverProjectsDialog());
  container.querySelector("#btnOpenDataDir")?.addEventListener("click", () => app.openDataDir());

  let currentStorage = null;
  const loadStorage = async () => {
    const host = container.querySelector("#storageInfoHost");
    if (!host) return;
    try {
      const info = await API.getStorageInfo();
      if (!isAlive()) return;
      currentStorage = info;
      host.innerHTML = `
        <div class="status-ribbon" style="margin-bottom:8px">
          <span class="ribbon-item">主目录 <strong class="mono">${esc(info.dataRoot)}</strong></span>
          <span class="ribbon-item">已用空间 <strong class="mono">${formatBytes(info.totalBytes)}</strong></span>
          <span class="ribbon-item">托管工具 <strong>${info.managedToolCount} 项</strong></span>
          ${info.isCDrive ? `<span class="tag tag-warn">位于 C 盘（建议迁移）</span>` : `<span class="tag tag-ok">非系统盘</span>`}
          ${info.isAscii ? `<span class="tag tag-ok">纯 ASCII 路径</span>` : `<span class="tag tag-warn">含非 ASCII 字符</span>`}
        </div>
        <div class="muted" style="font-size:11.5px">
          构建依赖缓存重定向至 <span class="mono">${esc(info.cacheDir)}</span>（${info.isAscii ? "Cargo · " : ""}npm · pnpm · pip · uv · Tauri）。
        </div>
      `;
    } catch (err) {
      host.innerHTML = `<span class="err">${esc(err)}</span>`;
    }
  };
  loadStorage();

  container.querySelector("#btnOpenToolsDir")?.addEventListener("click", () => {
    if (!currentStorage) return;
    API.openInExplorer(currentStorage.toolsDir).catch(() => API.openInExplorer(currentStorage.dataRoot));
  });

  container.querySelector("#btnConsolidateTools")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "收归中…", async () => {
      try {
        const res = await API.consolidateToolsAndCaches();
        const items = res.summary || [];
        toast(items.length ? `收归完成：${items.join("；")}` : "受管工具与依赖缓存目录均已就绪", "ok", 4200);
        await loadStorage();
      } catch (err) {
        if (app.termMode === "hidden") app.setTermMode("docked");
        toast(`收归失败：${err}`, "err", 5000);
      }
    })
  );

  container.querySelector("#btnMigrateDir")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "迁移中…", async () => {
      try {
        const picked = await API.pickFolder("选择新的 ShipForge 数据与工具存储目录（推荐纯 ASCII 非 C 盘路径）");
        if (!picked) return;
        const ok = await confirmDialog({
          title: "确认迁移数据、便携工具与依赖缓存",
          message: "系统将把现有配置、版本隔离数据、托管工具与便携工具逐文件 SHA-256 校验迁移至新目录，并自动重定向依赖缓存。",
          detail: `当前目录：${currentStorage?.dataRoot || ""}\n目标目录：${picked}`,
          confirmText: "开始安全迁移",
        });
        if (!ok) return;
        const updated = await API.migrateStorageRoot(picked);
        toast(`迁移完成 → ${updated.dataRoot}（托管工具 ${updated.managedToolCount} 项）`, "ok", 4500);
        await loadStorage();
      } catch (err) {
        if (app.termMode === "hidden") app.setTermMode("docked");
        toast(`迁移失败：${err}`, "err", 5000);
      }
    })
  );
}

export function renderToolchainPane(container, app, isAlive = () => true) {
  container.innerHTML = `
    <section class="card">
      <div class="card-head">
        <div>
          <h2>本机环境工具链中心</h2>
          <div class="card-sub">查看全栈编译、打包与发布工具状态（按当前项目精准补全请前往「编译与环境」）</div>
        </div>
        <div class="field-row" style="gap:6px">
          <button class="btn btn-xs" id="btnScanTools" type="button">重新扫描</button>
          <button class="btn btn-xs btn-accent" id="btnRepairAll" type="button">补全缺失基础工具</button>
        </div>
      </div>
      <div id="toolTableHost"><span class="dim">正在并发体检工具链…</span></div>
    </section>
  `;

  let scanSeq = 0;
  const scan = async () => {
    const seq = ++scanSeq;
    const host = container.querySelector("#toolTableHost");
    if (!host) return;
    host.innerHTML = `<span class="dim">正在并发体检工具链…</span>`;
    try {
      const list = await API.scanToolchains();
      if (seq !== scanSeq || !isAlive()) return;
      host.innerHTML = `<div class="table-wrap"><table class="dt">
        <thead><tr><th>工具</th><th class="col-hide-sm">分类</th><th>状态</th><th>实测版本</th><th class="col-hide-md">路径</th><th style="width:76px">操作</th></tr></thead>
        <tbody>${list
          .map((t) => {
            const stTag =
              t.status === "ok"
                ? `<span class="tag tag-ok">就绪</span>`
                : t.status === "corrupted"
                  ? `<span class="tag tag-err" title="${esc(t.detail)}">损坏</span>`
                  : `<span class="tag tag-warn">缺失</span>`;
            const actBtn =
              t.status !== "ok" && t.repairable
                ? `<button class="btn btn-xs btn-primary" data-repair="${esc(t.id)}" type="button">${t.status === "corrupted" ? "修复" : "安装"}</button>`
                : t.status === "ok" && t.repairable
                  ? `<button class="btn btn-xs btn-ghost" data-repair="${esc(t.id)}" type="button">重修</button>`
                  : `<span class="dim">-</span>`;
            return `<tr>
              <td title="${esc(t.path || t.detail)}"><strong>${esc(t.name)}</strong>${t.managed ? ` <span class="tag tag-accent">托管</span>` : t.status === "ok" ? ` <span class="tag">系统</span>` : ""}</td>
              <td class="col-hide-sm"><span class="tag">${esc(t.category)}</span></td>
              <td>${stTag}</td>
              <td class="mono muted cell-wrap" title="${esc(t.detail)}">${esc(t.version || (t.status === "corrupted" ? "校验异常" : "-"))}</td>
              <td class="mono dim col-hide-md" style="max-width:190px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="${esc(t.path || t.detail)}">${esc(t.path || "-")}</td>
              <td>${actBtn}</td>
            </tr>`;
          })
          .join("")}</tbody>
      </table></div>`;

      host.querySelectorAll("[data-repair]").forEach((btn) => {
        btn.addEventListener("click", (ev) =>
          withBusy(ev.currentTarget, "修复中…", async () => {
            const id = btn.dataset.repair;
            try {
              const item = await API.repairToolchain(id);
              toast(`工具 ${item.name} 已修复就绪（${item.version}）`, "ok", 3500);
              await scan();
            } catch (err) {
              if (app.termMode === "hidden") app.setTermMode("docked");
              toast(`修复 ${id} 失败：${err}`, "err", 5500);
            }
          })
        );
      });
    } catch (err) {
      host.innerHTML = `<span class="err">${esc(err)}</span>`;
    }
  };
  scan();

  container.querySelector("#btnScanTools")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "扫描中…", scan)
  );
  container.querySelector("#btnRepairAll")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "一键修复中…", async () => {
      const ok = await confirmDialog({
        title: "一键修复核心缺失/损坏工具",
        message: "将自动下载并补全缺失或校验损坏的核心编译/打包/发布工具至 ShipForge 工具目录，并在完成后自动复核。",
        confirmText: "开始一键修复",
      });
      if (!ok) return;
      try {
        await API.repairAllEssentialToolchains();
        toast("核心工具链一键修复完成", "ok", 3600);
        await scan();
      } catch (err) {
        if (app.termMode === "hidden") app.setTermMode("docked");
        toast(`${err}`, "err", 5500);
        await scan();
      }
    })
  );
}

/** Legacy compatibility entry: routes cleanly through app.navigate() without hiding any page head */
export function renderManage(root, app) {
  app.navigate("version", "git");
}
