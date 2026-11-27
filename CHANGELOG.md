# ShipForge 变更记录

## 1.5.0 (2026-11-27) —— 全自动静默更新小窗、线上日志与 latest.json 联动同步、仓库切换至 Bireo/ShipForge

- **一键全自动更新，彻底告别手动卸载与安装向导**（`updater_host.rs`、`updater.rs`、`main.rs`、`preferences.js`）：
  - 点击「一键自动更新」后，主程序自动校验安全边界并退出，无缝切换为独立置顶的品牌进度小窗（`shipforge-updater.exe` 独立宿主与隔离 WebView2 目录，零文件锁冲突）。
  - 安装版自动通过 NSIS `/S /UPDATE /D=<app_dir>` 执行静默就地升级（跳过旧版卸载确认与向导页），便携版改用纯 Rust 原子备份替换与异常自动回滚，不再依赖隐藏 PowerShell 脚本；完成后自动拉起新版并异步清理临时宿主目录。
  - 若遇安装目录权限受限，弹窗内直接提供「管理员重试」「手动安装」与「打开原版本」兜底动作，绝不中断失联。
- **融合 ShipForge 官方 Logo 的专属进度弹窗**（`src/web/updater.html`）：
  - 以 ShipForge 等距锻造立方体 Logo 为视觉中心：外圈采用语义化 `<progress>` 圆环进度光晕，内置动态旋转蓝图构造环、悬浮脉动熔铸琥珀核心与微抬精密顶盖动效，配合 4 步进度轨道（下载 · 校验 · 安装 · 启动）与浅/深双色自适应主题。
  - 新增 `--preview-updater` 预览模式与端到端单元测试覆盖。
- **线上 Release 说明与 `latest.json` 一键联动同步**（`repo_client.rs`、`packager.rs`、`main.rs`、`release.js`、`version.js`）：
  - 在「产物与仓库 → 线上发行版」编辑面板新增「同步覆盖线上 latest.json」开关，保存 Release 说明时可选仅覆盖更新本地与线上 `latest.json`（不重传安装包/便携包，强制版本一致性与公网 URL / SHA-256 / 远端产物存在性安全门禁，支持 GitHub 含草稿 / Gitee / Gitea / GitLab）。
  - 在「版本与源码 → 变更日志」撰写与阅读视图新增「同步线上日志」一键按钮，写入变更记录时自动同步本地同版本 `latest.json`。
- **修复 Git 更新日志草拟器被 `docs:` 提交劫持的 Bug**（`git_ops.rs`、`main.rs`、`version.js`）：
  - 当区间内存在 `pre-build` 快照或工作区源码/引擎改动且仅有 `docs:`/`chore:` 类提交时，优先在「更新内容」顶部列出源码与子引擎改动摘要及增删行数；草拟接口改为传入真实项目配置与目标版本号。
- **GitHub 仓库与更新源迁移至 `Bireo/ShipForge`**（`updater.rs`、`updater_host.rs`、`preferences.js`、`tools/publish-release.ps1`、`website/`、`README`）：同步切换在线更新源白名单、发包脚本与 GitHub Pages（`https://bireo.github.io/ShipForge/`）。

## 1.4.0 (2026-11-26) —— 界面去重收口、内置官方在线自动更新与独立 GitHub 仓库部署

- **界面去重与布局收口**（`index.html`、`styles.css`、`preferences.js`、`overview.js`、`version.js`、`manage.js`、`package.js`）：
  - 删除左侧栏底部重复的「偏好设置 (`Ctrl+,`)」按钮，仅保留顶部工具栏唯一入口（带新版本提示圆点）并重新平衡侧栏底部按钮与版本信息间距。
  - 统一清理跨页面重复功能按钮：移除「版本与源码」顶栏与「通用与外观 / 当前项目配置」中重复的「打开数据目录」（保留于「存储与扫描」）及重复的「移出此项目」（保留于「当前项目配置」与项目右键菜单）；移除「发版总览」顶栏与工具栏重复的「变动扫描」及重复的「配置向导」入口；移除「分析与分包」顶栏与分段标签重复的切换按钮。
- **内置官方在线自动更新引擎**（`updater.rs`、`preferences.js`、`app.js`）：
  - 完整移植三通道容错更新架构（`latest.json` 主源 → 302 标签直链 → GitHub Releases API / 零限流 Atom 预发布源），严格锁定 `3347567186/ShipForge` 官方仓库域名与路径、拦截 HTTPS 降级与非官方重定向。
  - 支持断点续传、取消下载、SHA-256 与 PE/ZIP 文件头双校验、安装版 (NSIS) 与便携版 (ZIP) 双形态自动升级（便携版解压防 Zip Slip 与 ZIP 炸弹、替换失败自动回滚旧版），且在有运行中构建/发版任务时拒绝中断安装。
  - 偏好设置新增独立「版本与更新」面板（收口版本信息、通道切换、启动静默检测、缓存清理与离线/远端变更记录同步），启动后延迟 6 秒静默检查更新。
- **独立 GitHub 仓库与发包推送体系**：新增 `tools/publish-release.ps1`、`LICENSE`、`.gitignore`、中英文 `README`、GitHub Pages 官网及 `.github/workflows` 自动化配置。

## 1.3.17 (2026-11-26) —— 右键菜单按功能区域显示，不再每次全量堆叠

- **右键菜单只显示被点击区域需要的功能**（`context-menu.js`）：去掉原来每次右键都追加在末尾的全局区块（变动扫描、命令面板、终端开关、折叠侧栏、深色外观、强调色、偏好设置、使用说明书）。现在各区域只显示自己的操作：输入框（剪切/复制/粘贴/全选/清空）、选中文字（复制）、项目行（切换/打开目录/配置/重新识别/启动/移出）、执行终端（复制/清空日志、配色、停靠/浮窗/全屏/隐藏）、产物行、线上版本、提交、标签、变更文件各自的专属操作。
- **侧栏空白处**新增只含「导入本地项目…」「折叠/展开左侧栏」的菜单；**页面空白处**只显示当前页相关操作，外加「变动扫描 / 刷新状态」和「命令面板」。弹窗内和没有可用操作的位置不再弹出菜单。
- 主题、强调色、偏好设置、说明书仍可通过顶部工具栏按钮、快捷键和命令面板使用。

## 1.3.16 (2026-11-26) —— 红色仅用于异常：执行终端与全局配色规范重设计

