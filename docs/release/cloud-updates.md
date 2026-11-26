# Matou 云端更新发布手册

## 客户端读取哪些产物

Matou 从打包时写入的 HTTPS 地址读取更新清单。原生 macOS 根据 `stable-mac.yml` 下载 DMG。同一份清单也包含供 macOS 0.2.x 从旧版升级的完整 ZIP，旧版安装前校验应用签名。Windows 根据各自架构的 `stable.yml` 下载已签名的 EXE 安装器。

| 平台 | 打包产物 | 更新清单 |
| --- | --- | --- |
| macOS arm64 / x64 | `Matou-<version>-mac-<arch>.zip`、`Matou-<version>-mac-<arch>.dmg`、`SHA256SUMS-<version>-mac-<arch>.txt` | `stable-mac.yml` |
| Windows x64 / ARM64 | `Matou-<version>-win-<arch>.exe`、可选的同名便携 ZIP、`SHA256SUMS-<version>-win-<arch>.txt`（ARM64 文件名用 `arm64`） | `stable.yml`，指向 EXE |

各架构使用独立的发布目录和对应的更新地址。Windows ARM64 默认使用 `/stable/win-arm64`，可用 `MATOU_WIN_ARM64_UPDATE_BASE_URL` 或 `--update-base-url` 指定。

## 准备发布机

在 `apps/native/release-config.json` 提升发布版本，同步四个原生 crate 的版本及 `apps/native/Cargo.lock`。准备对应架构的 GPUI、Ghostty、Cargo 依赖和本地 sidecar。[原生工作区](../../apps/native/README.md)

- macOS 发布机的钥匙串需要 `mac.identity` 指定的 Developer ID Application 和私钥。正式构建保持 `com.matou.desktop`、Team `JAKX72KQ6X` 和固定的 designated requirement，签名使用 Apple 安全时间戳。当前打包脚本未接入公证。退出本机码头后，用 `pnpm install:mac --source <绝对路径.app>` 更新本地安装。
- Windows 发布机需要 Inno Setup 7、Windows SDK SignTool、`MATOU_WIN_SIGN_CERT_SHA1` 指定的签名证书，以及匹配架构的 Visual Studio 2022 app-local CRT 文件。可用 `--vc-runtime-dir` 指定 CRT 目录。

## 构建与打包

macOS 在对应架构构建随包组件，默认使用 arm64。Intel 构建给 `package:mac` 传 `--arch x64`，并准备 x64 Rust 工具链、终端库和两个 sidecar。

```bash
pnpm accounts:build
pnpm router:build
pnpm package:mac
```

macOS 默认从 `apps/native/package-resources/` 读取随包组件，产物写入 `apps/native/release/`。

Windows 在对应架构准备账户组件和路由，使用 `router:build` 输出的目录作为 `--model-router`。以下 PowerShell 命令构建 x64 安装器并附带便携 ZIP。`routerDirectory` 填上一条命令打印出的目录。

```powershell
pnpm accounts:build
pnpm router:build --platform=win32 --arch=x64
$routerDirectory = 'C:\path\to\model-router-output'
pnpm package:win --arch x64 --model-router $routerDirectory --with-zip
```

构建 ARM64 时将上述两个架构参数改为 `arm64`，并准备 ARM64 账户组件和 CRT。Windows 打包会按目标准备 GPUI 和 Ghostty，并离线编译 Rust；先用 `cargo fetch --manifest-path apps/native/Cargo.toml --locked` 准备 Cargo 依赖。默认 x64 写入 `apps/native/release/`，ARM64 写入其 `win-arm64/` 子目录。

Windows 正式打包默认生成 EXE；`--with-zip` 额外生成便携 ZIP。只做便携分发时用 `--portable-only`，该模式不生成更新清单。两端均可用 `--out-dir` 选择新的输出目录，或同时提供 `--desktop-bin` 和 `--runtime-bin` 使用预编译产物。

## 上传顺序

更新服务器需要公开可读的 HTTPS、准确的 `Content-Length`、正确的文件类型和 Range 支持。用 `GET`、`HEAD` 和返回 `206 Partial Content` 的 Range 请求检查。

1. 上传 ZIP、DMG 或 EXE 及 SHA256 校验文件到临时位置。
2. 对照各自的清单与校验文件验证 SHA-512、SHA256 和大小。便携 ZIP 用 SHA256 校验。
3. 把产物移入对应架构的正式目录。
4. 最后原子替换 `stable-mac.yml` 或 `stable.yml`。
5. 从外网读取清单及其引用的每个 URL，检查大小、哈希及 Range 响应。

安装产物可长期缓存。更新清单使用 `Cache-Control: no-cache` 或较短 TTL。发布凭据留在发布环境。

## Nginx 最小配置

```nginx
location /matou/stable/ {
    alias /srv/matou-updates/stable/;
    types {
        application/octet-stream zip exe;
        application/x-apple-diskimage dmg;
        text/yaml yml;
    }
}

location ~ ^/matou/stable/(?<manifest>.+[.]yml)$ {
    alias /srv/matou-updates/stable/$manifest;
    default_type text/yaml;
    add_header Cache-Control "no-cache, must-revalidate" always;
}
```

## macOS 0.2.x 跨代更新验收

1. 在隔离环境安装对应架构的 0.2.x，创建运行中和空闲的会话。
2. 从隔离更新地址手动检查，确认旧版下载完整 ZIP，且下载时可继续输入。
3. 下载后选「空闲后自动更新」，验证运行会话结束前应用不会重启。
4. 会话转为空闲后，确认应用只重启一次，恢复原工作区和画布，并只提示一次更新完成。
5. 在原生版本上复查更新入口，确认使用同一清单中的 DMG。

Windows 在 x64 和 ARM64 上分别验证更新清单选择对应架构的签名 EXE、覆盖升级和会话恢复。单独解压便携 ZIP 验证启动。

## 回退

发现异常时，将更新清单原子恢复到上一稳定版本，阻止更多客户端发现异常版本。已升级用户通过更高 SemVer 的修复版本恢复。
