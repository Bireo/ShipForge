import { API, categoryLabel, esc, formatBytes, parseCommandLine } from "../api.js";
import { confirmDialog, toast, withBusy } from "../ui.js";

const TIER_LABELS = {
  toolchain: "编译工具与运行时",
  project: "项目本地依赖与配置",
  publish: "仓库发布准备",
};

export function renderCompile(root, app, epoch) {
  const ov = app.overview;
  if (!ov) return;
  const { project, rootExists, versions } = ov;
  const activeTab = app.subTabs.compile || "presets";

  const subDesc =
    activeTab === "prereq"
      ? "仅检测当前项目实际所需的工具、本地依赖与仓库配置，确认清单后一键补全"
      : activeTab === "actions"
        ? "直接执行临时命令或单项构建、测试、打包与维护工序"
        : "推荐匹配模板高亮置顶，一键套用测试、编译与打包流程，支持自定义语言与外壳组合";

  root.innerHTML = `
    <div class="page-wrap">
      <div class="page-head">
        <div>
          <h1>编译与环境</h1>
          <div class="page-head-sub">${esc(subDesc)}</div>
        </div>
        <div class="head-actions">
          <button class="btn" id="cpRescanBtn" type="button" ${rootExists ? "" : "disabled"}>重新识别项目</button>
        </div>
      </div>

      <div id="compileBody" class="page-body-fill"></div>
    </div>
  `;

  document.getElementById("cpRescanBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "识别中…", async () => {
      try {
        await API.rescanProject(project.id);
        await app.reloadConfig(true);
        toast("已重新识别项目配置", "ok");
      } catch (err) {
        toast(`${err}`, "err");
      }
    })
  );

  const body = document.getElementById("compileBody");
  if (activeTab === "prereq") {
    return renderPrereqTab(body, app, project, rootExists, epoch);
  }
  if (activeTab === "actions") {
    return renderActionsTab(body, app, project, versions, rootExists);
  }
  return renderPresetsTab(body, app, project, rootExists, epoch);
}

