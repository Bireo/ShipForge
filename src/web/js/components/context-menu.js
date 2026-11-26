import { API, esc } from "../api.js";
import { copyText, toast } from "../ui.js";

const SUPPORTS_NATIVE_FOCUSGROUP = "focusGroup" in HTMLElement.prototype;
const SUPPORTS_POPOVER = Object.hasOwn(HTMLElement.prototype, "popover");
const SUPPORTS_ANCHOR =
  typeof CSS !== "undefined" &&
  CSS.supports("position-area: block-end") &&
  CSS.supports("anchor-name: --a");

let initialized = false;
let lastFocusedBeforeOpen = null;
let currentApp = null;
let activeAnchorTrigger = null;
let cachedAnchor = null;
let cachedPopover = null;
let cachedGroup = null;
let menuOpenSeq = 0;

function supportsSelectionRange(el) {
  if (!el || (el.tagName !== "INPUT" && el.tagName !== "TEXTAREA")) return false;
  try {
    return typeof el.selectionStart === "number";
  } catch {
    return false;
  }
}

function getElements() {
  if (!cachedAnchor) cachedAnchor = document.getElementById("sfMenuAnchor");
  if (!cachedPopover) cachedPopover = document.getElementById("sfContextMenu");
  if (!cachedGroup) cachedGroup = document.getElementById("sfContextMenuGroup");
  return {
    anchor: cachedAnchor,
    popover: cachedPopover,
    group: cachedGroup,
  };
}

function isEditableInput(el) {
  if (!el) return false;
  if (el.tagName === "TEXTAREA") return true;
  if (el.tagName === "INPUT") {
    const t = (el.type || "text").toLowerCase();
    return !["checkbox", "radio", "range", "button", "submit", "reset", "file", "color"].includes(t);
  }
  return Boolean(el.isContentEditable);
}

function getEditableTarget(target) {
  if (!target || !(target instanceof Element)) return null;
  const match = target.closest(
    'textarea, input:not([type="checkbox"]):not([type="radio"]):not([type="range"]):not([type="button"]):not([type="submit"]), [contenteditable="true"], [contenteditable=""]'
  );
  return match && isEditableInput(match) ? match : null;
}

function getSelectedTextIn(editable) {
  if (editable && supportsSelectionRange(editable)) {
    const start = editable.selectionStart ?? 0;
    const end = editable.selectionEnd ?? 0;
    return end > start ? editable.value.slice(start, end) : "";
  }
  const winSel = (window.getSelection?.()?.toString() || "").trim();
  if (winSel) return winSel;
  // Number/special inputs don't expose selectionStart in Chromium: fall back to full value for copy convenience
  if (editable && editable.tagName === "INPUT" && editable.value) {
    return String(editable.value);
  }
  return "";
}

function replaceSelectionInInput(el, text, savedStart = null, savedEnd = null) {
  if (!el || el.disabled || el.readOnly) return;
  if (el.tagName === "INPUT" || el.tagName === "TEXTAREA") {
    el.focus();
    if (supportsSelectionRange(el)) {
      const start = savedStart ?? el.selectionStart ?? el.value.length;
      const end = savedEnd ?? el.selectionEnd ?? el.value.length;
      el.setRangeText(text, start, end, "end");
    } else {
      // `<input type="number">` throws InvalidStateError on setRangeText; update value directly
      el.value = text;
    }
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  } else if (el.isContentEditable) {
    el.focus();
    document.execCommand?.("insertText", false, text);
  }
}

function resolveReleaseDir(proj) {
  if (!proj) return "";
  const rel = proj.releaseDir || "release";
  return /^[a-zA-Z]:/.test(rel) ? rel : `${proj.root}\\${rel}`;
}

export function closeBuiltInContextMenu() {
  const { anchor, popover } = getElements();
  if (!popover) return;
  if (activeAnchorTrigger) {
    activeAnchorTrigger.setAttribute("aria-expanded", "false");
    activeAnchorTrigger = null;
  }
  try {
    if (SUPPORTS_POPOVER) {
      if (popover.matches(":popover-open")) popover.hidePopover();
    } else {
      popover.classList.remove("sf-ctx-open");
    }
  } catch {}
  // Reclaim to document.body immediately on hide so any host modal <dialog> removal can never detach them
  if (anchor && document.body && (anchor.parentElement !== document.body || popover.parentElement !== document.body)) {
    document.body.append(anchor, popover);
  }
}

