# Matou 云端更新发布手册

## 发布模型

Matou 使用 `electron-updater` 的 generic HTTPS provider。客户端读取打包时写入的更新地址，检查 `stable-mac.yml`，下载对应 ZIP 与 blockmap，并由 macOS 签名链验证后安装。

首阶段产物：

- `Matou-<version>-mac-arm64.dmg`
- `Matou-<version>-mac-arm64.zip`
- `Matou-<version>-mac-arm64.zip.blockmap`
- `stable-mac.yml`

x64 构建使用相同命名并将架构替换为 `x64`。

## 第一次接入服务器

服务器地址示例：

```text
https://updates.example.com/matou/stable
```

该目录需要：

- HTTPS 证书有效且证书链完整。
- 支持 `GET`、`HEAD` 和 Range 请求，Range 响应返回 `206 Partial Content`。
- 返回准确的 `Content-Length`、`Content-Type` 和 `Accept-Ranges: bytes`。
- DMG、ZIP、blockmap 可长期缓存；更新清单使用 `Cache-Control: no-cache` 或较短 TTL。
- 清单与产物公开读取，发布凭据只留在 CI，不进入 App。

## 版本与签名

每次发布前把 `apps/desktop/package.json` 的 `version` 提升为严格递增的 SemVer。macOS 自动安装要求 Developer ID Application 签名；发布机或 CI 配置：

```bash
export CSC_LINK=/secure/path/DeveloperIDApplication.p12
export CSC_KEY_PASSWORD='CERTIFICATE_PASSWORD'
export APPLE_API_KEY=/secure/path/AuthKey_KEY_ID.p8
export APPLE_API_KEY_ID='KEY_ID'
export APPLE_API_ISSUER='ISSUER_UUID'
```

`CSC_LINK` 也可以使用 CI Secret 注入的 base64 或 HTTPS 地址。公证凭据按 electron-builder 当前支持的 Apple API Key 变量提供。

## 构建

```bash
export MATOU_UPDATE_BASE_URL='https://updates.example.com/matou/stable'
export MATOU_MAC_ARCH='arm64' # Intel 发布机使用 x64
pnpm package:mac
```

arm64 与 x64 分别在对应架构的 macOS 发布机上构建，避免 Runtime 中的原生 PTY 依赖带入错误架构。CI 以 `MATOU_MAC_ARCH=arm64` 和 `MATOU_MAC_ARCH=x64` 两个任务产出独立安装包；每个产物上传前都需验证应用主程序和 PTY 原生模块架构。

本机只验证包结构、跳过签名发现时：

```bash
export MATOU_UPDATE_BASE_URL='https://updates.example.invalid/matou/stable'
export CSC_IDENTITY_AUTO_DISCOVERY=false
pnpm build
node tooling/prepare-package-resources.mjs
pnpm exec electron-builder --projectDir apps/desktop --mac dmg zip --arm64 --publish never -c.mac.hardenedRuntime=false
```

未签名产物仅用于界面和清单验证，不进入正式更新目录。

## 上传顺序

发布必须保持“清单最后可见”：

1. 上传 ZIP、DMG 及所有 blockmap 到临时路径。
2. 校验文件 SHA-512、大小和 HTTPS Range 下载。
3. 将二进制及 blockmap 移入 stable 目录。
4. 最后原子替换 `stable-mac.yml`。
5. 从外网读取清单中的每个 URL，并校验文件存在及长度一致。

清单提前发布会让客户端看到一个尚未完整上传的版本，从而产生下载失败。

## Nginx 最小配置

```nginx
location /matou/stable/ {
    alias /srv/matou-updates/stable/;
    add_header Accept-Ranges bytes always;
    types {
        application/octet-stream zip blockmap;
        application/x-apple-diskimage dmg;
        text/yaml yml;
    }
}

location ~ ^/matou/stable/.*\.yml$ {
    alias /srv/matou-updates/stable/$uri;
    add_header Cache-Control "no-cache, must-revalidate" always;
}
```

实际部署时按服务器目录调整 `alias`，并用 `curl -I` 与 Range 请求验证响应头。

## 客户端验收

1. 安装版本 N，并确认工作区中存在一个运行会话和一个空闲会话。
2. 在服务器发布版本 N+1。
3. 客户端手动检查更新，确认只出现轻量更新入口。
4. 开始后台下载，关闭浮层，确认终端输入和下载均继续。
5. 下载完成后选择“空闲后自动更新”，确认运行会话结束前不重启。
6. 会话转为空闲后确认只重启一次，并恢复原工作区和画布。
7. 确认新版本首次启动只显示一次更新完成提示。

## 回退

发现新版本异常时，先把更新清单原子恢复到上一稳定版本，阻止更多客户端发现异常版本。已经安装的客户端通过发布更高 SemVer 的修复版本前进修复；stable 通道不使用版本降级。