function renderPresetsTab(container, app, project, rootExists, epoch) {
  container.innerHTML = `
    <div id="cpHeroHost"></div>

    <div>
      <div class="sf-section-title">
        <span>全部框架构建预设</span>
        <span class="dim" style="font-weight:400">当前适配：<strong>${esc(project.adapterLabel)}</strong></span>
      </div>
      <div class="preset-grid" id="cpPresetGrid"><span class="dim">正在加载预设模板…</span></div>
    </div>

    <section class="card card-soft">
      <details class="disclosure" style="border-top:none;padding-top:0">
        <summary style="font-size:13px;font-weight:600;color:var(--text-1)">自定义语言与外壳矩阵（混合项目 / 进阶微调）</summary>
        <div id="cpMatrixHost" style="margin-top:12px"><span class="dim">展开后加载语言与外壳矩阵…</span></div>
      </details>
    </section>
  `;

  const wireApplyButtons = (scope) => {
    scope.querySelectorAll("[data-apply-preset]").forEach((btn) => {
      btn.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "套用中…", async () => {
          try {
            const updated = await API.applyFrameworkPreset(project.id, btn.dataset.applyPreset);
            await app.reloadConfig(true);
            toast(`已套用「${updated.adapterLabel}」，共配置 ${updated.actions.length} 项工序`, "ok", 3200);
          } catch (err) {
            toast(`${err}`, "err", 4000);
          }
        })
      );
    });
  };

  API.listFrameworkPresets(project.id)
    .then((presets) => {
      if (epoch !== app.renderEpoch) return;
      const heroHost = document.getElementById("cpHeroHost");
      const grid = document.getElementById("cpPresetGrid");
      if (!grid) return;

      const rec = presets.find((p) => p.recommended) || presets[0];
      const others = presets.filter((p) => p !== rec);

      if (heroHost && rec) {
        heroHost.innerHTML = `
          <div class="preset-hero">
            <div class="preset-hero-main">
              <div class="preset-hero-title">
                <span>${esc(rec.name)}</span>
                <span class="tag tag-accent">${rec.recommended ? "当前项目推荐匹配" : "首选模板"}</span>
                ${rec.outputs.map((o) => `<span class="tag">${esc(o)}</span>`).join("")}
              </div>
              <div class="muted" style="font-size:12.5px">${esc(rec.summary)}</div>
              <div class="dim" style="font-size:11.5px">前置依赖：${esc(rec.requirements.join(" · "))}</div>
            </div>
            <button class="btn btn-accent" data-apply-preset="${esc(rec.id)}" type="button" ${rootExists ? "" : "disabled"}>一键套用推荐模板</button>
          </div>
        `;
        wireApplyButtons(heroHost);
      }

      grid.innerHTML = others
        .map(
          (p) => `<div class="preset-card">
            <div class="preset-card-head">
              <span class="preset-card-title">${esc(p.name)}</span>
              ${p.recommended ? `<span class="tag tag-accent">推荐</span>` : ""}
            </div>
            <div class="preset-card-desc">${esc(p.summary)}</div>
            <div class="field-row" style="gap:4px">
              ${p.outputs.map((o) => `<span class="tag">${esc(o)}</span>`).join("")}
            </div>
            <div class="preset-card-foot">
              <span class="dim" style="font-size:11px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="${esc(p.requirements.join(" · "))}">需：${esc(p.requirements.join(" · "))}</span>
              <button class="btn btn-xs" data-apply-preset="${esc(p.id)}" type="button" ${rootExists ? "" : "disabled"}>套用</button>
            </div>
          </div>`
        )
        .join("");

      wireApplyButtons(grid);
    })
    .catch((err) => {
      const grid = document.getElementById("cpPresetGrid");
      if (grid) grid.innerHTML = `<div class="notice notice-err">${esc(err)}</div>`;
    });

  // Lazy-load fine-grained language × shell matrix
  const det = container.querySelector("details");
  let loadedMx = false;
  det?.addEventListener("toggle", () => {
    if (!det.open || loadedMx || !rootExists) return;
    loadedMx = true;
    let selLang = project.primaryLanguage || "custom";
    let selShell = project.shellType || "portable";
    const host = document.getElementById("cpMatrixHost");
    API.getProjectProfileMatrix(project.id)
      .then((mx) => {
        if (epoch !== app.renderEpoch || !host) return;
        const detLang = new Set(mx.languages.map((l) => l.id));
        const detShell = new Set(mx.shells.map((s) => s.id));
        const draw = () => {
          host.innerHTML = `
            <div class="field" style="margin-bottom:12px">
              <label>开发语言</label>
              <div class="matrix-grid">
                ${mx.supportedLanguages
                  .map(
                    (l) => `<button class="matrix-tile ${selLang === l.id ? "active" : ""} ${detLang.has(l.id) ? "detected" : ""}" data-l="${esc(l.id)}" type="button">
                      <div class="matrix-tile-title"><span>${esc(l.name)}</span>${detLang.has(l.id) ? `<span class="tag tag-ok">已探测</span>` : ""}</div>
                    </button>`
                  )
                  .join("")}
              </div>
            </div>
            <div class="field" style="margin-bottom:12px">
              <label>打包外壳</label>
              <div class="matrix-grid">
                ${mx.supportedShells
                  .map(
                    (s) => `<button class="matrix-tile ${selShell === s.id ? "active" : ""} ${detShell.has(s.id) ? "detected" : ""}" data-s="${esc(s.id)}" type="button">
                      <div class="matrix-tile-title"><span>${esc(s.name)}</span>${detShell.has(s.id) ? `<span class="tag tag-ok">已探测</span>` : ""}</div>
                    </button>`
                  )
                  .join("")}
              </div>
            </div>
            <div class="field-row" style="justify-content:flex-end">
              <button class="btn btn-primary btn-xs" id="mxSaveBtn" type="button">保存语言与外壳组合</button>
            </div>
          `;
          host.querySelectorAll("[data-l]").forEach((b) =>
            b.addEventListener("click", () => {
              selLang = b.dataset.l;
              draw();
            })
          );
          host.querySelectorAll("[data-s]").forEach((b) =>
            b.addEventListener("click", () => {
              selShell = b.dataset.s;
              draw();
            })
          );
          host.querySelector("#mxSaveBtn")?.addEventListener("click", (e) =>
            withBusy(e.currentTarget, "保存中…", async () => {
              try {
                await API.applyProjectProfile(project.id, selLang, selShell);
                await app.reloadConfig(true);
                toast(`已切换为 ${selLang} / ${selShell}`, "ok");
              } catch (err) {
                toast(`保存语言与外壳组合失败：${err}`, "err", 4200);
              }
            })
          );
        };
        draw();
      })
      .catch((err) => {
        if (host) host.innerHTML = `<div class="notice notice-err">${esc(err)}</div>`;
      });
  });
}