/**
 * Render menu items into the shared top-layer popover and open at (x, y) or anchored to an element.
 * Item shape:
 * - `{ sep: true }` -> native `<hr class="sf-menu-sep">`
 * - `{ header: string }` -> compact macOS section header
 * - `{ label, hint?, danger?, disabled?, checked?, onSelect }`
 */
function showMenuItems(items, { x = 0, y = 0, triggerEl = null, label = "快捷操作菜单", fromKeyboard = false }) {
  const { anchor, popover, group } = getElements();
  if (!anchor || !popover || !group) return;

  // Clean duplicate / leading / trailing separators
  const cleaned = [];
  for (const it of items) {
    if (!it) continue;
    if (it.sep) {
      if (!cleaned.length || cleaned[cleaned.length - 1].sep) continue;
    }
    cleaned.push(it);
  }
  while (cleaned.length && cleaned[cleaned.length - 1].sep) cleaned.pop();
  if (!cleaned.length) return;

  lastFocusedBeforeOpen = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  if (activeAnchorTrigger && activeAnchorTrigger !== triggerEl) {
    activeAnchorTrigger.setAttribute("aria-expanded", "false");
  }
  activeAnchorTrigger = triggerEl || null;
  if (activeAnchorTrigger) activeAnchorTrigger.setAttribute("aria-expanded", "true");

  group.setAttribute("aria-label", label);
  const hasChecks = cleaned.some((it) => typeof it.checked === "boolean");
  group.classList.toggle("sf-menu-has-checks", hasChecks);
  if (!SUPPORTS_NATIVE_FOCUSGROUP) {
    group.setAttribute("role", "menu");
  }

  group.innerHTML = cleaned
    .map((item, idx) => {
      if (item.sep) return `<hr class="sf-menu-sep">`;
      if (item.header) return `<div class="sf-menu-header" aria-hidden="true">${esc(item.header)}</div>`;
      const isCheck = typeof item.checked === "boolean";
      const roleAttr = isCheck
        ? `role="menuitemcheckbox" aria-checked="${item.checked ? "true" : "false"}"`
        : !SUPPORTS_NATIVE_FOCUSGROUP
          ? `role="menuitem"`
          : "";
      const cls = [
        "sf-menu-item",
        item.danger ? "danger" : "",
        isCheck ? "sf-menu-checkable" : "",
      ]
        .filter(Boolean)
        .join(" ");
      return `<button class="${cls}" type="button" data-ctx-idx="${idx}" ${roleAttr} ${item.disabled ? "disabled" : ""}>
        <span class="sf-menu-label">${esc(item.label)}</span>
        ${item.hint ? `<span class="sf-menu-hint" aria-hidden="true">${esc(item.hint)}</span>` : ""}
      </button>`;
    })
    .join("");

  const openSeq = ++menuOpenSeq;
  group.querySelectorAll("[data-ctx-idx]").forEach((btn) => {
    const item = cleaned[Number(btn.dataset.ctxIdx)];
    if (!item || item.disabled) return;
    btn.addEventListener("pointermove", () => {
      if (document.activeElement !== btn) btn.focus({ preventScroll: true });
    });
    btn.addEventListener("click", async () => {
      const keepBrief = typeof item.checked === "boolean";
      if (keepBrief) {
        const nextChecked = !item.checked;
        item.checked = nextChecked;
        btn.setAttribute("aria-checked", nextChecked ? "true" : "false");
      } else {
        // Close menu BEFORE awaiting onSelect so modals (confirmDialog/choiceDialog) are never obscured by the menu
        closeBuiltInContextMenu();
      }
      try {
        await item.onSelect?.();
      } catch (err) {
        toast(`${err}`, "err");
      }
      if (keepBrief) {
        setTimeout(() => {
          if (openSeq === menuOpenSeq) closeBuiltInContextMenu();
        }, 110);
      }
    });
  });

  // Ensure popover lives inside the topmost open modal <dialog> (if any) so it is never made inert by showModal(),
  // and reclaim back to document.body when that modal closes so dlg.remove() can never detach the menu permanently.
  const openDialogs = [...document.querySelectorAll("dialog[open]")];
  const hostContainer = openDialogs.length ? openDialogs[openDialogs.length - 1] : document.body;
  if (anchor.parentElement !== hostContainer || popover.parentElement !== hostContainer) {
    hostContainer.append(anchor, popover);
    if (hostContainer.tagName === "DIALOG") {
      hostContainer.addEventListener(
        "close",
        () => {
          closeBuiltInContextMenu();
          if (anchor.parentElement === hostContainer || !anchor.isConnected) {
            document.body.append(anchor, popover);
          }
        },
        { once: true }
      );
    }
  }

  // Place invisible 0x0 anchor at pointer or trigger button bottom-left
  let ax = x;
  let ay = y;
  if (triggerEl) {
    const r = triggerEl.getBoundingClientRect();
    ax = r.left;
    ay = r.bottom + 4;
  } else if (ax === 0 && ay === 0 && lastFocusedBeforeOpen) {
    const r = lastFocusedBeforeOpen.getBoundingClientRect();
    ax = Math.min(window.innerWidth - 24, Math.max(12, r.left + r.width / 2));
    ay = Math.min(window.innerHeight - 24, Math.max(12, r.top + r.height / 2));
  }

  anchor.style.left = `${Math.round(ax)}px`;
  anchor.style.top = `${Math.round(ay)}px`;

  if (SUPPORTS_POPOVER) {
    try {
      if (popover.matches(":popover-open")) popover.hidePopover();
    } catch {}
    try {
      popover.showPopover({ source: anchor });
    } catch {
      popover.showPopover();
    }
  } else {
    popover.classList.add("sf-ctx-open");
  }

  // Fallback viewport clamping when native CSS anchor positioning isn't active
  if (!SUPPORTS_ANCHOR) {
    const rect = popover.getBoundingClientRect();
    const mw = rect.width || 220;
    const mh = rect.height || 240;
    const left = ax + mw + 10 > window.innerWidth ? Math.max(8, ax - mw) : Math.max(8, ax);
    const top = ay + mh + 10 > window.innerHeight ? Math.max(8, ay - mh) : Math.max(8, ay);
    popover.style.left = `${Math.round(left)}px`;
    popover.style.top = `${Math.round(top)}px`;
  }

  if (fromKeyboard) {
    const first = group.querySelector("button:not(:disabled)");
    first?.focus({ preventScroll: true });
  }
}

