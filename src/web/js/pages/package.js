import { API, esc, formatBytes, formatDuration } from "../api.js";
import { confirmDialog, promptDialog, toast, withBusy } from "../ui.js";

export function renderPackage(root, app, epoch) {
  const ov = app.overview;
  if (!ov) return;
  const { project, rootExists, versions } = ov;
  const activeTab = app.subTabs.package || "analyze";

  root.innerHTML = `
    <div class="page-wrap">
      <div class="page-head">
        <div>
          <h1>分析与分包</h1>
          <div class="page-head-sub">${
            activeTab === "pack"
              ? "按预设或自定义方案选择打包范围、切分仓库兼容分卷，并保存常用分包预设"
              : "分析项目目录体积分布、识别体积大户与敏感文件并一键加入排除规则"
          }</div>
        </div>
        <div class="head-actions">
          ${
            activeTab === "analyze"
              ? `<button class="btn btn-primary" id="anScanBtn" type="button" ${rootExists ? "" : "disabled"}>重新扫描文件</button>`
              : `<button class="btn btn-ghost" id="anGoReleaseBtn" type="button">产物与仓库 →</button>`
          }
        </div>
      </div>

      <div id="packageBody" class="page-body-fill"></div>
    </div>
  `;

  document.getElementById("anGoReleaseBtn")?.addEventListener("click", () => app.navigate("release", "artifacts"));

  const body = document.getElementById("packageBody");
  if (activeTab === "pack") {
    renderPackWorkshop(body, app, project, versions, rootExists, epoch);
  } else {
    renderAnalyzeSection(body, app, project, rootExists, epoch);
  }
}