function renderPrereqTab(container, app, project, rootExists, epoch) {
  container.innerHTML = `
    <div class="field-row" style="justify-content:space-between">
      <div class="muted" style="font-size:12.5px">勾选缺失项后可先预览安装位置与体积，确认后再执行补全</div>
      <div class="field-row" style="gap:6px">
        <button class="btn btn-xs" id="pqRefreshBtn" type="button" ${rootExists ? "" : "disabled"}>重新检查</button>
        <button class="btn btn-xs btn-accent" id="pqPlanRepairBtn" type="button" ${rootExists ? "" : "disabled"}>预览并一键补全</button>
      </div>
    </div>
    <div id="pqHost"><span class="dim">正在检查前置环境…</span></div>
  `;

  let prereqSeq = 0;
  const loadPrereq = async () => {
    const seq = ++prereqSeq;
    const host = document.getElementById("pqHost");
    if (!host) return;
    if (!rootExists) {
      host.innerHTML = `<div class="notice notice-err">项目根目录不存在（${esc(project.root)}），无法执行环境自检。</div>`;
      return;
    }
    host.innerHTML = `<span class="dim">正在检查环境与依赖…</span>`;
    try {
      const rep = await API.inspectProjectPrerequisites(project.id);
      if (seq !== prereqSeq || epoch !== app.renderEpoch) return;
      const hr = rep.headroom;
      const statusHeadline = rep.ready
        ? "编译环境已就绪"
        : rep.missingRequired > 0
          ? `缺少 ${rep.missingRequired} 项必要前置`
          : !hr.diskOk
            ? "目标磁盘可用空间不足"
            : "存在未就绪项";
      host.innerHTML = `
        <div class="status-ribbon" style="margin-bottom:14px">
          <span class="ribbon-item">
            <span class="dot ${rep.ready ? "dot-ok" : "dot-err"}"></span>
            <strong>${statusHeadline}</strong>
            ${rep.warnings ? `<span class="tag tag-warn">${rep.warnings} 项可优化</span>` : ""}
          </span>
          <span class="ribbon-item">磁盘 ${esc(hr.targetDrive)} 可用 <strong class="mono ${hr.diskOk ? "" : "err"}">${formatBytes(hr.diskFreeBytes)}</strong></span>
          <span class="ribbon-item">安全并发 <strong class="mono">${hr.safeConcurrency} 线程</strong></span>
        </div>

        <div class="prereq-tiers-grid">
        ${["toolchain", "project", "publish"]
          .map((tier) => {
            const list = rep.items.filter((i) => i.tier === tier);
            if (!list.length) return "";
            return `<div class="prereq-tier-item">
              <div class="sf-section-title">${esc(TIER_LABELS[tier])}</div>
              <div class="sf-group">
                ${list
                  .map((it) => {
                    const cls =
                      it.status === "ok"
                        ? "tag-ok"
                        : it.status === "corrupted" || it.required
                          ? "tag-err"
                          : "tag-warn";
                    const stTxt =
                      it.status === "ok"
                        ? "就绪"
                        : it.status === "corrupted"
                          ? "已损坏"
                          : it.required
                            ? "缺少"
                            : "待完善";
                    return `<div class="sf-row">
                      <div class="sf-row-main">
                        <div class="sf-row-title">
                          ${it.status !== "ok" && it.repairable ? `<input type="checkbox" name="pqPick" value="${esc(it.id)}" checked style="accent-color:var(--accent-btn)">` : ""}
                          <span>${esc(it.name)}</span>
                          <span class="tag ${cls}">${stTxt}</span>
                          ${it.estimatedMb > 0 && it.status !== "ok" ? `<span class="dim mono" style="font-size:11px">约 ${it.estimatedMb} MB</span>` : ""}
                        </div>
                        <div class="sf-row-desc">${esc(it.reason)} · ${esc(it.detail)}</div>
                      </div>
                      ${
                        it.id === "pub:repo_binding" && it.status !== "ok"
                          ? `<div class="sf-row-ctrl"><button class="btn btn-xs btn-primary" data-pq-go="release" type="button">去绑定仓库</button></div>`
                          : ""
                      }
                    </div>`;
                  })
                  .join("")}
              </div>
            </div>`;
          })
          .join("")}
        </div>
      `;
      host.querySelectorAll("[data-pq-go]").forEach((btn) =>
        btn.addEventListener("click", () => app.navigate("release", "repos"))
      );
    } catch (err) {
      if (seq !== prereqSeq || epoch !== app.renderEpoch) return;
      host.innerHTML = `<div class="notice notice-err">${esc(err)}</div>`;
    }
  };
  loadPrereq();

  document.getElementById("pqRefreshBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "自检中…", loadPrereq)
  );
  document.getElementById("pqPlanRepairBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "生成清单…", async () => {
      if (app.isProjectBusy(project.id)) {
        toast("当前项目正在执行任务，请等待完成后再补全依赖", "warn", 3200);
        return;
      }
      const allPickers = [...container.querySelectorAll('input[name="pqPick"]')];
      const checked = allPickers.filter((el) => el.checked).map((el) => el.value);
      if (allPickers.length > 0 && !checked.length) {
        toast("请至少勾选一项要补全的前置依赖", "warn", 2600);
        return;
      }
      if (!allPickers.length) {
        toast("当前没有需要自动补全的项目", "ok");
        return;
      }
      try {
        const plan = await API.planPrerequisiteRepair(project.id, checked);
        if (epoch !== app.renderEpoch) return;
        if (!plan.items.length) {
          toast("当前没有需要自动补全的项目", "ok");
          return;
        }
        const detailLines = plan.items
          .map((it) => `• ${it.name} → ${it.targetPath}${it.estimatedMb ? ` (约 ${it.estimatedMb} MB)` : ""}`)
          .join("\n");
        const ok = await confirmDialog({
          title: "确认补全以下缺失依赖",
          message: plan.summary,
          detail: detailLines,
          confirmText: "同意并开始补全",
          danger: !plan.diskOk,
        });
        if (!ok || epoch !== app.renderEpoch) return;

        app.openTerminal();
        const res = await API.executePrerequisiteRepair(
          project.id,
          plan.items.map((i) => i.id)
        );
        toast(
          res.failed.length ? `成功 ${res.succeeded.length} 项，失败 ${res.failed.length} 项` : `已补全 ${res.succeeded.length} 项`,
          res.failed.length ? "warn" : "ok",
          3600
        );
        if (epoch === app.renderEpoch) loadPrereq();
      } catch (err) {
        toast(`${err}`, "err", 4200);
      }
    })
  );
}