function buildProjectItems(proj, app) {
  const isActive = app.activeProject?.id === proj.id;
  const items = [];
  items.push({ header: `项目 · ${proj.name}` });
  if (!isActive) {
    items.push({
      label: "切换到此项目",
      onSelect: () => app.switchProject(proj.id),
    });
  }
  items.push(
    {
      label: "打开项目目录",
      onSelect: () => API.openInExplorer(proj.root).catch((e) => toast(`${e}`, "err")),
    },
    {
      label: "项目配置…",
      onSelect: async () => {
        if (app.activeProject?.id !== proj.id) await app.switchProject(proj.id);
        app.openPreferences("project");
      },
    },
    {
      label: "重新识别项目",
      onSelect: async () => {
        try {
          await API.rescanProject(proj.id);
          await app.reloadConfig(true);
          toast(`已重新识别「${proj.name}」`, "ok");
        } catch (err) {
          toast(`重新识别失败：${err}`, "err", 4200);
        }
      },
    }
  );
  if (proj.launchExe) {
    items.push({
      label: "启动项目主程序",
      onSelect: () =>
        API.launchProjectExe(proj.id, null)
          .then((m) => toast(m, "ok"))
          .catch((e) => toast(`${e}`, "err")),
    });
  }
  items.push(
    { sep: true },
    {
      label: "从列表移出此项目…",
      danger: true,
      onSelect: () => app.removeProjectWithConfirm(proj),
    }
  );
  return items;
}

