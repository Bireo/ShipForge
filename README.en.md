<div align="center">

<img src="src/src-tauri/icons/128x128.png" width="76" height="76" alt="ShipForge Logo" />

# ShipForge

A desktop build, packaging, versioning, and multi-platform release studio for Windows developers. Built with Tauri v2 and native Rust, MIT licensed, with built-in auto-updates.

[![Website](https://img.shields.io/badge/Website-Live%20Site-d98a00?style=flat-square)](https://3347567186.github.io/ShipForge/?lang=en)
[![Latest Release](https://img.shields.io/github/v/release/3347567186/ShipForge?style=flat-square&color=2ea043&label=Latest)](https://github.com/3347567186/ShipForge/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/Windows-10%2F11%20x64-0078D6?style=flat-square)](#download--install)

[简体中文](README.md) | **English**

</div>

---

## Features

- **Multi-framework detection & build presets**: Supports Rust, Node/TS, Python, Go, C#/.NET, Dart/Flutter, C/C++ (CMake), Java (Maven/Gradle), and shells including Tauri, Electron, Wails, Flutter Desktop, PyInstaller, NSIS, and Inno Setup.
- **Project-scoped prerequisite checks**: Inspects only the toolchains, local dependencies, and repository setup needed by the active project, showing a transparent plan before installing anything.
- **Directory size analysis & split packaging**: Analyzes folder footprints and large files, blocks `.env`, private keys, and `.git` by default, and supports multi-volume ZIP splitting with checksums.
- **Atomic version sync & changelog management**: Keeps `tauri.conf.json`, `Cargo.toml`, `package.json`, `pyproject.toml`, `*.csproj`, `pubspec.yaml`, and custom regex targets in sync, with automatic changelog extraction and write-back.
- **Direct multi-platform release publishing**: Native WinHTTP streaming uploads to GitHub, Gitee, GitLab, and Gitea with DPAPI-encrypted token storage, binary-first / manifest-last upload order, and online release edit or recall.
- **Transactional rollback & guardrails**: Protects verified build artifacts, rolls back incomplete local version or Git mutations on failure/abort, and checks disk/memory headroom before running builds.
- **Built-in auto-updater**: Checks official GitHub Releases for both NSIS Installer and Portable ZIP builds, enforces SHA-256 and binary header checks, and rolls back automatically if a portable swap fails.

---

## Download & Install

Requires **Windows 10 / 11 (x64)**.

- **Website**: https://3347567186.github.io/ShipForge/?lang=en
- **Installer**: Download `ShipForge_*_x64-setup.exe` from [GitHub Releases](https://github.com/3347567186/ShipForge/releases/latest)
- **Portable ZIP**: Download `ShipForge_*_x64-portable.zip`, extract, and run `shipforge.exe`
- **Checksums & updates**: Every release ships with `SHA256SUMS.txt` and `latest.json`. Open `Preferences -> 版本与更新` inside the app to check, download, and install updates.

---

## Build & Publish Updates Locally

Requirements: Windows x64, Rust 1.80+, Node.js 20+, PowerShell 7 (`pwsh`), NSIS 3.x.

```powershell
git clone https://github.com/3347567186/ShipForge.git
cd ShipForge

# 1. Run frontend + Rust test gate
pwsh -NoProfile -ExecutionPolicy Bypass -File .\build.ps1 -TestsOnly

# 2. Build binary, NSIS installer, portable ZIP & checksums
pwsh -NoProfile -ExecutionPolicy Bypass -File .\build.ps1 -Bump patch

# 3. Generate latest.json and publish to GitHub Releases (requires gh auth login)
pwsh -NoProfile -ExecutionPolicy Bypass -File .\tools\publish-release.ps1 -UploadGitHub

# 4. Run a local loopback update feed for end-to-end testing (127.0.0.1)
pwsh -NoProfile -ExecutionPolicy Bypass -File .\tools\publish-release.ps1 -ServeLocal -Port 18765
```

---

## License

Licensed under [MIT](LICENSE). Issues and PRs are welcome at [GitHub Issues](https://github.com/3347567186/ShipForge/issues).
