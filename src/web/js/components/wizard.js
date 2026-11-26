import { API, esc } from "../api.js";
import { attachLightDismissFallback, toast, withBusy } from "../ui.js";

/**
 * Skippable 3-step onboarding wizard (macOS Sequoia sheet style)
 */
export function openOnboardingWizard(app) {
  let step = app.activeProject ? 2 : 1;
  let renderSeq = 0;
  let draftRepoSlug = "";
  let draftProv = "";
  let draftBaseUrl = "";
  let selectedCredentialId = "";

  const dlg = document.createElement("dialog");
  dlg.className = "hr-dialog";
  dlg.setAttribute("closedby", "any");
  dlg.setAttribute("aria-label", "快速配置向导");

  const closeAndDone = async (markCompleted = true) => {
    try {
      if (dlg.open) dlg.close("");
    } catch {}
    dlg.remove();
    if (markCompleted) {
      try {
        app.config = await API.setOnboardingCompleted(true);
      } catch {}
    }
  };

  // Prevent orphan <dialog> leak when user dismisses via Esc or backdrop click
  dlg.addEventListener("close", () => {
    dlg.remove();
  });
  attachLightDismissFallback(dlg);

  const render = async () => {
    const seq = ++renderSeq;
    const proj = app.activeProject;
    dlg.innerHTML = `
      <div class="hr-head">
        <strong style="font-size:14.5px">快速配置向导</strong>
        <button class="btn btn-xs btn-ghost" id="wzSkipBtn" type="button">跳过向导</button>
      </div>
      <div class="hr-body scroller">
        <div class="wizard-steps">
          <div class="wizard-step-tab ${step === 1 ? "active" : step > 1 ? "done" : ""}">1. 导入项目</div>
          <div class="wizard-step-tab ${step === 2 ? "active" : step > 2 ? "done" : ""}">2. 确认外壳与依赖</div>
          <div class="wizard-step-tab ${step === 3 ? "active" : ""}">3. 连接发布仓库</div>
        </div>
        <div id="wzBody"><div class="dim">正在加载…</div></div>
      </div>
      <div class="hr-foot">
        <div class="field-row" style="gap:6px">
          ${step > 1 ? `<button class="btn" id="wzPrevBtn" type="button">上一步</button>` : ""}
        </div>
        <div class="field-row" style="gap:6px">
          ${
            step < 3
              ? `<button class="btn btn-accent" id="wzNextBtn" type="button" ${step === 1 && !proj ? "disabled" : ""}>下一步</button>`
              : `<button class="btn btn-accent" id="wzFinishBtn" type="button">完成并进入工作台</button>`
          }
        </div>
      </div>
    `;

    dlg.querySelector("#wzSkipBtn")?.addEventListener("click", () => closeAndDone(true));
    dlg.querySelector("#wzPrevBtn")?.addEventListener("click", () => {
      step = Math.max(1, step - 1);
      render();
    });
    dlg.querySelector("#wzNextBtn")?.addEventListener("click", () => {
      step = Math.min(3, step + 1);
      render();
    });
    dlg.querySelector("#wzFinishBtn")?.addEventListener("click", async () => {
      await closeAndDone(true);
      await app.refreshOverview(true);
    });

    const host = dlg.querySelector("#wzBody");
    if (!host) return;

    if (step === 1) {
      host.innerHTML = `
        <div class="hr-block">
          <div class="hr-block-title">选择你要构建和发布的软件目录</div>
          ${
            proj
              ? `<div style="font-size:12.5px;margin-bottom:10px">当前已选项目：<strong>${esc(proj.name)}</strong><div class="mono muted" style="font-size:11.5px">${esc(proj.root)}</div></div>`
              : `<div class="muted" style="font-size:12.5px;margin-bottom:10px">尚未导入项目。可选择单个项目目录，或扫描工作区自动识别。</div>`
          }
          <div class="field-row" style="gap:8px">
            <button class="btn btn-primary" id="wzPickDirBtn" type="button">选择项目目录</button>
            <button class="btn" id="wzScanWsBtn" type="button">扫描工作区目录</button>
          </div>
        </div>
      `;
      host.querySelector("#wzPickDirBtn")?.addEventListener("click", async () => {
        const dir = await API.pickFolder("选择项目根目录");
        if (!dir) return;
        try {
          await API.addProjectByPath(dir);
          await app.reloadConfig(true);
          render();
        } catch (err) {
          toast(`${err}`, "err");
        }
      });
      host.querySelector("#wzScanWsBtn")?.addEventListener("click", async () => {
        await closeAndDone(false);
        app.openDiscoverProjectsDialog();
      });
    } else if (step === 2 && proj) {
      try {
        const [matrix, prereq] = await Promise.all([
          API.getProjectProfileMatrix(proj.id),
          API.inspectProjectPrerequisites(proj.id),
        ]);
        if (seq !== renderSeq || !dlg.isConnected) return;
        const missingRepairable = prereq.items.filter((i) => i.status !== "ok" && i.repairable);
        host.innerHTML = `
          <div class="hr-block" style="margin-bottom:10px">
            <div class="hr-block-title">识别到的语言与打包外壳</div>
            <div class="field-row" style="gap:8px;flex-wrap:wrap">
              <span class="tag tag-ok">主语言：${esc(proj.primaryLanguage || matrix.primaryLanguage)}</span>
              <span class="tag tag-ok">外壳/打包：${esc(proj.shellType || matrix.primaryShell)}</span>
              <span class="muted" style="font-size:11.5px">（稍后可在「编译与环境」自由切换）</span>
            </div>
          </div>
          <div class="hr-block">
            <div class="hr-block-title">当前项目前置环境检查（仅查本项目所需项）</div>
            ${prereq.items
              .map(
                (it) => `<div class="hr-row">
                  <div>
                    <span class="dot ${it.status === "ok" ? "dot-ok" : it.required ? "dot-err" : "dot-warn"}" style="margin-right:6px"></span>
                    <strong>${esc(it.name)}</strong>
                  </div>
                  <span class="muted">${esc(it.detail)}</span>
                </div>`
              )
              .join("")}
            ${
              missingRepairable.length
                ? `<div class="field-row" style="margin-top:10px;justify-content:flex-end">
                    <button class="btn btn-primary" id="wzRepairBtn" type="button">同意并一键补全 ${missingRepairable.length} 项缺失</button>
                  </div>`
                : `<div class="muted" style="margin-top:8px;font-size:12px">核心编译前置已就绪，可直接进入下一步。</div>`
            }
          </div>
        `;
        host.querySelector("#wzRepairBtn")?.addEventListener("click", (e) =>
          withBusy(e.currentTarget, "补全中…", async () => {
            try {
              const ids = missingRepairable.map((i) => i.id);
              const res = await API.executePrerequisiteRepair(proj.id, ids);
              toast(
                res.failed.length
                  ? `部分补全完成（失败 ${res.failed.length} 项）`
                  : `已补全 ${res.succeeded.length} 项前置依赖`,
                res.failed.length ? "warn" : "ok"
              );
              render();
            } catch (err) {
              toast(`${err}`, "err");
            }
          })
        );
      } catch (err) {
        host.innerHTML = `<div class="notice notice-err">${esc(err)}</div>`;
      }
    } else if (step === 3 && proj) {
      const creds = await API.listRepoCredentials().catch(() => []);
      if (seq !== renderSeq || !dlg.isConnected) return;
      const curProv = draftProv || proj.repoBinding?.provider || "github";
      const curBase = draftBaseUrl || proj.repoBinding?.baseUrl || "";
      const curSlug = draftRepoSlug || proj.repoBinding?.repoSlug || proj.githubRepo || "";
      if (!selectedCredentialId) selectedCredentialId = proj.repoBinding?.credentialId || "";
      host.innerHTML = `
        <div class="hr-block" style="margin-bottom:10px">
          <div class="hr-block-title">1. 添加仓库平台令牌（Windows DPAPI 加密保存，不落明文）</div>
          <div class="grid-2" style="gap:8px;margin-bottom:8px">
            <div class="field">
              <label for="wzProv">托管平台</label>
              <select class="select" id="wzProv">
                <option value="github" ${curProv === "github" ? "selected" : ""}>GitHub</option>
                <option value="gitee" ${curProv === "gitee" ? "selected" : ""}>Gitee (码云)</option>
                <option value="gitlab" ${curProv === "gitlab" ? "selected" : ""}>GitLab (官方或自建)</option>
                <option value="gitea" ${curProv === "gitea" ? "selected" : ""}>Gitea / Forgejo</option>
              </select>
            </div>
            <div class="field">
              <label for="wzBaseUrl">服务地址（官方留空，自建填 https://…）</label>
              <input class="input mono" id="wzBaseUrl" value="${esc(curBase)}" placeholder="默认官方地址">
            </div>
          </div>
          <div class="field-row" style="gap:6px">
            <input class="input mono" id="wzToken" type="password" style="flex:1" placeholder="粘贴个人访问令牌 (PAT)，需含 repo/release 权限">
            <button class="btn btn-primary" id="wzSaveTokBtn" type="button">验证并保存令牌</button>
          </div>
          ${
            creds.length
              ? `<div class="dim" style="margin-top:6px;font-size:11.5px">已保存凭据：${creds.map((c) => `${esc(c.provider)}:${esc(c.username)} (${esc(c.maskedToken)})`).join(" · ")}</div>`
              : ""
          }
        </div>
        <div class="hr-block">
          <div class="hr-block-title">2. 选择或填写目标仓库并绑定到本地 Git</div>
          <div class="field-row" style="gap:6px;margin-bottom:8px">
            <input class="input mono" id="wzRepoSlug" style="flex:1" value="${esc(curSlug)}" placeholder="owner/repo">
            <button class="btn" id="wzLoadReposBtn" type="button">拉取我的仓库列表</button>
          </div>
          <select class="select hidden" id="wzRepoSelect" style="margin-bottom:8px"></select>
          <div class="field-row" style="justify-content:space-between">
            <label class="sf-switch-label" style="font-size:12px">
              <input class="sf-switch" type="checkbox" switch id="wzInitGit" checked>
              <span>未初始化 Git 时自动初始化并绑定 origin</span>
            </label>
            <button class="btn btn-primary" id="wzBindBtn" type="button">保存仓库绑定</button>
          </div>
        </div>
      `;

      host.querySelector("#wzSaveTokBtn")?.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "验证中…", async () => {
          const provider = host.querySelector("#wzProv").value;
          const baseUrl = host.querySelector("#wzBaseUrl").value.trim();
          const token = host.querySelector("#wzToken").value.trim();
          draftProv = provider;
          draftBaseUrl = baseUrl;
          draftRepoSlug = host.querySelector("#wzRepoSlug")?.value.trim() || draftRepoSlug;
          if (!token) {
            toast("请先粘贴访问令牌 (PAT)", "warn");
            host.querySelector("#wzToken")?.focus();
            return;
          }
          try {
            const meta = await API.verifyAndSaveRepoCredential({ provider, baseUrl, token });
            selectedCredentialId = meta.id || selectedCredentialId;
            toast(`已验证并加密保存：${meta.username}`, "ok");
            render();
          } catch (err) {
            toast(`${err}`, "err", 4200);
          }
        })
      );

      host.querySelector("#wzLoadReposBtn")?.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "拉取中…", async () => {
          const provider = host.querySelector("#wzProv").value;
          const baseUrl = host.querySelector("#wzBaseUrl").value.trim();
          try {
            const repos = await API.listRemoteRepos(provider, baseUrl, selectedCredentialId || "");
            const sel = host.querySelector("#wzRepoSelect");
            if (!repos.length) {
              toast("未查询到可写仓库", "warn");
              return;
            }
            sel.innerHTML =
              `<option value="">— 从列表中选择仓库 —</option>` +
              repos
                .map(
                  (r) =>
                    `<option value="${esc(r.fullName)}" data-clone="${esc(r.cloneUrl)}">${esc(r.fullName)}${r.private ? " [私有]" : ""}</option>`
                )
                .join("");
            sel.classList.remove("hidden");
            sel.onchange = () => {
              if (sel.value) {
                host.querySelector("#wzRepoSlug").value = sel.value;
                draftRepoSlug = sel.value;
              }
            };
          } catch (err) {
            toast(`${err}`, "err", 4200);
          }
        })
      );

      host.querySelector("#wzBindBtn")?.addEventListener("click", (e) =>
        withBusy(e.currentTarget, "绑定中…", async () => {
          const provider = host.querySelector("#wzProv").value;
          const baseUrl = host.querySelector("#wzBaseUrl").value.trim();
          const repoSlug = host.querySelector("#wzRepoSlug").value.trim();
          if (!repoSlug || !repoSlug.includes("/")) {
            toast("请填写或选择正确的仓库标识（owner/repo）", "warn");
            host.querySelector("#wzRepoSlug")?.focus();
            return;
          }
          const sel = host.querySelector("#wzRepoSelect");
          const cloneUrl = sel?.selectedOptions?.[0]?.dataset?.clone || null;
          const initGitIfNeeded = host.querySelector("#wzInitGit").checked;
          try {
            await API.bindProjectRepo({
              projectId: proj.id,
              provider,
              baseUrl,
              repoSlug,
              credentialId: selectedCredentialId || "",
              cloneUrl,
              initGitIfNeeded,
            });
            draftProv = provider;
            draftBaseUrl = baseUrl;
            draftRepoSlug = repoSlug;
            await app.reloadConfig(true);
            toast(`已绑定仓库：${repoSlug}`, "ok");
          } catch (err) {
            toast(`${err}`, "err", 4200);
          }
        })
      );
    }
  };

  dlg.addEventListener("cancel", (e) => {
    e.preventDefault();
    closeAndDone(true);
  });

  document.body.appendChild(dlg);
  dlg.showModal();
  render();
}