- **执行终端**（`api.js`、`styles.css`）：红色只用于真实错误（`✗`、`[红]`、`error:` / `error[E0425]:`、`fatal:`、`panicked at`、`test result: FAILED`、非零失败数、`构建失败：…`、非零退出码）。不再因为输出来自 stderr 就变红，因此 cargo 的 `Compiling …`、`Info Looking up …` 等正常进度保持普通文字色；`失败 0`、`0 failed` 这类零失败汇总也不再误判为错误。警告（`warning:` / `警告`）用琥珀色，成功用绿色，章节标题用主题色。
- **全局配色**：「停止」「删除」「移出此项目」「撤回发布」「停止服务」及右键菜单危险项等正常操作改为中性样式（靠二次确认对话框保护），悬停也不再变红。红色只保留给异常状态：错误提示、失败/损坏/缺失标签、异常状态圆点和错误 Toast。
- 前端自测新增终端分级与配色约束（20/20）。

## 1.3.15 (2026-11-26) —— 无新增改动的重复构建版本自动沿用最近一条真实更新日志

- **重复构建/重发布版本不再显示「正式发布。」占位**（`main.rs`）：当目标版本没有任何新的代码、文档改动（例如因构建发布出错而重复发布的 v1.47.2 / v1.47.3），且变更记录里已有最近一条真实日志时，自动沿用该条内容（去掉原标题行，避免写入变更记录时出现重复标题），来源标签显示为「沿用 v1.47.1 日志（v1.47.3 无新增改动）」。有真实新改动的版本仍按原有优先级提取。

## 1.3.14 (2026-11-26) —— 根治「同步状态后 8 秒必现重复变动提示」与更新日志真内容提取修复

### 一、彻底根治「同步状态 / 忽略」后 8 秒反复弹出变动横幅
- **同源化 `ChangeReport` 与 `deployReadiness`**（`main.rs`）：修复 `get_project_overview` 漏算 `change_report.deploy_readiness` 导致前端每次点「同步状态」后 `deployReadiness` 归零、8 秒后 `detect_project_changes` 比对 `uiOutOfSync` 恒为 `true` 的死循环根因；将总览与后台扫描统一抽为唯一共享函数 `build_change_report_from_parts`，首帧即完整呈现「部署与推送（免重编）」卡片且与后台扫描 100% 零漂移。
- **前端防并发竞态与基准对齐**（`app.js`）：`probeProjectChanges` 增加 `ovSeq !== this.overviewReqSeq` 丢弃过时在途扫描结果；点「同步状态」先确认后端基线再刷新；点「忽略」同步更新内存 `App.overview.changeReport` 基线且不打断当前表单，杜绝无新改动时再次弹窗。

### 二、更新日志真内容提取全面升级（消灭空壳占位与裸文件路径）
- **识破空壳占位条目并禁止污染变更记录**（`version.rs`、`overview.js`、`release.js`）：重构 `is_bare_default_release_note` 剥离无标题的版本号/日期行，精准识破 `## 1.47.3 (2026-11-26)\n\nQDisk v1.47.3 正式发布。` 等空壳占位符，不再误标绿色「变更记录」；前后端同步禁止将未编辑的默认占位文案写入 `变更记录.md`；`upsert_changelog_entry_with_summary` 按原行切片保真写回，彻底消除改写无关历史行 CRLF 造成的污染 Diff。
- **自动聚合自上次正式发布以来的真实变更记录**（`version.rs`、`git_ops.rs`、`main.rs`）：新增 `extract_changelog_entries_since`（`changelog_range` 层级），当目标版本尚无精确条目但变更记录中存在大于上一正式发布 Tag（或已封版版本）且 `≤` 目标版本的实质记录（如 QDisk `v1.46.16` → `1.47.2` 之间的 `1.46.17`/`1.47.0`/`1.47.1`）时，自动聚合提取真实人工更新日志；Git 改动提取优先解析变更记录新增正文与提交正文，屏蔽纯版本文件与裸变更记录路径输出，并将纯代码改动收敛为简洁中文模块摘要；当确实无新版本实质说明时诚实提示最新有效版本号并提供一键「载入 vX.Y.Z 日志」。

## 1.3.13 (2026-11-26) —— 全量功能深度体检修复与双轮红队安全加固

### 一、核心流程与数据准确性深度修复
- **向导绑定与多平台仓库一致性**：修复新建项目向导第 3 步绑定仓库缺少 `credentialId` 参数导致的必现 IPC 报错；修复「项目设置」保存仓库未同步 `repoBinding.repoSlug` 的静默失效及非 GitHub（Gitee/GitLab/Gitea）双字段裂脑；绑定仓库时校验 `git remote` 退出码、保留匹配 SSH 远端并清理过时 `githubRepo`；未绑定仓库的非 Git 项目不再误报 `ready_to_deploy` 并提供一键跳转绑定入口。
- **产物状态实时防漂移与智能降级**（`artifacts.rs`）：升级产物追踪至毫秒级磁盘 mtime（`file_mtime_unix_ms`）+ 体积 + SHA-256 + 发布 URL 标签校验（同时覆盖 GitHub `/tag/` 与 GitLab `/-/releases/`）；外部重编二进制、`latest.json` 或 `SHA256SUMS.txt` 后自动从陈旧 `published` 降级，彻底消除「重编新版仍显示旧版已发布链接」的误导，并兼容旧版无时间戳元数据自动降级。
- **构建脚本内省、子引擎打包与链接器兜底**（`detector.rs`、`presets.rs`、`toolchain.rs`、`runner.rs`）：统一由 `build.ps1` 参数内省生成动作清单（不再覆盖 `-AppOnly`/`-Engine`/`-Portable`/`-Status`/`-Clean`）；补齐 PowerShell `Join-Path` 锁文件与 `$JournalPath` 台账解析；混合型项目默认打包方案自动纳入子引擎目录与 `THIRD_PARTY.md`；当系统 `PATH` 缺少 `link.exe`/`gcc.exe` 时自动为子进程补齐本地 MinGW 链接器目录；`packager.rs` 拦截空的 `custom` 下载基准 URL，`preflight.rs` `.gitignore` 去重写入并校验外部构建锁。
- **变更记录升序/降序智能排布与全链路 SemVer 2.0.0**（`version.rs`、`version.js`）：空变更记录路径自动兜底 `CHANGELOG.md`；前端列表统一按最新版本号（含完整预发布序）置顶展示，写入统一走智能升序/降序原地更新，不再破坏 QDisk 等升序 `变更记录.md` 的文档结构。
- **前端交互稳定性、任务隔离与状态清理**：修复 `app.js` 后台 8 秒自动扫描误查 `#pageHost` 导致 `#pageViewport` 展开面板塌陷与表单重置的问题，并增加 `_ovFormTouchedByProj` 保护用户手选发版配置；修复「部署与发布」`loadReleases` 异步刷新冲掉正在编辑的发布说明/资产勾选及跨区抢焦点；修复 `__system__` 任务无法取消与日志丢失；修复「环境自检」快速点击乱序覆盖（`prereqSeq`）及移除项目后前端残留状态泄漏；本地调试启动统一透传全局 `default_port` 与 `SHIPFORGE_UPDATE_FEED`/`UPDATE_FEED_URL`。

