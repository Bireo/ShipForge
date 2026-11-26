import { esc } from "./api.js";

const ICONS = {
  rocket: `<svg viewBox="0 0 20 20" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M11.5 3.5c3.5-.5 5 1 4.5 4.5-.6 3.5-3.4 6.3-6.9 6.9l-3.6-3.6c.6-3.5 3.4-6.3 6-7.8z"/><circle cx="12.5" cy="7.5" r="1.2"/><path d="M5.5 11.3L3.5 13.3l.5 2.7 2.7.5 2-2"/></svg>`,
  hammer: `<svg viewBox="0 0 20 20" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M11.7 4.3l4 4-2 2-4-4 2-2z"/><path d="M10.7 7.3L3.8 14.2a1.4 1.4 0 1 0 2 2l6.9-6.9"/></svg>`,
  package: `<svg viewBox="0 0 20 20" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M10 2.8L16.5 6.4v7.2L10 17.2 3.5 13.6V6.4L10 2.8z"/><path d="M3.5 6.4L10 10l6.5-3.6M10 10v7.2"/></svg>`,
  cloud: `<svg viewBox="0 0 20 20" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6.5 15.5h8a3.2 3.2 0 0 0 .6-6.3 4.8 4.8 0 0 0-9.2-1.2A3.3 3.3 0 0 0 6.5 15.5z"/><path d="M10 12.5V8.3M8.2 10l1.8-1.8 1.8 1.8"/></svg>`,
  tag: `<svg viewBox="0 0 20 20" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 10.2V4.5a1 1 0 0 1 1-1h5.7l6.3 6.3a1.4 1.4 0 0 1 0 2l-4.7 4.7a1.4 1.4 0 0 1-2 0l-6.3-6.3z"/><circle cx="7.2" cy="7.2" r="1"/></svg>`,
  sliders: `<svg viewBox="0 0 20 20" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M3.5 6h13M3.5 10h13M3.5 14h13"/><circle cx="7" cy="6" r="1.6" fill="var(--sidebar-bg)"/><circle cx="13" cy="10" r="1.6" fill="var(--sidebar-bg)"/><circle cx="8.5" cy="14" r="1.6" fill="var(--sidebar-bg)"/></svg>`,
  folder: `<svg viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M2.8 5.5a1.5 1.5 0 0 1 1.5-1.5h3.5l1.6 1.8h6.3a1.5 1.5 0 0 1 1.5 1.5v7.2a1.5 1.5 0 0 1-1.5 1.5H4.3a1.5 1.5 0 0 1-1.5-1.5V5.5z"/></svg>`,
  key: `<svg viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="7" cy="10" r="3.5"/><path d="M10.5 10H17M14.5 10v2.5M17 10v2"/></svg>`,
  shield: `<svg viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M10 3l6 2.5v4.8c0 3.4-2.5 5.8-6 6.7-3.5-.9-6-3.3-6-6.7V5.5L10 3z"/></svg>`,
  wrench: `<svg viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M13.5 3.5a3.8 3.8 0 0 0-4.9 4.9L4 13a1.6 1.6 0 1 0 2.3 2.3l4.6-4.6a3.8 3.8 0 0 0 4.9-4.9l-2 2-2.3-2.3 2-2z"/></svg>`,
  drive: `<svg viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="14" height="12" rx="2.2"/><path d="M3 12h14"/><circle cx="14" cy="14" r=".7" fill="currentColor" stroke="none"/></svg>`,
  palette: `<svg viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="10" cy="10" r="7"/><circle cx="7.5" cy="8.2" r=".8" fill="currentColor"/><circle cx="10.5" cy="6.8" r=".8" fill="currentColor"/><circle cx="13" cy="9" r=".8" fill="currentColor"/><path d="M10.5 13.2a1.7 1.7 0 0 1 1.7-1.7h2.3"/></svg>`,
  download: `<svg viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M10 3.5v8.8M6.6 9.3L10 12.7l3.4-3.4"/><path d="M3.5 14.5v1.3a1.2 1.2 0 0 0 1.2 1.2h10.6a1.2 1.2 0 0 0 1.2-1.2v-1.3"/></svg>`,
};

export function hydrateIcons(root = document) {
  root.querySelectorAll("[data-ico]").forEach((el) => {
    const key = el.getAttribute("data-ico");
    if (ICONS[key]) el.innerHTML = ICONS[key];
  });
}

