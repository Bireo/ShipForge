# 第三方组件与许可

ShipForge 的构建与发布功能依赖以下开源组件，全部静态编译或随源码分发，无额外外部运行时要求。

## Rust Crates

| 组件 | 版本 | 许可证 | 用途 |
|---|---|---|---|
| tauri | 2 | MIT / Apache-2.0 | 桌面窗口壳与原生 IPC |
| tauri-plugin-single-instance | 2 | MIT / Apache-2.0 | 单实例窗口聚焦 |
| serde / serde_json | 1 | MIT / Apache-2.0 | 配置与清单序列化 |
| anyhow | 1 | MIT / Apache-2.0 | 错误链路传递 |
| chrono | 0.4 | MIT / Apache-2.0 | 构建台账与发布时间戳 |
| regex | 1 | MIT / Apache-2.0 | 版本号与变更记录解析 |
| rfd | 0.15 | MIT | 原生目录选择对话框 |
| sha2 | 0.10 | MIT / Apache-2.0 | 产物与在线更新 SHA-256 校验 |
| walkdir | 2.5 | MIT / Unlicense | 项目目录分析与扫描 |
| flate2 + zlib-rs | 1.1 | MIT / Apache-2.0 | Gzip / Deflate 压缩 |
| tar | 0.4 | MIT / Apache-2.0 | `.tar.gz` 归档打包 |
| zip | 9.6 | MIT | 便携包压缩与安全解压 |
| windows-sys | 0.61 | MIT / Apache-2.0 | WinHTTP 流式传输、DPAPI 令牌加密与系统资源探测 |

各 crate 完整许可证文本可于 https://crates.io 对应版本页面查阅。

## 字体资源

- **Inter** (`src/web/assets/fonts/Inter-latin.woff2`)：基于 SIL Open Font License 1.1（完整原文见 [OFL.txt](src/web/assets/fonts/OFL.txt)）。