### 二、双轮红队对抗审查与并发/安全护栏
- **全异步 RAII 独占锁与配置安全校验**（`main.rs`）：为全部 Git 变更、归档清理、Release 撤回及独立部署命令加装项目级 RAII 锁并移入 `spawn_blocking` 闭包内严格对齐任务生命周期；`validate_project_config` 拦截 UNC 路径（防 SMB NTLM 泄漏）、目录穿越、非 `.jsonl` 台账与非法 `repoSlug`；`list_project_entries` 严格过滤嵌套子项目目录。
- **双轮红队回归验证**：两轮红队共定位并修复 16 项假设漏洞与边界竞态，补齐全部后端 Rust 单元测试（104 项全通、strict 0 警告）与前端自测（18/18 全通）。

## 1.3.12 (2026-11-26) —— 外部已打包成品智能探测、全框架免重编独立「部署与推送」及双轮红队加固

### 一、已构建成品智能探测（消除「已打包但未发布却显示无改动」盲区）
- **跨框架就绪态检测引擎**（`deploy.rs`）：新增 `inspect_deploy_readiness` 并在所有总览加载与后台自动扫描中实时输出 `deployReadiness`（`ready_to_deploy` / `needs_build` / `partial_artifacts` / `published`）；全面识别 Tauri、Electron、Wails、Flutter、Python、Go、Node、Java、.NET、C++ 等多框架分发格式（`.exe`/`.msi`/`.msix`/`.dmg`/`.pkg`/`.deb`/`.rpm`/`.AppImage`/`.apk`/`.aab`/`.zip`/`.tar.gz`/`.tgz`/`.tar.xz`/`.7z`/`.jar`/`.whl`/`.nupkg`/`.vsix`/分卷 `.partNNN`），并兼容无版本号后缀但由对齐 `latest.json` 引用的通用二进制产物。
- **精准新鲜度与标签对齐核验**：仅源码/子引擎/配置变动或磁盘 mtime 确实晚于成品时才判定过期（仅改 `变更记录.md`/文档绝不误判已编成品过期，「先构建后提交」亦不误报）；自动识别 `build-*` 内部构建标签与正式发布标签 `vX.Y.Z`，并核验 `vX.Y.Z` 是否严格指向当前 `HEAD`（若指向旧提交立即阻断防错发）。
- **自动扫描与状态栏联动**（`app.js`、`overview.js`）：外部 CLI 构建完成后自动探测并提示「已完成构建打包，仅差部署推送」；顶部状态条不再误报「工作区已同步」，同步修复 `overview.js` `!a.archived` 属性名错拼。

### 二、全框架免重编独立「部署与推送」功能（`deploy_existing_release`）
- **独立部署推送工作台**：在「发版总览」顶部新增「部署与推送（免重编）」专属卡片，展示当前版本就绪成品、总体积、待执行动作清单、本次发布说明来源预览及独立 Git/Release 策略选择。
- **全链路原子安全护栏**：前置预检校验版本一致性、远端凭据可用性、标签对齐性与防 `v0.0.0` 错发；执行期自动完成成品 SHA-256 核验、多框架 `latest.json`（含 `extras` 多产物数组）与 `SHA256SUMS.txt` 自动补齐/复检、Git 提交/打标/推送与多平台流式上传（二进制优先、清单最后），事务默认开启 `build_verified = true` 保护已有成品永不误删，完成后封版并弹出一次性《完整性与健康分析报告》。

## 1.3.11 (2026-11-26) —— 变更记录智能回写与缺失版本精准提示

### 一、一键「写入变更记录」与来源诊断强化
- **智能增改不重标题、自动适配升序/降序**：新增 `upsert_changelog_entry` 后端命令并在「发版总览」「生成更新清单」「发布新版本」三处增设「写入变更记录」快捷按钮；自动识别升序尾插（如 QDisk `变更记录.md`）与降序顶插（如标准 `CHANGELOG.md`），同版本原地替换不重复标题，自动避免双层 `## 版本号` 嵌套并同步保存版本草稿。
- **缺失版本原因直白提示**：当项目存在变更记录文档但尚未记录目标版本号时，来源徽章明确提示 `变更记录.md 尚无 vX.Y.Z，已从 Git 改动提取`（或工作区改动提取），一目了然。

## 1.3.10 (2026-11-26) —— 全链路更新内容自动读取填写（可一键清空自写）与清单/Release 一致性同步

### 一、六级智能更新内容自动提取引擎（自动读取为主，清空自写为辅）
- **多源文档 + 代码改动智能自动提取引擎**：新增 `resolve_release_notes_detail` 引擎，按「① 独立版本发布文档（`RELEASE_vX.Y.Z.md` / `docs/.../RELEASE_vX.Y.Z.md`）→ ② 项目变更记录精确版本段（`CHANGELOG.md` / `变更记录.md` / `RELEASES.md` / `HISTORY.md`，支持 `##`/`###` 与 `[]`/`【】`）→ ③ `README.md` 精确版本段或「更新日志」分区（严格屏蔽非日志营销正文）→ ④ 已保存版本草稿 → ⑤ 变更记录 `## Unreleased` / `## 待发布` 区 → ⑥ Git 提交历史 + 真实代码 Diff + 未提交工作区变动 → ⑦ 非 Git 工作区/版本桶变动分类汇总 → ⑧ 默认兜底」自动合成可读 Markdown 更新日志，彻底根治最近几版仅显示「正式发布。」的问题。
- **全入口默认自动填写 + 一键清空/重读**：在「发版总览 · 一键构建并发布」（`#plNotes`）、「产物与仓库 · 生成更新清单」（`#mfNotes`）与「发布新版本」（`#mpNotes`）三处统一默认自动加载并展示来源标签，提供「重新读取」与「清空」按钮；支持手动编辑保护（防后台自动扫描或切版本号冲掉已写内容）与隐私脱敏（自动遮蔽用户个人目录、URL 凭据与令牌前缀，保留正常系统路径）。
- **流水线与手动发布双端一致性同步**：`run_pipeline` 锁定同一份解析结果同时写入 `latest.json` 与远端 Release；`publish_release_multi` 上传 `latest.json` 前自动同步匹配版本的 `"notes"` 字段，确保软件内更新弹窗与 GitHub/Gitee/GitLab/Gitea Release 内容 100% 一致。

