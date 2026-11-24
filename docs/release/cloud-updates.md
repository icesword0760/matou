# Matou 云端更新发布手册

## 发布模型

原生 Matou 读取打包时写入的 generic HTTPS 更新地址。macOS 使用 `stable-mac.yml` 发现更新并下载 DMG。Electron 0.2.x 可读取同一份清单，选择 ZIP 并验证应用签名后安装。Windows 使用 `stable.yml` 查找 EXE 安装器。

macOS 产物包括

- `Matou-<version>-mac-arm64.dmg`
- `Matou-<version>-mac-arm64.zip`
- ZIP 和 DMG 各自的 `.blockmap`，本机有可用的 `app-builder` 时生成
- `stable-mac.yml`
- `SHA256SUMS-<version>-mac-arm64.txt`

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

每次发布前提升 `apps/desktop/package.json` 的 `version`，同步四个原生 workspace 包和 `apps/native/Cargo.lock`。

macOS 发布机的钥匙串须具备 `build.mac.identity` 指定的 Developer ID Application 证书及其私钥。正式包保留 `com.matou.desktop`、Team `JAKX72KQ6X` 和固定的 designated requirement，并使用 Apple 安全时间戳。原生流水线无 Apple 公证步骤。本地替换前先退出旧应用，再用 `pnpm install:mac --source <绝对路径.app>`。

## 构建

```bash
pnpm package:mac
pnpm package:win
```

产物默认保存在 `apps/native/release/`。每个架构使用独立输出目录，可用 `--out-dir` 指定。macOS 默认 arm64，Intel 使用 `--arch x64`，并准备对应架构的 Claude accounts 和 model-router。资源默认从 `apps/desktop/package-resources` 读取。两平台均支持使用预编译的 `--desktop-bin` 和 `--runtime-bin`。

Windows ZIP 解压后可直接运行。要让 Windows 更新器打开安装器，使用 `--installer <文件.exe>` 生成指向现有 EXE 的 `stable.yml`。默认生成的 ZIP 清单用于手动分发。旧 Electron 打包入口为 `pnpm package:mac:legacy`。

## 上传顺序

发布必须保持“清单最后可见”：

1. 上传清单引用的产物及已生成的 blockmap 到临时路径。
2. 校验文件 SHA-512、大小和 HTTPS Range 下载。
3. 将二进制及 blockmap 移入 stable 目录。
4. 最后原子替换 `stable-mac.yml` 或 `stable.yml`。
5. 从外网读取清单中的每个 URL，并校验文件存在及长度一致。

清单提前发布会让客户端看到一个尚未完整上传的版本，从而产生下载失败。

## Nginx 最小配置

```nginx
location /matou/stable/ {
    alias /srv/matou-updates/stable/;
    add_header Accept-Ranges bytes always;
    types {
        application/octet-stream zip exe blockmap;
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

## Electron 0.2.x 跨代更新验收

1. 安装版本 N，并确认工作区中存在一个运行会话和一个空闲会话。
2. 在服务器发布版本 N+1。
3. 客户端手动检查更新，确认只出现轻量更新入口。
4. 开始后台下载，关闭浮层，确认终端输入和下载均继续。
5. 下载完成后选择“空闲后自动更新”，确认运行会话结束前不重启。
6. 会话转为空闲后确认只重启一次，并恢复原工作区和画布。
7. 确认新版本首次启动只显示一次更新完成提示。

## 回退

发现新版本异常时，先把更新清单原子恢复到上一稳定版本，阻止更多客户端发现异常版本。已经安装的客户端通过发布更高 SemVer 的修复版本前进修复；stable 通道不使用版本降级。
