import { API, esc } from "../api.js";
import {
  attachLightDismissFallback,
  confirmDialog,
  getAccentPreference,
  getStoredTheme,
  setAccentPreference,
  setThemePreference,
  toast,
} from "../ui.js";

let activeDlg = null;

export function closeCommandPalette() {
  if (activeDlg) {
    try {
      if (activeDlg.open) activeDlg.close("");
    } catch {}
    activeDlg.remove();
    activeDlg = null;
  }
}

function buildCommands(app) {
  const cmds = [];
  const proj = app.activeProject;

  // 1. Workspace Navigation
  const navItems = [
    { group: "工作区导航", title: "发版总览", sub: "状态总览、一键构建并发布与近期记录", kbd: "", run: () => app.navigate("overview") },
    { group: "工作区导航", title: "编译与环境 · 框架模板", sub: "推荐框架模板与语言外壳矩阵", kbd: "", run: () => app.navigate("compile", "presets") },
    { group: "工作区导航", title: "编译与环境 · 环境自检", sub: "检查并一键补全项目前置依赖", kbd: "", run: () => app.navigate("compile", "prereq") },
    { group: "工作区导航", title: "编译与环境 · 命令与工序", sub: "执行临时命令或单项构建步骤", kbd: "", run: () => app.navigate("compile", "actions") },
    { group: "工作区导航", title: "分析与分包 · 体积与敏感分析", sub: "分析目录体积分布与敏感文件拦截", kbd: "", run: () => app.navigate("package", "analyze") },
    { group: "工作区导航", title: "分析与分包 · 分包与分卷工坊", sub: "按预设打包、切分分卷与保存自定义方案", kbd: "", run: () => app.navigate("package", "pack") },
    { group: "工作区导航", title: "产物与仓库 · 产物分级与清单", sub: "SHA-256 核验、阶段流转与 latest.json 生成", kbd: "", run: () => app.navigate("release", "artifacts") },
    { group: "工作区导航", title: "产物与仓库 · 多平台发布", sub: "GitHub / Gitee / GitLab / Gitea 发布、编辑、下架与撤回", kbd: "", run: () => app.navigate("release", "repos") },
    { group: "工作区导航", title: "产物与仓库 · 本机更新源", sub: "启动 127.0.0.1 本地测试更新源", kbd: "", run: () => app.navigate("release", "local") },
    { group: "工作区导航", title: "版本与源码 · 变更日志", sub: "Apple Notes 双栏编写与阅读 CHANGELOG", kbd: "", run: () => app.navigate("version", "changelog") },
    { group: "工作区导航", title: "版本与源码 · 版本同步", sub: "多配置文件原子同步与版本隔离工作区", kbd: "", run: () => app.navigate("version", "sync") },
    { group: "工作区导航", title: "版本与源码 · Git 源码", sub: "提交并推送、撤回未推送提交、打标与删除标签", kbd: "", run: () => app.navigate("version", "git") },
    { group: "工作区导航", title: "版本与源码 · 构建台账", sub: "构建锁监控与历史构建记录", kbd: "", run: () => app.navigate("version", "journal") },
  ];
  cmds.push(...navItems);

  // 2. Quick Actions & Preferences
  cmds.push(
    { group: "快捷操作", title: "扫描项目改动与新版本", sub: "探测源码、版本与产物变动", kbd: "F5", run: () => app.probeProjectChangesNow() },
    { group: "快捷操作", title: "切换执行终端显示", sub: "打开或隐藏 5 态交互终端", kbd: "Ctrl+`", run: () => app.toggleTerminal() },
    { group: "快捷操作", title: "折叠 / 展开左侧栏", sub: "扩大主画布空间", kbd: "Ctrl+B", run: () => app.toggleSidebar() },
    { group: "快捷操作", title: "偏好设置 · 通用与外观", sub: "主题、强调色与自动监测设置", kbd: "Ctrl+,", run: () => app.openPreferences("general") },
    { group: "快捷操作", title: "偏好设置 · 当前项目配置", sub: "路径、版本源与自定义工序", kbd: "", run: () => app.openPreferences("project") },
    { group: "快捷操作", title: "偏好设置 · 存储与扫描", sub: "工作区扫描目录与数据盘迁移", kbd: "", run: () => app.openPreferences("storage") },
    { group: "快捷操作", title: "偏好设置 · 工具链中心", sub: "15 项本机工具链体检与一键修复", kbd: "", run: () => app.openPreferences("toolchain") },
    { group: "快捷操作", title: "偏好设置 · DPAPI 凭据保险箱", sub: "管理多平台仓库访问令牌", kbd: "", run: () => app.openPreferences("credentials") },
    { group: "快捷操作", title: "偏好设置 · 安全缓冲", sub: "磁盘余量、并发上限与自动回滚保护", kbd: "", run: () => app.openPreferences("guardrails") },
    { group: "快捷操作", title: "偏好设置 · 版本与更新", sub: "检查 ShipForge 官方更新、下载安装与查看变更记录", kbd: "", run: () => app.openPreferences("updates") },
    { group: "快捷操作", title: "打开 ShipForge 数据目录", sub: "在资源管理器打开配置与工具根目录", kbd: "", run: () => app.openDataDir() },
    { group: "快捷操作", title: "导入本地项目", sub: "选择一个软件项目目录加入工作台", kbd: "", run: () => app.importProject() },
    { group: "快捷操作", title: "自动扫描工作区项目", sub: "批量发现并导入本地代码仓库", kbd: "", run: () => app.openDiscoverProjectsDialog() },
    { group: "快捷操作", title: "打开 3 步快速配置向导", sub: "导入项目 → 环境依赖 → 仓库绑定", kbd: "", run: () => app.openOnboardingWizard() },
    { group: "快捷操作", title: "打开使用说明书", sub: "查看当前页面操作说明与快捷键", kbd: "F1", run: () => app.openManual() }
  );

  // 3. Switch Projects
  for (const p of app.config.projects || []) {
    const isCur = proj?.id === p.id;
    cmds.push({
      group: "切换项目",
      title: `${p.name}${isCur ? "（当前）" : ""}`,
      sub: `${p.kind.toUpperCase()} · ${p.root}`,
      kbd: isCur ? "当前" : "切换",
      run: () => app.switchProject(p.id),
    });
  }

  // 4. Current Project Actions
  if (proj && Array.isArray(proj.actions)) {
    for (const act of proj.actions) {
      cmds.push({
        group: `当前项目工序 (${proj.name})`,
        title: act.label,
        sub: act.description || `${act.command} ${(act.args || []).join(" ")}`,
        kbd: "运行",
        run: async () => {
          if (act.dangerous) {
            const ok = await confirmDialog({
              title: `确认执行：${act.label}`,
              message: act.description || "该操作会清理缓存或修改已有产物，确认继续？",
              detail: `${act.command} ${(act.args || []).join(" ")}`,
              confirmText: "继续执行",
              danger: true,
            });
            if (!ok) return;
          }
          app.runAction(act, {});
        },
      });
    }
  }

  // 5. Appearance & Accent
  const curTheme = getStoredTheme() || "system";
  const curAccent = getAccentPreference();
  cmds.push(
    {
      group: "外观与强调色",
      title: `外观：浅色模式${curTheme === "light" ? " ✓" : ""}`,
      sub: "Apple Notes 柔和灰侧栏 + 纯白画布",
      kbd: "",
      run: () => { setThemePreference("light"); toast("已切换为浅色外观", "ok", 1400); },
    },
    {
      group: "外观与强调色",
      title: `外观：深色模式${curTheme === "dark" ? " ✓" : ""}`,
      sub: "macOS Sequoia 深色画布",
      kbd: "",
      run: () => { setThemePreference("dark"); toast("已切换为深色外观", "ok", 1400); },
    },
    {
      group: "外观与强调色",
      title: `外观：跟随系统${curTheme === "system" ? " ✓" : ""}`,
      sub: "自动适配 Windows 浅/深色设置",
      kbd: "",
      run: () => { setThemePreference("system"); toast("外观已设为跟随系统", "ok", 1400); },
    },
    {
      group: "外观与强调色",
      title: `强调色：Apple Notes 暖金琥珀${curAccent === "amber" ? " ✓" : ""}`,
      sub: "默认暖金琥珀（#D98A00 / #F5B731）",
      kbd: "",
      run: () => { setAccentPreference("amber"); toast("已切换为 Apple Notes 暖金琥珀", "ok", 1400); },
    },
    {
      group: "外观与强调色",
      title: `强调色：macOS 系统蓝${curAccent === "blue" ? " ✓" : ""}`,
      sub: "经典 macOS 蓝色强调（#007AFF / #0A84FF）",
      kbd: "",
      run: () => { setAccentPreference("blue"); toast("已切换为 macOS 系统蓝", "ok", 1400); },
    }
  );

  if (proj) {
    cmds.push({
      group: "项目管理",
      title: `在资源管理器打开「${proj.name}」`,
      sub: proj.root,
      kbd: "",
      run: () => API.openInExplorer(proj.root).catch((e) => toast(`${e}`, "err")),
    });
  }

  return cmds;
}