function buildContextItems(target, app) {
  const items = [];
  const proj = app.activeProject;
  const el = target instanceof Element ? target : null;

  // 1. Editable Input / Textarea / ContentEditable
  const editable = getEditableTarget(el);
  if (editable) {
    const sel = getSelectedTextIn(editable);
    const hasRange = supportsSelectionRange(editable);
    const savedStart = hasRange ? editable.selectionStart : null;
    const savedEnd = hasRange ? editable.selectionEnd : null;
    const canEdit = !editable.disabled && !editable.readOnly;
    items.push(
      { header: "文本编辑" },
      {
        label: "剪切",
        hint: "Ctrl+X",
        disabled: !sel || !canEdit,
        onSelect: async () => {
          await copyText(sel, "已剪切到剪贴板");
          replaceSelectionInInput(editable, "", savedStart, savedEnd);
        },
      },
      {
        label: "复制",
        hint: "Ctrl+C",
        disabled: !sel,
        onSelect: () => copyText(sel, "已复制选中文本"),
      },
      {
        label: "粘贴",
        hint: "Ctrl+V",
        disabled: !canEdit,
        onSelect: async () => {
          try {
            const text = await navigator.clipboard.readText();
            if (text) replaceSelectionInInput(editable, text, savedStart, savedEnd);
          } catch {
            toast("无法读取剪贴板，请使用 Ctrl+V 粘贴", "warn");
          }
        },
      },
      {
        label: "全选",
        hint: "Ctrl+A",
        onSelect: () => {
          editable.focus();
          if (typeof editable.select === "function") {
            editable.select();
          } else {
            const range = document.createRange();
            range.selectNodeContents(editable);
            const s = window.getSelection();
            s?.removeAllRanges();
            s?.addRange(range);
          }
        },
      }
    );
    if (canEdit && (editable.value || editable.textContent || "").length > 0) {
      items.push({
        label: "清空输入",
        onSelect: () => {
          if (editable.tagName === "INPUT" || editable.tagName === "TEXTAREA") {
            editable.value = "";
            editable.dispatchEvent(new Event("input", { bubbles: true }));
            editable.dispatchEvent(new Event("change", { bubbles: true }));
          } else {
            editable.textContent = "";
          }
          editable.focus();
        },
      });
    }
    items.push({ sep: true });
  } else {
    // 2. Non-input Selected Text (Terminal logs / JSON code-box / Changelog body)
    const selectedText = (window.getSelection?.()?.toString() || "").trim();
    if (selectedText) {
      items.push(
        {
          label: `复制选中内容 (${selectedText.length > 18 ? `${selectedText.slice(0, 18)}…` : selectedText})`,
          hint: "Ctrl+C",
          onSelect: () => copyText(selectedText, "已复制选中文本"),
        },
        { sep: true }
      );
    }
  }

  // 3. Specific Contextual Region Detection
  const projRow = el?.closest(".sb-proj-row[data-pid]");
  if (projRow) {
    const p = (app.config?.projects || []).find((x) => x.id === projRow.dataset.pid);
    if (p) {
      items.push(...buildProjectItems(p, app), { sep: true });
    }
  } else if (el?.closest("#termShell")) {
    items.push(
      { header: "执行终端" },
      {
        label: "复制全部日志",
        onSelect: () => {
          const text = (app.logs || []).map((l) => `[${l.ts}] ${l.line}`).join("\n");
          copyText(text || "(无日志)", "已复制全部终端日志");
        },
      },
      {
        label: "清空终端日志",
        onSelect: () => document.getElementById("btnClearLog")?.click(),
      },
      {
        label: "切换终端配色（跟随 / 深色高对比）",
        onSelect: () => app.toggleTermTone(),
      },
      {
        label: app.termMode === "floating" ? "停靠到底部" : "切换为自由浮窗",
        onSelect: () => app.setTermMode(app.termMode === "floating" ? "docked" : "floating"),
      },
      {
        label: app.termMode === "fullscreen" ? "退出全屏终端" : "全屏展开终端",
        onSelect: () => app.setTermMode(app.termMode === "fullscreen" ? "docked" : "fullscreen"),
      },
      {
        label: "隐藏执行终端",
        hint: "Ctrl+`",
        onSelect: () => app.setTermMode("hidden"),
      },
      { sep: true }
    );
  } else if (el?.closest("#relArtifactTableHost tbody tr")) {
    const tr = el.closest("tr");
    const shaBtn = tr?.querySelector("[data-sha]");
    const markOkBtn = tr?.querySelector("[data-mark-ok]");
    const markReadyBtn = tr?.querySelector("[data-mark-ready]");
    const openUrlBtn = tr?.querySelector("[data-open-url]");
    const archBtn = tr?.querySelector("[data-arch]");
    const fileName =
      markOkBtn?.dataset.markOk ||
      markReadyBtn?.dataset.markReady ||
      archBtn?.dataset.arch ||
      tr?.cells?.[0]?.textContent?.trim().split(" ")[0] ||
      "";
    items.push({ header: fileName ? `产物 · ${fileName}` : "产物操作" });
    if (fileName) {
      items.push({
        label: "复制文件名",
        onSelect: () => copyText(fileName, "已复制文件名"),
      });
    }
    if (shaBtn) {
      items.push({
        label: "计算并复制 SHA-256",
        onSelect: () => shaBtn.click(),
      });
    }
    if (markOkBtn) {
      items.push({
        label: "标记为「已验证」",
        onSelect: () => markOkBtn.click(),
      });
    }
    if (markReadyBtn) {
      items.push({
        label: "标为「可发布」",
        onSelect: () => markReadyBtn.click(),
      });
    }
    if (openUrlBtn) {
      items.push({
        label: "在浏览器查看线上",
        onSelect: () => openUrlBtn.click(),
      });
    }
    if (proj) {
      items.push({
        label: "打开产物目录",
        onSelect: () => API.openInExplorer(resolveReleaseDir(proj)).catch((e) => toast(`${e}`, "err")),
      });
    }
    if (archBtn) {
      items.push({
        label: "归档此产物",
        onSelect: () => archBtn.click(),
      });
    }
    items.push({ sep: true });
  } else if (el?.closest(".notes-item[data-rel-key]")) {
    const itemBtn = el.closest(".notes-item[data-rel-key]");
    const tagText = itemBtn.querySelector(".notes-item-title")?.textContent?.trim() || "";
    items.push(
      { header: tagText ? `线上版本 · ${tagText}` : "线上版本" },
      {
        label: "管理 / 编辑 / 撤回此版本",
        onSelect: () => itemBtn.click(),
      },
      {
        label: "复制 Tag 名称",
        disabled: !tagText,
        onSelect: () => copyText(tagText, `已复制 ${tagText}`),
      },
      {
        label: "新建 / 上传 Release",
        onSelect: () => document.getElementById("mpNewReleaseBtn")?.click(),
      },
      {
        label: "刷新线上版本列表",
        onSelect: () => document.getElementById("mpLoadListBtn")?.click(),
      },
      { sep: true }
    );
  } else if (el?.closest(".commit-row")) {
    const row = el.closest(".commit-row");
    const sha = row.querySelector(".commit-sha")?.textContent?.trim() || "";
    const subject = row.querySelector(".commit-subject")?.textContent?.trim() || "";
    const undoBtn = row.querySelector("#gitUndoCommitBtn");
    items.push(
      { header: sha ? `提交 · ${sha}` : "Git 提交" },
      {
        label: "复制 Commit SHA",
        disabled: !sha,
        onSelect: () => copyText(sha, `已复制 ${sha}`),
      },
      {
        label: "复制提交说明",
        disabled: !subject,
        onSelect: () => copyText(subject, "已复制提交说明"),
      }
    );
    if (undoBtn) {
      items.push({
        label: "安全撤回此未推送提交…",
        danger: true,
        onSelect: () => undoBtn.click(),
      });
    }
    items.push({ sep: true });
  } else if (el?.closest(".git-tag-chip")) {
    const chip = el.closest(".git-tag-chip");
    const tag = chip.querySelector(".git-tag-name")?.textContent?.trim() || "";
    const delBtn = chip.querySelector("[data-del-tag]");
    items.push(
      { header: tag ? `Git 标签 · ${tag}` : "Git 标签" },
      {
        label: "复制标签名",
        disabled: !tag,
        onSelect: () => copyText(tag, `已复制标签 ${tag}`),
      },
      {
        label: "删除此标签…",
        danger: true,
        disabled: !delBtn,
        onSelect: () => delBtn?.click(),
      },
      { sep: true }
    );
  } else if (el?.closest("table.dt tbody tr") && app.currentPage === "version" && app.subTabs.version === "git") {
    const tr = el.closest("tr");
    const filePath = tr?.cells?.[1]?.textContent?.trim() || "";
    items.push(
      { header: "工作区变更文件" },
      {
        label: "复制文件路径",
        disabled: !filePath,
        onSelect: () => copyText(filePath, "已复制路径"),
      },
      {
        label: "提交全部变更",
        onSelect: () => document.getElementById("gitCommitBtn")?.click(),
      },
      {
        label: "提交并推送到远端",
        onSelect: () => document.getElementById("gitCommitPushBtn")?.click(),
      },
      { sep: true }
    );
  } else if (el?.closest("#sidebar") && !el.closest(".sidebar-footer")) {
    items.push(
      { header: "项目列表" },
      {
        label: "导入本地项目…",
        onSelect: () => app.importProject(),
      },
      {
        label: document.getElementById("appShell")?.classList.contains("sidebar-collapsed")
          ? "展开左侧栏"
          : "折叠左侧栏",
        hint: "Ctrl+B",
        onSelect: () => app.toggleSidebar(),
      },
      { sep: true }
    );
  } else if (!editable && !el?.closest("dialog[open]") && !el?.closest("#sidebar")) {
    // 4. Current Page Contextual Quick Actions (when right-clicking general canvas area)
    const page = app.currentPage || "overview";
    if (page === "overview" && proj) {
      items.push(
        { header: `当前页 · 发版总览` },
        {
          label: "打开项目目录",
          onSelect: () => API.openInExplorer(proj.root).catch((e) => toast(`${e}`, "err")),
        },
        ...(proj.launchExe
          ? [
              {
                label: "启动项目主程序",
                onSelect: () =>
                  API.launchProjectExe(proj.id, null)
                    .then((m) => toast(m, "ok"))
                    .catch((e) => toast(`${e}`, "err")),
              },
            ]
          : []),
        {
          label: "打开 3 步配置向导…",
          onSelect: () => app.openOnboardingWizard(),
        },
        { sep: true }
      );
    } else if (page === "compile") {
      items.push(
        { header: "当前页 · 编译与环境" },
        { label: "切换到：框架模板", onSelect: () => app.navigate("compile", "presets") },
        { label: "切换到：环境自检", onSelect: () => app.navigate("compile", "prereq") },
        { label: "切换到：命令与工序", onSelect: () => app.navigate("compile", "actions") },
        ...(proj
          ? [
              {
                label: "重新识别项目环境",
                onSelect: async () => {
                  try {
                    await API.rescanProject(proj.id);
                    await app.reloadConfig(true);
                    toast("已重新识别项目", "ok");
                  } catch (err) {
                    toast(`重新识别失败：${err}`, "err", 4200);
                  }
                },
              },
            ]
          : []),
        { sep: true }
      );
    } else if (page === "package") {
      items.push(
        { header: "当前页 · 分析与分包" },
        { label: "切换到：体积与敏感分析", onSelect: () => app.navigate("package", "analyze") },
        { label: "切换到：分包与分卷工坊", onSelect: () => app.navigate("package", "pack") },
        ...(proj
          ? [
              {
                label: "打开产物目录",
                onSelect: () => API.openInExplorer(resolveReleaseDir(proj)).catch((e) => toast(`${e}`, "err")),
              },
            ]
          : []),
        { sep: true }
      );
    } else if (page === "release") {
      items.push(
        { header: "当前页 · 产物与仓库" },
        { label: "产物分级与清单", onSelect: () => app.navigate("release", "artifacts") },
        { label: "多平台发布与撤回", onSelect: () => app.navigate("release", "repos") },
        { label: "本机测试更新源", onSelect: () => app.navigate("release", "local") },
        ...(proj
          ? [
              {
                label: "打开产物目录",
                onSelect: () => API.openInExplorer(resolveReleaseDir(proj)).catch((e) => toast(`${e}`, "err")),
              },
            ]
          : []),
        { sep: true }
      );
    } else if (page === "version") {
      items.push(
        { header: "当前页 · 版本与源码" },
        { label: "变更日志 (CHANGELOG)", onSelect: () => app.navigate("version", "changelog") },
        { label: "版本同步与隔离", onSelect: () => app.navigate("version", "sync") },
        { label: "Git 源码与标签", onSelect: () => app.navigate("version", "git") },
        { label: "构建台账", onSelect: () => app.navigate("version", "journal") },
        { label: "打开 ShipForge 数据目录", onSelect: () => app.openDataDir() },
        { sep: true }
      );
    }
    // Blank-canvas quick actions only (page-relevant); theme / sidebar / preferences / manual stay in
    // their own toolbar buttons, shortcuts and the Spotlight palette — never dumped into every menu.
    items.push(
      {
        label: "变动扫描 / 刷新状态",
        hint: "F5",
        disabled: !proj,
        onSelect: () => app.probeProjectChanges(true),
      },
      {
        label: "Spotlight 命令面板…",
        hint: "Ctrl+K",
        onSelect: () => app.openCommandPalette(),
      }
    );
  }

  // Region-scoped menu: ONLY the items that belong to the clicked area are shown.
  return items;
}