export function toast(message, kind = "ok", ms = 3000) {
  const stack = document.getElementById("toastStack");
  if (!stack) return;
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = String(message);
  stack.appendChild(el);
  setTimeout(() => {
    el.style.opacity = "0";
    setTimeout(() => el.remove(), 180);
  }, ms);
}

export async function copyText(text, label = "已复制") {
  const s = String(text);
  try {
    await navigator.clipboard.writeText(s);
    toast(label, "ok", 1600);
    return;
  } catch {}
  try {
    const ta = document.createElement("textarea");
    ta.value = s;
    ta.setAttribute("readonly", "");
    ta.style.cssText = "position:fixed;top:-9999px;left:-9999px;opacity:0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand?.("copy");
    ta.remove();
    if (ok) {
      toast(label, "ok", 1600);
      return;
    }
  } catch {}
  toast("复制失败，请手动选中复制", "warn");
}

/** Prevents double-click duplicate submissions on any async button action */
export async function withBusy(btn, busyText, fn) {
  if (!btn || btn.disabled) return;
  const orig = btn.textContent;
  btn.disabled = true;
  btn.dataset.busyLock = "1";
  if (busyText) btn.textContent = busyText;
  try {
    return await fn();
  } finally {
    if (btn.isConnected) {
      delete btn.dataset.busyLock;
      btn.disabled = false;
      btn.textContent = orig;
    }
  }
}

/** Light-dismiss pointerdown+pointerup fallback for <dialog closedby="any"> when browser lacks closedBy */
export function attachLightDismissFallback(dlg) {
  if (!dlg || "closedBy" in HTMLDialogElement.prototype) return;
  const outside = (e) => {
    const r = dlg.getBoundingClientRect();
    return e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom;
  };
  let pressedOutside = false;
  dlg.addEventListener("pointerdown", (e) => {
    pressedOutside = outside(e);
  });
  dlg.addEventListener("pointerup", (e) => {
    if (pressedOutside && outside(e)) dlg.close("");
    pressedOutside = false;
  });
}

// Native <dialog closedby="any"> manager (re-entrancy safe: gracefully settles any open modal before reopening)
let dialogInitialized = false;
let activeResolver = null;

function ensureDialogSetup() {
  const dlg = document.getElementById("modalDialog");
  if (!dlg || dialogInitialized) return dlg;
  dialogInitialized = true;

  document.getElementById("modalCloseBtn")?.addEventListener("click", () => dlg.close(""));
  attachLightDismissFallback(dlg);

  dlg.addEventListener("close", () => {
    const val = dlg.returnValue;
    dlg.returnValue = "";
    if (activeResolver) {
      const resolve = activeResolver;
      activeResolver = null;
      resolve(val === "confirm");
    }
  });

  return dlg;
}

/**
 * Safely dismiss any currently open `#modalDialog` and settle its pending Promise
 * so re-entrant calls NEVER throw `InvalidStateError` or hang callers forever.
 */
function prepareModalForReopen(dlg) {
  if (activeResolver) {
    const prev = activeResolver;
    activeResolver = null;
    try {
      prev(false);
    } catch {}
  }
  if (dlg.open) {
    try {
      dlg.close("");
    } catch {}
  }
  dlg.returnValue = "";
}

export function confirmDialog({ title, message, detail = "", confirmText = "确认执行", danger = false }) {
  const dlg = ensureDialogSetup();
  if (!dlg) return Promise.resolve(false);
  prepareModalForReopen(dlg);
  document.getElementById("modalTitle").textContent = title;
  document.getElementById("modalBody").innerHTML = `<p style="margin:0;line-height:1.55">${esc(message)}</p>${
    detail ? `<div class="code-box">${esc(detail)}</div>` : ""
  }`;
  document.getElementById("modalFoot").innerHTML = `
    <button class="btn" type="button" id="mCancelBtn" ${danger ? "autofocus" : ""}>取消</button>
    <button class="btn ${danger ? "btn-danger" : "btn-primary"}" type="button" id="mConfirmBtn" ${danger ? "" : "autofocus"}>${esc(confirmText)}</button>
  `;
  document.getElementById("mCancelBtn").onclick = () => dlg.close("");
  document.getElementById("mConfirmBtn").onclick = () => dlg.close("confirm");

  return new Promise((resolve) => {
    let settled = false;
    activeResolver = (ok) => {
      if (settled) return;
      settled = true;
      resolve(Boolean(ok));
    };
    dlg.showModal();
  });
}