## 1.3.9 (2026-11-26) —— 事务回滚已验收成品保护、全链路编译/测试/打包极限提速与双轮红队加固

### 一、已验收构建防误删回滚保护与发版防连跳
- **根治发布失败误删已编译成品**：在 `TransactionSnapshot` 新增持久化检查点 `build_verified`（编译验收通过即原子落盘）；当 `remote_pushed || build_verified` 时，`rollback_and_verify` 同步保留版本配置、本地 Git 版本提交/标签与 `release/` 已验收安装包/便携包及清单，彻底避免网络/上传波动误删刚编译好的成品或导致版本与 Git 裂脑；同时在异常中断崩溃恢复中同样遵守已验收成品保护。
- **状态文案与健康报告精准引导**：`main.rs` 在已保留验收产物时输出「已保留产物待续传」而非误导性的「已回滚至初始状态」；`health_report.rs` 自动检测当前版本成品与 `latest.json` 对齐状态，提供一键直达「直接重试上传发布（无需重编）」按钮。
- **防版本双跳与上传抗抖动**：`overview.js` 在版本桶尚未构建且未封版、且 `release/` 无同版本活跃产物时自动默认「保持当前版本」，防止手动递增后再次 `+0.0.1` 双跳；`repo_client.rs` 提升 WinHTTP 连接/收发超时至 30s/180s、流式写缓冲提升 4× 至 256 KiB，并在重试退避前实时回报进度。

### 二、零缩水全链路编译、测试与打包提速
- **Rust 发布编译提速（零缩水）**：ShipForge 与 QDisk 的 `[profile.release]` 升级为 crate 级并行 + 增量编译（`lto = false`, `codegen-units = 16`, `incremental = true`, 保留 `opt-level = "s"` + `panic = "abort"` + `strip = true`）搭配 `rust-lld` 链接器，彻底消除跨 250+ crate 的全量 ThinLTO 瓶颈，单模块增量发布编译从 `~2m10s` 降至 `~10.4s`（提速 12×）。
- **测试门禁多路分片与零警告加固**：
  - `TidyAssistant\build.ps1` 将 183 条 `test_engine_rpc` 按 TestCase 类 LPT 均衡分片至 6 个隔离 Python 进程并行执行，内置 `total_tests == loadTestsFromModule().countTestCases()` 数学零漏测断言，并与 `cargo test` + `cargo check --release` + 8 个前端自测 100% 重叠执行（全量门禁由 1m49s 降至 ~21s）。
  - `ShipForge\build.ps1` 测试门禁升级为全量前端 17 个 JS/MJS `node --check` + `api.selftest.mjs` 与 `cargo test` + `cargo check --release` 严格 0 警告门禁并行执行，并支持源码指纹安全复用。
- **打包与校验并发原子化**：`TidyAssistant\build.ps1` 新增 `Compress-DirectorySmart` 智能压缩（已压缩媒体/归档 `NoCompression`，可执行文件/DLL/脚本 `Optimal`）+ `robocopy /MT:8` + 临时包结构完整性实测校验后原子落盘；`ShipForge\build.ps1` 与 `TidyAssistant\tools\publish-release.ps1` 引入纯 C# `Parallel.ForEach` 并发 SHA-256（256 KB 缓冲）与原子清单写盘，并保证二进制资产优先于清单上传。

## 1.3.8 (2026-11-26) —— 全功能数据准确性深度整改、多框架前置依赖精准探测与双轮红队加固

### 一、多框架与打包前置依赖探测全面校准
- **四轴组合依赖判定（语言 × 打包壳 × 子引擎 × 脚本/Git）**：彻底重构 `preflight.rs`，为 Tauri/Rust 补齐原生链接器（`linker`：MSVC `link.exe` / MinGW `gcc` / LLVM `lld` / `rustc` 内置 `gcc-ld`，显式剔除 Git for Windows 伪 `usr\bin\link.exe`），为混合型 Tauri/Electron 项目自动识别 `src/engine/` 内嵌 Python 子引擎并纳入前置检查，按 `tauri.conf.json` 实际 `bundle.targets` 决定 NSIS 必要性，补齐 Wails WebView2、Java Gradle/Wrapper、C++ 必填编译器与非 Git 项目可选 Git 判定。
- **工具链真实性防误报**：`toolchain.rs` 移除“退出码非零但 stderr 含 `version` 即放行”的误报逻辑，新增 `$CARGO_HOME\bin` 与托管缓存兜底；WebView2 强制校验 `msedgewebview2.exe` 存在并按数字版本段排序取最高版；`.venv` 强制校验 `Scripts\python.exe` 存在防残缺目录误报；`node_modules` 校验非空且支持 `frontend/`、`web/`、`src/` 子目录。
- **一键补全防错与磁盘余量联动**：修复取消全部勾选却静默全量修复的漏洞，`PrereqReport.ready` 联动 `headroom.diskOk`；`cfg:version_sync` 按 SemVer 2.0.0 数值预发布序对齐至最高版本（绝不降级）；`dep:python_venv` 不再吞掉 `requirements.txt` 安装报错且在无 `uv` 时自动通过 `.venv\Scripts\pip.exe` 装包。