function renderAnalyzeSection(container, app, project, rootExists, epoch) {
  container.innerHTML = `
    <div id="anSummaryHost"></div>
    <div id="anResultHost">
      <div class="card card-soft muted" style="font-size:12.5px">
        自动跳过 .git / node_modules / target 等巨型缓存目录，快速识别可执行文件、动态库、大文件及 .env / 私钥等敏感文件。
      </div>
    </div>
  `;

  const paintReport = (rep) => {
    const sumHost = document.getElementById("anSummaryHost");
    const host = document.getElementById("anResultHost");
    if (!host) return;
    const maxBytes = Math.max(1, rep.totalBytes);

    if (sumHost) {
      sumHost.innerHTML = `
        <div class="sf-status-strip">
          <div class="sf-status-cell" style="cursor:default">
            <span class="sf-status-label">扫描文件总数</span>
            <span class="sf-status-val mono">${rep.totalFiles} 个</span>
            <span class="sf-status-sub">跳过 ${rep.skippedDirs.length} 类缓存目录</span>
          </div>
          <div class="sf-status-cell" style="cursor:default">
            <span class="sf-status-label">目录有效体积</span>
            <span class="sf-status-val mono">${formatBytes(rep.totalBytes)}</span>
            <span class="sf-status-sub">耗时 ${formatDuration(rep.durationMs)}</span>
          </div>
          <div class="sf-status-cell" style="cursor:default">
            <span class="sf-status-label"><span class="dot ${rep.sensitiveWarnings.length ? "dot-warn" : "dot-ok"}"></span>敏感文件检测</span>
            <span class="sf-status-val">${rep.sensitiveWarnings.length ? `${rep.sensitiveWarnings.length} 项告警` : "未发现风险"}</span>
            <span class="sf-status-sub">${rep.sensitiveWarnings.length ? "打包默认拦截，防密钥泄漏" : "目录安全干净"}</span>
          </div>
          <div class="sf-status-cell" style="cursor:default">
            <span class="sf-status-label">文件分类数</span>
            <span class="sf-status-val mono">${rep.categories.length} 类</span>
            <span class="sf-status-sub">最大单项 ${rep.largestFiles[0] ? formatBytes(rep.largestFiles[0].sizeBytes) : "-"}</span>
          </div>
        </div>
      `;
    }

    host.innerHTML = `
      ${
        rep.truncated
          ? `<div class="notice notice-warn" style="margin-bottom:14px">
              <div>目录文件数超过 80,000 上限，当前统计仅涵盖前 ${rep.totalFiles} 个文件（已自动跳过缓存目录），建议清理多余临时文件后再分析。</div>
            </div>`
          : ""
      }
      ${
        rep.sensitiveWarnings.length
          ? `<div class="notice notice-warn" style="margin-bottom:14px">
              <div>检测到 ${rep.sensitiveWarnings.length} 个敏感文件：${rep.sensitiveWarnings
                .slice(0, 6)
                .map((s) => `<span class="mono">${esc(s.relPath)}</span>`)
                .join("、")}</div>
              <button class="btn btn-xs btn-primary" id="anAddSensitiveExclBtn" type="button">一键加入排除规则并去打包</button>
            </div>`
          : ""
      }
      <div class="grid-2 pack-workshop-grid">
        <section class="card">
          <div class="card-head"><h2>按类型体积分布</h2></div>
          <div style="display:flex;flex-direction:column;gap:8px">
            ${rep.categories
              .slice(0, 8)
              .map((c) => {
                const pct = Math.max(2, Math.round((c.totalBytes / maxBytes) * 100));
                return `<div>
                  <div class="field-row" style="justify-content:space-between;font-size:12px;margin-bottom:3px">
                    <span>${esc(c.label)} <span class="dim">(${c.fileCount})</span></span>
                    <span class="mono muted">${formatBytes(c.totalBytes)}</span>
                  </div>
                  <div class="bar-meter"><div class="bar-meter-fill" style="width:${pct}%"></div></div>
                </div>`;
              })
              .join("")}
          </div>
        </section>

        <section class="card">
          <div class="card-head"><h2>体积最大文件 (Top 10)</h2></div>
          <div class="row-list">
            ${rep.largestFiles
              .slice(0, 10)
              .map(
                (f) => `<div class="list-row" style="padding:6px 10px">
                  <span class="mono" style="font-size:11.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="${esc(f.relPath)}">${esc(f.relPath)}</span>
                  <span class="mono muted" style="font-size:11.5px;white-space:nowrap">${formatBytes(f.sizeBytes)}</span>
                </div>`
              )
              .join("")}
          </div>
        </section>
      </div>
      ${
        rep.skippedDirs.length
          ? `<div class="dim" style="font-size:11.5px;margin-top:8px">已自动跳过缓存目录：${esc(rep.skippedDirs.join(", "))}</div>`
          : ""
      }
    `;

    document.getElementById("anAddSensitiveExclBtn")?.addEventListener("click", () => {
      const existing = app.packageState.extraExcludesByProj.get(project.id) || [];
      const merged = new Set([...existing, ...rep.sensitiveWarnings.map((s) => s.relPath)]);
      app.packageState.extraExcludesByProj.set(project.id, [...merged]);
      toast(`已将 ${rep.sensitiveWarnings.length} 个敏感文件加入分包排除规则`, "ok");
      app.navigate("package", "pack");
    });
  };

  let scanSeq = 0;
  const runAnalysis = async () => {
    const seq = ++scanSeq;
    const host = document.getElementById("anResultHost");
    if (!host) return;
    host.innerHTML = `<div class="card card-soft dim">正在扫描分析项目目录…</div>`;
    try {
      const rep = await API.analyzeProjectFiles(project.id);
      if (seq !== scanSeq || epoch !== app.renderEpoch || !host.isConnected) return;
      app.packageState.lastAnalysisByProj.set(project.id, rep);
      paintReport(rep);
    } catch (err) {
      if (seq === scanSeq && epoch === app.renderEpoch && host.isConnected) {
        host.innerHTML = `<div class="notice notice-err">${esc(err)}</div>`;
      }
    }
  };

  document.getElementById("anScanBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "扫描中…", runAnalysis)
  );

  const cached = app.packageState.lastAnalysisByProj.get(project.id);
  if (cached) paintReport(cached);
  else if (rootExists) runAnalysis();
}