/**
 * Flexible macOS Sequoia modal dialog supporting grouped toggle-switch options and multiple action buttons.
 * Returns `{ action: string | null, options: Record<string, boolean> }`.
 */
export function choiceDialog({
  title,
  message,
  detail = "",
  notice = "",
  noticeKind = "warn",
  options = [],
  buttons = [],
  danger = false,
}) {
  const dlg = ensureDialogSetup();
  if (!dlg) return Promise.resolve({ action: null, options: {} });
  prepareModalForReopen(dlg);
  document.getElementById("modalTitle").textContent = title;

  const optsHtml = options.length
    ? `<div class="sf-group" style="margin-top:2px">${options
        .map(
          (o, idx) => `<div class="sf-row">
            <div class="sf-row-meta">
              <div class="sf-row-title">${esc(o.label)}</div>
              ${o.desc ? `<div class="sf-row-desc">${esc(o.desc)}</div>` : ""}
            </div>
            <div class="sf-row-ctrl">
              <input class="sf-switch" type="checkbox" switch role="switch" id="mChoiceOpt_${idx}" data-opt-key="${esc(o.key)}" ${o.checked ? "checked" : ""} ${o.disabled ? "disabled" : ""} aria-label="${esc(o.label)}">
            </div>
          </div>`
        )
        .join("")}</div>`
    : "";

  const noticeHtml = notice
    ? `<div class="notice notice-${esc(noticeKind)}" style="font-size:12px">${esc(notice)}</div>`
    : "";

  document.getElementById("modalBody").innerHTML = `
    <p style="margin:0;line-height:1.55">${esc(message)}</p>
    ${noticeHtml}
    ${detail ? `<div class="code-box">${esc(detail)}</div>` : ""}
    ${optsHtml}
  `;

  const resolvedButtons = buttons.length
    ? buttons
    : [
        { id: "cancel", label: "取消", cls: "", autofocus: danger },
        {
          id: "confirm",
          label: "确认执行",
          cls: danger ? "btn-danger" : "btn-primary",
          autofocus: !danger,
        },
      ];

  const foot = document.getElementById("modalFoot");
  foot.innerHTML = resolvedButtons
    .map(
      (b, idx) =>
        `<button class="btn ${esc(b.cls || "")}" type="button" id="mChoiceBtn_${idx}" ${b.autofocus ? "autofocus" : ""}>${esc(b.label)}</button>`
    )
    .join("");

  const readOptions = () => {
    const out = {};
    dlg.querySelectorAll("[data-opt-key]").forEach((el) => {
      out[el.dataset.optKey] = !!el.checked;
    });
    return out;
  };

  return new Promise((resolve) => {
    let settled = false;
    const finish = (action) => {
      if (settled) return;
      settled = true;
      const opts = readOptions();
      activeResolver = null;
      if (dlg.open) dlg.close("");
      resolve({ action, options: opts });
    };
    resolvedButtons.forEach((b, idx) => {
      const el = document.getElementById(`mChoiceBtn_${idx}`);
      if (el) el.onclick = () => finish(b.id === "cancel" ? null : b.id);
    });
    activeResolver = () => {
      if (!settled) {
        settled = true;
        resolve({ action: null, options: readOptions() });
      }
    };
    dlg.showModal();
  });
}

export function showContentDialog({ title, htmlBody, closeText = "关闭", actions = [] }) {
  const dlg = ensureDialogSetup();
  if (!dlg) return;
  prepareModalForReopen(dlg);
  document.getElementById("modalTitle").textContent = title;
  document.getElementById("modalBody").innerHTML = htmlBody;
  const foot = document.getElementById("modalFoot");
  const closeCls = actions.length ? "btn" : "btn btn-primary";
  foot.innerHTML =
    actions
      .map(
        (a, idx) =>
          `<button class="btn ${esc(a.cls || "")}" type="button" id="mContentAct_${idx}">${esc(a.label)}</button>`
      )
      .join("") +
    `<button class="${closeCls}" type="button" id="mOkClose" ${actions.length ? "" : "autofocus"}>${esc(closeText)}</button>`;
  actions.forEach((a, idx) => {
    const btn = document.getElementById(`mContentAct_${idx}`);
    if (btn && typeof a.onClick === "function") {
      btn.onclick = () => a.onClick(dlg, btn);
    }
  });
  document.getElementById("mOkClose").onclick = () => dlg.close("");
  dlg.showModal();
}