function renderActionsTab(container, app, project, versions, rootExists) {
  const hasAnyParam = project.actions.some((a) => a.supportsBump || a.supportsChannel || a.supportsSkipTests);

  container.innerHTML = `
    <div class="sf-group">
      <div class="sf-row" style="gap:8px;flex-wrap:wrap">
        <input class="input mono" id="ccCmd" style="flex:1;min-width:220px" placeholder="运行临时命令，例如 cargo test / npm run build">
        <input class="input mono" id="ccCwd" style="width:150px" placeholder="子目录（可选）">
        <button class="btn btn-primary" id="ccRunBtn" type="button" ${rootExists ? "" : "disabled"}>运行命令</button>
      </div>
    </div>

    ${
      hasAnyParam
        ? `<div class="status-ribbon" style="justify-content:space-between">
            <div class="field-row">
              <span class="muted" style="font-weight:600">脚本参数：</span>
              <select class="select" id="bdBump" style="height:26px">
                <option value="">保持 v${esc(versions.current)}</option>
                <option value="patch">-Bump patch</option>
                <option value="minor">-Bump minor</option>
                <option value="major">-Bump major</option>
              </select>
              <select class="select" id="bdChannel" style="height:26px">
                <option value="stable">stable</option>
                <option value="beta">beta</option>
              </select>
              <label class="sf-switch-label" style="font-size:12px">
                <input class="sf-switch" type="checkbox" switch id="bdSkipTests">
                <span>跳过测试</span>
              </label>
            </div>
          </div>`
        : ""
    }

    <div>
      <div class="sf-section-title">
        <span>已配置构建与维护工序 (${project.actions.length})</span>
        <button class="btn btn-xs btn-ghost" id="cpEditActionsBtn" type="button">编辑自定义工序…</button>
      </div>
      <div class="sf-group sf-group-plain actions-list-grid">
        ${
          project.actions.length
            ? project.actions
                .map(
                  (a) => `<div class="sf-row">
                    <div class="sf-row-main">
                      <div class="sf-row-title">
                        <span>${esc(a.label)}</span>
                        <span class="tag">${esc(categoryLabel(a.category))}</span>
                      </div>
                      <div class="sf-row-desc mono">${esc(a.description || `${a.command} ${a.args.join(" ")}`)}</div>
                    </div>
                    <div class="sf-row-ctrl">
                      <button class="btn btn-xs ${a.dangerous ? "btn-danger" : "btn-primary"}" data-bd-act="${esc(a.id)}" type="button" ${rootExists ? "" : "disabled"}>执行</button>
                    </div>
                  </div>`
                )
                .join("")
            : `<div class="sf-row"><span class="muted">暂无构建步骤，请先在「框架模板」一键套用。</span></div>`
        }
      </div>
    </div>
  `;

  document.getElementById("cpEditActionsBtn")?.addEventListener("click", () => app.openPreferences("project"));

  const syncActionBusyState = () => {
    const busy = app.isProjectBusy(project.id);
    container.querySelectorAll("[data-bd-act]").forEach((btn) => {
      if (!btn.dataset.busyLock) btn.disabled = !rootExists || busy;
    });
    const ccBtn = document.getElementById("ccRunBtn");
    if (ccBtn && !ccBtn.dataset.busyLock) ccBtn.disabled = !rootExists || busy;
  };
  syncActionBusyState();
  window.addEventListener("sf-task-state-changed", syncActionBusyState);

  container.querySelectorAll("[data-bd-act]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      if (app.isProjectBusy(project.id)) {
        toast("当前项目正在执行任务，请等待完成后再运行", "warn", 3000);
        return;
      }
      const act = project.actions.find((a) => a.id === btn.dataset.bdAct);
      if (!act) return;
      if (act.dangerous) {
        const ok = await confirmDialog({
          title: `确认执行：${act.label}`,
          message: act.description || "该操作会清理缓存或修改产物，确认继续？",
          detail: `${act.command} ${act.args.join(" ")}`,
          confirmText: "继续执行",
          danger: true,
        });
        if (!ok) return;
      }
      const bump = document.getElementById("bdBump")?.value || "";
      const channel = document.getElementById("bdChannel")?.value || "stable";
      const skipTests = document.getElementById("bdSkipTests")?.checked || false;
      app.runAction(act, { bump, channel, skipTests });
    });
  });

  const runAdHoc = async (triggerBtn = null) => {
    if (app.isProjectBusy(project.id)) {
      toast("当前项目正在执行任务，请等待完成后再运行命令", "warn", 3000);
      return;
    }
    const raw = document.getElementById("ccCmd").value.trim();
    const cwd = document.getElementById("ccCwd").value.trim();
    const { command, args } = parseCommandLine(raw);
    if (!command) {
      toast("请先输入命令", "warn");
      return;
    }
    const exec = async () => {
      try {
        const tid = await API.runCustomCommand({
          projectId: project.id,
          label: `命令：${command}`,
          command,
          args,
          cwd,
        });
        app.setTaskRunning(tid, `命令 ${command}`, false, project.id);
      } catch (err) {
        toast(`${err}`, "err");
      }
    };
    if (triggerBtn) {
      await withBusy(triggerBtn, "启动中…", exec);
      syncActionBusyState();
    } else {
      await exec();
    }
  };

  document.getElementById("ccRunBtn")?.addEventListener("click", (e) => runAdHoc(e.currentTarget));
  document.getElementById("ccCmd")?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      runAdHoc(document.getElementById("ccRunBtn"));
    }
  });

  return () => window.removeEventListener("sf-task-state-changed", syncActionBusyState);
}
