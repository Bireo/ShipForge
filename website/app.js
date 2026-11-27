/* ============================================================================
   ShipForge 轻筑 — Website Interaction & Motion Engine
   - Two-state dark mode via <meta name="color-scheme">
   - Bilingual ZH/EN switcher
   - Scroll-driven fallback for hero 3D perspective stage, shrinking header & progress hairline
   - Accessible stage tabs with keyboard nav, beforematch & CSS Anchor underline
   - Gradual bidirectional blur-to-crisp float-in reveal with 64px hysteresis & bottom guard
   - Interactive metrics chart toggle + cubic-ease count-up numbers
   - Live GitHub star count & latest.json manifest hydration
   - Native <dialog closedby="any"> Sponsor modal & Shared-Element View-Transition Lightbox
   ============================================================================ */

(() => {
  'use strict';

  const REPO_SLUG = 'Bireo/ShipForge';

  const COPY = {
    zh: {
      brand: '轻筑',
      'nav.showcase': '核心流程',
      'nav.bench': '工程指标',
      'nav.download': '下载获取',
      'nav.star': '点 Star',
      'nav.get': '立即下载',
      'hero.title': '本地构建、分包与多仓发布<br>收进一座 <span class="title-accent">Windows 轻筑工作台</span>',
      'hero.sub': '原生 Rust + Tauri v2，安装包仅 3.4 MB。自动识别 8 类常见工程与前置依赖，拦截 <code>.env</code>/密钥泄漏并按 95 MB 自动分卷，多文件版本号原子同步，流式发布至 GitHub / Gitee / GitLab / Gitea，自带 SHA-256 在线更新。',
      'hero.dlExe': '下载安装版',
      'hero.dlZip': '便携免安装版',
      'hero.viewGh': '查看源码仓库',
      'tabs.overview': '发版总览',
      'tabs.compile': '编译与环境',
      'tabs.package': '分析与分包',
      'tabs.release': '多仓发布',
      'story.eyebrow': 'END-TO-END PIPELINE · 核心发版链路',
      'story.title': '从代码写完到用户收到更新，每一步可看、可控、可回滚',
      'story.s1Title': '成品就绪感知与免重编直推',
      'story.s1Desc': '切入项目即比对当前版本号、源码树与 <code>release/</code> 产物状态。若成品已通过测试并对齐 <code>SHA256SUMS.txt</code> 与 <code>latest.json</code>，直接一键补齐校验并部署远端，不再重复浪费编译时间。',
      'story.s1Proof': '多项目隔离运行态 · 跨工程不串日志、不串状态',
      'story.s2Title': '多框架编译预设与环境前置自检',
      'story.s2Desc': '自动探测 Rust / Node / Python / Go / .NET / Flutter / C++ / Java 与 Tauri / Electron / Wails / NSIS 桌面外壳。开编前核验磁盘余量、内存水位与工具链版本，支持一键托管下载便携版工具链，失败自动弹出健康诊断并原路回滚版本号。',
      'story.s2Proof': '开编前挡住 90% 环境报错 · 进程树取消即刻释放锁',
      'story.s3Title': '体积分析、敏感拦截与大包智能分卷',
      'story.s3Desc': '毫秒级扫描产物分布并列出最大文件排行，自动跳过 <code>target/</code> 与 <code>node_modules/</code>。打包阶段默认拦截 <code>.env</code>、私钥证书与凭据文件，超过 Gitee 100 MB 单文件上限时按 95 MB 自动切分卷并附带自解压合并脚本。',
      'story.s3Proof': '防密钥外泄硬门禁 · ZIP / 便携包模板一键复用',
      'story.s4Title': '多仓流式发布、版本同步与在线自动更新',
      'story.s4Desc': '统一管理 GitHub、Gitee、GitLab 与 Gitea 凭据（本机 Windows DPAPI 加密，明文不落盘），支持 64 KB 流式上传、原子 Draft 先传二进制再公开、在线编辑说明并同步 <code>latest.json</code>；同时内置三通道容错更新器，安装版静默升级、便携版备份替换且异常自动回滚。',
      'story.s4Proof': 'WinHTTP 流式发版 · SHA-256 全链路核验与回滚',
      'bench.eyebrow': 'ENGINEERING METRICS · 实测工程指标',
      'bench.title': '原生轻量化与自动化门禁，每一项数据可源码复验',
      'bench.desc': '纯原生 Rust 后端 + 零框架 WebView2 前端（0 <code>node_modules</code>），所有门禁由 <code>build.ps1</code> 自动执行并强制 0 警告通过。',
      'bench.chartTitle': '典型体积与发版效率对比',
      'bench.modeFootprint': '体积占用',
      'bench.modePipeline': '门禁耗时',
      'bench.s1Title': 'NSIS 安装包实测体积',
      'bench.s1Meta': 'Rust + Tauri v2 · 0 第三方网络与压缩外部依赖',
      'bench.s2Title': 'Rust 后端单测全绿',
      'bench.s2Meta': '覆盖版本号、分包防泄漏、原子更新与回滚逻辑',
      'bench.s3Title': '前端与官网契约门禁',
      'bench.s3Meta': '零警告 Release 编译 + 语法/字体/交互全自动检查',
      'bench.s4Title': '原生流式多仓发布',
      'bench.s4Unit': '平台',
      'bench.s4Meta': 'GitHub · Gitee · GitLab · Gitea，凭据 DPAPI 加密',
      'dl.eyebrow': 'GET SHIPFORGE · 立即获取',
      'dl.title': '两个版本，30 秒接入你的 Windows 工程',
      'dl.desc': '项目列表与 DPAPI 加密凭据保存在 <code>%APPDATA%\\ShipForge</code>，升级重装不丢配置；支持一键核验 SHA-256。',
      'dl.rec': '推荐 · 日常主力',
      'dl.port': '免安装 · 随身便携',
      'dl.exeTitle': 'Windows 安装版 (NSIS x64)',
      'dl.exeDesc': '标准安装向导，自动创建桌面与开始菜单入口、干净卸载；后续更新可在软件内直接核验 SHA-256 静默升级。',
      'dl.exeBtn': '下载安装版 (.exe)',
      'dl.zipTitle': 'Windows 便携版 (Portable ZIP)',
      'dl.zipDesc': '解压到任意目录双击即用，不写注册表；内置更新器升级前自动备份旧版，写入异常自动原样回滚。',
      'dl.zipBtn': '下载便携版 (.zip)',
      'dev.summary': '开发者说明：三层架构、源码门禁与一键发版命令',
      'dev.l1': '零框架 WebView2 前端',
      'dev.l2': 'Rust 原生引擎 (Tauri v2)',
      'dev.l3': '三通道原子更新器',
      'dev.copy': '复制命令',
      'dev.copied': '已复制 ✓',
      'foot.note': 'MIT 开源 · 本地优先、零遥测、零广告',
      'foot.coffee': '请作者喝杯咖啡',
      'sponsor.title': '请作者喝杯咖啡',
      'sponsor.sub': '软件独立开发与长期维护不易，如果 ShipForge 轻筑帮你省下了重复打包和发版的时间，欢迎请作者喝杯咖啡，感谢支持！',
      'sponsor.wechat': '微信支付 · 扫一扫赞赏',
      'sponsor.alipay': '支付宝 · 扫一扫赞赏',
      'sponsor.note': 'ShipForge 永久 MIT 开源免费、零广告 · 随心赞赏，在 GitHub 点 Star 同样是莫大的鼓励',
      'sponsor.close': '谢谢，先逛逛',
      'lightbox.hint': '查看大图',
      'lightbox.zoomIn': '2× 细节',
      'lightbox.zoomOut': '还原 1×',
      'lightbox.keys': '← / → 切换 · 滚轮/点击缩放 · Esc 关闭'
    },
    en: {
      brand: 'Studio',
      'nav.showcase': 'Workflow',
      'nav.bench': 'Metrics',
      'nav.download': 'Download',
      'nav.star': 'Star',
      'nav.get': 'Download',
      'hero.title': 'Local builds, smart packaging & multi-forge releases<br>in one <span class="title-accent">Windows Studio</span>',
      'hero.sub': 'Native Rust + Tauri v2 in a 3.4 MB installer. Detects 8 project stacks with preflight checks, blocks <code>.env</code>/key leaks with 95 MB split ZIPs, syncs multi-file versions atomically, streams releases to GitHub / Gitee / GitLab / Gitea, and ships with SHA-256 auto-updates.',
      'hero.dlExe': 'Download Installer',
      'hero.dlZip': 'Portable ZIP',
      'hero.viewGh': 'View Repository',
      'tabs.overview': 'Overview',
      'tabs.compile': 'Compile & Preflight',
      'tabs.package': 'Analyze & Package',
      'tabs.release': 'Multi-Forge',
      'story.eyebrow': 'END-TO-END PIPELINE · RELEASE WORKFLOW',
      'story.title': 'From clean commit to verified user update — visible, guarded, reversible',
      'story.s1Title': 'Artifact Readiness & Zero-Rebuild Deploy',
      'story.s1Desc': 'Inspects active version files, working tree, and <code>release/</code> outputs on switch. When artifacts already match <code>SHA256SUMS.txt</code> and <code>latest.json</code>, push straight to your forge without wasting another build cycle.',
      'story.s1Proof': 'Per-project isolated runtime · Zero log or state bleed across repos',
      'story.s2Title': 'Multi-Stack Build Matrix & Preflight Inspector',
      'story.s2Desc': 'Auto-detects Rust, Node, Python, Go, .NET, Flutter, C++, Java alongside Tauri, Electron, Wails, and NSIS. Verifies disk headroom, memory, and toolchains before launching, with portable toolchain bootstrap and automatic version rollback on failure.',
      'story.s2Proof': 'Catches 90% of setup issues upfront · Process-tree cancel frees locks immediately',
      'story.s3Title': 'Size Breakdown, Secret Guard & 95 MB Split ZIPs',
      'story.s3Desc': 'Scans output weight and largest files in milliseconds while skipping <code>target/</code> and <code>node_modules/</code>. Blocks <code>.env</code> and private keys by default, and splits large payloads into 95 MB Gitee-safe volumes with a merge script.',
      'story.s3Proof': 'Hard gate against accidental secret leaks · Reusable packaging profiles',
      'story.s4Title': 'Multi-Forge Streaming Upload & 3-Tier Auto-Updater',
      'story.s4Desc': 'Manages GitHub, Gitee, GitLab, and Gitea credentials locally with Windows DPAPI encryption. Uploads via 64 KB WinHTTP streaming with atomic draft-first publishing, online notes + <code>latest.json</code> sync, and a built-in SHA-256 updater with rollback.',
      'story.s4Proof': '4 forges · 3-channel fallback updater · End-to-end SHA-256 verification',
      'bench.eyebrow': 'ENGINEERING METRICS · REPRODUCIBLE DATA',
      'bench.title': 'Native footprint and automated gates — verifiable straight from source',
      'bench.desc': 'Pure Rust backend + framework-free WebView2 UI (0 <code>node_modules</code>). Every gate runs inside <code>build.ps1</code> with strict zero-warning enforcement.',
      'bench.chartTitle': 'Footprint & Gate Timing Breakdown',
      'bench.modeFootprint': 'Footprint',
      'bench.modePipeline': 'Gate Timing',
      'bench.s1Title': 'NSIS Installer Size',
      'bench.s1Meta': 'Rust + Tauri v2 · Zero external HTTP or ZIP runtime deps',
      'bench.s2Title': 'Rust Unit Tests Green',
      'bench.s2Meta': 'Covers versioning, leak guard, atomic updates & rollback',
      'bench.s3Title': 'Frontend & Site Gates',
      'bench.s3Meta': 'Zero-warning Release + syntax, typography & contract checks',
      'bench.s4Title': 'Streaming Multi-Forge',
      'bench.s4Unit': 'Forges',
      'bench.s4Meta': 'GitHub · Gitee · GitLab · Gitea with Windows DPAPI vault',
      'dl.eyebrow': 'GET SHIPFORGE · DOWNLOAD',
      'dl.title': 'Two clean packages, ready in 30 seconds',
      'dl.desc': 'Projects and DPAPI-encrypted tokens live in <code>%APPDATA%\\ShipForge</code> so upgrades never overwrite your config. Verified via SHA-256.',
      'dl.rec': 'Recommended · Daily Driver',
      'dl.port': 'No Install · Portable',
      'dl.exeTitle': 'Windows Installer (NSIS x64)',
      'dl.exeDesc': 'Standard Windows setup with Start Menu shortcut, clean uninstall, and silent in-app SHA-256 upgrades.',
      'dl.exeBtn': 'Download Installer (.exe)',
      'dl.zipTitle': 'Windows Portable (ZIP)',
      'dl.zipDesc': 'Extract anywhere and launch with zero registry entries. Built-in updater backs up old binaries and rolls back on failure.',
      'dl.zipBtn': 'Download Portable (.zip)',
      'dev.summary': 'Developer notes: 3-tier architecture, test gates & build commands',
      'dev.l1': 'Framework-Free WebView2 UI',
      'dev.l2': 'Native Rust Engine (Tauri v2)',
      'dev.l3': '3-Tier Atomic Updater',
      'dev.copy': 'Copy commands',
      'dev.copied': 'Copied ✓',
      'foot.note': 'MIT Open Source · Local-first, zero telemetry, zero ads',
      'foot.coffee': 'Buy me a coffee',
      'sponsor.title': 'Buy the author a coffee',
      'sponsor.sub': 'Independent software takes continuous care. If ShipForge saves you time on every build and release, feel free to support development!',
      'sponsor.wechat': 'WeChat Pay · Scan to support',
      'sponsor.alipay': 'Alipay · Scan to support',
      'sponsor.note': 'ShipForge is permanently MIT open-source and ad-free. Starring on GitHub helps just as much!',
      'sponsor.close': 'Thanks, keep browsing',
      'lightbox.hint': 'Inspect full size',
      'lightbox.zoomIn': '2× Zoom',
      'lightbox.zoomOut': 'Reset 1×',
      'lightbox.keys': '← / → Switch · Wheel/Click zoom · Esc close'
    }
  };

  const BENCH_DATA = {
    footprint: {
      subZh: 'Windows x64 Release · ShipForge v1.5.1',
      subEn: 'Windows x64 Release · ShipForge v1.5.1',
      noteZh: '* 复用系统内置 WebView2 运行时与 WinHTTP 网络栈，无需捆绑 100 MB+ Chromium，冷启动瞬间进入工作台。',
      noteEn: '* Reuses system WebView2 and WinHTTP instead of bundling 100 MB+ Chromium for instant cold startup.',
      rows: [
        { lblZh: 'NSIS 安装包 (x64-setup.exe)', lblEn: 'NSIS Installer (x64-setup.exe)', valZh: '3.4 MB · 含图标与干净卸载', valEn: '3.4 MB · Clean setup & uninstall', scale: 0.28, cls: 'accent' },
        { lblZh: '便携压缩包 (portable.zip)', lblEn: 'Portable Archive (portable.zip)', valZh: '4.7 MB · 解压即用支持回滚', valEn: '4.7 MB · Extract & run w/ rollback', scale: 0.39, cls: 'warm' },
        { lblZh: '原生主程序 (shipforge.exe)', lblEn: 'Native Binary (shipforge.exe)', valZh: '11.3 MB · 单文件零外部 DLL', valEn: '11.3 MB · Single native binary', scale: 0.94, cls: 'ink' },
        { lblZh: '前端构建依赖 (node_modules)', lblEn: 'Frontend Deps (node_modules)', valZh: '0 KB · 原生 ES Modules', valEn: '0 KB · Pure ES Modules', scale: 0.04, cls: 'muted' }
      ]
    },
    pipeline: {
      subZh: 'build.ps1 自动化门禁实测 · 16 核 Win11 x64',
      subEn: 'build.ps1 Automated Gate Timing · 16-Core Win11 x64',
      noteZh: '* 每次发版自动执行 Rust 单元测试、零警告 Release 检查、JS 语法校验与前端/官网契约自测，杜绝带病发布。',
      noteEn: '* Every release runs Rust unit tests, zero-warning release check, JS syntax check, and UI/website selftests automatically.',
      rows: [
        { lblZh: 'JS 语法 + 前端/官网自测 (34 项)', lblEn: 'JS Syntax + UI/Site Selftests (34)', valZh: '0.4 s · 零依赖秒级通过', valEn: '0.4 s · Instant zero-dep pass', scale: 0.08, cls: 'accent' },
        { lblZh: 'Rust 单元测试门禁 (112 项)', lblEn: 'Rust Unit Test Gate (112 tests)', valZh: '2.6 s · 覆盖版本/分包/更新器', valEn: '2.6 s · Versioning/Pack/Updater', scale: 0.24, cls: 'warm' },
        { lblZh: '增量零警告 Release 编译', lblEn: 'Incremental 0-Warning Release', valZh: '14.8 s · -D warnings 严格门禁', valEn: '14.8 s · Strict -D warnings gate', scale: 0.68, cls: 'ink' },
        { lblZh: '全量打包 + SHA256 + latest.json', lblEn: 'Full Bundle + SHA256 + Manifest', valZh: '22.4 s · 产出 NSIS + ZIP + 校验', valEn: '22.4 s · NSIS + ZIP + Checksums', scale: 1.0, cls: 'muted' }
      ]
    }
  };

  const LIGHTBOX_ITEMS = [
    {
      id: 'overview',
      src: 'assets/screenshots/01-overview-pipeline.png',
      titleZh: '发版总览与免重编直推',
      titleEn: 'Release Overview & Zero-Rebuild Deploy',
      capZh: '成品就绪感知 · 免重编直推 · 一键发版流水线与跨项目隔离',
      capEn: 'Artifact readiness · Direct deploy · Isolated multi-project pipeline'
    },
    {
      id: 'compile',
      src: 'assets/screenshots/02-compile-prereq.png',
      titleZh: '多框架编译预设与环境前置自检',
      titleEn: 'Multi-Stack Matrix & Preflight Inspector',
      capZh: '8 类语言 × 5 种桌面外壳 · 磁盘/内存/工具链前置检查与托管安装',
      capEn: '8 stacks × 5 desktop shells · Disk, RAM & toolchain preflight'
    },
    {
      id: 'package',
      src: 'assets/screenshots/03-package-split.png',
      titleZh: '体积构成分析与防泄漏智能分包',
      titleEn: 'Bundle Analyzer & 95 MB Split Packager',
      capZh: '毫秒级定位最大文件 · 拦截 .env/私钥外泄 · 95 MB Gitee 分卷',
      capEn: 'Millisecond file weight breakdown · Secret leak guard · 95 MB split ZIP'
    },
    {
      id: 'release',
      src: 'assets/screenshots/04-release-repos.png',
      titleZh: '多仓库流式发布与在线回撤管理',
      titleEn: 'Multi-Forge Streaming Releases & Recall',
      capZh: 'GitHub / Gitee / GitLab / Gitea 统一凭据管理 · 64 KB 流式上传与在线编辑',
      capEn: 'Unified GitHub/Gitee/GitLab/Gitea vault · 64 KB streaming & inline edits'
    },
    {
      id: 'changelog',
      src: 'assets/screenshots/05-version-changelog.png',
      titleZh: '多文件版本号同步与变更日志工作台',
      titleEn: 'Multi-File Version Sync & Changelog Studio',
      capZh: '同步写入 tauri.conf.json / Cargo.toml / package.json · 一键同步线上 Release 说明',
      capEn: 'Atomic multi-file version bumps · Sync release notes & latest.json online'
    },
    {
      id: 'updater',
      src: 'assets/screenshots/06-updater-prefs.png',
      titleZh: '内置三通道 SHA-256 在线自动更新（深色主题）',
      titleEn: 'Built-in 3-Tier SHA-256 Auto-Updater (Dark Mode)',
      capZh: 'latest.json / 302 直链 / API 三级容错 · 安装版静默升级、便携版自动备份回滚',
      capEn: '3-tier resilient manifest lookup · Silent installer & atomic portable rollback'
    }
  ];

  // ---------------- State ----------------
  let curLang = 'zh';
  let curBenchMode = 'footprint';
  let mediaUiReady = false;
  let currentVersion = '1.5.1';

  // ---------------- 1. Two-State Dark Mode Toggle (dark-mode-toggle) ----------------
  const metaScheme = document.querySelector('meta[name="color-scheme"]');
  const themeBtn = document.getElementById('theme-toggle');

  function getActiveScheme() {
    const val = metaScheme ? metaScheme.content : 'light dark';
    if (val === 'light' || val === 'dark') return val;
    return window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }

  function setScheme(next) {
    if (!metaScheme) return;
    metaScheme.content = next;
    try {
      localStorage.setItem('shipforge-site-scheme', next);
    } catch (_) {}
  }

  if (themeBtn) {
    themeBtn.addEventListener('click', () => {
      setScheme(getActiveScheme() === 'dark' ? 'light' : 'dark');
    });
  }

  // ---------------- 2. Bilingual ZH / EN Switcher ----------------
  const langBtn = document.getElementById('lang-toggle');
  const langLabel = document.getElementById('lang-label');

  function applyLang(lang) {
    curLang = lang === 'en' ? 'en' : 'zh';
    document.documentElement.lang = curLang === 'en' ? 'en' : 'zh-CN';
    const dict = COPY[curLang];
    document.querySelectorAll('[data-i18n]').forEach((el) => {
      const k = el.getAttribute('data-i18n');
      if (k && Object.prototype.hasOwnProperty.call(dict, k)) {
        el.innerHTML = dict[k];
      }
    });
    if (langLabel) langLabel.textContent = curLang === 'en' ? '中' : 'EN';
    const activeTab = document.querySelector('.stage-tabs [role="tab"][aria-selected="true"]');
    updateStageCaption(activeTab);
    renderBenchMode(curBenchMode, false);
    const chromeTitle = document.getElementById('win-chrome-title');
    if (chromeTitle) {
      chromeTitle.textContent = curLang === 'en'
        ? `ShipForge Studio v${currentVersion}`
        : `ShipForge 轻筑 v${currentVersion}`;
    }
    if (mediaUiReady) syncLightboxLanguage();
    try {
      localStorage.setItem('shipforge-site-lang', curLang);
    } catch (_) {}
  }

  if (langBtn) {
    langBtn.addEventListener('click', () => applyLang(curLang === 'zh' ? 'en' : 'zh'));
  }

  // ---------------- 3. Interactive Stage Tabs (Anchor Underline + Keyboard + beforematch) ----------------
  const tabs = Array.from(document.querySelectorAll('.stage-tabs [role="tab"]'));
  const panels = Array.from(document.querySelectorAll('.stage-panels [role="tabpanel"]'));
  const stageCaption = document.getElementById('stage-caption');

  function updateStageCaption(tabEl) {
    if (!stageCaption || !tabEl) return;
    const cap = curLang === 'en' ? tabEl.dataset.capEn : tabEl.dataset.capZh;
    if (cap) stageCaption.textContent = cap;
  }

  function activateTab(tabEl, focusTab) {
    if (!tabEl) return;
    const panelId = tabEl.dataset.panel;
    tabs.forEach((t) => {
      const active = t === tabEl;
      t.setAttribute('aria-selected', active ? 'true' : 'false');
      t.setAttribute('tabindex', active ? '0' : '-1');
    });
    panels.forEach((p) => {
      if (p.id === panelId) {
        p.removeAttribute('hidden');
      } else {
        p.setAttribute('hidden', 'until-found');
      }
    });
    updateStageCaption(tabEl);
    if (focusTab && typeof tabEl.focus === 'function') tabEl.focus();
  }

  tabs.forEach((t, idx) => {
    t.addEventListener('click', () => activateTab(t, false));
    t.addEventListener('keydown', (e) => {
      let nextIdx = -1;
      if (e.key === 'ArrowRight') nextIdx = (idx + 1) % tabs.length;
      else if (e.key === 'ArrowLeft') nextIdx = (idx - 1 + tabs.length) % tabs.length;
      else if (e.key === 'Home') nextIdx = 0;
      else if (e.key === 'End') nextIdx = tabs.length - 1;
      if (nextIdx >= 0) {
        e.preventDefault();
        activateTab(tabs[nextIdx], true);
      }
    });
  });

  panels.forEach((p) => {
    p.addEventListener('beforematch', () => {
      const ownerTab = tabs.find((t) => t.dataset.panel === p.id);
      if (ownerTab) activateTab(ownerTab, false);
    });
  });

  // ---------------- 4. Scroll Choreography Fallback (Progress + Shrinking Header + 3D Stage + Active Nav) ----------------
  const headerEl = document.getElementById('site-header');
  const progressEl = document.getElementById('scroll-progress');
  const heroCopyEl = document.querySelector('[data-scroll-hero="copy"]');
  const stageFrameEl = document.getElementById('stage-frame');
  const navLinks = Array.from(document.querySelectorAll('.site-nav a[data-section]'));
  const navSections = navLinks
    .map((a) => ({ a, el: document.getElementById(a.dataset.section) }))
    .filter((x) => x.el);

  const hasNativeScroll = typeof CSS !== 'undefined' &&
    typeof CSS.supports === 'function' &&
    CSS.supports('(animation-timeline: scroll()) and (animation-range: 0% 100%)');

  function isReducedMotion() {
    return !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
  }

  let scrollTicking = false;
  function onScrollUpdate() {
    scrollTicking = false;
    const y = window.scrollY || window.pageYOffset || 0;
    const docH = Math.max(1, (document.documentElement.scrollHeight || 1) - (window.innerHeight || 1));
    const p = Math.max(0, Math.min(1, y / docH));

    if (!hasNativeScroll) {
      if (progressEl) progressEl.style.setProperty('--scroll-p', p.toFixed(4));
      if (headerEl) headerEl.classList.toggle('scrolled', y > 24);
      if (!isReducedMotion()) {
        if (heroCopyEl) {
          const t = Math.max(0, Math.min(1, (y - 40) / 460));
          heroCopyEl.style.transform = `translateY(${(-28 * t).toFixed(1)}px) scale(${(1 - 0.02 * t).toFixed(4)})`;
          heroCopyEl.style.opacity = String((1 - 0.65 * t).toFixed(3));
        }
        if (stageFrameEl) {
          const s = Math.max(0, Math.min(1, y / 760));
          let rx;
          let sc;
          let ty;
          if (s <= 0.55) {
            const k = s / 0.55;
            rx = 7.5 * (1 - k);
            sc = 0.945 + 0.055 * k;
            ty = 18 * (1 - k);
          } else {
            const k = (s - 0.55) / 0.45;
            rx = -2 * k;
            sc = 1 - 0.012 * k;
            ty = -10 * k;
          }
          stageFrameEl.style.transform = `rotateX(${rx.toFixed(2)}deg) scale(${sc.toFixed(4)}) translateY(${ty.toFixed(1)}px)`;
        }
      }
    }

    // Active section nav spy
    const probeY = y + Math.min(240, (window.innerHeight || 800) * 0.34);
    let activeSec = null;
    for (let i = 0; i < navSections.length; i++) {
      if (navSections[i].el.offsetTop <= probeY) activeSec = navSections[i];
    }
    navLinks.forEach((a) => a.classList.toggle('active', !!(activeSec && activeSec.a === a)));
  }

  window.addEventListener('scroll', () => {
    if (!scrollTicking) {
      scrollTicking = true;
      requestAnimationFrame(onScrollUpdate);
    }
  }, { passive: true });
  onScrollUpdate();

  // ---------------- 5. Gradual Bidirectional Float-In Scroll Reveal (64px Hysteresis + Bottom Guard) ----------------
  const floatTargets = Array.from(document.querySelectorAll('.float-in, .float-group'));
  if (!isReducedMotion() && typeof IntersectionObserver !== 'undefined' && floatTargets.length > 0) {
    document.documentElement.classList.add('js-float-ready');

    const floatObserver = new IntersectionObserver((entries) => {
      const vh = window.innerHeight || 900;
      const nearBottom = (window.scrollY + vh) >= (document.documentElement.scrollHeight - 48);

      entries.forEach((entry) => {
        const el = entry.target;
        const rect = entry.boundingClientRect;
        if (entry.isIntersecting || nearBottom) {
          el.classList.remove('from-top');
          el.classList.add('is-visible');
          triggerCountUpsInside(el);
        } else {
          // 64px hysteresis outside viewport prevents edge flicker
          if (rect.bottom < -64) {
            el.classList.remove('is-visible');
            el.classList.add('from-top');
          } else if (rect.top > vh + 64) {
            el.classList.remove('is-visible');
            el.classList.remove('from-top');
          }
        }
      });
    }, {
      threshold: [0, 0.08, 0.16],
      rootMargin: '0px 0px -6% 0px'
    });

    floatTargets.forEach((el) => floatObserver.observe(el));

    // Bottom-of-page safety reveal so final cards never remain hidden on short screens
    window.addEventListener('scroll', () => {
      if ((window.scrollY + (window.innerHeight || 900)) >= (document.documentElement.scrollHeight - 48)) {
        floatTargets.forEach((el) => {
          if (!el.classList.contains('is-visible')) {
            el.classList.remove('from-top');
            el.classList.add('is-visible');
            triggerCountUpsInside(el);
          }
        });
      }
    }, { passive: true });
  } else {
    floatTargets.forEach((el) => el.classList.add('is-visible'));
  }

  // ---------------- 6. Count-Up Numbers Animation ----------------
  const countedSet = new WeakSet();
  function triggerCountUpsInside(rootEl) {
    const nums = rootEl.matches && rootEl.matches('.count-up')
      ? [rootEl]
      : Array.from(rootEl.querySelectorAll('.count-up'));
    nums.forEach((el) => {
      if (countedSet.has(el)) return;
      countedSet.add(el);
      const target = Number(el.dataset.val);
      const dec = Number(el.dataset.dec || 0);
      if (!Number.isFinite(target) || isReducedMotion()) {
        el.textContent = target.toFixed(dec);
        return;
      }
      const start = performance.now();
      const dur = 920;
      function frame(now) {
        const t = Math.min(1, (now - start) / dur);
        const eased = 1 - Math.pow(1 - t, 3);
        el.textContent = (target * eased).toFixed(dec);
        if (t < 1) requestAnimationFrame(frame);
        else el.textContent = target.toFixed(dec);
      }
      requestAnimationFrame(frame);
    });
  }

  // ---------------- 7. Interactive Benchmark Chart Mode Switch ----------------
  const segBtns = Array.from(document.querySelectorAll('.seg-btn[data-bench-mode]'));
  const benchSubLabel = document.getElementById('bench-sub-label');
  const benchNote = document.getElementById('bench-mode-note');

  function renderBenchMode(mode, animate) {
    const ds = BENCH_DATA[mode] || BENCH_DATA.footprint;
    curBenchMode = mode in BENCH_DATA ? mode : 'footprint';
    segBtns.forEach((b) => {
      const active = b.dataset.benchMode === curBenchMode;
      b.classList.toggle('active', active);
      b.setAttribute('aria-pressed', active ? 'true' : 'false');
    });
    if (benchSubLabel) benchSubLabel.textContent = curLang === 'en' ? ds.subEn : ds.subZh;
    if (benchNote) benchNote.textContent = curLang === 'en' ? ds.noteEn : ds.noteZh;

    ds.rows.forEach((row, i) => {
      const lbl = document.getElementById(`bm-lbl-${i}`);
      const val = document.getElementById(`bm-val-${i}`);
      const bar = document.getElementById(`bm-bar-${i}`);
      if (lbl) lbl.textContent = curLang === 'en' ? row.lblEn : row.lblZh;
      if (val) val.textContent = curLang === 'en' ? row.valEn : row.valZh;
      if (bar) {
        bar.className = `bar-fill ${row.cls}`;
        if (animate && !isReducedMotion()) {
          bar.style.setProperty('--scale', '0.02');
          requestAnimationFrame(() => {
            requestAnimationFrame(() => bar.style.setProperty('--scale', String(row.scale)));
          });
        } else {
          bar.style.setProperty('--scale', String(row.scale));
        }
      }
    });
  }

  segBtns.forEach((b) => {
    b.addEventListener('click', () => {
      if (b.dataset.benchMode !== curBenchMode) {
        renderBenchMode(b.dataset.benchMode, true);
      }
    });
  });

  // ---------------- 8. Copy Developer Build Commands (Clipboard + Legacy Fallback) ----------------
  const copyBtn = document.getElementById('copy-dev-cmd');
  const copyCode = document.getElementById('dev-cmd-text');
  const copyStatus = document.getElementById('copy-dev-status');

  async function copyTextSafe(text) {
    if (navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
      try {
        await navigator.clipboard.writeText(text);
        return true;
      } catch (_) {}
    }
    try {
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.setAttribute('readonly', '');
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      const ok = document.execCommand('copy');
      document.body.removeChild(ta);
      return ok;
    } catch (_) {
      return false;
    }
  }

  if (copyBtn && copyCode) {
    let copyTimer = null;
    copyBtn.addEventListener('click', async () => {
      const ok = await copyTextSafe(copyCode.textContent || '');
      if (ok) {
        copyBtn.textContent = COPY[curLang]['dev.copied'];
        if (copyStatus) copyStatus.textContent = COPY[curLang]['dev.copied'];
        clearTimeout(copyTimer);
        copyTimer = setTimeout(() => {
          copyBtn.textContent = COPY[curLang]['dev.copy'];
        }, 1800);
      }
    });
  }

  // ---------------- 9. Hydrate Live latest.json & GitHub Star Count ----------------
  function formatMB(bytes) {
    if (!Number.isFinite(bytes) || bytes <= 0) return '';
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  async function hydrateLatestManifest() {
    try {
      const res = await fetch('latest.json', { cache: 'no-cache' });
      if (!res.ok) return;
      const data = await res.json();
      const ver = String(data.version || '').replace(/^v/i, '').trim();
      if (!ver) return;
      currentVersion = ver;
      const tag = `v${ver}`;
      const baseDl = `https://github.com/${REPO_SLUG}/releases/download/${tag}`;

      const inst = data.installer || {};
      const port = data.portable || {};
      const instUrl = inst.url || `${baseDl}/ShipForge_${ver}_x64-setup.exe`;
      const portUrl = port.url || `${baseDl}/ShipForge_${ver}_x64-portable.zip`;
      const instSize = formatMB(Number(inst.size || 0)) || '3.4 MB';
      const portSize = formatMB(Number(port.size || 0)) || '4.7 MB';

      const setHref = (id, url) => { const el = document.getElementById(id); if (el) el.href = url; };
      const setText = (id, txt) => { const el = document.getElementById(id); if (el) el.textContent = txt; };

      setHref('hero-dl-installer', instUrl);
      setHref('hero-dl-portable', portUrl);
      setHref('dl-btn-exe', instUrl);
      setHref('dl-btn-zip', portUrl);
      setHref('dl-sha-link', `${baseDl}/SHA256SUMS.txt`);

      setText('hero-exe-size', instSize);
      setText('hero-zip-size', portSize);
      setText('dl-exe-ver', tag);
      setText('dl-zip-ver', tag);
      setText('dl-exe-name', `${inst.filename || `ShipForge_${ver}_x64-setup.exe`} · ${instSize}`);
      setText('dl-zip-name', `${port.filename || `ShipForge_${ver}_x64-portable.zip`} · ${portSize}`);
      setText('win-chrome-title', curLang === 'en' ? `ShipForge Studio ${tag}` : `ShipForge 轻筑 ${tag}`);

      BENCH_DATA.footprint.subZh = `Windows x64 Release · ShipForge ${tag}`;
      BENCH_DATA.footprint.subEn = `Windows x64 Release · ShipForge ${tag}`;
      if (curBenchMode === 'footprint' && benchSubLabel) {
        benchSubLabel.textContent = BENCH_DATA.footprint.subZh;
      }
    } catch (_) {}
  }

  async function hydrateStarCount() {
    const countEl = document.getElementById('header-star-count');
    if (!countEl) return;
    try {
      const res = await fetch(`https://api.github.com/repos/${REPO_SLUG}`, {
        headers: { Accept: 'application/vnd.github+json' }
      });
      if (!res.ok) return;
      const json = await res.json();
      if (typeof json.stargazers_count === 'number') {
        countEl.textContent = json.stargazers_count >= 1000
          ? `${(json.stargazers_count / 1000).toFixed(1)}k`
          : String(json.stargazers_count);
        countEl.hidden = false;
      }
    } catch (_) {}
  }

  hydrateLatestManifest();
  hydrateStarCount();

  // ---------------- 10. Top-Layer Dialog Close Helper ----------------
  async function closeTopLayerDialog(dialogEl, getSeq, bumpSeq) {
    if (!dialogEl || !dialogEl.open) return;
    const mySeq = bumpSeq();
    if (isReducedMotion()) {
      dialogEl.removeAttribute('data-closing');
      dialogEl.close();
      return;
    }
    dialogEl.setAttribute('data-closing', '');
    await new Promise((resolve) => {
      let done = false;
      const finish = () => {
        if (done) return;
        done = true;
        dialogEl.removeEventListener('transitionend', onEnd);
        resolve();
      };
      const onEnd = (e) => {
        if (e.target === dialogEl) finish();
      };
      dialogEl.addEventListener('transitionend', onEnd);
      setTimeout(finish, 320);
    });
    if (getSeq() === mySeq) {
      dialogEl.removeAttribute('data-closing');
      if (dialogEl.open) dialogEl.close();
    }
  }

  // ---------------- 11. Sponsor Modal Dialog ----------------
  const sponsorDialog = document.getElementById('sponsor-dialog');
  const sponsorTriggers = [
    document.getElementById('header-coffee-btn'),
    document.getElementById('footer-coffee-btn')
  ].filter(Boolean);
  const sponsorCloseX = document.getElementById('sponsor-close-x');
  const sponsorCloseBtn = document.getElementById('sponsor-close-btn');
  let sponsorSeq = 0;

  function openSponsor() {
    if (!sponsorDialog) return;
    sponsorSeq++;
    sponsorDialog.removeAttribute('data-closing');
    if (!sponsorDialog.open) sponsorDialog.showModal();
  }

  function closeSponsor() {
    closeTopLayerDialog(sponsorDialog, () => sponsorSeq, () => ++sponsorSeq);
  }

  sponsorTriggers.forEach((btn) => btn.addEventListener('click', openSponsor));
  if (sponsorCloseX) sponsorCloseX.addEventListener('click', closeSponsor);
  if (sponsorCloseBtn) sponsorCloseBtn.addEventListener('click', closeSponsor);
  if (sponsorDialog) {
    sponsorDialog.addEventListener('click', (e) => {
      if (e.target === sponsorDialog) closeSponsor();
    });
    sponsorDialog.addEventListener('cancel', (e) => {
      e.preventDefault();
      closeSponsor();
    });
  }

  // ---------------- 12. Shared-Element Full-Viewport Image Lightbox ----------------
  const lightboxDialog = document.getElementById('media-lightbox');
  const lbShell = document.getElementById('lb-shell');
  const lbBody = document.getElementById('lb-body');
  const lbViewport = document.getElementById('lb-viewport');
  const lbImg = document.getElementById('lb-img');
  const lbTitle = document.getElementById('lb-title');
  const lbCaption = document.getElementById('lb-caption');
  const lbCounter = document.getElementById('lb-counter');
  const lbLive = document.getElementById('lb-live');
  const lbZoomBtn = document.getElementById('lb-zoom-btn');
  const lbZoomLabel = document.getElementById('lb-zoom-label');
  const lbCloseBtn = document.getElementById('lb-close-btn');
  const lbPrevBtn = document.getElementById('lb-prev');
  const lbNextBtn = document.getElementById('lb-next');
  const zoomTriggers = Array.from(document.querySelectorAll('.zoom-trigger[data-lightbox-id]'));

  let lbIndex = 0;
  let lbSeq = 0;
  let lbOriginTrigger = null;
  let lbOriginImg = null;
  let lbOriginId = '';
  let lbZoomed = false;
  let lbScale = 1;
  let lbPanX = 0;
  let lbPanY = 0;
  let lbDragging = false;
  let lbDidDrag = false;
  let lbDragStartX = 0;
  let lbDragStartY = 0;
  let lbTransitioning = false;

  function clearAllTransitionNames() {
    if (lbImg && lbImg.style) lbImg.style.viewTransitionName = '';
    zoomTriggers.forEach((btn) => {
      const img = btn.querySelector ? btn.querySelector('img') : null;
      if (img && img.style) img.style.viewTransitionName = '';
    });
    if (document.documentElement && document.documentElement.dataset) {
      delete document.documentElement.dataset.vtMedia;
    }
    if (lightboxDialog) lightboxDialog.removeAttribute('data-vt-active');
  }

  function clampPan(x, y) {
    const scale = lbScale;
    if (scale <= 1.02 || !lbViewport || !lbImg || typeof lbViewport.getBoundingClientRect !== 'function') {
      return { x: 0, y: 0 };
    }
    const vr = lbViewport.getBoundingClientRect();
    const maxX = Math.max(120, Math.round(vr.width * (scale - 1) * 0.55));
    const maxY = Math.max(90, Math.round(vr.height * (scale - 1) * 0.55));
    return {
      x: Math.max(-maxX, Math.min(maxX, x)),
      y: Math.max(-maxY, Math.min(maxY, y))
    };
  }

  function applyZoomTransform() {
    if (!lbImg || !lbImg.style) return;
    lbZoomed = lbScale > 1.02;
    if (lbZoomed) {
      const clamped = clampPan(lbPanX, lbPanY);
      lbPanX = clamped.x;
      lbPanY = clamped.y;
      lbImg.style.transform = `translate3d(${lbPanX}px, ${lbPanY}px, 0) scale(${lbScale.toFixed(2)})`;
    } else {
      lbScale = 1;
      lbPanX = 0;
      lbPanY = 0;
      lbImg.style.transform = '';
    }
    if (lbViewport) lbViewport.classList.toggle('is-zoomed', lbZoomed);
    if (lbZoomBtn) lbZoomBtn.setAttribute('aria-pressed', lbZoomed ? 'true' : 'false');
    if (lbZoomLabel) {
      lbZoomLabel.textContent = COPY[curLang][lbZoomed ? 'lightbox.zoomOut' : 'lightbox.zoomIn'];
    }
  }

  function resetLightboxZoom() {
    lbZoomed = false;
    lbScale = 1;
    lbPanX = 0;
    lbPanY = 0;
    lbDragging = false;
    if (lbViewport) lbViewport.classList.remove('is-dragging');
    applyZoomTransform();
  }

  function renderLightboxItem(index, announce) {
    const total = LIGHTBOX_ITEMS.length;
    lbIndex = ((index % total) + total) % total;
    const item = LIGHTBOX_ITEMS[lbIndex];
    const title = curLang === 'en' ? item.titleEn : item.titleZh;
    const caption = curLang === 'en' ? item.capEn : item.capZh;

    resetLightboxZoom();
    if (lbImg) {
      lbImg.src = item.src;
      lbImg.alt = title;
    }
    if (lbTitle) lbTitle.textContent = title;
    if (lbCaption) lbCaption.textContent = caption;
    if (lbCounter) lbCounter.textContent = `${lbIndex + 1} / ${total}`;
    if (announce && lbLive) {
      lbLive.textContent = `${lbIndex + 1} / ${total}: ${title}`;
    }
  }

  function syncLightboxLanguage() {
    zoomTriggers.forEach((btn) => {
      const id = btn.dataset ? btn.dataset.lightboxId : '';
      const item = LIGHTBOX_ITEMS.find((x) => x.id === id);
      if (!item) return;
      const title = curLang === 'en' ? item.titleEn : item.titleZh;
      btn.setAttribute('aria-label', curLang === 'en' ? `Inspect screenshot: ${title}` : `点击放大查看：${title}`);
    });
    if (lightboxDialog && lightboxDialog.open) {
      renderLightboxItem(lbIndex, false);
    } else if (lbZoomLabel) {
      lbZoomLabel.textContent = COPY[curLang][lbZoomed ? 'lightbox.zoomOut' : 'lightbox.zoomIn'];
    }
  }

  function getRectIfVisible(el) {
    if (!el || typeof el.getBoundingClientRect !== 'function') return null;
    const r = el.getBoundingClientRect();
    if (!r || r.width < 12 || r.height < 12) return null;
    const vh = window.innerHeight || 900;
    const vw = window.innerWidth || 1440;
    if (r.bottom < -40 || r.top > vh + 40 || r.right < -40 || r.left > vw + 40) return null;
    return r;
  }

  function findReturnImageForCurrentSlide() {
    const curItem = LIGHTBOX_ITEMS[lbIndex];
    if (!curItem) return null;
    if (lbOriginImg && curItem.id === lbOriginId && getRectIfVisible(lbOriginImg)) {
      return lbOriginImg;
    }
    for (let i = 0; i < zoomTriggers.length; i++) {
      const btn = zoomTriggers[i];
      if (!btn.dataset || btn.dataset.lightboxId !== curItem.id) continue;
      const img = btn.querySelector ? btn.querySelector('img') : null;
      if (img && getRectIfVisible(img)) return img;
    }
    return null;
  }

  function focusSafely(el) {
    if (!el || typeof el.focus !== 'function') return;
    try { el.focus({ preventScroll: true }); } catch (_) { el.focus(); }
  }

  async function waitImageReady(img) {
    if (!img || typeof img.decode !== 'function') return;
    try {
      await Promise.race([img.decode(), new Promise((r) => setTimeout(r, 120))]);
    } catch (_) {}
  }

  async function openLightbox(triggerEl) {
    if (!lightboxDialog || lbTransitioning) return;
    const targetId = triggerEl && triggerEl.dataset ? triggerEl.dataset.lightboxId : 'overview';
    const foundIdx = LIGHTBOX_ITEMS.findIndex((x) => x.id === targetId);
    lbOriginTrigger = triggerEl || document.activeElement;
    lbOriginImg = triggerEl && typeof triggerEl.querySelector === 'function'
      ? triggerEl.querySelector('img')
      : null;
    lbOriginId = foundIdx >= 0 ? targetId : 'overview';

    lbSeq++;
    lightboxDialog.removeAttribute('data-closing');
    renderLightboxItem(foundIdx >= 0 ? foundIdx : 0, true);
    await waitImageReady(lbImg);

    const reduced = isReducedMotion();
    const originRect = getRectIfVisible(lbOriginImg);

    // Path 1: Native View Transitions API (same-document-transitions)
    if (!reduced && originRect && typeof document.startViewTransition === 'function') {
      lbTransitioning = true;
      clearAllTransitionNames();
      lbOriginImg.style.viewTransitionName = 'shipforge-zoom-media';
      document.documentElement.dataset.vtMedia = 'open';
      lightboxDialog.setAttribute('data-vt-active', '');

      try {
        const vt = document.startViewTransition(() => {
          lbOriginImg.style.viewTransitionName = '';
          if (lbImg) lbImg.style.viewTransitionName = 'shipforge-zoom-media';
          if (!lightboxDialog.open) lightboxDialog.showModal();
        });
        await vt.finished.catch(() => {});
      } finally {
        clearAllTransitionNames();
        lbTransitioning = false;
        focusSafely(lbCloseBtn);
      }
      return;
    }

    // Path 2: Seamless WAAPI FLIP geometric morph fallback
    if (!lightboxDialog.open) lightboxDialog.showModal();
    if (!reduced && originRect && lbImg && typeof lbImg.animate === 'function') {
      const destRect = lbImg.getBoundingClientRect();
      if (destRect && destRect.width > 12 && destRect.height > 12) {
        const dx = (originRect.left + originRect.width / 2) - (destRect.left + destRect.width / 2);
        const dy = (originRect.top + originRect.height / 2) - (destRect.top + destRect.height / 2);
        const sx = originRect.width / destRect.width;
        const sy = originRect.height / destRect.height;
        lbImg.animate(
          [
            { transform: `translate3d(${dx}px, ${dy}px, 0) scale(${sx}, ${sy})`, borderRadius: '18px' },
            { transform: 'translate3d(0, 0, 0) scale(1, 1)', borderRadius: '14px' }
          ],
          { duration: 380, easing: 'cubic-bezier(0.22, 1, 0.36, 1)' }
        );
      }
    }
    focusSafely(lbCloseBtn);
  }

  async function closeLightbox() {
    if (!lightboxDialog || !lightboxDialog.open) return;
    if (lbTransitioning) {
      clearAllTransitionNames();
      lbTransitioning = false;
      lightboxDialog.close();
      focusSafely(lbOriginTrigger);
      return;
    }

    resetLightboxZoom();
    const reduced = isReducedMotion();
    const returnImg = findReturnImageForCurrentSlide();

    // Path 1: Reverse Shared-Element View Transition fly-back
    if (!reduced && returnImg && lbImg && typeof document.startViewTransition === 'function') {
      lbTransitioning = true;
      clearAllTransitionNames();
      lbImg.style.viewTransitionName = 'shipforge-zoom-media';
      document.documentElement.dataset.vtMedia = 'close';
      lightboxDialog.setAttribute('data-vt-active', '');

      try {
        const vt = document.startViewTransition(() => {
          lbImg.style.viewTransitionName = '';
          returnImg.style.viewTransitionName = 'shipforge-zoom-media';
          lightboxDialog.close();
        });
        await vt.finished.catch(() => {});
      } finally {
        clearAllTransitionNames();
        lbTransitioning = false;
        focusSafely(lbOriginTrigger);
      }
      return;
    }

    // Path 2: Reverse WAAPI FLIP fly-back fallback
    const returnRect = getRectIfVisible(returnImg);
    if (!reduced && returnRect && lbImg && typeof lbImg.animate === 'function') {
      const fromRect = lbImg.getBoundingClientRect();
      if (fromRect && fromRect.width > 12 && fromRect.height > 12) {
        lbTransitioning = true;
        const dx = (returnRect.left + returnRect.width / 2) - (fromRect.left + fromRect.width / 2);
        const dy = (returnRect.top + returnRect.height / 2) - (fromRect.top + fromRect.height / 2);
        const sx = returnRect.width / fromRect.width;
        const sy = returnRect.height / fromRect.height;
        const anim = lbImg.animate(
          [
            { transform: 'translate3d(0, 0, 0) scale(1, 1)', opacity: 1 },
            { transform: `translate3d(${dx}px, ${dy}px, 0) scale(${sx}, ${sy})`, opacity: 0.92 }
          ],
          { duration: 320, easing: 'cubic-bezier(0.22, 1, 0.36, 1)', fill: 'forwards' }
        );
        await Promise.race([anim.finished.catch(() => {}), new Promise((r) => setTimeout(r, 380))]);
        anim.cancel();
        lbTransitioning = false;
        lightboxDialog.close();
        focusSafely(lbOriginTrigger);
        return;
      }
    }

    // Path 3: Standard top-layer dialog exit
    await closeTopLayerDialog(lightboxDialog, () => lbSeq, () => ++lbSeq);
    focusSafely(lbOriginTrigger);
  }

  function stepLightbox(delta) {
    if (!lightboxDialog || !lightboxDialog.open) return;
    renderLightboxItem(lbIndex + delta, true);
    if (!isReducedMotion() && lbImg && typeof lbImg.animate === 'function') {
      const shift = delta > 0 ? 18 : -18;
      lbImg.animate(
        [
          { opacity: 0.45, transform: `translate3d(${shift}px, 0, 0) scale(0.985)` },
          { opacity: 1, transform: 'translate3d(0, 0, 0) scale(1)' }
        ],
        { duration: 220, easing: 'cubic-bezier(0.22, 1, 0.36, 1)' }
      );
    }
  }

  zoomTriggers.forEach((btn) => {
    btn.addEventListener('click', () => openLightbox(btn));
  });

  if (lbCloseBtn) lbCloseBtn.addEventListener('click', () => closeLightbox());
  if (lbPrevBtn) lbPrevBtn.addEventListener('click', () => stepLightbox(-1));
  if (lbNextBtn) lbNextBtn.addEventListener('click', () => stepLightbox(1));

  if (lbZoomBtn) {
    lbZoomBtn.addEventListener('click', () => {
      lbScale = lbZoomed ? 1 : 2;
      if (lbScale <= 1) { lbPanX = 0; lbPanY = 0; }
      applyZoomTransform();
    });
  }

  if (lbImg) {
    lbImg.addEventListener('click', () => {
      if (lbDidDrag) {
        lbDidDrag = false;
        return;
      }
      lbScale = lbZoomed ? 1 : 2;
      if (lbScale <= 1) { lbPanX = 0; lbPanY = 0; }
      applyZoomTransform();
    });

    lbImg.addEventListener('pointerdown', (e) => {
      if (!lbZoomed) return;
      lbDragging = true;
      lbDidDrag = false;
      lbDragStartX = e.clientX - lbPanX;
      lbDragStartY = e.clientY - lbPanY;
      if (lbViewport) lbViewport.classList.add('is-dragging');
      if (typeof lbImg.setPointerCapture === 'function') {
        try { lbImg.setPointerCapture(e.pointerId); } catch (_) {}
      }
    });
  }

  if (lbViewport) {
    lbViewport.addEventListener('wheel', (e) => {
      if (!lightboxDialog || !lightboxDialog.open) return;
      e.preventDefault();
      const delta = e.deltaY < 0 ? 0.25 : -0.25;
      lbScale = Math.max(1, Math.min(2.5, Number((lbScale + delta).toFixed(2))));
      applyZoomTransform();
    }, { passive: false });
  }

  window.addEventListener('pointermove', (e) => {
    if (!lbDragging || !lbZoomed) return;
    const nextX = e.clientX - lbDragStartX;
    const nextY = e.clientY - lbDragStartY;
    if (Math.abs(nextX - lbPanX) > 3 || Math.abs(nextY - lbPanY) > 3) {
      lbDidDrag = true;
    }
    const clamped = clampPan(nextX, nextY);
    lbPanX = clamped.x;
    lbPanY = clamped.y;
    applyZoomTransform();
  });

  window.addEventListener('pointerup', () => {
    if (!lbDragging) return;
    lbDragging = false;
    if (lbViewport) lbViewport.classList.remove('is-dragging');
  });

  if (lightboxDialog) {
    lightboxDialog.addEventListener('keydown', (event) => {
      if (event.key === 'ArrowRight') {
        event.preventDefault();
        stepLightbox(1);
      } else if (event.key === 'ArrowLeft') {
        event.preventDefault();
        stepLightbox(-1);
      }
    });

    lightboxDialog.addEventListener('click', (event) => {
      const t = event.target;
      if (t === lightboxDialog || t === lbShell || t === lbBody || t === lbViewport) {
        closeLightbox();
      }
    });

    lightboxDialog.addEventListener('cancel', (event) => {
      event.preventDefault();
      closeLightbox();
    });
  }

  // Restore saved language if any
  try {
    const savedLang = localStorage.getItem('shipforge-site-lang');
    if (savedLang === 'en') applyLang('en');
  } catch (_) {}

  mediaUiReady = true;
  syncLightboxLanguage();
})();
