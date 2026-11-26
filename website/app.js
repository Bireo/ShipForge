(function () {
  const I18N = {
    en: {
      "nav.features": "Features",
      "nav.workflow": "Workflow",
      "nav.download": "Download",
      "nav.get": "Download",
      "hero.title": "Build, package, sync versions, and ship from one desktop window",
      "hero.sub": "A local Windows release studio for Rust, Tauri, Node, Python, Go, .NET, and Flutter projects. Publish directly to GitHub, Gitee, GitLab, or Gitea with built-in auto-updates.",
      "hero.dlSetup": "Download Installer (.exe)",
      "hero.dlPort": "Download Portable (.zip)",
      "feat.heading": "Features",
      "feat.f1.t": "Multi-framework detection",
      "feat.f1.d": "Detects Rust, Node/TS, Python, Go, .NET, Flutter, C++, Java, and common packaging shells with project-scoped prerequisite checks.",
      "feat.f2.t": "Size analysis & split archives",
      "feat.f2.d": "Inspects directory sizes, blocks .env, private keys, and .git by default, and supports multi-volume ZIP archives with checksums.",
      "feat.f3.t": "Atomic version sync",
      "feat.f3.d": "Syncs Cargo.toml, tauri.conf.json, package.json, pyproject.toml, and custom regex files together with changelog notes.",
      "feat.f4.t": "Direct multi-platform publishing",
      "feat.f4.d": "Streams releases via native WinHTTP to GitHub, Gitee, GitLab, and Gitea using DPAPI-encrypted tokens.",
      "feat.f5.t": "Transactional rollback",
      "feat.f5.d": "Protects verified build artifacts and rolls back incomplete local version or Git changes on failure or abort.",
      "feat.f6.t": "Built-in auto-updater",
      "feat.f6.d": "Checks official GitHub Releases for installer and portable builds, verifies SHA-256 and binary headers, and rolls back failed swaps.",
      "wf.heading": "Build & Publish Locally",
      "dl.heading": "Download Latest Release",
      "dl.setupTitle": "Windows Installer (NSIS)",
      "dl.portTitle": "Windows Portable (ZIP)",
      "dl.btn": "Download",
      "dl.notes": "Release Notes",
    },
  };

  const zhOriginals = {};
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    zhOriginals[el.dataset.i18n] = el.textContent;
  });

  let lang = new URLSearchParams(location.search).get("lang") === "en" ? "en" : "zh";
  const langBtn = document.getElementById("langToggle");

  function applyLang() {
    document.documentElement.lang = lang === "en" ? "en" : "zh-CN";
    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const k = el.dataset.i18n;
      el.textContent = lang === "en" ? (I18N.en[k] || zhOriginals[k]) : zhOriginals[k];
    });
    if (langBtn) langBtn.textContent = lang === "en" ? "中文" : "EN";
  }

  langBtn?.addEventListener("click", () => {
    lang = lang === "en" ? "zh" : "en";
    applyLang();
  });
  applyLang();

  function fmtMB(bytes) {
    if (!bytes) return "";
    return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
  }

  fetch("latest.json", { cache: "no-cache" })
    .then((r) => (r.ok ? r.json() : null))
    .then((m) => {
      if (!m || !m.version) return;
      const badge = document.getElementById("verBadge");
      const meta = document.getElementById("heroMeta");
      if (badge) badge.textContent = `v${m.version} · Windows 10/11 x64 · MIT`;
      if (meta) meta.textContent = `Latest v${m.version} · ${String(m.pubDate || "").slice(0, 10)}`;

      if (m.installer?.url) {
        document.getElementById("heroSetupBtn")?.setAttribute("href", m.installer.url);
        document.getElementById("dlSetupLink")?.setAttribute("href", m.installer.url);
        const sm = document.getElementById("dlSetupMeta");
        if (sm) sm.textContent = `${m.installer.file} · ${fmtMB(m.installer.size)}`;
      }
      if (m.portable?.url) {
        document.getElementById("heroPortableBtn")?.setAttribute("href", m.portable.url);
        document.getElementById("dlPortLink")?.setAttribute("href", m.portable.url);
        const pm = document.getElementById("dlPortMeta");
        if (pm) pm.textContent = `${m.portable.file} · ${fmtMB(m.portable.size)}`;
      }
      if (m.notes) {
        const nc = document.getElementById("notesCard");
        const pre = document.getElementById("releaseNotes");
        if (nc && pre) {
          pre.textContent = m.notes;
          nc.hidden = false;
        }
      }
    })
    .catch(() => {
      const meta = document.getElementById("heroMeta");
      if (meta) meta.textContent = "GitHub Releases · Windows 10/11 x64";
    });

  fetch("repo-meta.json", { cache: "no-cache" })
    .then((r) => (r.ok ? r.json() : null))
    .then((d) => {
      if (d && Number(d.stars) > 0) {
        const el = document.getElementById("starCount");
        if (el) {
          el.textContent = `★ ${d.stars}`;
          el.hidden = false;
        }
      }
    })
    .catch(() => {});
})();
