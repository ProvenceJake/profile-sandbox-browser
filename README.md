# Profile Harbor

适用于 macOS 15 及以上、Apple Silicon 的多 Profile 浏览器管理器。每个 Profile 使用独立的 Chrome 进程和数据目录，最多五个同时运行。管理器由 Rust 和 Tauri 2 实现，界面为本地 WebView。

## 安装与开始使用

从私有仓库的 [macOS 预发布版本](https://github.com/ProvenceJake/profile-sandbox-browser/releases/tag/v0.9.1) 下载 ZIP，解压后打开 Profile Harbor.app。本版本使用本机临时签名，Developer ID 签名及 Apple 公证仍待完成。

1. 在「节点与订阅」添加 HTTP/SOCKS5 节点，或导入含 proxies 列表的 Clash/Mihomo YAML 文件或订阅 URL。单个节点的其他受支持协议也可用 YAML 导入。来源中的名称用于在刷新后匹配节点。
2. 在「浏览器 Profile」创建工作区并绑定节点。自动模式首次使用时下载 DB-IP 离线城市库，手动模式可指定时区、语言和坐标。Profile 时区决定本地时间的显示方式；浏览器与 Mac 共用实际时钟。
3. 点击「启动」。管理器先经绑定节点验证出口，再从 Chrome 检查出口、请求头，以及页面和 Worker 的时区、首选语言、默认日期与数字格式。浏览器首页显示 Profile 名称。网页地理位置权限继续由用户逐站授权。
4. 在运行卡片上查看资源、停止或切换节点。出口地区变化时可更新环境、保留原环境或取消。切换时关闭此 Profile 的旧网络连接，更改时区或语言会重新启动该浏览器。
5. 关闭管理窗口后可从菜单栏重新打开，或选择停止所有 Profile 并退出。运行记录支持查看事件、启动耗时、P50/P95 和按 Profile 筛选的资源采样。

## 存储与清理

- Chrome 默认复用本机已安装的 Google Chrome。「运行时」支持下载安装固定版官方 Chrome，并切回系统 Chrome。下载内容校验 SHA-256，浏览器启动前验证 Google 签名并检查版本降级。
- 「复制配置」创建一个使用相同节点和环境的空白浏览器工作区。删除的 Profile 进入回收目录，可从「运行时」恢复或立即清除。管理器运行期间定期清理已保留七天的数据。
- 元数据和资源采样保存在应用私有目录的 SQLite WAL 数据库，采样保留 30 天。订阅 URL 和节点配置保存在 macOS 钥匙串。Mihomo 工作期间使用私有的临时 provider 文件，内核关闭时清理。
- 窗口关闭后销毁 WebView。最后一个 Profile 停止五分钟后回收代理内核。每个运行槽位有单独的浏览器代理入口，代理关闭或重新校验时切断旧连接。菜单栏管理器异常退出后，CDP 管道和小型原生守护进程分别负责浏览器和代理内核的退出。
- 浏览器数据目录隔离以外，macOS 摄像头和麦克风等系统许可按 Chrome 应用包管理。已分配的本机代理入口仅监听回环地址，当前版本没有对其他本机进程实施入口认证。

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

[设计规划](docs/plan-v1.md) · [首轮 M0 记录](docs/m0.md) · [桌面版本验收记录](docs/desktop-0.9.md)
