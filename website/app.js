(function () {
  "use strict";

  const SCHEME_KEY = "sf-site-scheme";
  const LANG_KEY = "sf-site-lang";

  const SHOTS = [
    {
      src: "assets/screenshots/01-overview-pipeline.png",
      zhTitle: "发版总览 · 免重编部署与一键构建",
      enTitle: "Release Overview · No-Rebuild Deploy & Pipeline",
      zhCap: "发版总览：自动识别本地就绪成品，支持免重编部署推送与一键版本构建。",
      enCap: "Release Overview: Detects externally built artifacts for no-rebuild deployment alongside one-click build pipelines.",
    },
    {
      src: "assets/screenshots/02-compile-prereq.png",
      zhTitle: "编译与环境 · 项目级前置自检",
      enTitle: "Compile & Environment · Scoped Prerequisite Check",
      zhCap: "编译与环境：按当前项目语言与打包壳核验工具链、依赖与磁盘余量，确认后按需补全。",
      enCap: "Compile & Environment: Inspects only toolchains, dependencies, and disk headroom needed by the active project.",
    },
    {
      src: "assets/screenshots/03-package-split.png",
      zhTitle: "分析与分包 · 体积分布与敏感拦截",
      enTitle: "Analyze & Split · Size Breakdown & Secret Guard",
      zhCap: "分析与分包：统计目录体积与 Top 大文件，默认拦截 .env 与私钥文件防泄漏。",
      enCap: "Analyze & Split: Maps directory footprints and largest files while blocking .env and private keys by default.",
    },
    {
      src: "assets/screenshots/04-release-repos.png",
      zhTitle: "产物与仓库 · 多平台直连发布",
      enTitle: "Artifacts & Repos · Multi-Platform Publishing",
      zhCap: "产物与仓库：WinHTTP 流式上传至 GitHub / Gitee / GitLab / Gitea，支持编辑、草稿与撤回。",
      enCap: "Artifacts & Repos: Streams assets via WinHTTP to GitHub, Gitee, GitLab, or Gitea with edit and recall support.",
    },
    {
      src: "assets/screenshots/05-version-changelog.png",
      zhTitle: "版本与源码 · 双栏变更日志与版本同步",
      enTitle: "Version & Source · Split Changelog & Atomic Sync",
      zhCap: "版本与源码：双栏排版浏览与撰写变更记录，多配置文件版本号原子对齐。",
      enCap: "Version & Source: Split-pane changelog editing with atomic version synchronization across project configs.",
    },
    {
      src: "assets/screenshots/06-updater-prefs.png",
      zhTitle: "偏好设置 · 版本与官方自动更新",
      enTitle: "Preferences · Version & Official Auto-Updater",
      zhCap: "偏好设置：三通道容错官方更新，支持断点续传、SHA-256 校验与便携版自动回滚。",
      enCap: "Preferences: Three-tier official updater with resumable downloads, SHA-256 verification, and portable rollback.",
    },
  ];

  const I18N = {
    en: {
      "brand.cn": "",
      "nav.pipeline": "Pipeline",
      "nav.showcase": "Modules",
      "nav.bench": "Metrics",
      "nav.download": "Download",
      "nav.star": "Star",
      "nav.get": "Download",
      "hero.title": "Build, package, sync versions, and ship from one desktop studio",
      "hero.sub": "A local Windows release workbench built with Tauri v2 and native Rust. Detects frameworks and build scripts, guards secrets before packaging, publishes to GitHub / Gitee / GitLab / Gitea via WinHTTP + DPAPI, and includes SHA-256 auto-updates.",
      "hero.dlSetup": "Download Installer (.exe)",
      "hero.dlPort": "Portable ZIP (.zip)",
      "hero.facts": "Zero external runtime · 112 Rust + 21 JS tests · Transactional rollback",
      "tab.overview": "Overview",
      "tab.compile": "Compile & Prereq",
      "tab.package": "Analyze & Split",
      "tab.release": "Multi-Repo",
      "tab.version": "Version & Log",
      "tab.updater": "Auto-Updater",
      "stage.hint": "Click screenshot to zoom",
      "pipe.heading": "A controlled local release pipeline",
      "pipe.desc": "Every stage from workspace scan to remote release upload is verified with preflight checks and rollback protection.",
      "pipe.s1.t": "Detect & Reconcile",
      "pipe.s1.d": "Detects languages, shells, build scripts, and version files across both Git and non-Git folders.",
      "pipe.s2.t": "Scoped Preflight",
      "pipe.s2.d": "Checks only toolchains, dependencies, and disk headroom required by the active project before installing anything.",
      "pipe.s3.t": "Build & Package",
      "pipe.s3.d": "Captures live build logs, blocks .env and private keys by default, and splits oversized archives for repo limits.",
      "pipe.s4.t": "Verify & Manifest",
      "pipe.s4.d": "Computes SHA-256 in parallel and generates latest.json + SHA256SUMS.txt with version alignment checks.",
      "pipe.s5.t": "Atomic Publish",
      "pipe.s5.d": "Streams binaries via WinHTTP first and publishes latest.json last, keeping verified local artifacts safe on errors.",
      "show.heading": "Built for everyday shipping workflows",
      "show.desc": "Clean, low-noise desktop layout with a single entry point for every setting. Click any capture to inspect full-size.",
      "show.f1.t": "Project-scoped prerequisite checks without bloat",
      "show.f1.d": "Inspects only the compilers and tools needed by your language and shell combination (MSVC, MinGW, rust-lld, NSIS, Node, Python) and previews the repair plan before making changes.",
      "show.f1.p1": "Supports 8 languages and shells including Tauri, Electron, Wails, PyInstaller, and NSIS",
      "show.f1.p2": "Introspects local build.ps1 switches and preserves custom project actions",
      "show.f2.t": "See disk footprint first, block sensitive files by default",
      "show.f2.d": "Breaks down directory sizes by file type and highlights largest files. Flags .env, private keys, certificates, and .git with one-click exclusion and multi-volume splitting.",
      "show.f2.p1": "Skips target, node_modules, and .git cache folders automatically",
      "show.f2.p2": "Save reusable packaging profiles (portable ZIP, tar.gz, 95 MB split volumes)",
      "show.f3.t": "Direct multi-platform publishing with DPAPI token vault",
      "show.f3.d": "Native adapters for GitHub, Gitee, GitLab, and Gitea. Browse online releases on the left, create, edit, toggle draft, recall, or delete individual assets on the right.",
      "show.f3.p1": "Tokens are encrypted with Windows DPAPI with zero plaintext on disk",
      "show.f3.p2": "Validates latest.json hashes and versions before uploading to prevent broken feeds",
      "show.f4.t": "Atomic version sync and built-in official auto-updater",
      "show.f4.d": "Writes back ascending or descending changelogs cleanly and syncs versions across config files. ShipForge also includes a 3-tier GitHub Releases updater with resumable downloads and rollback.",
      "show.f4.p1": "Supports both NSIS Installer and Portable ZIP upgrades with SHA-256 & header checks",
      "show.f4.p2": "Refuses to interrupt active build or publish tasks before restarting to install",
      "bench.heading": "Measured Metrics & Framework Matrix",
      "bench.desc": "All figures measured directly from ShipForge v1.4.0 builds and tests on Windows 11 x64.",
      "bench.tabGate": "Artifacts & Gate",
      "bench.tabSpeed": "Build & Hash Speed",
      "bench.k1": "NSIS Installer Size",
      "bench.k2": "Rust + Frontend Tests",
      "bench.k3": "Parallel Test Gate",
      "bench.k4": "Language × Shell Matrix",
      "mat.langs": "Languages",
      "mat.shells": "Shells",
      "dl.heading": "Download ShipForge",
      "dl.desc": "Requires Windows 10 / 11 (x64). Every release ships with SHA256SUMS.txt and latest.json.",
      "dl.recommended": "Recommended",
      "dl.portableTag": "No Install",
      "dl.setupTitle": "Windows Installer (NSIS)",
      "dl.portTitle": "Windows Portable (ZIP)",
      "dl.copy": "Copy",
      "dl.btnSetup": "Download Installer",
      "dl.btnPort": "Download Portable ZIP",
      "dev.summary": "Build from source & publish updates locally (Developers)",
      "dl.notesTitle": "Latest Release Notes",
      "dl.allReleases": "All Releases →",
      "footer.sponsor": "Support",
      "sponsor.title": "Support ShipForge",
      "sponsor.desc": "ShipForge is free, open-source, and ad-free. If it saves you time when shipping builds, consider buying me a coffee or leaving a Star on GitHub.",
      "sponsor.wechat": "WeChat Pay",
      "sponsor.alipay": "Alipay",
    },
  };

  const BENCH_MODES = {
    gate: {
      zh: [
        { v: "3.40 MB", l: "NSIS 安装包体积", s: "便携 ZIP 4.71 MB · 主程序 11.32 MB" },
        { v: "112 + 21", l: "Rust + 前端自动化门禁", s: "Release 严格 0 警告 · 全绿通过" },
        { v: "21.4 s", l: "全量并行测试门禁耗时", s: "Rust 单测 + Release Check + 17 JS" },
        { v: "8 × 8", l: "语言与打包外壳支持矩阵", s: "GitHub · Gitee · GitLab · Gitea" },
      ],
      en: [
        { v: "3.40 MB", l: "NSIS Installer Size", s: "Portable ZIP 4.71 MB · Exe 11.32 MB" },
        { v: "112 + 21", l: "Rust + Frontend Tests", s: "Strict 0 Release warnings · All passing" },
        { v: "21.4 s", l: "Parallel Test Gate", s: "Rust tests + Release Check + 17 JS" },
        { v: "8 × 8", l: "Language × Shell Matrix", s: "GitHub · Gitee · GitLab · Gitea" },
      ],
    },
    speed: {
      zh: [
        { v: "1m12s", l: "全量测试 + 编译打包总耗时", s: "含 NSIS 安装包、便携 ZIP 与清单校验" },
        { v: "12×", l: "单模块增量发布编译提速", s: "rust-lld + 并行 codegen-units 优化" },
        { v: "256 KiB", l: "WinHTTP 流式上传缓冲", s: "30s/180s 超时保护 + 指数退避重试" },
        { v: "3 通道", l: "官方在线更新容错链路", s: "latest.json → 302 标签 → API / Atom" },
      ],
      en: [
        { v: "1m12s", l: "Full Gate + Release Build", s: "Includes NSIS, Portable ZIP & SHA-256" },
        { v: "12×", l: "Incremental Release Speedup", s: "rust-lld + parallel codegen-units" },
        { v: "256 KiB", l: "WinHTTP Streaming Buffer", s: "30s/180s timeouts + exponential backoff" },
        { v: "3 Tiers", l: "Resilient Auto-Update Feed", s: "latest.json → 302 Tag → API / Atom" },
      ],
    },
  };

  // Capture Chinese defaults from DOM
  const zhDefaults = {};
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    zhDefaults[el.dataset.i18n] = el.textContent;
  });

  // ---- Color Scheme (modern-web-guidance: dark-mode-toggle) ----
  const schemeMeta = document.querySelector('meta[name="color-scheme"]');
  function osScheme() {
    return window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  function getStoredScheme() {
    try {
      const s = localStorage.getItem(SCHEME_KEY);
      return s === "light" || s === "dark" ? s : null;
    } catch {
      return null;
    }
  }
  function renderedScheme() {
    return getStoredScheme() || osScheme();
  }
  function applySchemeMeta(val) {
    if (schemeMeta) schemeMeta.content = val === "light" || val === "dark" ? val : "light dark";
  }
  function toggleScheme() {
    const target = renderedScheme() === "dark" ? "light" : "dark";
    try {
      if (target === osScheme()) {
        localStorage.removeItem(SCHEME_KEY);
        applySchemeMeta("light dark");
      } else {
        localStorage.setItem(SCHEME_KEY, target);
        applySchemeMeta(target);
      }
    } catch {
      applySchemeMeta(target);
    }
  }
  document.getElementById("theme-toggle")?.addEventListener("click", toggleScheme);
  window.addEventListener("storage", (ev) => {
    if (ev.key === SCHEME_KEY) applySchemeMeta(getStoredScheme() || "light dark");
  });

  // ---- Language & Benchmarks ----
  let currentLang = (() => {
    const qp = new URLSearchParams(location.search).get("lang");
    if (qp === "en" || qp === "zh") return qp;
    try {
      const s = localStorage.getItem(LANG_KEY);
      if (s === "en" || s === "zh") return s;
    } catch {}
    return "zh";
  })();
  let currentBenchMode = "gate";
  let activeShotIndex = 0;

  function renderBenchMode(mode) {
    currentBenchMode = BENCH_MODES[mode] ? mode : "gate";
    const rows = BENCH_MODES[currentBenchMode][currentLang] || BENCH_MODES[currentBenchMode].zh;
    const grid = document.getElementById("bench-kpi-grid");
    if (grid) {
      rows.forEach((r, idx) => {
        const i = idx + 1;
        const vEl = grid.querySelector(`[data-kpi="v${i}"]`);
        const lEl = grid.querySelector(`[data-kpi="l${i}"]`);
        const sEl = grid.querySelector(`[data-kpi="s${i}"]`);
        if (vEl) vEl.textContent = r.v;
        if (lEl) lEl.textContent = r.l;
        if (sEl) sEl.textContent = r.s;
      });
    }
    document.querySelectorAll(".bench-btn").forEach((b) => {
      const on = b.dataset.bench === currentBenchMode;
      b.classList.toggle("active", on);
      b.setAttribute("aria-selected", String(on));
    });
  }

  function updateStageCaption(idx) {
    const cap = document.getElementById("stage-caption");
    const item = SHOTS[idx] || SHOTS[0];
    if (cap) cap.textContent = currentLang === "en" ? item.enCap : item.zhCap;
  }

  function applyLang(lang) {
    currentLang = lang === "en" ? "en" : "zh";
    try {
      localStorage.setItem(LANG_KEY, currentLang);
    } catch {}
    document.documentElement.lang = currentLang === "en" ? "en" : "zh-CN";
    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const key = el.dataset.i18n;
      el.textContent = currentLang === "en" ? (I18N.en[key] ?? zhDefaults[key]) : zhDefaults[key];
    });
    const label = document.getElementById("lang-label");
    if (label) label.textContent = currentLang === "en" ? "中文" : "EN";
    renderBenchMode(currentBenchMode);
    updateStageCaption(activeShotIndex);
    syncLightboxLang();
  }

  document.getElementById("lang-toggle")?.addEventListener("click", () => {
    applyLang(currentLang === "en" ? "zh" : "en");
  });

  document.querySelectorAll(".bench-btn").forEach((btn) => {
    btn.addEventListener("click", () => renderBenchMode(btn.dataset.bench));
  });

  // ---- Accessible Interactive Stage Tabs (Keyboard Arrow/Home/End) ----
  const tabs = Array.from(document.querySelectorAll(".stage-tab"));
  const panels = tabs.map((t) => document.getElementById(t.dataset.panel)).filter(Boolean);

  function selectTab(nextTab, moveFocus) {
    tabs.forEach((t) => {
      const on = t === nextTab;
      t.classList.toggle("active", on);
      t.setAttribute("aria-selected", String(on));
      t.tabIndex = on ? 0 : -1;
    });
    panels.forEach((p) => {
      const on = p.id === nextTab.dataset.panel;
      p.classList.toggle("active", on);
      p.hidden = !on;
    });
    activeShotIndex = Number(nextTab.dataset.shot) || 0;
    updateStageCaption(activeShotIndex);
    if (moveFocus) nextTab.focus();
  }

  tabs.forEach((tab, idx) => {
    tab.addEventListener("click", () => selectTab(tab, false));
    tab.addEventListener("keydown", (ev) => {
      if (ev.key === "ArrowRight") {
        ev.preventDefault();
        selectTab(tabs[(idx + 1) % tabs.length], true);
      } else if (ev.key === "ArrowLeft") {
        ev.preventDefault();
        selectTab(tabs[(idx - 1 + tabs.length) % tabs.length], true);
      } else if (ev.key === "Home") {
        ev.preventDefault();
        selectTab(tabs[0], true);
      } else if (ev.key === "End") {
        ev.preventDefault();
        selectTab(tabs[tabs.length - 1], true);
      }
    });
  });

  // ---- Scroll Progress Fallback + Shrinking Header + Nav Spy ----
  const headerEl = document.getElementById("site-header");
  const progEl = document.getElementById("scroll-progress");
  const nativeScrollTimeline = window.CSS && CSS.supports && CSS.supports("animation-timeline", "scroll()");
  const navLinks = Array.from(document.querySelectorAll(".site-nav a[data-section]"));
  const spySections = navLinks.map((a) => document.getElementById(a.dataset.section)).filter(Boolean);

  function syncOnScroll() {
    const y = window.scrollY || 0;
    if (headerEl) headerEl.classList.toggle("scrolled", y > 18);
    if (!nativeScrollTimeline && progEl) {
      const max = Math.max(1, document.documentElement.scrollHeight - window.innerHeight);
      progEl.style.transform = `scaleX(${Math.min(1, Math.max(0, y / max))})`;
    }
    let currentId = "";
    for (const sec of spySections) {
      if (sec.getBoundingClientRect().top <= 130) currentId = sec.id;
    }
    navLinks.forEach((a) => a.classList.toggle("active", a.dataset.section === currentId));
  }
  window.addEventListener("scroll", syncOnScroll, { passive: true });
  syncOnScroll();

  // ---- Toast & Clipboard with Legacy Fallback ----
  let toastTimer = null;
  function showToast(msg) {
    const el = document.getElementById("site-toast");
    if (!el) return;
    el.textContent = msg;
    el.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      el.hidden = true;
    }, 2200);
  }

  function legacyCopy(text) {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.top = "-9999px";
    document.body.appendChild(ta);
    ta.select();
    let ok = false;
    try {
      ok = document.execCommand("copy");
    } catch {
      ok = false;
    }
    ta.remove();
    return ok;
  }

  document.querySelectorAll("[data-copy-target]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const target = document.getElementById(btn.dataset.copyTarget);
      const txt = target ? target.textContent.trim() : "";
      if (!txt || txt.includes("加载中")) return;
      let ok = false;
      if (navigator.clipboard && navigator.clipboard.writeText) {
        try {
          await navigator.clipboard.writeText(txt);
          ok = true;
        } catch {
          ok = legacyCopy(txt);
        }
      } else {
        ok = legacyCopy(txt);
      }
      if (ok) showToast(currentLang === "en" ? "Copied to clipboard" : "已复制到剪贴板");
    });
  });

  // ---- Top-Layer Dialog Exit & Light-Dismiss Fallback (modern-web-guidance) ----
  let supportsDisplayTransition;
  function canTransitionDisplay() {
    if (supportsDisplayTransition !== undefined) return supportsDisplayTransition;
    if (!window.CSS?.supports?.("transition-behavior", "allow-discrete") || !document.body) {
      return false;
    }
    const probe = document.createElement("div");
    probe.style.cssText = "transition: display 1s allow-discrete !important; display: block;";
    document.body.appendChild(probe);
    getComputedStyle(probe).display;
    probe.style.display = "none";
    supportsDisplayTransition = getComputedStyle(probe).display === "block";
    probe.remove();
    return supportsDisplayTransition;
  }

  function supportsTopLayerExit() {
    return Boolean(window.CSS?.supports?.("overlay", "auto") && canTransitionDisplay());
  }

  async function closeTopLayerDialog(dialog) {
    if (!dialog || !dialog.open) return;
    if (!supportsTopLayerExit()) {
      dialog.setAttribute("data-closing", "");
      const anims = dialog.getAnimations ? dialog.getAnimations({ subtree: true }) : [];
      if (anims.length > 0) {
        await Promise.race([
          Promise.allSettled(anims.map((a) => a.finished)),
          new Promise((r) => setTimeout(r, 400)),
        ]);
      }
      if (!dialog.hasAttribute("data-closing")) return;
      dialog.removeAttribute("data-closing");
    }
    dialog.close();
  }

  function wireLightDismissFallback(dialog, onClose) {
    if (!dialog) return;
    dialog.addEventListener("cancel", (ev) => {
      ev.preventDefault();
      onClose();
    });
    if (!("closedBy" in HTMLDialogElement.prototype)) {
      dialog.addEventListener("click", (ev) => {
        if (ev.target !== dialog) return;
        const rect = dialog.getBoundingClientRect();
        const inside =
          rect.top <= ev.clientY &&
          ev.clientY <= rect.top + rect.height &&
          rect.left <= ev.clientX &&
          ev.clientX <= rect.left + rect.width;
        if (!inside) onClose();
      });
    }
  }

  // ---- Sponsor Dialog ----
  const sponsorDlg = document.getElementById("sponsor-dialog");
  let sponsorReturnEl = null;
  function openSponsor(trigger) {
    if (!sponsorDlg) return;
    sponsorReturnEl = trigger || document.activeElement;
    sponsorDlg.removeAttribute("data-closing");
    sponsorDlg.showModal();
  }
  async function closeSponsor() {
    await closeTopLayerDialog(sponsorDlg);
    if (sponsorReturnEl && sponsorReturnEl.focus) sponsorReturnEl.focus();
  }
  document.getElementById("header-coffee-btn")?.addEventListener("click", (e) => openSponsor(e.currentTarget));
  document.getElementById("footer-coffee-btn")?.addEventListener("click", (e) => openSponsor(e.currentTarget));
  document.getElementById("sponsor-close")?.addEventListener("click", closeSponsor);
  wireLightDismissFallback(sponsorDlg, closeSponsor);

  // ---- Media Lightbox (Same-Document View Transition + Zoom/Pan + Keyboard) ----
  const lbDlg = document.getElementById("media-lightbox");
  const lbImg = document.getElementById("lb-img");
  const lbStage = document.getElementById("lb-stage");
  const lbCount = document.getElementById("lb-count");
  const lbTitle = document.getElementById("lb-title");
  const lbCap = document.getElementById("lb-caption");
  const lbZoomReset = document.getElementById("lb-zoom-reset");
  const lbCloseBtn = document.getElementById("lb-close");

  let lbIndex = 0;
  let lbScale = 1;
  let lbTx = 0;
  let lbTy = 0;
  let lbTriggerEl = null;
  let dragging = false;
  let dragX = 0;
  let dragY = 0;

  function isReducedMotion() {
    return window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  }

  function applyLbTransform() {
    if (!lbImg) return;
    lbImg.style.transform = `translate(${lbTx}px, ${lbTy}px) scale(${lbScale})`;
    if (lbZoomReset) lbZoomReset.textContent = `${Math.round(lbScale * 100)}%`;
  }

  function resetLbZoom() {
    lbScale = 1;
    lbTx = 0;
    lbTy = 0;
    applyLbTransform();
  }

  function syncLightboxLang() {
    const item = SHOTS[lbIndex] || SHOTS[0];
    if (lbCount) lbCount.textContent = `${lbIndex + 1} / ${SHOTS.length}`;
    if (lbTitle) lbTitle.textContent = currentLang === "en" ? item.enTitle : item.zhTitle;
    if (lbCap) lbCap.textContent = currentLang === "en" ? item.enCap : item.zhCap;
  }

  function renderLbSlide(idx) {
    lbIndex = (idx + SHOTS.length) % SHOTS.length;
    const item = SHOTS[lbIndex];
    if (lbImg) {
      lbImg.src = item.src;
      lbImg.alt = currentLang === "en" ? item.enTitle : item.zhTitle;
    }
    resetLbZoom();
    syncLightboxLang();
  }

  function clearTransitionNames() {
    document.querySelectorAll(".shot-trigger img").forEach((img) => {
      img.style.viewTransitionName = "";
    });
    if (lbImg) lbImg.style.viewTransitionName = "";
  }

  function openLightbox(triggerEl, idx) {
    if (!lbDlg || !lbImg) return;
    lbTriggerEl = triggerEl;
    renderLbSlide(idx);
    lbDlg.removeAttribute("data-closing");

    const srcImg = triggerEl ? triggerEl.querySelector("img") : null;
    if (document.startViewTransition && !isReducedMotion() && srcImg) {
      clearTransitionNames();
      srcImg.style.viewTransitionName = "sf-lightbox-media";
      const vt = document.startViewTransition(() => {
        srcImg.style.viewTransitionName = "";
        lbImg.style.viewTransitionName = "sf-lightbox-media";
        lbDlg.showModal();
      });
      vt.finished.finally(() => {
        clearTransitionNames();
        lbCloseBtn?.focus();
      });
    } else {
      lbDlg.showModal();
      lbCloseBtn?.focus();
    }
  }

  async function closeLightbox() {
    if (!lbDlg || !lbDlg.open) return;
    clearTransitionNames();
    await closeTopLayerDialog(lbDlg);
    if (lbTriggerEl && lbTriggerEl.focus) lbTriggerEl.focus();
  }

  document.querySelectorAll(".shot-trigger[data-lb-index]").forEach((btn) => {
    btn.addEventListener("click", () => openLightbox(btn, Number(btn.dataset.lbIndex) || 0));
  });

  lbCloseBtn?.addEventListener("click", closeLightbox);
  document.getElementById("lb-prev")?.addEventListener("click", () => renderLbSlide(lbIndex - 1));
  document.getElementById("lb-next")?.addEventListener("click", () => renderLbSlide(lbIndex + 1));
  document.getElementById("lb-zoom-in")?.addEventListener("click", () => {
    lbScale = Math.min(3, +(lbScale + 0.5).toFixed(2));
    applyLbTransform();
  });
  document.getElementById("lb-zoom-out")?.addEventListener("click", () => {
    lbScale = Math.max(1, +(lbScale - 0.5).toFixed(2));
    if (lbScale === 1) {
      lbTx = 0;
      lbTy = 0;
    }
    applyLbTransform();
  });
  lbZoomReset?.addEventListener("click", resetLbZoom);
  wireLightDismissFallback(lbDlg, closeLightbox);

  lbStage?.addEventListener("pointerdown", (ev) => {
    if (lbScale <= 1) return;
    dragging = true;
    dragX = ev.clientX - lbTx;
    dragY = ev.clientY - lbTy;
    lbStage.classList.add("dragging");
    lbStage.setPointerCapture?.(ev.pointerId);
  });
  lbStage?.addEventListener("pointermove", (ev) => {
    if (!dragging) return;
    lbTx = Math.max(-600, Math.min(600, ev.clientX - dragX));
    lbTy = Math.max(-400, Math.min(400, ev.clientY - dragY));
    applyLbTransform();
  });
  lbStage?.addEventListener("pointerup", () => {
    dragging = false;
    lbStage.classList.remove("dragging");
  });
  lbStage?.addEventListener("wheel", (ev) => {
    ev.preventDefault();
    const delta = ev.deltaY < 0 ? 0.25 : -0.25;
    lbScale = Math.max(1, Math.min(3, +(lbScale + delta).toFixed(2)));
    if (lbScale === 1) {
      lbTx = 0;
      lbTy = 0;
    }
    applyLbTransform();
  }, { passive: false });

  lbDlg?.addEventListener("keydown", (ev) => {
    if (!lbDlg.open) return;
    if (ev.key === "ArrowLeft") {
      ev.preventDefault();
      renderLbSlide(lbIndex - 1);
    } else if (ev.key === "ArrowRight") {
      ev.preventDefault();
      renderLbSlide(lbIndex + 1);
    } else if (ev.key === "+" || ev.key === "=") {
      ev.preventDefault();
      lbScale = Math.min(3, +(lbScale + 0.5).toFixed(2));
      applyLbTransform();
    } else if (ev.key === "-") {
      ev.preventDefault();
      lbScale = Math.max(1, +(lbScale - 0.5).toFixed(2));
      if (lbScale === 1) { lbTx = 0; lbTy = 0; }
      applyLbTransform();
    } else if (ev.key === "0") {
      ev.preventDefault();
      resetLbZoom();
    }
  });

  // ---- Hydrate latest.json & repo-meta.json ----
  function fmtMiB(bytes) {
    const n = Number(bytes) || 0;
    return n > 0 ? `${(n / (1024 * 1024)).toFixed(2)} MB` : "";
  }

  fetch("latest.json", { cache: "no-cache" })
    .then((r) => (r.ok ? r.json() : null))
    .then((m) => {
      if (!m || !m.version) return;
      const pill = document.getElementById("hero-ver-pill");
      if (pill) pill.textContent = `v${m.version} · Windows 10/11 x64 · Tauri v2 + Rust · MIT`;

      if (m.installer?.url) {
        const sizeTxt = fmtMiB(m.installer.size);
        document.getElementById("hero-setup-btn")?.setAttribute("href", m.installer.url);
        document.getElementById("dl-setup-link")?.setAttribute("href", m.installer.url);
        const hs = document.getElementById("hero-setup-size");
        const ds = document.getElementById("dl-setup-size");
        const df = document.getElementById("dl-setup-file");
        const sh = document.getElementById("dl-setup-sha");
        if (hs && sizeTxt) hs.textContent = sizeTxt;
        if (ds && sizeTxt) ds.textContent = sizeTxt;
        if (df && m.installer.file) df.textContent = m.installer.file;
        if (sh && m.installer.sha256) sh.textContent = m.installer.sha256;
      }

      if (m.portable?.url) {
        const sizeTxt = fmtMiB(m.portable.size);
        document.getElementById("hero-portable-btn")?.setAttribute("href", m.portable.url);
        document.getElementById("dl-port-link")?.setAttribute("href", m.portable.url);
        const hp = document.getElementById("hero-port-size");
        const dp = document.getElementById("dl-port-size");
        const pf = document.getElementById("dl-port-file");
        const ps = document.getElementById("dl-port-sha");
        if (hp && sizeTxt) hp.textContent = sizeTxt;
        if (dp && sizeTxt) dp.textContent = sizeTxt;
        if (pf && m.portable.file) pf.textContent = m.portable.file;
        if (ps && m.portable.sha256) ps.textContent = m.portable.sha256;
      }

      if (m.notes) {
        const box = document.getElementById("release-notes-box");
        const pre = document.getElementById("release-notes-text");
        if (box && pre) {
          pre.textContent = m.notes;
          box.hidden = false;
        }
      }
    })
    .catch(() => {});

  fetch("repo-meta.json", { cache: "no-cache" })
    .then((r) => (r.ok ? r.json() : null))
    .then((meta) => {
      const count = document.getElementById("header-star-count");
      if (count && meta && Number(meta.stars) > 0) {
        count.textContent = String(meta.stars);
        count.hidden = false;
      }
    })
    .catch(() => {});

  applyLang(currentLang);
})();
