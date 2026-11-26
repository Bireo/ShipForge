import { esc } from "../api.js";

const SECTIONS = [
  {
    id: "overview",
    title: "1. 发版总览与一键流水线",
    items: [
      "顶部 4 格发丝状态条直观展示版本、环境、目标仓库与 Git/产物状态，点击任意格均可直达对应页。",
      "一键构建并发布实时预览下一版本号（基于 previewBump）；执行中途取消或出错时自动强杀子进程、安全回滚版本与 Git 改动，并在完成后弹出一次性健康报告。",
      "左侧栏下方「我的项目」仿 Apple Notes 文件夹直出列表，支持一键切换与行内快捷菜单（含打开目录、项目配置、重新识别与移出列表）。",
    ],
  },
  {
    id: "compile",
    title: "2. 编译与环境（框架模板 · 环境自检 · 命令与工序）",
    items: [
      "「框架模板」自动高亮置顶当前匹配的推荐框架，下方网格提供全栈预设，展开「语言与外壳矩阵」可自由微调组合。",
      "「环境自检」只检查当前项目实际所需的工具、依赖与发布配置，支持先预览清单再一键补全。",
      "「命令与工序」支持运行临时命令或单项构建、测试与维护步骤。",
    ],
  },
  {
    id: "package",
    title: "3. 分析与分包（体积与敏感分析 · 分包与分卷工坊）",
    items: [
      "「体积与敏感分析」自动跳过缓存目录，展示按类型体积占比、Top 10 大文件与敏感文件告警，并可一键加入排除规则。",
      "「分包与分卷工坊」支持 ZIP / TAR.GZ 压缩、按仓库单文件限制自动分卷（如 Gitee 95 MB），并可将常用配置保存或删除为自定义分包预设。",
    ],
  },
  {
    id: "release",
    title: "4. 产物与仓库（产物分级与清单 · 多平台发布 · 本机更新源）",
    items: [
      "「产物分级与清单」按 待验证 → 已验证 → 可发布 → 已上架 → 已归档 流转，SHA-256 核验自动识别未入清单旧包且绝不降级已就绪产物。",
      "「多平台发布」采用 Apple Notes 双栏布局，原生直连 GitHub / Gitee / GitLab / Gitea：左栏浏览全部线上版本，右栏发布新版本或编辑说明、下架为草稿、删除单个资产及彻底撤回发布（可选同步删除远端与本地 Tag）。",
      "「本机更新源」在 127.0.0.1 启动测试更新源，动态内存重写 latest.json 用于客户端冒烟测试。",
    ],
  },
  {
    id: "version",
    title: "5. 版本与源码 & 偏好设置快捷键",
    items: [
      "「变更日志」采用 Apple Notes 左历史列表 + 右纯白编辑/阅读双栏排版；「版本同步」原子同步多配置与 Cargo.lock；「Git 源码」支持一键提交并推送、未提交文件推送拦截提醒、安全撤回未推送本地提交以及管理/删除本地与远端标签。",
      "偏好设置可通过顶部工具栏右侧齿轮图标、命令面板或快捷键 Ctrl+, 打开，提供 macOS 双栏面板：通用与外观（浅深主题 + 暖金琥珀 / 系统蓝强调色）、当前项目配置、存储与扫描、工具链中心、DPAPI 凭据保险箱、安全缓冲、版本与更新（检查与安装 ShipForge 官方更新）。",
      "全局快捷键与右键面板：全界面内置 macOS 风格情境右键菜单（自动适配输入框剪贴板、项目行、产物行、线上版本、Git 提交/标签与终端）；快捷键含 Ctrl+K 命令面板、Ctrl+, 偏好设置、Ctrl+B 折叠侧栏、F5 变动扫描、Ctrl+` 5 态执行终端、F1 使用说明、Esc 退出全屏终端或关闭弹窗。",
    ],
  },
];

let backdropEl = null;

export function openManualDrawer(currentPage = "overview", app = null) {
  closeManualDrawer();

  const el = document.createElement("div");
  el.className = "manual-drawer-backdrop";
  el.innerHTML = `
    <aside class="manual-drawer" role="dialog" aria-label="ShipForge 使用说明书">
      <div class="manual-drawer-head">
        <div>
          <strong style="font-size:14px">使用说明书</strong>
          <div class="dim" style="font-size:11.5px">已定位到当前工作区操作说明</div>
        </div>
        <div class="field-row" style="gap:6px">
          <button class="btn btn-xs" id="manualOpenWizardBtn" type="button">3 步上手向导</button>
          <button class="icon-btn" id="manualCloseBtn" type="button" aria-label="关闭">✕</button>
        </div>
      </div>
      <div class="manual-drawer-body scroller">
        ${SECTIONS.map(
          (sec) => `<section class="manual-section ${sec.id === currentPage ? "highlight" : ""}" data-sec="${esc(sec.id)}">
            <h3>${esc(sec.title)}</h3>
            <ol>${sec.items.map((line) => `<li>${esc(line)}</li>`).join("")}</ol>
          </section>`
        ).join("")}
      </div>
    </aside>
  `;

  el.addEventListener("click", (e) => {
    if (e.target === el) closeManualDrawer();
  });
  el.querySelector("#manualCloseBtn")?.addEventListener("click", closeManualDrawer);
  el.querySelector("#manualOpenWizardBtn")?.addEventListener("click", () => {
    closeManualDrawer();
    if (app && typeof app.openOnboardingWizard === "function") {
      app.openOnboardingWizard();
    }
  });

  document.body.appendChild(el);
  backdropEl = el;
  const targetSec = el.querySelector(`[data-sec="${currentPage}"]`);
  if (targetSec) targetSec.scrollIntoView({ block: "nearest" });
}

export function closeManualDrawer() {
  if (backdropEl) {
    backdropEl.remove();
    backdropEl = null;
  }
}