### 二、版本同步、清单防投毒、体积分析与健康报告准确性加固
- **版本一致性严判**：`version.rs::read_project_versions` 要求全部已配置版本文件均存在且解析无错才可判定 `synced = true`，杜绝缺失/损坏文件被误报为“全部一致”。
- **产物版本提取与发版清单防投毒**：重构 `packager.rs::extract_artifact_version` 彻底消除 `-win64.zip` / `-setup.exe` / `.zip.part001` 贪婪吞并后缀的错误版本数据；`validate_manifest_for_publish` 与 `health_report.rs` 同步拦截空清单、相对 URL、非法路径穿越、缺失 64 位 SHA-256、磁盘体积漂移以及“清单版本与产物文件名版本错配”的投毒清单；前端 `release.js` 不再静默回退预选旧版产物。
- **分析器与流水线预检加固**：`analyzer.rs` 修正无扩展名文件分类与 `.dSYM` 大小写死分支，隔离嵌套子项目文件并显式透传 80,000 文件截断提示；`preflight_pipeline` 在启动长耗时构建前提前校验版本文件完整性与远端发布令牌可用性。

## 1.3.7 (2026-11-25) —— 多项目强数据隔离、双模式语义变更分类与发布流式校验加固

### 一、多项目 / 跨内容全链路数据隔离
- **磁盘工作区抗碰撞键 + 归属锚定**：统一 `storage.rs` 与 `artifacts.rs` 的工作区目录键（保留既有小写目录兼容，对大小写/标点/保留名追加确定性 SHA-256 短后缀），并通过 `workspace_owner.json` 锚定规范化根目录，彻底杜绝同名/大小写碰撞、移除后孤儿工作区误继承以及跨项目根目录劫持。
- **已封版版本桶不可变保护 & 私有/共享台账强过滤**：禁止后续未提交改动覆写 `sealed == true` 版本桶的 `changedFiles`；所有构建日志/台账写入 `projectRoot` 并在读取时做严格根路径过滤，防止共享 `builds.jsonl` 与复用项目 ID 串台。
- **前端运行态全量按项目分桶**：将任务、进度条、终端日志、流水线步骤、自动探测横幅与探测锁全部改造为按 `projectId` 隔离的 Map 结构；所有后端事件携带 `projectId`（含 512 槽 FIFO 任务路由表防清空与已完成任务防迟到回写）；本机更新源（`127.0.0.1`）与健康报告强绑定项目归属，屏蔽跨项目联调启动。

### 二、Git + 非 Git 双模式自动扫描与六维语义变更分类
- **非 Git 项目本地文件指纹基线监控**：支持无 `.git` 项目（如 ShipForge 自身）持久化 `fs_baseline.json` 监控，自动剔除构建缓存、`release/`、`launch_exe`、`engine_exe_path`、锁/日志临时文件与嵌套子项目，精准输出 `A/M/D` 变动清单。
- **六维语义分类 & 同屏状态零矛盾**：自动将变动分类为子引擎（`engine`）、配置/版本（`config`）、文档（`docs`）、产物（`artifact`）、源码（`source`）与资源（`other`），支持 Git 重命名与引号路径精准归类；自动探测加入 `switchSeq + activeProject.id` 竞态栅栏并在非编辑态静默对账，根除探测条与 KPI 卡片数字矛盾。

### 三、发布上传校验、断点续传服务与测试数据隔离加固
- **多平台流式上传顺序与远端字节复核**：优先上传二进制安装包与校验表、最后上传 `latest.json`（杜绝客户端半途拉到残缺清单），上传后复核远端 `size`；本机测试源支持 HTTP `Range` 206/416 断点续传并拦截 ADS 路径。
- **构建脚本与测试隔离加固**：`#[cfg(test)]` 自动重定向至独立临时目录（严禁单测写入生产 `D:\Tool\ShipForge`）；`build.ps1` 增加主程序退出等待与重试覆盖、NSIS 产物强制断言、便携 ZIP 结构校验与原子 `SHA256SUMS.txt` 复检。

## 1.3.6 (2026-11-24) —— 全链路构建/打包/验证多核加速与自适应紧凑计时 UI

### 一、全阶段构建、打包与验证性能优化
- **Rust 多核 ThinLTO + `rust-lld` 快速链接**：发布配置由单线程 Fat LTO (`lto = true, codegen-units = 1`) 升级为多核并行 `lto = "thin"` + `codegen-units = 8`，`build.ps1` 自动探测工具链内置 `gcc-ld\ld.lld.exe` 启用多核链接（缺省自动无感回退 GNU `ld`），大幅缩短发布编译耗时同时保持二进制体积与运行性能基线。
- **测试门禁并行化**：ShipForge 与 QDisk 构建脚本均改为原生异步进程并发执行前端自测与后端测试，隔离临时目录零冲突。
- **打包与校验引擎全链路提速**：`packager.rs` 引入 256 KB 缓冲读写、`tar.gz` 写盘同步流式 SHA-256（`HashingWriter` 零二次读盘）、ZIP 智能识别已压缩媒体/归档格式直存（`Stored`）+ Deflate Level 5、以及基于 `std::thread::scope` 的有界并发多文件哈希（`verify_checksums` / `generate_manifest` / 去重后的 `validate_manifest_for_publish`）。
- **项目总览与 Git 状态并发探测**：`get_project_overview` 与 `git_ops::inspect_git` 全面改用 `std::thread::scope` 并发探测，只读 `git status` 增加 `--no-optional-locks` 彻底消除后台自动侦测与构建提交的 `.git/index.lock` 竞争。

### 二、自适应紧凑计时显示与防挤压 UI 适配
- **按需递进计时格式（`59.9s → 1m0s → 1h0m0s → 1d1h1m1s`）**：前后端及构建脚本统一落地 `formatDuration` / `format_duration_ms` / `Format-CompactDuration`，60 秒前精确至 `0.1s`、达到 `59.95s` 平滑进位至 `1m0s`（绝不出现 `60.0s`），仅按实际累计时长显示已到达的单位，兼容转换历史台账中的纯秒记录；前端改用单调时钟 `performance.now()` 防系统校时跳变。
- **等宽数字与防挤压排版**：顶部执行进度条、终端标题栏（采用尾段保活弹性结构，窄窗下任务名自动省略而计时始终完整清晰）、右下角迷你胶囊及台账耗时列全面启用 `font-variant-numeric: tabular-nums`、`white-space: nowrap` 与预留最小宽度，杜绝跳字、换行与文字变形。

## 1.3.5 (2026-11-24) —— 预发布升级脚本防护、构建期版本写互斥锁与 LTO 静默编译心跳