export function openCommandPalette(app) {
  if (activeDlg?.open) {
    closeCommandPalette();
    return;
  }
  closeCommandPalette();

  const allCommands = buildCommands(app);
  let filtered = allCommands;
  let selectedIdx = 0;

  const dlg = document.createElement("dialog");
  dlg.className = "spotlight-dialog";
  dlg.setAttribute("closedby", "any");
  dlg.setAttribute("aria-label", "Spotlight 命令面板");
  dlg.innerHTML = `
    <div class="spotlight-head">
      <svg viewBox="0 0 20 20" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" style="color:var(--text-3);flex-shrink:0">
        <circle cx="9" cy="9" r="5.5"/><path d="M13.2 13.2L17 17"/>
      </svg>
      <input class="spotlight-input" id="spInput" type="search" placeholder="搜索页面、项目、构建工序或外观命令…" autocomplete="off" autofocus>
      <span class="spotlight-kbd">Esc</span>
    </div>
    <div class="spotlight-list scroller" id="spList" role="listbox"></div>
    <div class="spotlight-foot">
      <span>↑↓ 选择 · Enter 执行</span>
      <span class="mono" id="spCount"></span>
    </div>
  `;

  attachLightDismissFallback(dlg);
  dlg.addEventListener("close", () => {
    dlg.remove();
    if (activeDlg === dlg) activeDlg = null;
  });

  const inp = dlg.querySelector("#spInput");
  const listEl = dlg.querySelector("#spList");
  const countEl = dlg.querySelector("#spCount");

  const execute = (idx) => {
    const item = filtered[idx];
    if (!item) return;
    closeCommandPalette();
    item.run();
  };

  const renderList = () => {
    const q = inp.value.trim().toLowerCase();
    filtered = q
      ? allCommands.filter((c) => `${c.group} ${c.title} ${c.sub}`.toLowerCase().includes(q))
      : allCommands;
    selectedIdx = Math.max(0, Math.min(selectedIdx, Math.max(0, filtered.length - 1)));
    if (countEl) countEl.textContent = `${filtered.length} 项命令`;

    if (!filtered.length) {
      listEl.innerHTML = `<div class="dim" style="padding:26px 12px;text-align:center;font-size:12.5px">未找到匹配「${esc(inp.value)}」的命令</div>`;
      return;
    }

    let lastGroup = "";
    const parts = [];
    filtered.forEach((cmd, idx) => {
      if (cmd.group !== lastGroup) {
        lastGroup = cmd.group;
        parts.push(`<div class="spotlight-group">${esc(cmd.group)}</div>`);
      }
      parts.push(`
        <button class="spotlight-item ${idx === selectedIdx ? "selected" : ""}" data-sp-idx="${idx}" type="button" role="option" aria-selected="${idx === selectedIdx}">
          <div class="spotlight-item-main">
            <span class="spotlight-item-title">${esc(cmd.title)}</span>
            <span class="spotlight-item-sub">${esc(cmd.sub)}</span>
          </div>
          ${cmd.kbd ? `<span class="spotlight-kbd">${esc(cmd.kbd)}</span>` : ""}
        </button>
      `);
    });
    listEl.innerHTML = parts.join("");

    listEl.querySelectorAll("[data-sp-idx]").forEach((btn) => {
      btn.addEventListener("mouseenter", () => {
        const idx = Number(btn.dataset.spIdx);
        if (idx === selectedIdx) return;
        listEl.querySelector(".spotlight-item.selected")?.classList.remove("selected");
        btn.classList.add("selected");
        selectedIdx = idx;
      });
      btn.addEventListener("click", () => execute(Number(btn.dataset.spIdx)));
    });
  };

  inp.addEventListener("input", () => {
    selectedIdx = 0;
    renderList();
  });

  inp.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (!filtered.length) return;
      selectedIdx = (selectedIdx + 1) % filtered.length;
      renderList();
      listEl.querySelector(".spotlight-item.selected")?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (!filtered.length) return;
      selectedIdx = (selectedIdx - 1 + filtered.length) % filtered.length;
      renderList();
      listEl.querySelector(".spotlight-item.selected")?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      execute(selectedIdx);
    }
  });

  document.body.appendChild(dlg);
  activeDlg = dlg;
  dlg.showModal();
  renderList();
  inp.focus();
}