export function openProjectRowMenu(proj, triggerBtn, app) {
  const { popover } = getElements();
  const isOpen =
    activeAnchorTrigger === triggerBtn &&
    (SUPPORTS_POPOVER ? popover?.matches(":popover-open") : popover?.classList.contains("sf-ctx-open"));
  if (isOpen) {
    closeBuiltInContextMenu();
    return;
  }
  const items = buildProjectItems(proj, app);
  showMenuItems(items, {
    triggerEl: triggerBtn,
    label: `${proj.name} 项目操作`,
    fromKeyboard: false,
  });
}

export function initBuiltInContextMenu(app) {
  currentApp = app;
  if (initialized) return;
  initialized = true;

  const { popover, group } = getElements();
  if (!popover || !group) return;

  // Fallback keyboard navigation when native HTML focusgroup is not yet supported
  if (!SUPPORTS_NATIVE_FOCUSGROUP) {
    group.addEventListener(
      "keydown",
      (event) => {
        if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
        const buttons = [...group.querySelectorAll("button:not(:disabled)")];
        if (!buttons.length) return;
        const cur = buttons.indexOf(document.activeElement);
        let next = 0;
        if (event.key === "Home") next = 0;
        else if (event.key === "End") next = buttons.length - 1;
        else if (cur < 0) next = event.key === "ArrowUp" ? buttons.length - 1 : 0;
        else next = (cur + (event.key === "ArrowUp" ? -1 : 1) + buttons.length) % buttons.length;
        event.preventDefault();
        event.stopImmediatePropagation();
        buttons[next].focus({ preventScroll: true });
      },
      true
    );
  }

  popover.addEventListener("keydown", (event) => {
    if (event.key === "Tab") {
      closeBuiltInContextMenu();
    } else if (event.key === "Escape") {
      event.stopPropagation();
      closeBuiltInContextMenu();
    }
  });

  if (SUPPORTS_POPOVER) {
    popover.addEventListener("toggle", (event) => {
      if (event.newState === "closed") {
        if (activeAnchorTrigger) {
          activeAnchorTrigger.setAttribute("aria-expanded", "false");
          activeAnchorTrigger = null;
        }
        if (lastFocusedBeforeOpen && lastFocusedBeforeOpen.isConnected) {
          lastFocusedBeforeOpen.focus({ preventScroll: true });
        }
        lastFocusedBeforeOpen = null;
      }
    });
  } else {
    document.addEventListener("pointerdown", (e) => {
      if (popover.classList.contains("sf-ctx-open") && !popover.contains(e.target)) {
        closeBuiltInContextMenu();
      }
    });
  }

  // Intercept OS right-click menu globally across all views and modals
  document.addEventListener(
    "contextmenu",
    (e) => {
      e.preventDefault();
      if (!currentApp) return;
      const fromKeyboard = e.clientX === 0 && e.clientY === 0;
      const items = buildContextItems(e.target, currentApp);
      showMenuItems(items, {
        x: e.clientX,
        y: e.clientY,
        label: "快捷操作菜单",
        fromKeyboard,
      });
    },
    true
  );

  // Dismiss cleanly on window blur or resize
  window.addEventListener("blur", closeBuiltInContextMenu);
  window.addEventListener("resize", closeBuiltInContextMenu);
}