### 修复与红队对抗加固
- **预发布版递增零崩溃防护**：`run_pipeline` 与 `run_project_action` 引入 `should_delegate_bump_to_script` 策略——当项目当前版本含预发布后缀（如 `1.46.7-beta.1`）或显式指定预发布目标版本时，不再把 `-Bump` / `-Version` 盲目委托给可能不兼容预发布解析的外部 `build.ps1`，改由 ShipForge 原子计算并同步写入全部版本文件后再启动构建，彻底杜绝外部脚本 `[int]"7-beta"` 崩溃与跳号问题。
- **构建期间版本号原子互斥锁（根除中途改版致 10 分钟构建作废）**：`apply_version` 先后校验项目执行锁（含 `__storage__` 迁移锁）与外部活跃构建锁（PID 实测存活），并以瞬时互斥锁消除 TOCTOU 竞态窗口；`preflight_pipeline` 与 `run_project_action` 写版本前同步拦截外部活跃构建；前端「总览 / 版本隔离」页实时监听 `sf-task-state-changed` 动态禁用版本修改与重复启动按钮（含 `withBusy` 释放后状态回正修复）。
- **静默编译心跳与多轮日志分隔**：`TaskRunner::run_streaming_sync` 新增基于 `Condvar` 的低开销静默心跳线程（每 25s 静默推送进度微调与阶段提示、任务结束/取消毫秒级退出，跨 `stdout/stderr/heartbeat` 采用 `AtomicU8::fetch_max` 保证进度严格单调不回退）；每次启动新任务自动插入终端分隔横幅，避免多轮构建日志混杂误读。
- **子引擎过期探测对齐与 SemVer 严格校验**：`build_guard::check_engine_stale` 排除 `test_*.py` / `*_test.py` 与 `.md`，新增监控 `.rs`/`.toml`；前后端与构建脚本统一收紧 SemVer 预发布段正则（拦截 `1.2.3-`、`1.2.3-beta..1`、`1.2.3-.beta` 等非法串）。

## 1.3.4 (2026-11-24) —— 远端待推送状态清晰可见与任务中止竞态/残锁加固

### 修复与红队对抗加固
- **新增 Git 远端同步状态精准展示（根除“误以为已上传远端”的视觉误导）**：`GitStatus` 新增 `aheadCount` / `behindCount` / `hasUpstream` 实测字段；当本地存在 `pre-build` 快照等未推送提交时，总览与 Git 状态条明确标红/标橙显示 **`↑ N 待推送`**（悬停提示未上传远端、点击直达 Git 推送）与 **`↓ N 落后`** / **`仅本地分支`**，仅在工作区干净且与远端完全同步时才显示「已与远端同步」。
- **彻底封堵“秒点停止”预启动竞态与流水线跨步取消缺口**：`TaskRunner::run_streaming_sync` 改用与 `cancel()` 一致的双锁顺序（`pipelines → running`）原子检测子进程启动窗口期内的取消信号（含独立单动作预取消标记与 `release_project` 自动回收）；流水线内 `git commit / tag / push` 与 `publish` 子步骤前逐项校验取消标记，并在中止时明确提示「未执行远端推送与发布」。
- **中止构建自动清理残留死锁**：强杀 `build.ps1` 子进程树若导致 `.build.lock` 来不及被脚本 `finally` 释放，中止后自动校验死 PID 并安全清理残留构建锁，避免阻塞下次构建。

## 1.3.3 (2026-11-24) —— 彻底修复 `.gitignore` 下自动提交报错与流水线构建后误报「连续修改」

### 修复与红队对抗加固
- **彻底修复 `git add -A -- . :(exclude)release` 在 `.gitignore` 忽略 `release/` 时直接报错**：弃用会触发 Git 路径规约校验冲突的 `:(exclude)` 语法，改为通过 NUL 分隔的 `git -c core.quotepath=false status --porcelain=v1 -z -uall` 精准解析未忽略变更、过滤产物目录并分批暂存，原生兼容 `.gitignore` 忽略/未忽略两种仓库形态、中文路径、`-` 开头文件名与删除/重命名。
- **自动拦截冲突与误暂存产物**：在提交前自动检测未解决的合并冲突（`UU`/`AA`/`DD`）防误提交冲突标记，并自动取消已被手动预暂存的 `release/` 产物文件（含 0 提交的新仓库 `unborn HEAD` 场景）。
- **彻底消除一键发版流水线刚构建完成即误报「检测到连续修改」**：在流水线的「生成清单」与「Git 提交」步骤完成后同步刷新版本隔离桶指纹（不增加 `buildCount`），确保流水线自身的产物与提交更新不会触发虚假连续修改警告，同时后续真实的二次代码改动依然精准预警。

## 1.3.2 (2026-11-24) —— 便携工具与依赖缓存真正跟随迁移、零限流一键修复与双轮红队加固

