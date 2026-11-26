<div align="center">

<img src="src/src-tauri/icons/128x128.png" width="76" height="76" alt="ShipForge Logo" />

# ShipForge · 轻筑

面向 Windows 本地软件开发者的构建、打包、版本同步与多平台发布桌面工具。基于 Tauri v2 + 原生 Rust 构建，MIT 开源，支持在线自动更新。

[![Website](https://img.shields.io/badge/%E5%AE%98%E7%BD%91-Live%20Site-d98a00?style=flat-square)](https://3347567186.github.io/ShipForge/)
[![Latest Release](https://img.shields.io/github/v/release/3347567186/ShipForge?style=flat-square&color=2ea043&label=%E6%9C%80%E6%96%B0%E7%89%88%E6%9C%AC)](https://github.com/3347567186/ShipForge/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/Windows-10%2F11%20x64-0078D6?style=flat-square)](#下载与安装)

**简体中文** | [English](README.en.md)

</div>

---

## 主要功能

- **多框架项目识别与编译模板**：支持 Rust、Node/TS、Python、Go、C#/.NET、Dart/Flutter、C/C++ (CMake)、Java (Maven/Gradle)，以及 Tauri、Electron、Wails、Flutter Desktop、PyInstaller、NSIS、Inno Setup 等常见打包外壳，可自动识别混合项目与本地构建脚本。
- **项目级前置环境检查**：只检查当前项目需要的编译器、依赖与仓库配置，列出修改清单并在确认后执行补全。
- **文件体积分析与分包工坊**：扫描目录体积分布与大文件，默认拦截 `.env`、私钥和 `.git` 目录，支持按平台限制切分分卷压缩包并生成校验文件。
- **多配置文件版本同步与变更日志**：原子同步 `tauri.conf.json`、`Cargo.toml`、`package.json`、`pyproject.toml`、`*.csproj`、`pubspec.yaml` 或自定义正则版本源，支持按版本自动读取或回写 `CHANGELOG.md`。
- **多平台仓库直连发布**：通过原生 WinHTTP 流式上传至 GitHub、Gitee、GitLab、Gitea（支持编辑、草稿切换与撤回发布），访问令牌使用 Windows DPAPI 加密存储，支持二进制优先、清单最后上传顺序。
- **事务保护与故障回滚**：发版或打包异常中断时自动保留已验收成品或回滚未完成的版本与本地 Git 提交，并生成一次性健康检查报告。
- **内置官方在线更新**：支持安装版（NSIS）与便携版（ZIP）自动检测 GitHub Releases 新版本，下载强制校验 SHA-256 与文件头，便携版替换失败自动回滚。

---

## 下载与安装

支持 **Windows 10 / 11 (x64)**。

- **官网**：https://3347567186.github.io/ShipForge/
- **安装版**：在 [GitHub Releases](https://github.com/3347567186/ShipForge/releases/latest) 下载 `ShipForge_*_x64-setup.exe`
- **便携版**：在同一页面下载 `ShipForge_*_x64-portable.zip`，解压后直接运行 `shipforge.exe`
- **校验与更新**：每次发布附带 `SHA256SUMS.txt` 与 `latest.json`。可在软件「偏好设置 → 版本与更新」中一键检查、下载并升级。

---

## 本地构建与独立推送更新

环境要求：Windows x64、Rust 1.80+、Node.js 20+、PowerShell 7 (`pwsh`)、NSIS 3.x。

```powershell
git clone https://github.com/3347567186/ShipForge.git
cd ShipForge

# 1. 运行前端与 Rust 测试门禁
pwsh -NoProfile -ExecutionPolicy Bypass -File .\build.ps1 -TestsOnly

# 2. 编译主程序、NSIS 安装包、便携 ZIP 并生成校验文件
pwsh -NoProfile -ExecutionPolicy Bypass -File .\build.ps1 -Bump patch

# 3. 生成 latest.json 并推送到 GitHub Releases（需先 gh auth login）
pwsh -NoProfile -ExecutionPolicy Bypass -File .\tools\publish-release.ps1 -UploadGitHub

# 4. 本地回环测试在线更新（127.0.0.1）
pwsh -NoProfile -ExecutionPolicy Bypass -File .\tools\publish-release.ps1 -ServeLocal -Port 18765
```

---

## 许可

本项目基于 [MIT License](LICENSE) 开源。欢迎提交 [Issue](https://github.com/3347567186/ShipForge/issues) 或 Pull Request。
