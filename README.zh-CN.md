<p align="center"><img src="src-tauri/icons/icon.png" width="120" height="120" alt="Profile Harbor 应用图标"></p>
<h1 align="center">Profile Harbor</h1>
<p align="center">在 Mac 上同时管理五个 Chrome 配置文件，直接查看各自的出口 IP 和时区。</p>
<p align="center">
  <img src="https://img.shields.io/badge/macOS-15%2B-334155?style=flat-square" alt="macOS 15+">
  <img src="https://img.shields.io/badge/Mac-Apple_Silicon-21695E?style=flat-square" alt="Apple Silicon">
  <a href="https://github.com/ProvenceJake/profile-sandbox-browser/releases/tag/v0.9.3"><img src="https://img.shields.io/badge/prerelease-v0.9.3-526581?style=flat-square" alt="0.9.3 预发布版"></a>
</p>
<p align="center"><a href="README.md">English</a> · <strong>简体中文</strong></p>
<p align="center"><a href="https://github.com/ProvenceJake/profile-sandbox-browser/releases/download/v0.9.3/Profile-Harbor-0.9.3-mac-arm64.zip"><strong>下载 macOS 版</strong></a> · <a href="https://github.com/ProvenceJake/profile-sandbox-browser/releases">版本记录</a> · <a href="https://github.com/ProvenceJake/profile-sandbox-browser/issues">反馈问题</a></p>

---

每个配置文件使用独立的 Chrome 进程、数据目录和代理连接。停止后，列表会保留上次校验的出口 IP。管理器使用 Rust 和 Tauri 2 开发，管理界面为中文，使用本地 WebView。

## 安装与开始使用

从 [macOS 预发布版本](https://github.com/ProvenceJake/profile-sandbox-browser/releases/tag/v0.9.3) 下载 ZIP，解压后打开 Profile Harbor.app。此版本使用临时签名，尚未获得 Developer ID 签名或 Apple 公证。

1. 在「节点与订阅」添加 HTTP/SOCKS5 节点，或导入含 proxies 列表的 Clash/Mihomo YAML 文件或订阅 URL。单个节点的其他受支持协议也可用 YAML 导入。来源中的名称用于在刷新后匹配节点。
2. 在「配置文件」创建工作区并绑定节点。自动模式首次使用时下载 DB-IP 离线城市库，手动模式可按中文名称选择语言和城市时区，预览当地时间，也支持自定义语言标签、时区和坐标。浏览器与 Mac 共用实际时钟，网页按 Profile 时区显示当地时间。
3. 点击「启动」。管理器先经绑定节点验证出口，再从 Chrome 检查出口、请求头，以及页面和 Worker 的时区、首选语言、默认日期与数字格式。浏览器首页显示 Profile 名称、语言、时区与当地时间。Chrome 内置界面使用其支持的对应语言。网页地理位置权限继续由用户逐站授权。
4. 首页逐项显示出口 IP 和时区，停止后标出上次出口，首次启动前显示待校验。运行时可停止浏览器，或在「更多操作」中切换节点。出口地区变化时可更新环境、保留原环境或取消。切换时关闭此 Profile 的旧网络连接，更改时区或语言会重新启动该浏览器。
5. 关闭管理窗口后可从菜单栏重新打开，或选择停止所有 Profile 并退出。运行记录支持查看事件、启动耗时、P50/P95 和按 Profile 筛选的资源采样。

## 存储与清理

- Chrome 默认复用本机已安装的 Google Chrome。「运行时」支持下载安装固定版官方 Chrome，并切回系统 Chrome。下载内容校验 SHA-256，浏览器启动前验证 Google 签名并检查版本降级。
- 「复制配置」创建一个使用相同节点和环境的空白浏览器工作区。删除的 Profile 进入回收目录，可从「运行时」恢复或立即清除。管理器运行期间定期清理已保留七天的数据。
- 元数据和资源采样保存在应用私有目录的 SQLite WAL 数据库，采样保留 30 天。订阅 URL 和节点配置保存在 macOS 钥匙串。Mihomo 工作期间使用私有的临时 provider 文件，内核关闭时清理。
- 窗口关闭后销毁 WebView。最后一个 Profile 停止五分钟后回收代理内核。每个运行槽位有单独的浏览器代理入口，代理关闭或重新校验时切断旧连接。菜单栏管理器异常退出后，CDP 管道和小型原生守护进程分别负责浏览器和代理内核的退出。
- 各 Profile 共用 Mac 的系统字体和实际时钟。macOS 摄像头和麦克风等系统许可按 Chrome 应用包管理。已分配的本机代理入口仅监听回环地址，当前版本没有对其他本机进程实施入口认证。

## 构建与验证

构建需要 Apple Silicon Mac、Xcode 命令行工具和 Rust stable。脚本获取哈希固定的官方 Mihomo，输出应用包与可上传的 ZIP。

```sh
scripts/package.sh
```

本机集成回归需要 Node.js 22+、可用的登录钥匙串和已安装的 Google Chrome。临时 Profile、数据库和节点只用于测试。以下命令在完成打包后运行。

```sh
HARBOR_TEST_ENGINE="$PWD/dist/Profile Harbor.app/Contents/MacOS/mihomo" node tests/desktop.mjs
MIHOMO_BIN="$PWD/dist/Profile Harbor.app/Contents/MacOS/mihomo" node probes/m0.mjs
```

发布者可为打包脚本设置 MACOS_SIGN_IDENTITY 和 NOTARY_PROFILE，分别用于 Developer ID 签名和 notarytool 凭据配置。构建工作目录为项目内的 .build，发布输出位于 dist，均不进入 Git。

[第三方组件与数据来源](resources/licenses/Attribution.txt) · [设计规划](docs/plan-v1.md) · [首轮 M0 记录](docs/m0.md) · [桌面版本验收记录](docs/desktop-0.9.md)