export function promptDialog({ title, label, placeholder = "", initialValue = "", confirmText = "保存" }) {
  const dlg = ensureDialogSetup();
  if (!dlg) return Promise.resolve(null);
  prepareModalForReopen(dlg);
  document.getElementById("modalTitle").textContent = title;
  document.getElementById("modalBody").innerHTML = `
    <div class="field">
      <label for="mPromptInput">${esc(label)}</label>
      <input class="input" id="mPromptInput" value="${esc(initialValue)}" placeholder="${esc(placeholder)}" autofocus>
    </div>
  `;
  document.getElementById("modalFoot").innerHTML = `
    <button class="btn" type="button" id="mPromptCancel">取消</button>
    <button class="btn btn-primary" type="button" id="mPromptOk">${esc(confirmText)}</button>
  `;

  return new Promise((resolve) => {
    let settled = false;
    const inp = document.getElementById("mPromptInput");
    const done = (val) => {
      if (settled) return;
      settled = true;
      activeResolver = null;
      if (dlg.open) dlg.close("");
      resolve(val);
    };
    document.getElementById("mPromptCancel").onclick = () => done(null);
    document.getElementById("mPromptOk").onclick = () => done(inp.value.trim());
    inp.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        done(inp.value.trim());
      }
    });
    activeResolver = () => {
      if (settled) return;
      settled = true;
      resolve(null);
    };
    dlg.showModal();
    inp.select();
  });
}

// ============================================================
// Theme (System / Light / Dark) & Accent (Notes Amber / macOS Blue)
// ============================================================
const THEME_KEY = "color-scheme";
const ACCENT_KEY = "sf-accent";
const darkMQ = window.matchMedia("(prefers-color-scheme: dark)");

export function getStoredTheme() {
  try {
    const v = localStorage.getItem(THEME_KEY);
    return v === "light" || v === "dark" ? v : null;
  } catch {
    return null;
  }
}

export function getSystemScheme() {
  return darkMQ.matches ? "dark" : "light";
}

export function applyThemeMeta(stored) {
  const meta = document.querySelector('meta[name="color-scheme"]');
  if (meta) meta.content = stored === "light" || stored === "dark" ? stored : "light dark";
}

export function setThemePreference(mode) {
  try {
    if (mode === "light" || mode === "dark") localStorage.setItem(THEME_KEY, mode);
    else localStorage.removeItem(THEME_KEY);
  } catch {}
  applyThemeMeta(mode === "light" || mode === "dark" ? mode : null);
  window.dispatchEvent(new CustomEvent("sf-theme-changed"));
}

export function toggleHeaderTheme() {
  const stored = getStoredTheme();
  const system = getSystemScheme();
  const rendered = stored || system;
  const target = rendered === "dark" ? "light" : "dark";
  if (target === system) setThemePreference("system");
  else setThemePreference(target);
}

export function getAccentPreference() {
  try {
    return localStorage.getItem(ACCENT_KEY) === "blue" ? "blue" : "amber";
  } catch {
    return "amber";
  }
}

export function applyAccentAttr(accent) {
  const val = accent === "blue" ? "blue" : "amber";
  if (val === "blue") document.documentElement.dataset.accent = "blue";
  else delete document.documentElement.dataset.accent;
}

export function setAccentPreference(accent) {
  const val = accent === "blue" ? "blue" : "amber";
  try {
    if (val === "blue") localStorage.setItem(ACCENT_KEY, "blue");
    else localStorage.removeItem(ACCENT_KEY);
  } catch {}
  applyAccentAttr(val);
  window.dispatchEvent(new CustomEvent("sf-accent-changed", { detail: val }));
}

window.addEventListener("storage", (e) => {
  if (e.key === THEME_KEY) {
    applyThemeMeta(getStoredTheme());
    window.dispatchEvent(new CustomEvent("sf-theme-changed"));
  } else if (e.key === ACCENT_KEY) {
    applyAccentAttr(getAccentPreference());
    window.dispatchEvent(new CustomEvent("sf-accent-changed", { detail: getAccentPreference() }));
  }
});

darkMQ.addEventListener("change", () => {
  window.dispatchEvent(new CustomEvent("sf-theme-changed"));
});