### 一、工具与依赖真正跟随自定义目录迁移
- **迁移目录自动收归便携工具**：更改自定义目录后，不仅迁移 `config/tools/cache`，还会自动收归本机便携工具（`NSIS`、`Tauri CLI`、`uv`、`gh`）到新目录 `<dataRoot>\tools\`（逐文件 SHA-256 校验）；对 `%LOCALAPPDATA%\tauri\NSIS` 在校验通过后建立 **0 字节 NTFS 目录联接**安全释放 C 盘空间且保持 `cargo-tauri` 原生兼容。新增「收归工具与缓存」独立按钮，已迁目录可随时一键收归。
- **构建依赖缓存自动重定向**：所有构建/测试子进程自动将 `NPM_CONFIG_CACHE`、`npm_config_store_dir`、`PNPM_HOME`、`PIP_CACHE_DIR`、`UV_CACHE_DIR`、`TAURI_CACHE_DIR` 及纯 ASCII 路径下的 `CARGO_HOME` 重定向至 `<dataRoot>\cache\`（尊重项目显式配置与用户已有非 C 盘环境变量）。

### 二、彻底根治一键修复 `403 rate limit exceeded` 报错
- **全面去 `api.github.com` 依赖**：`cargo-tauri`、`uv`、`nsis` 改用零限流 GitHub Release 直链，`gh` 改用零限流 `HEAD` 重定向解析最新版本号；支持本地已有 `cargo-tauri.exe` 零网络优先复用、3 次自动重试、临时暂存区校验后原子落盘（杜绝失败残留空目录），并将真实 PowerShell 错误详情直透前端与内置终端。

### 三、两轮红队对抗审查与缺陷清零
- **第一轮**：修复二次跨盘迁移（A→B→C）导致 `%LOCALAPPDATA%\tauri\NSIS` 悬空断链问题（自动重指向最新活跃目录）；修复 `copy_file_verified` 临时后缀防同名冲突及失败清理；修复存储迁移与收归缺失执行锁并发保护；修复 Windows PowerShell 5.1 缺 `-UseBasicParsing` 与 `Tls13` 兼容；防止覆盖用户已有的非 C 盘 `CARGO_HOME` 镜像配置。
- **第二轮**：升级 `__storage__` 为全局排他原子互斥锁（彻底杜绝迁移过程中并发触发构建任务）；精确区分联接「已对齐」与「实际重指向」返回值；补齐真实 Windows NTFS Junction A→B→C 迁移与排他锁单元测试。

## 1.3.1 (2026-11-24) —— 智能 Git 推送防 Tag 冲突、推送过程实时可视与小窗提交历史重设计

### 修复与体验优化
- **彻底修复 `Push 提交与 Tags` 遇远端已存在 Tag 直接报错**：推送前通过 `git ls-remote --tags` 精准对比远端已有标签，仅分批推送本地未推送的新标签并自动跳过远端已存在的同名标签（根治 `! [rejected] vX.Y.Z (already exists)` 冲突报错）。
- **Git 推送全过程实时可视**：设置 `GIT_TERMINAL_PROMPT=0` 杜绝意外弹窗，合并捕获 Git `stdout`/`stderr` 并同步推送至顶部集成实时进度条与内置输出终端，无论成功或失败均清晰展示每一步进度与详细结果。
- **重设计「最近提交历史」与全站窄窗表格适配**：移除易被右侧裁切的三列固定表格，改用容器查询（`@container card`）自适应提交列表——小窗自动切换为 2 行紧凑排布、多标签拆分为独立可换行胶囊（默认展示最近 6 条，可一键展开全部 20 条），宽屏/全屏保持单行舒展排布；同步为构建台账、工具链体检、发布产物表补充窄窗自动收敛次要列与长文件名自然换行。

## 1.3.0 (2026-11-24) —— 流体自适应布局重铸、零滑条极简界面、公开分发去硬编码与 Git 自动化闭环

### 一、全站无滑条 + 小窗/全屏容器查询流体布局重铸
- **全站零滚动条视觉净化**：全局启用标准 `scrollbar-width: none`（配合 WebView2 `::-webkit-scrollbar` 隐藏回退），彻底消除所有界面、卡片、终端、弹窗中的原生滑条与箭头，同时保留鼠标滚轮、触控板与键盘平滑滚动。
- **小窗 ↔ 超宽全屏自适应布局**：取消 `1040px` 硬宽度束缚与列表微缩 `max-height` 牢笼；基于 CSS 容器查询（`@container page` 与卡片级 `@container card`）重构发版台、构建打包、版本日志、发布中心与设置页，小窗清晰不挤、全屏铺展均衡无大片空白。

### 二、未提交文件纳入自动发版流水线 + 清晰状态表达
- **一键发版流水线新增「4. Git 自动化」步骤**：支持在构建与清单生成后自动执行「提交未提交文件 + 打 Tag（`vX.Y.Z`）」或「提交 + 打 Tag + 推送远端（自动处理无 upstream 分支）」，并严格排除 `release/` 二进制产物目录防止安装包误提交进 Git 历史。
- **状态提示零歧义**：总览状态条将 `(1 改动)` 改为可点击的 **`(1 未提交)`**（悬停展示具体文件路径，点击直达 Git 页）；手动「探测改动」无新增变化但存在未提交文件时明确提示具体未提交文件列表。

### 三、公开分发彻底去硬编码 + 自动工作区项目发现 + 发布中心全可编辑
- **零个人路径与仓库硬编码**：彻底移除源码与测试中的个人目录/项目/仓库常量；首次启动基于安全目录守卫自动发现当前/同级工作区项目；在左侧项目切换器、空状态与设置页新增「自动扫描工作区发现项目」弹窗及可维护的 `scanRoots` 扫描目录。
- **发布中心全面可配置**：支持自由编辑并保存目标仓库（`owner/repo`）、一键自动探测 GitHub 仓库（严格解析 `git remote` / `tauri.conf.json` updater / `package.json` & `Cargo.toml` `repository` 字段，绝不误匹配依赖 URL）、一键打开 `gh auth login / switch` 切换账号、动态按 Tag 过滤或显示全部产物、一键从 CHANGELOG / 隔离草稿 / Git 提交载入发布说明。

### 四、两轮红队对抗审查与缺陷清零
- **第一轮**：新增 `git_commit_for_release` 排除 `release/` 目录防大二进制污染 Git；前置检查 `commit_tag_push` 远端存在性；重构 `detect_github_repo` 仅读结构化字段防依赖 URL 误匹配；新增 `is_safe_auto_discovery_dir` 禁止冷启动扫描桌面/下载/用户目录及超大目录，并为发现器增加 400 目录访问预算。
- **第二轮**：防御 `release_dir == "."` 误排除全仓；`git_push` 动态解析首个可用远端而非硬编码 `origin`；修复自动发现弹窗点击遮罩关闭时的 DOM 节点残留；为 `.card` 启用独立 `@container card` 查询彻底修复中等窗口双栏卡片内部表单挤压问题。

## 1.2.0 (2026-11-24) —— 终端重铸、实时进度条、版本隔离探测与工具链自愈迁移

### 一、全新交互式 5 态自适应终端与无乱码引擎
- **彻底解决终端双编码乱码**：执行 PowerShell `-File` 脚本时自动注入 UTF-8 控制台与管道编码，并在后端叠加 `heal_utf8_as_gbk_mojibake` 纵深修复，彻底终结 `鈹€`/`閹氬` 等乱码；支持语义化日志分级高亮（绿/青/琥珀/红）。
- **默认完全隐藏 + 5 态自由形态**：空闲时不占位；支持「隐藏 / 底部停靠（顶边拖拽与双击快速切换高度）/ 自由悬浮窗（向上拖拽脱离、自由拖动与右下角自由缩放、窗口缩放自动钳位）/ 右下角迷你胶囊 / 全屏（`Esc` 恢复）」，完美适配浅色纸张与深色黑曜主题，并支持一键高对比深色切换与 `Ctrl+\`` 呼出。

### 二、原生集成实时进度条（取代左上角角标）
- 移除顶栏左上角「正在打包」角标，改在工作区顶部无痕嵌入原生 `<progress>` 实时进度条，实时展示阶段名、明细、百分比与耗时，支持构建里程碑推演、打包逐文件进度、迁移/修复进度、中止按钮与完成自动优雅收起。

