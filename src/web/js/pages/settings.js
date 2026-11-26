import { API, esc, formatBytes } from "../api.js";
import { toast, withBusy } from "../ui.js";

/**
 * Guardrails & Performance Headroom pane (rendered cleanly inside Preferences -> 安全缓冲).
 * No longer hides manage.js page heads or wraps duplicate .page-wrap elements.
 */
export function renderGuardrailsPane(container, app, project, isAlive = () => true) {
  const g = app.config.guardrails || {
    minFreeDiskMb: 2048,
    diskHeadroomMultiplier: 3,
    maxConcurrency: 0,
    blockSensitiveFiles: true,
    autoRollbackOnAbort: true,
    splitWarningMb: 95,
  };

  container.innerHTML = `
    <div>
      <div class="sf-section-title">
        <span>实时硬件与安全缓冲状态</span>
        <button class="btn btn-xs btn-ghost" id="grRefreshBtn" type="button">刷新检测</button>
      </div>
      <div id="grHeadroomHost"><div class="sf-group"><div class="sf-row"><span class="dim">正在检测系统资源余量…</span></div></div></div>
    </div>

    <div>
      <div class="sf-section-title">
        <span>安全缓冲与事务保护参数</span>
        <button class="btn btn-xs btn-primary" id="grSaveBtn" type="button">保存参数</button>
      </div>
      <div class="sf-group">
        <div class="sf-row">
          <div class="sf-row-main">
            <div class="sf-row-title">最小磁盘剩余缓冲 (MB)</div>
            <div class="sf-row-desc">编译、打包或补全依赖前核验目标盘可用空间下限</div>
          </div>
          <div class="sf-row-ctrl">
            <input class="input mono" id="grMinDisk" type="number" min="256" step="256" style="width:116px" value="${g.minFreeDiskMb ?? 2048}">
          </div>
        </div>

        <div class="sf-row">
          <div class="sf-row-main">
            <div class="sf-row-title">预估产物体积安全倍率</div>
            <div class="sf-row-desc">按预计产物大小的倍数预留临时压缩与校验空间</div>
          </div>
          <div class="sf-row-ctrl">
            <input class="input mono" id="grDiskMult" type="number" min="1" max="10" style="width:116px" value="${g.diskHeadroomMultiplier ?? 3}">
          </div>
        </div>

        <div class="sf-row">
          <div class="sf-row-main">
            <div class="sf-row-title">并发线程上限</div>
            <div class="sf-row-desc">填 0 表示自动保留系统余量（防高负载卡顿）</div>
          </div>
          <div class="sf-row-ctrl">
            <input class="input mono" id="grMaxConc" type="number" min="0" max="32" style="width:116px" value="${g.maxConcurrency ?? 0}">
          </div>
        </div>

        <div class="sf-row">
          <div class="sf-row-main">
            <div class="sf-row-title">大文件分卷提示阈值 (MB)</div>
            <div class="sf-row-desc">单包超过该阈值时提示切分仓库兼容分卷（例如 Gitee 95 MB）</div>
          </div>
          <div class="sf-row-ctrl">
            <input class="input mono" id="grSplitWarn" type="number" min="20" max="2000" style="width:116px" value="${g.splitWarningMb ?? 95}">
          </div>
        </div>

        <div class="sf-row">
          <div class="sf-row-main">
            <div class="sf-row-title">中止或失败时自动事务安全回滚</div>
            <div class="sf-row-desc">自动还原版本号、变更日志、本地新建提交/标签并清理半成品与锁</div>
          </div>
          <div class="sf-row-ctrl">
            <input class="sf-switch" type="checkbox" switch id="grAutoRollback" ${g.autoRollbackOnAbort !== false ? "checked" : ""}>
          </div>
        </div>

        <div class="sf-row">
          <div class="sf-row-main">
            <div class="sf-row-title">打包默认拦截敏感文件</div>
            <div class="sf-row-desc">自动阻止 .env、私钥、证书等敏感文件打入公开压缩包</div>
          </div>
          <div class="sf-row-ctrl">
            <input class="sf-switch" type="checkbox" switch id="grBlockSens" ${g.blockSensitiveFiles !== false ? "checked" : ""}>
          </div>
        </div>
      </div>
    </div>
  `;

  const loadHeadroom = async () => {
    const host = container.querySelector("#grHeadroomHost");
    if (!host) return;
    try {
      const hr = await API.getSystemHeadroom(project?.id || null);
      if (!isAlive()) return;
      host.innerHTML = `
        <div class="status-ribbon">
          <span class="ribbon-item">
            <span class="dot ${hr.diskOk ? "dot-ok" : "dot-err"}"></span>
            目标盘 ${esc(hr.targetDrive)} 可用 <strong class="mono">${formatBytes(hr.diskFreeBytes)}</strong>
            <span class="dim">（下限 ${formatBytes(hr.diskRequiredBytes)}）</span>
          </span>
          <span class="ribbon-item">
            <span class="dot ${hr.memOk ? "dot-ok" : "dot-warn"}"></span>
            可用内存 <strong class="mono">${formatBytes(hr.memAvailBytes)}</strong> / ${formatBytes(hr.memTotalBytes)}
          </span>
          <span class="ribbon-item">
            安全并发 <strong class="mono">${hr.safeConcurrency} / ${hr.cpuCores} 核</strong>
          </span>
          <span class="ribbon-item">
            <span class="tag ${hr.cacheOnCDrive ? "tag-warn" : "tag-ok"}">${hr.cacheOnCDrive ? "缓存位于 C 盘" : "缓存位于非系统盘"}</span>
          </span>
        </div>
      `;
    } catch (err) {
      host.innerHTML = `<div class="notice notice-err">${esc(err)}</div>`;
    }
  };
  loadHeadroom();

  container.querySelector("#grRefreshBtn")?.addEventListener("click", loadHeadroom);
  container.querySelector("#grSaveBtn")?.addEventListener("click", (e) =>
    withBusy(e.currentTarget, "保存中…", async () => {
      try {
        const rawMinDisk = Number(container.querySelector("#grMinDisk").value);
        const rawMult = Number(container.querySelector("#grDiskMult").value);
        const rawConc = Number(container.querySelector("#grMaxConc").value);
        const rawSplit = Number(container.querySelector("#grSplitWarn").value);
        const next = {
          minFreeDiskMb: Math.max(256, Math.trunc(Number.isFinite(rawMinDisk) ? rawMinDisk : 2048)),
          diskHeadroomMultiplier: Math.min(10, Math.max(1, Math.trunc(Number.isFinite(rawMult) ? rawMult : 3))),
          maxConcurrency: Math.min(32, Math.max(0, Math.trunc(Number.isFinite(rawConc) ? rawConc : 0))),
          splitWarningMb: Math.min(2000, Math.max(20, Math.trunc(Number.isFinite(rawSplit) ? rawSplit : 95))),
          autoRollbackOnAbort: container.querySelector("#grAutoRollback").checked,
          blockSensitiveFiles: container.querySelector("#grBlockSens").checked,
        };
        app.config = await API.updateGuardrailSettings(next);
        toast("已保存安全缓冲参数", "ok");
        loadHeadroom();
      } catch (err) {
        toast(`${err}`, "err");
      }
    })
  );
}

export function renderSettings(root, app) {
  app.openPreferences("general");
}
