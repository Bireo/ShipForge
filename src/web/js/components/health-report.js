import { API, esc, formatBytes, formatDuration } from "../api.js";
import { toast } from "../ui.js";

let activeDialog = null;

/**
 * Show an ephemeral, one-time Integrity & Health Analysis Report.
 * Strictly memory-only: NEVER saved to disk or localStorage; destroyed immediately upon closing.
 */
export function showOneTimeHealthReport(report, app) {
  if (!report) return;
  if (activeDialog) {
    try {
      activeDialog.close("");
      activeDialog.remove();
    } catch {}
    activeDialog = null;
  }

  const dlg = document.createElement("dialog");
  dlg.className = "hr-dialog";
  dlg.setAttribute("aria-label", "完整性与健康分析报告");

  const statusTag =
    report.outcome === "ok"
      ? `<span class="tag tag-ok">校验通过</span>`
      : report.outcome === "rolled_back"
        ? `<span class="tag tag-warn">已安全回滚</span>`
        : `<span class="tag tag-err">存在异常</span>`;

  const renderCheckList = (items = []) =>
    items
      .map(
        (it) => `<div class="hr-row">
          <div>
            <span class="dot ${it.status === "ok" ? "dot-ok" : it.status === "warn" ? "dot-warn" : "dot-err"}" style="margin-right:6px"></span>
            <strong>${esc(it.label)}</strong>
          </div>
          <span class="muted" style="text-align:right">${esc(it.detail)}</span>
        </div>`
      )
      .join("");

  const rollbackHtml = report.rollbackProof
    ? `<div class="hr-block">
        <div class="hr-block-title">事务回滚复核（${esc(report.rollbackProof.triggeredBy)} · ${formatDuration(report.rollbackProof.durationMs)}）</div>
        ${(report.rollbackProof.checks || [])
          .map(
            (c) => `<div class="hr-row">
              <div><span class="dot ${c.restored ? "dot-ok" : "dot-err"}" style="margin-right:6px"></span>${esc(c.item)}</div>
              <span class="muted">${esc(c.detail)}</span>
            </div>`
          )
          .join("")}
      </div>`
    : "";

  const hr = report.headroom || {};
  const diskGb = hr.diskFreeBytes ? formatBytes(hr.diskFreeBytes) : "-";
  const reqGb = hr.diskRequiredBytes ? formatBytes(hr.diskRequiredBytes) : "-";
  const memGb = hr.memAvailBytes ? formatBytes(hr.memAvailBytes) : "-";

  const advisoriesHtml = (report.advisories || []).length
    ? `<div class="hr-block">
        <div class="hr-block-title">性能与稳定性建议</div>
        ${report.advisories
          .map(
            (a) => `<div class="hr-row">
              <div><span class="dot ${a.level === "warn" ? "dot-warn" : "dot-ok"}" style="margin-right:6px"></span><strong>${esc(a.title)}</strong></div>
              <span class="muted" style="text-align:right;max-width:60%">${esc(a.detail)}</span>
            </div>`
          )
          .join("")}
      </div>`
    : "";

  const nextBtns = (report.nextSteps || [])
    .map(
      (s) =>
        `<button class="btn btn-xs" data-hr-next="${esc(s.id)}" data-hr-page="${esc(s.targetPage)}" title="${esc(s.hint)}" type="button">${esc(s.label)}</button>`
    )
    .join("");

  const reportProjectId = report.projectId || "";
  const reportProjectName = report.projectName || reportProjectId;

  dlg.innerHTML = `
    <div class="hr-head">
      <div class="field-row" style="gap:8px">
        <strong style="font-size:14.5px">完整性与健康分析报告</strong>
        ${reportProjectName ? `<span class="tag">${esc(reportProjectName)}</span>` : ""}
        ${statusTag}
        <span class="mono muted" style="font-size:11.5px">v${esc(report.version)} · ${formatDuration(report.durationMs)}</span>
      </div>
      <span class="dim" style="font-size:12px">仅本次展示，关闭即销毁</span>
    </div>
    <div class="hr-body scroller">
      <div style="font-size:12.5px;color:var(--text-1)">${esc(report.summary)}</div>
      ${rollbackHtml}
      <div class="hr-block">
        <div class="hr-block-title">产物完整性检查</div>
        ${renderCheckList(report.integrityChecks)}
      </div>
      <div class="hr-block">
        <div class="hr-block-title">版本、仓库与锁一致性</div>
        ${renderCheckList(report.consistencyChecks)}
      </div>
      <div class="hr-block">
        <div class="hr-block-title">系统安全缓冲余量</div>
        <div class="hr-row">
          <span>目标磁盘 (${esc(hr.targetDrive || "-")})</span>
          <span class="mono ${hr.diskOk ? "" : "err"}">可用 ${diskGb}（安全下限 ${reqGb}）</span>
        </div>
        <div class="hr-row">
          <span>物理内存与并发水位</span>
          <span class="mono">可用内存 ${memGb} · 并发上限 ${hr.safeConcurrency || "-"} / ${hr.cpuCores || "-"} 核</span>
        </div>
      </div>
      ${advisoriesHtml}
    </div>
    <div class="hr-foot">
      <div class="field-row" style="gap:6px">${nextBtns}</div>
      <button class="btn btn-accent" id="hrDismissBtn" type="button" autofocus>已阅并销毁报告</button>
    </div>
  `;

  const destroy = () => {
    try {
      if (dlg.open) dlg.close("");
    } catch {}
    dlg.innerHTML = "";
    dlg.remove();
    if (activeDialog === dlg) activeDialog = null;
    report = null;
  };

  dlg.addEventListener("cancel", (e) => {
    e.preventDefault();
    destroy();
  });
  dlg.querySelector("#hrDismissBtn")?.addEventListener("click", destroy);
  dlg.querySelectorAll("[data-hr-next]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const targetPage = btn.dataset.hrPage;
      const stepId = btn.dataset.hrNext;
      destroy();
      if (reportProjectId && app.activeProject?.id !== reportProjectId) {
        await app.switchProject(reportProjectId);
      }
      if (stepId === "smoke-launch") {
        app.navigate("overview");
        const launchPid = reportProjectId || app.activeProject?.id;
        if (launchPid) {
          try {
            toast(await API.launchProjectExe(launchPid), "ok");
          } catch (err) {
            toast(`${err}`, "err");
          }
        }
      } else if (targetPage) {
        app.navigate(targetPage);
      }
    });
  });

  document.body.appendChild(dlg);
  activeDialog = dlg;
  dlg.showModal();
}