### 三、新版本与改动自动/手动探测 + 严格版本隔离桶
- **自动监听 + 手动探测（`F5`）**：后台每 8 秒轻量静默监测版本号、Git HEAD、脏文件、引擎过期与产物变更，发现变动弹出顶部提醒条；支持随时手动即时探测。
- **按版本隔离防混淆**：为每个版本号建立独立隔离桶（`workspaces/<project>/<ver>.json`），独立记录基线 SHA、改动清单、构建次数、构建指纹、专属草稿与封版状态；同一版本构建或封版后发生连续修改时自动预警，支持一键「递增 Patch 隔离版本」，切换版本时自动载入对应隔离草稿，杜绝跨版本更新数据混淆。

### 四、自定义数据/工具目录原子迁移与工具链自愈
- **自定义存储目录 + SHA-256 原子迁移 + 安全清理旧目录**：支持将数据与受管工具目录一键迁移至自定义盘符，逐文件 SHA-256 校验、失败自动回滚、原子切换 `pointer.json`，并在迁移完成后仅删除 ShipForge 自身子目录（空旧目录自动移除、绝不误删非 ShipForge 文件）。
- **彻底修复中文用户名路径乱码与一键修复补全**：彻底弃用 `where.exe` 改用原生 Unicode `PATH` 扫描，修复中文用户名（如 `十二`）导致的路径 `` 乱码与版本探测失败；检测工具缺失与结构损坏（如 NSIS 缺少 `Stubs`/`Include`），支持单个与批量一键自动下载补全至 `<dataRoot>\tools\` 并复核校验。

### 五、双轮红队对抗审查与缺陷清零
- **第一轮**：修复 Windows `\\?\` 规范化导致的父子路径误判、拦截非空无关目录迁移目标、移除旧目录递归 `remove_dir_all` 改用受控子树清理、修复 PowerShell `; exit $LASTEXITCODE` 误伤 `robocopy` 退出码、修复 `gh`/`uv` 压缩包嵌套目录拍平、修复后台轮询空闲重复写盘与悬浮终端窗口缩放出界。
- **第二轮**：修复 PowerShell 相对脚本名 `& 'build.ps1'` 缺 `.\` 前缀导致的 `CommandNotFoundException`、加固 `heal_utf8_as_gbk_mojibake` 字符长度与控制符防误伤条件、重构批量工具修复进度单调映射防进度回跳与中途误收起、补齐 `cargo-tauri.zip` 嵌套拍平、补齐全屏终端 `Esc` 退出与缩放 `pointercancel` 清理、补齐版本草稿切换自动隔离载入及端到端迁移单测。

## 1.1.0 (2026-11-24) —— 界面全面精简重构与双轮红队对抗审查加固

### 一、UI 与交互体验重构
- **极简顶栏 + 单槽位项目切换器**：精简顶栏冗余标签，侧栏收敛为「单槽位项目下拉切换器 + 5 大主导航」（发版台 / 构建打包 / 版本日志 / 发布中心 / 源码与设置）。
- **页内子标签分层**：拆分原「构建」「发布」「设置」页面的多表单堆叠，单视图聚焦单一任务；用清爽单行状态条与 Linear 行式列表取代密堆积卡片与裸命令字符串。
- **窗口与终端体验补齐**：补齐无边框窗口 8 向缩放热区、底部实时终端可拖拽高度调节，以及全局按钮防重复点击（`withBusy`）与异步渲染竞态保护（`renderEpoch`）。

### 二、两轮红队对抗式审查缺陷清零
- **流水线生命周期与前置拦截**：拆分 `task-finished`（单动作）与 `pipeline-finished`（整条流水线）事件，彻底修复多步骤流水线中途丢失停止按钮及无 Shell 步骤时进度条卡死问题；新增 `preflight_pipeline` 在修改版本号前拦截无效配置；修复仅跑测试门禁不构建时误跳过测试的逻辑漏洞；新增单项目并发执行互斥锁。
- **异步化与性能加固**：所有重 I/O、哈希、ZIP/TAR.GZ 压缩、Git 与 GitHub 网络命令改为 `spawn_blocking` 异步执行；所有热路径正则改为 `OnceLock` 零重复编译。
- **版本与打包安全加固**：支持 Tauri v1 `package.version`、Cargo `[workspace.package]` 与行尾注释、JSON 顶层作用域替换（不误伤嵌套依赖）及写入失败自动回滚；打包器新增临时文件原子落盘、防递归自包含、路径去重、自动补后缀与大小写无关通配排除。
- **发布防泄漏与本机联调革新**：GitHub 上传前自动校验 `latest.json`（拦截 `127.0.0.1`/`localhost` 泄漏、版本错配、缺失资产与 SHA-256 漂移）；本机 HTTP 更新测试源支持**内存中动态重写 `latest.json`**（永不污染磁盘生产清单）及线程 `JoinHandle` 确定性停服防端口占用竞态。
- **安全边界与跨版本迁移**：`open_external_url` 彻底弃用 `cmd /C start` 改用严格字符白名单 + `rundll32 url.dll,FileProtocolHandler` 杜绝命令注入；Git Tag 严格拦截 `-` 开头参数注入；`build.ps1` 自动内省参数块杜绝生成幻影开关；配置新增 v1→v2 自动迁移。

## 1.0.0 (2026-11-24) —— 首个正式版本发布

### 核心能力
- **多格式自动适配引擎**：自动识别 Tauri（含 TidyAssistant/QDisk 的 `build.ps1` + 锁/引擎过期检查）、Rust/Cargo、Electron、Node.js/Web、Python/PyInstaller、Go、.NET/C#、Flutter 与 NSIS/Inno Setup 项目。
- **一键发版流水线**：按序编排「版本更新 → 测试门禁 → 构建与打包 → 清单与校验生成 → GitHub Releases 上传」，带步骤进度条与全局可折叠实时输出终端。
- **原生 Rust 通用打包工坊**：内置零外部依赖 ZIP / TAR.GZ 打包器、流式 SHA-256 校验与核验、旧包留存清理与归档。
- **原子版本同步与变更日志工作台**：同步读写多份配置文件版本号，支持从 Git 提交一键草拟变更记录。
- **发布中心与本机联调更新源**：内置 `latest.json` + `SHA256SUMS.txt` 生成器、GitHub CLI (`gh`) 一键上传发布与纯 Rust 本机回环 HTTP 更新测试源。
