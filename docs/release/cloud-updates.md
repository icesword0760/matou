# Matou 云端更新发布手册

## 客户端读取哪些产物

原生客户端和正式打包脚本默认使用 `https://github.com/icesword0760/matou/releases/latest/download`。三个平台的清单和安装产物可以作为附件放在同一个 GitHub Release 中。原生 macOS 下载 DMG，同一份清单也提供 macOS 0.2.x 升级用的完整 ZIP，旧版安装前会校验应用签名。Windows 按架构下载已签名的 EXE 安装器。

| 平台 | 打包产物 | 更新清单 |
| --- | --- | --- |
| macOS arm64 / x64 | `Matou-<version>-mac-<arch>.zip`、`Matou-<version>-mac-<arch>.dmg`、`SHA256SUMS-<version>-mac-<arch>.txt` | `stable-mac.yml` |
| Windows x64 | `Matou-<version>-win-x64.exe`、可选的同名便携 ZIP、`SHA256SUMS-<version>-win-x64.txt` | `stable.yml` |
| Windows ARM64 | `Matou-<version>-win-arm64.exe`、可选的同名便携 ZIP、`SHA256SUMS-<version>-win-arm64.txt` | `stable-win-arm64.yml` |

Windows ARM64 找不到自己的清单时不会改读 x64 的 `stable.yml`。macOS 每次按单一架构构建，生成的 `stable-mac.yml` 指向该次打包的 DMG 和 ZIP。

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

## 发布到 GitHub Releases

1. 创建 Release 草稿，将三个平台的清单、安装产物和 SHA256 校验文件作为同一层级的附件上传。
2. 对照每份清单确认安装包文件名、大小和 SHA-512，再用 SHA256 校验文件核对附件。便携 ZIP 也用 SHA256 校验。
3. 确认附件齐全后发布，并将该版本设为 Latest。[GitHub Release 管理文档](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository)
4. 从外网读取默认地址下的三份清单并跟随重定向，验证清单引用的每个附件可以下载且哈希一致。

## 自定义镜像与旧版本过渡

`pnpm package:mac` 和 `pnpm package:win` 可用 `--update-base-url <URL>` 指定镜像或旧服务器，也可用 `MATOU_UPDATE_BASE_URL` 设置。Windows ARM64 打包时的覆盖优先级从高到低为 `--update-base-url`、`MATOU_WIN_ARM64_UPDATE_BASE_URL`、`MATOU_UPDATE_BASE_URL`，然后才使用 GitHub 默认地址。原生客户端启动时也可用相应环境变量覆盖随包配置。

旧版安装在升级前仍会请求原有地址与清单文件名。若通过 `https://updates.baize-ailabs.com/stable` 提供过渡版本，就在旧版请求的地址保留对应清单和安装包。旧 Windows ARM64 请求的是 `/stable/win-arm64/stable.yml`，新版 Windows ARM64 即使显式配置同一个旧目录，也会请求其中的 `stable-win-arm64.yml`。同一台服务器继续服务两代时，可同时提供这两个文件名。

自建镜像先放好安装产物和校验文件，再原子替换清单。用公开 HTTPS 提供下载，验证 `Content-Length` 和 Range 响应。安装产物可长期缓存，清单使用 `Cache-Control: no-cache` 或较短 TTL。

### 自建镜像的 Nginx 示例

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

发现异常时，可将附件齐全的上一稳定 GitHub Release 设回 Latest，并读回三份清单确认默认地址已切换。自建镜像可原子恢复上一稳定版本的清单。已升级用户通过更高版本号的修复版本恢复。