function renderPackWorkshop(container, app, project, versions, rootExists, epoch) {
  let profiles = structuredClone(project.packageProfiles || []);
  const defaultZip = `${project.artifactPrefix || project.productName || project.id}_${versions.current}_x64-portable.zip`;
  const baseExcludes = ["*.pdb", "*.map", "*.log", ".env*", "__pycache__", ".DS_Store"];
  const extraExcludes = app.packageState.extraExcludesByProj.get(project.id) || [];
  const initialExcludes = [...new Set([...baseExcludes, ...extraExcludes])].join(", ");

  container.innerHTML = `
    <!-- Custom & Built-in Package Profiles Bar (wired to API.savePackageProfiles) -->
    <section class="card card-soft" style="padding:12px 15px;gap:10px">
      <div class="field-row" style="justify-content:space-between">
        <span style="font-size:12.5px;font-weight:600">分包方案预设</span>
        <button class="btn btn-xs btn-soft" id="pkSavePresetBtn" type="button" ${rootExists ? "" : "disabled"}>+ 保存当前配置为预设</button>
      </div>
      <div class="chip-grid" id="pkPresetBar"></div>
    </section>

    <div class="grid-2 pack-workshop-grid">
      <section class="card">
        <div class="card-head">
          <h2>1. 选择打包范围</h2>
          <div class="field-row" style="gap:4px">
            <button class="btn btn-xs btn-ghost" id="pkSelectAllBtn" type="button">全选</button>
            <button class="btn btn-xs btn-ghost" id="pkSelectCoreBtn" type="button">核心产物</button>
            <button class="btn btn-xs btn-ghost" id="pkClearSelBtn" type="button">清空</button>
          </div>
        </div>
        <div class="field" style="flex:1">
          <label>根目录条目</label>
          <div class="chip-grid" id="pkEntryChips"><span class="dim">正在读取目录…</span></div>
        </div>
        <div class="field">
          <label for="pkExcludes">排除通配符（逗号分隔）</label>
          <input class="input mono" id="pkExcludes" value="${esc(initialExcludes)}">
        </div>
      </section>

      <section class="card">
        <div class="card-head">
          <h2>2. 输出格式与分卷设置</h2>
          <span class="tag tag-ok">原生流式压缩</span>
        </div>
        <div class="grid-2">
          <div class="field">
            <label for="pkFormat">压缩格式</label>
            <select class="select" id="pkFormat">
              <option value="zip">ZIP (Windows / 通用)</option>
              <option value="targz">TAR.GZ (Linux / 源码)</option>
            </select>
          </div>
          <div class="field">
            <label for="pkSplitMb">自动分卷（适配仓库单文件上限）</label>
            <select class="select" id="pkSplitMb">
              <option value="">不分卷（单文件输出）</option>
              <option value="50">每卷 50 MB</option>
              <option value="95">每卷 95 MB（Gitee 100 MB 兼容）</option>
              <option value="500">每卷 500 MB</option>
              <option value="1900">每卷 1.9 GB（GitHub 2 GB 兼容）</option>
            </select>
          </div>
        </div>
        <div class="field">
          <label for="pkOutName">输出文件名</label>
          <input class="input mono" id="pkOutName" value="${esc(defaultZip)}">
        </div>
        <div class="field" style="flex:1">
          <label for="pkReadme">附带说明文件（可选，写入 使用说明.txt）</label>
          <textarea class="textarea" id="pkReadme" style="min-height:86px;flex:1" placeholder="留空则不额外写入说明文件…"></textarea>
        </div>
        <div class="field-row" style="justify-content:space-between;padding-top:4px;border-top:1px solid var(--line)">
          <label class="sf-switch-label" style="font-size:12px">
            <input class="sf-switch" type="checkbox" switch id="pkAllowSensitive">
            <span>允许包含敏感文件</span>
          </label>
          <button class="btn btn-accent" id="pkRunBtn" type="button" ${rootExists ? "" : "disabled"}>开始打包</button>
        </div>
        <div class="mono muted" id="pkResultInfo" style="font-size:11.5px"></div>
      </section>
    </div>
  `;

  const engineTopDir = (project.engineExePath || "")
    .trim()
    .split(/[\\/]/)
    .find((seg) => seg && seg !== ".");
  const coreDefaults = new Set(
    [
      project.launchExe,
      engineTopDir,
      "WebView2Loader.dll",
      "LICENSE",
      "README.md",
      "THIRD_PARTY.md",
      "dist",
    ].filter(Boolean)
  );

  let entriesLoaded = false;
  let pendingPresetSources = null;
  if (!app.packageState.lastPackResultByProj) {
    app.packageState.lastPackResultByProj = new Map();
  }
  const savedPackInfo = app.packageState.lastPackResultByProj.get(project.id);
  if (savedPackInfo) {
    const infoEl = document.getElementById("pkResultInfo");
    if (infoEl) infoEl.textContent = savedPackInfo;
  }

  const persistProfiles = async (nextList, okMsg) => {
    const updatedProj = await API.savePackageProfiles(project.id, nextList);
    profiles = structuredClone(updatedProj.packageProfiles || nextList);
    if (app.overview?.project?.id === project.id) {
      app.overview.project.packageProfiles = profiles;
    }
    const cfgProj = (app.config.projects || []).find((p) => p.id === project.id);
    if (cfgProj) cfgProj.packageProfiles = profiles;
    renderPresetBar();
    toast(okMsg, "ok", 2200);
  };

  const renderPresetBar = () => {
    const bar = document.getElementById("pkPresetBar");
    if (!bar) return;
    if (!profiles.length) {
      bar.innerHTML = `<span class="dim" style="font-size:12px">暂无已保存分包预设，配置好下方选项后点击「+ 保存当前配置为预设」。</span>`;
      return;
    }
    bar.innerHTML = profiles
      .map(
        (p) => `<span class="pk-preset-chip">
          <button class="pk-preset-chip-btn" data-pk-preset="${esc(p.id)}" type="button" title="${esc(p.nameTemplate || p.format)}">${esc(p.name)}</button>
          <button class="pk-preset-chip-del" data-pk-del="${esc(p.id)}" type="button" title="删除此预设" aria-label="删除预设 ${esc(p.name)}">✕</button>
        </span>`
      )
      .join("");

    bar.querySelectorAll("[data-pk-preset]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const p = profiles.find((x) => x.id === btn.dataset.pkPreset);
        if (!p) return;
        document.getElementById("pkFormat").value = p.format || "zip";
        document.getElementById("pkOutName").value = (p.nameTemplate || defaultZip).replaceAll(
          "{version}",
          versions.current
        );
        document.getElementById("pkExcludes").value = (p.excludes || []).join(", ");
        document.getElementById("pkSplitMb").value = p.splitSizeMb ? String(p.splitSizeMb) : "";
        const readmeEl = document.getElementById("pkReadme");
        if (readmeEl) readmeEl.value = p.includeReadme ? p.readmeContent || "" : "";
        if (p.sources && p.sources.length) {
          if (entriesLoaded) {
            const set = new Set(p.sources);
            container.querySelectorAll('input[name="pkSrc"]').forEach((el) => {
              el.checked = set.has(el.value);
            });
          } else {
            pendingPresetSources = p.sources;
          }
        } else if (entriesLoaded) {
          const anyChecked = container.querySelectorAll('input[name="pkSrc"]:checked').length > 0;
          if (!anyChecked) {
            container.querySelectorAll('input[name="pkSrc"]').forEach((el) => {
              el.checked = coreDefaults.has(el.value);
            });
          }
        }
        toast(`已应用方案：${p.name}`, "ok", 1400);
      });
    });

    bar.querySelectorAll("[data-pk-del]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const target = profiles.find((x) => x.id === btn.dataset.pkDel);
        if (!target) return;
        const ok = await confirmDialog({
          title: `删除分包预设：${target.name}`,
          message: "删除后将从当前项目配置中移除该分包方案，不影响已生成的压缩包。",
          confirmText: "删除预设",
          danger: true,
        });
        if (!ok) return;
        try {
          const nextList = profiles.filter((x) => x.id !== target.id);
          await persistProfiles(nextList, `已删除预设「${target.name}」`);
        } catch (err) {
          toast(`删除失败：${err}`, "err");
        }
      });
    });
  };
  renderPresetBar();

  document.getElementById("pkSavePresetBtn")?.addEventListener("click", async () => {
    const name = await promptDialog({
      title: "保存当前分包配置为预设",
      label: "预设名称",
      placeholder: "例如：Gitee 95MB 精简分卷包",
      confirmText: "保存预设",
    });
    if (!name) return;
    const sources = [...container.querySelectorAll('input[name="pkSrc"]:checked')].map((el) => el.value);
    const format = document.getElementById("pkFormat").value || "zip";
    const outNameRaw = document.getElementById("pkOutName").value.trim() || defaultZip;
    const nameTemplate = versions.current ? outNameRaw.replaceAll(versions.current, "{version}") : outNameRaw;
    const excludes = document
      .getElementById("pkExcludes")
      .value.split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    const splitVal = document.getElementById("pkSplitMb").value;
    const splitSizeMb = splitVal ? Number(splitVal) : null;
    const readmeContent = document.getElementById("pkReadme").value;

    const existingIdx = profiles.findIndex((p) => p.name === name);
    const entry = {
      id: existingIdx >= 0 ? profiles[existingIdx].id : `custom-${Date.now()}`,
      name,
      format,
      nameTemplate,
      sources,
      excludes,
      splitSizeMb,
      includeReadme: Boolean(readmeContent.trim()),
      readmeContent,
    };
    const nextList = [...profiles];
    if (existingIdx >= 0) nextList[existingIdx] = entry;
    else nextList.push(entry);

    try {
      await persistProfiles(nextList, `已保存预设「${name}」`);
    } catch (err) {
      toast(`保存预设失败：${err}`, "err");
    }
  });

  if (!rootExists) {
    const box = document.getElementById("pkEntryChips");
    if (box) box.innerHTML = `<span class="err" style="font-size:12px">项目根目录不存在，无法读取条目</span>`;
  } else {
    API.listProjectEntries(project.id)
      .then((entries) => {
        if (epoch !== app.renderEpoch) return;
        const box = document.getElementById("pkEntryChips");
        if (!box) return;
        entriesLoaded = true;
        const activeSet = pendingPresetSources ? new Set(pendingPresetSources) : coreDefaults;
        pendingPresetSources = null;
        box.innerHTML = entries
          .map(
            (name) => `<label class="check-pill">
              <input type="checkbox" name="pkSrc" value="${esc(name)}" ${activeSet.has(name) ? "checked" : ""}>
              <span class="mono">${esc(name)}</span>
            </label>`
          )
          .join("");
      })
      .catch((err) => {
        if (epoch !== app.renderEpoch) return;
        const box = document.getElementById("pkEntryChips");
        if (box) box.innerHTML = `<span class="err" style="font-size:12px">读取目录失败：${esc(err)}</span>`;
      });
  }

  document.getElementById("pkSelectAllBtn")?.addEventListener("click", () => {
    container.querySelectorAll('input[name="pkSrc"]').forEach((el) => (el.checked = true));
  });
  document.getElementById("pkSelectCoreBtn")?.addEventListener("click", () => {
    container.querySelectorAll('input[name="pkSrc"]').forEach((el) => (el.checked = coreDefaults.has(el.value)));
  });
  document.getElementById("pkClearSelBtn")?.addEventListener("click", () => {
    container.querySelectorAll('input[name="pkSrc"]').forEach((el) => (el.checked = false));
  });

  document.getElementById("pkFormat")?.addEventListener("change", (e) => {
    const inp = document.getElementById("pkOutName");
    if (!inp) return;
    const base = inp.value.replace(/\.(zip|tar\.gz)$/i, "");
    inp.value = e.target.value === "targz" ? `${base}.tar.gz` : `${base}.zip`;
  });

  document.getElementById("pkRunBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "压缩中…", async () => {
      const sources = [...container.querySelectorAll('input[name="pkSrc"]:checked')].map((el) => el.value);
      const format = document.getElementById("pkFormat").value;
      const outputName = document.getElementById("pkOutName").value.trim();
      if (!sources.length) {
        toast("请至少勾选一个要打包的文件或目录", "warn");
        return;
      }
      if (!outputName) {
        toast("请填写输出包文件名", "warn");
        document.getElementById("pkOutName")?.focus();
        return;
      }
      const splitVal = document.getElementById("pkSplitMb").value;
      const splitSizeMb = splitVal ? Number(splitVal) : null;
      const allowSensitive = document.getElementById("pkAllowSensitive").checked;
      const excludes = document
        .getElementById("pkExcludes")
        .value.split(",")
        .map((s) => s.trim())
        .filter(Boolean);
      const readmeContent = document.getElementById("pkReadme").value;
      try {
        const res = await API.packPortableArchive({
          projectId: project.id,
          format,
          outputName,
          sources,
          excludes,
          readmeName: readmeContent.trim() ? "使用说明.txt" : null,
          readmeContent: readmeContent.trim() ? readmeContent : null,
          splitSizeMb,
          allowSensitive,
        });
        const ratio = res.rawBytes ? ((res.compressedBytes / res.rawBytes) * 100).toFixed(1) : "0";
        const summaryLine = `${res.fileName}：${res.fileCount} 文件 · ${formatBytes(res.compressedBytes)} (${ratio}%) · ${formatDuration(res.durationMs)}${
          res.splitParts?.length ? ` · 已切分 ${res.splitParts.length} 卷` : ""
        }`;
        app.packageState.lastPackResultByProj.set(project.id, summaryLine);
        const info = document.getElementById("pkResultInfo");
        if (info) info.textContent = summaryLine;
        toast(`打包完成：${res.fileName}`, "ok", 3200);
        await app.refreshOverview(true);
      } catch (err) {
        toast(`${err}`, "err", 4800);
      }
    })
  );
}
