# 多 Profile 沙盒浏览器 V1 开发规划

Nov 23, 2026 · @Jake

## 一、原方案评审

原方案的大方向是对的：独立 Chromium 进程、单 Mihomo 内核、Rust 控制、不关沙盒、先跑通单 Profile 闭环。这些全部保留。需要修正的是下面 8 处，它们会在实现阶段直接变成 bug 或返工。

| # | 原方案说法 | 问题 | 调整 |
| --- | --- | --- | --- |
| 1 | 每个 Profile「独立 DNS」 | Mihomo 的 DNS 是全局配置，listener 不能各配一套 DNS | 浏览器走代理时域名交给远端节点解析（HTTP CONNECT / socks5h），本地不做解析；用泄漏测试验证，而不是靠配置声明 |
| 2 | 新增 Profile 就加一个 listener | 改 listeners 需要重载配置，可能打断所有正在运行 Profile 的连接 | 改为「固定槽位池」：启动时预建 N 个 listener + N 个 selector，Profile 运行时占用槽位，只改 selector，不重载配置（见第四节） |
| 3 | 端口固定 7901–7904 | 端口可能被占用；且 Profile 数量和端口绑死 | 端口在启动时探测分配，Profile 不记端口，只记节点 |
| 4 | 切换节点只需更新 selector | 已建立的长连接仍走旧节点，出口会混用 | 切换后按 listener 关闭存量连接，再重新校验出口 IP |
| 5 | 时区/语言/地理位置全靠 CDP 注入 | 漏掉一个 target（Worker、弹窗、预渲染页）就不一致 | 能用进程级配置的先用进程级（`TZ` 环境变量、`--lang`、`--accept-lang`、Preferences），CDP 只做补充 |
| 6 | 未提 WebRTC | WebRTC 可绕过 HTTP 代理暴露真实 IP，这是最常见的泄漏 | 启动参数强制 `--force-webrtc-ip-handling-policy=disable_non_proxied_udp` |
| 7 | `--remote-debugging-pipe` 直接可用 | macOS 上走 fd 3/4：Rust 用 pre\_exec 把两根管道 dup2 到 fd 3/4 即可，难度不大 | 列入 M0 技术验证，确认读写与退出行为 |
| 8 | 测试规模「9 个标签页」 | 1/3/5 个 Profile × 3 标签页应为 3/9/15 个，9 只是中间档 | 测试矩阵重写为 3 档并发 × 3 类页面（见第九节） |

另有两处缺口：原方案没有说明节点从哪来（订阅导入 / 手动添加），也没有定义性能指标的具体数值和测量口径。这两部分在第四节和第九节补上。

## 二、V1 目标与非目标

V1 只解决一件事：最多 5 个 Profile 同时运行，每个 Profile 的存储完全隔离、出口 IP 经过校验、时区/语言/位置与出口一致，且没有网络泄漏。

**目标**

1. Profile 创建、编辑、删除、复制；存储数量不限，并发上限 5。
2. 节点管理：导入 Clash/Mihomo 订阅或手动添加节点，Profile 绑定一个固定节点。
3. 启动前校验出口 IP，并按出口地区自动推导时区、语言、坐标（可手动覆盖）。
4. 出口不可用时「失败即断网」（fail-closed），绝不回落直连。
5. 运行中可显式切换节点，切换后自动重校验。
6. 资源监控：每个 Profile 的内存、CPU、启动耗时可查。

**非目标（V1 不做）**

- 深度指纹修改（Canvas、WebGL、字体、硬件参数）：V1 只保证「环境一致」，不做「指纹伪装」。
- 自动化脚本 / 对外 API：预留接口，M4 再做。
- 扩展管理、团队协作、云同步。
- 其他平台：只做原生 macOS 15 及以上、Apple Silicon（arm64），不做 Intel Mac、Windows、Linux，也不做通用二进制。

## 三、总体架构与进程模型

常驻的只有 Rust 核心（托盘）。WebView 只在窗口打开时存在；Mihomo 在第一个 Profile 启动时拉起；Chromium 按需启动，最多 5 个，都是管理器的子进程。

&#91;embedded content: 进程架构 · 1 个管理器、1 个 Mihomo、最多 5 个 Chromium\]

所有控制都从 Rust 核心发出；浏览器与 Mihomo 之间只有代理流量，没有控制通道。关闭窗口即销毁 WebView，只留 Rust 核心在托盘运行。

项目结构沿用原方案的目录划分，增加两处：`src-tauri/src/supervisor/`（进程守护与孤儿清理），以及 `tests/fixtures/`（本地测试页面、IP 回显服务、DNS 记录服务）。

## 四、代理层：Mihomo 固定槽位池

核心改动：把「Profile ↔ 端口」的固定绑定，改成「运行中的 Profile ↔ 槽位」的动态绑定。Mihomo 启动后配置不再变，所有变动都通过 External Controller 改 selector 完成。

### 4.1 配置模板

Rust 在启动 Mihomo 前生成配置：探测一段可用端口，建 5 个槽位（与并发上限一致），每个槽位一个 listener + 一个 selector。

```yaml
external-controller-unix: <应用数据目录>/mihomo.sock  # 版本不支持时改用 127.0.0.1:<随机端口> + secret
mode: rule
log-level: warning
geo-auto-update: false
dns: { enable: false }              # 域名交给远端节点解析，本地不解析
tun: { enable: false }
sniffer: { enable: false }
profile: { store-selected: false }  # 选择状态由 Rust 管理

proxy-providers:
  sub-main: { type: file, path: ./providers/sub-main.yaml }

proxy-groups:
  - name: SLOT_1
    type: select
    use: [sub-main]
    proxies: [REJECT]   # 默认指向 REJECT：未分配的槽位不通网
  # SLOT_2 ~ SLOT_5 同上

listeners:
  - { name: slot-1, type: mixed, listen: 127.0.0.1, port: 17901, proxy: SLOT_1 }
  # slot-2 ~ slot-5 同上，端口由探测结果决定

rules:
  - MATCH,REJECT        # 只有槽位入口能出网；不加载 GeoIP/GeoSite 规则集，省内存
```

要点：槽位默认指向 REJECT，释放槽位时也切回 REJECT。这样任何时候「没有明确分配的入口」都不通网，天然 fail-closed。

### 4.2 启动一个 Profile 的代理步骤

1. 从空闲槽位中取一个（内存中的槽位表 + 互斥锁）。
2. `PUT /proxies/SLOT_n`，把 selector 指向该 Profile 绑定的节点。
3. 从 Rust 用 reqwest 经该槽位端口请求自建的 IP 回显接口，拿到出口 IP。
4. 用本地 GeoIP 库（如 MaxMind GeoLite2）查出口地区，推导时区/语言/坐标；与 Profile 上次记录不一致时提示用户。
5. 校验通过后才启动浏览器。任一步失败：槽位切回 REJECT、释放，Profile 进入 Error。

### 4.3 运行中切换节点

1. `PUT /proxies/SLOT_n` 改为新节点。
2. `GET /connections`，按入站名过滤出属于 `slot-n` 的连接，逐个 `DELETE /connections/{id}`。入站字段名（如 `metadata.inboundName`）需在 M0 按所用 Mihomo 版本确认。
3. 重新校验出口；地区变化时弹窗询问是否同步更新时区/语言（需要重启浏览器才能完全生效）。

### 4.4 Mihomo 进程管理

- 按需启动：第一个 Profile 启动时才拉起 Mihomo，最后一个 Profile 停止 5 分钟后退出。没有 Profile 运行时不常驻。
- 单进程是单点故障：崩溃后所有 Profile 断网（但不会泄漏，因为浏览器配置了显式代理，不会回落直连）。
- 守护：用 kqueue 监听进程退出，崩溃立即感知，不靠轮询；另每 30 秒请求一次 `GET /version` 检测卡死。进程退出或连续 2 次探测失败即重启，用同一份配置和端口，按槽位表恢复 selector，并重新校验每个运行中 Profile 的出口。
- 订阅更新只刷新 proxy-provider（`PUT /providers/proxies/{name}`），不重载整个配置。更新后如果某个运行中 Profile 的节点消失，该槽位切回 REJECT 并告警。

## 五、浏览器启动与环境一致性

原则：能在进程启动时定下来的，就不靠 CDP 逐页注入。进程级配置对所有标签页、Worker、弹窗天然生效，CDP 只补进程级做不到的部分。

### 5.1 启动参数

```bash
TZ=Asia/Tokyo \
"<runtime>/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing" \
  --user-data-dir="<profiles>/<profile-id>" \
  --proxy-server="http://127.0.0.1:<槽位端口>" \
  --proxy-bypass-list="<-loopback>" \
  --force-webrtc-ip-handling-policy=disable_non_proxied_udp \
  --lang=ja-JP \
  --accept-lang=ja-JP,ja \
  --disk-cache-size=524288000 \
  --disable-features=Translate,MediaRouter,OptimizationHints \
  --no-first-run \
  --no-default-browser-check \
  --remote-debugging-pipe
```

- `<-loopback>`：让本机地址也走代理（即被拒绝），避免网页探测本机服务。若影响实际使用，可去掉。
- `disable_non_proxied_udp`：WebRTC 不走代理的 UDP 全部禁止。HTTP 代理下 QUIC 也会自动关闭。
- `TZ` 环境变量：在 macOS 上对整个浏览器进程树生效；M0 用测试页确认 Worker 内也一致。
- 首次创建 Profile 时，预写 `Default/Preferences`：`intl.accept_languages`、关闭启动时恢复会话、关闭密码管理器的自动保存提示等。
- 不使用 `--no-sandbox`、`--single-process`、`--disable-site-isolation-trials`（同原方案）。

### 5.2 CDP 补充（浏览器级会话）

1. `Target.setDiscoverTargets({discover: true})`。
2. `Target.setAutoAttach({autoAttach: true, waitForDebuggerOnStart: true, flatten: true})`：新 target 先暂停，等设置完再放行，避免页面脚本在覆盖前运行。
3. 每个新 attach 的 session：按 target 类型设置 `Emulation.setTimezoneOverride`（仅当 `TZ` 不可用）、`Emulation.setLocaleOverride`、`Emulation.setGeolocationOverride`，然后 `Runtime.runIfWaitingForDebugger`。
4. 地理位置权限：默认保持「询问」，由用户在网页上自己授权；不统一自动授权。
5. Service Worker / Shared Worker 是否支持上述 Emulation 指令，在 M0 用测试页实测。

### 5.3 一致性自检

浏览器启动后，由 Rust 在一个隐藏标签页中执行自检脚本，比对以下值与 Profile 配置是否一致，不一致则告警：

- `Intl.DateTimeFormat().resolvedOptions().timeZone`（主页面 + 一个 Worker 内）
- `navigator.language` / `navigator.languages`、请求头 `Accept-Language`
- 自建回显接口看到的出口 IP

### 5.4 Chromium 运行时

- 推荐 Chrome for Testing 的 mac-arm64 构建（自带签名）或自编译 Chromium，按版本号分目录存放；安装包不内置，首次启动下载并校验哈希。
- 每个 Profile 记录最后使用的 Chromium 版本。新版本创建的 user-data-dir 用旧版本打开可能损坏，禁止降级。
- 至少每月跟进一次 Chromium 安全更新；每次启动前用 codesign --verify 校验签名。

## 六、Profile 生命周期与故障处理

每个 Profile 由 Rust 中的一个状态机驱动，所有状态变化通过事件推给前端。规则只有一条：离开 Running 的任何路径，都先把槽位切回 REJECT。

&#91;embedded content: Profile 状态机 · 7 个状态\]

各状态均设超时：Verifying 一般 10 秒，Launching 20 秒，超时即进入 Error。

| 故障 | 检测方式 | 处理 |
| --- | --- | --- |
| 出口 IP 地区与上次不同 | Verifying 比对 last\_exit\_ip | 暂停在 Verifying，弹窗让用户选择继续 / 更新环境 / 取消 |
| Chromium 崩溃或被用户关闭 | 子进程退出 + CDP 管道断开 | 进入 Stopping，记录 end\_reason；不自动重启 |
| user-data-dir 已被锁 | 启动前检查 SingletonLock 及对应 PID | PID 不存在则清锁；存在则报错，提示用户关闭 |
| Mihomo 崩溃 | 探活失败 | 重启 Mihomo，按槽位表恢复 selector，所有 Running 的 Profile 重新校验出口 |
| 管理器崩溃 | 下次启动发现孤儿进程 | 按记录的 PID 清理（做法见第十二节） |

## 七、数据模型与存储

SQLite（WAL）只存元数据和指标；浏览器数据留在各自的 user-data-dir；密码、订阅 URL 等敏感值存系统钥匙串（Rust `keyring` crate），数据库只存引用。

| 表 | 关键字段 | 说明 |
| --- | --- | --- |
| profiles | id, name, data\_dir, node\_id, env\_mode(auto/manual), timezone, locale, lat, lng, chromium\_version, status, last\_exit\_ip, last\_used\_at | status 只存持久状态（Stopped/Error），运行态在内存 |
| subscriptions | id, name, secret\_ref, updated\_at, node\_count | secret\_ref 指向钥匙串中的订阅 URL |
| nodes | id, subscription\_id, name, type, region\_hint, last\_check\_ok | 手动节点 subscription\_id 为空 |
| launches | id, profile\_id, started\_at, ready\_ms, exit\_ip, exit\_country, slot, end\_reason | 每次启动一行，用于 P50/P95 统计 |
| metrics | launch\_id, ts, rss\_mb, cpu\_pct, tab\_count | 一般 10 秒一个点，批量写入；保留 30 天 |
| events | id, profile\_id, ts, level, kind, detail | 启动失败、出口不一致、崩溃等 |

其他约定：

- 数据库迁移用 `sqlx` migrations 或 `refinery`，版本号随应用发布。
- 生成的 Mihomo 配置和 provider 文件放在应用数据目录，文件权限仅当前用户可读。
- 删除 Profile = 先移入回收目录，7 天后或用户确认后再真正删除 user-data-dir。

## 八、安全基线

安全基线的重点是「本机其他进程不能控制或借用本应用的能力」。以下每条都是 V1 必须项。

| 面 | 风险 | 措施 |
| --- | --- | --- |
| CDP | 调试端口可被本机任意进程连接，等于完全控制浏览器 | 用 `--remote-debugging-pipe`；备选方案若用端口，必须是 `--remote-debugging-port=0` + 读 `DevToolsActivePort`，并仅在启动窗口期间存在 |
| External Controller | 可任意切换节点、读取连接 | 仅绑 127.0.0.1、随机端口、每次启动随机 secret；优先用 Unix socket 控制器（external-controller-unix，需确认所用版本支持），socket 放在应用数据目录内 |
| 槽位 listener | 本机其他程序可借用代理端口 | 未分配槽位指向 REJECT；V1 接受已分配槽位无认证的风险，V2 考虑 listener `users` 认证 + CDP `Fetch.authRequired` 自动应答 |
| Tauri IPC | 前端被注入后可调用任意命令 | Tauri v2 capabilities 白名单、严格 CSP、不加载远程页面；命令参数在 Rust 侧校验（如 data\_dir 必须在 profiles 根目录内） |
| 凭据 | 订阅 URL、节点密码明文落盘 | 系统钥匙串；日志中脱敏 |
| 下载的运行时 | Chromium / Mihomo / GeoIP 库被篡改 | 固定下载源 + SHA-256 校验；Mihomo（darwin-arm64）作为 Tauri sidecar 随应用一起签名、公证 |
| 自动化 API（M4） | 本机接口被滥用 | 默认关闭；开启时仅 127.0.0.1 + 随机 token，界面上常驻提示 |

## 九、性能与隔离验收标准

验收分两类：隔离与泄漏是硬性门槛，一项不过就不发布；性能指标先用 M0 实测基线校准，下面是初始目标。

### 9.1 测试矩阵

每个 Profile 开 3 个标签页，并发档位 × 页面类型共 9 组：

| 并发 Profile | 标签页总数 | 页面类型 |
| --- | --- | --- |
| 1 | 3 | 轻量 / 普通 / 重型 各跑一轮 |
| 3 | 9 | 同上 |
| 5 | 15 | 同上 |

测试页面用本地自建的固定页面（静态页、模拟管理后台、嵌入视频的页面），不用外部网站，保证每次结果可比。每组至少 20 次启动取 P50/P95，稳定运行 10 分钟后取内存。

### 9.2 测量口径

- **启动耗时**：从点击「启动」到首个标签页 `Page.loadEventFired`，拆成「槽位分配 + 出口校验」和「浏览器启动 + 首页加载」两段记录。
- **冷启动**：重启系统或清空系统文件缓存后的首次启动，单独统计。
- **内存**：按 Profile 的进程树汇总，口径统一用 phys\_footprint（与活动监视器的「内存」列一致），通过 proc\_pid\_rusage 读取。不用 RSS 相加（共享内存会重复计算）。
- **CPU**：空闲 5 分钟内的平均值。

### 9.3 初始性能目标（M0 后校准）

| 指标 | 初始目标 |
| --- | --- |
| 托盘态（窗口关闭、无 Profile）总占用 | ≤ 25 MB，CPU ≈ 0 |
| 窗口打开（Rust 核心 + WebView 进程） | ≤ 150 MB，空闲 CPU < 1% |
| Mihomo（5 个槽位都在用） | ≤ 80 MB |
| 管理器冷启动到窗口可操作 | ≤ 1 s |
| 热启动 P95（单 Profile，Mihomo 已运行，含出口校验） | ≤ 基线 + 1.5 s |
| 5 Profile × 3 轻量页 总内存 | 记录基线，后续版本不得恶化 > 10% |

管理器本身的开销是我们能控制的；Chromium 的内存主要由页面决定，目标是「不增加额外开销」，而不是压缩浏览器内存。

### 9.4 隔离与泄漏（硬性门槛）

- [ ] 各 Profile 的 Cookie、localStorage、IndexedDB、缓存互不可见
- [ ] 每个 Profile 的出口 IP 等于其绑定节点，5 个并发时无串号
- [ ] WebRTC 不暴露本机公网 IP 或内网 IP
- [ ] DNS 无本地解析：用自建权威 DNS 记录来源 IP，来源必须是节点侧
- [ ] IPv6：本机有 IPv6 时也不泄漏
- [ ] Mihomo 被杀掉时，所有 Profile 断网而非直连
- [ ] 切换节点后，旧连接全部关闭，新请求全走新节点
- [ ] 时区/语言在主页面、iframe、Worker、新窗口中一致
- [ ] 管理器崩溃后，没有残留的 Chromium / Mihomo 孤儿进程

## 十一、里程碑与交付计划

按 1–2 名开发估算，M0–M3 共 8–11 周到 V1。每段必须过门槛才进入下一段，尤其不要在 M1 闭环稳定前做完整 UI。

&#91;embedded content: 里程碑 M0–M4 · 每段的交付与出口门槛\]

M0 只写一次性验证代码，结论写回本文档第十二节；M1 起按第三节的目录结构写正式代码。周数是初估，M0 结束后按实际难度重排。

## 十二、风险与已定决策

只做 macOS 15+ Apple Silicon 后，原先两项 Windows 风险基本消失。剩下的风险集中在签名公证和子进程管理；原先的待决策项已全部定下（表格最后 4 行）。

| 风险 / 决策 | 影响 / 选择 | 应对 / 说明 |
| --- | --- | --- |
| 签名与公证 | 未公证的应用或 sidecar 会被 Gatekeeper 拦截 | Developer ID 签名 + hardened runtime + notarytool 公证，Mihomo 一起签；不走 App Store、不开 App Sandbox（需要启动外部进程）；M3 之前先完整跑通一次 |
| 管理器崩溃留下孤儿进程 | macOS 没有 `PR_SET_PDEATHSIG`，子进程不会跟着退出 | 记录所有子进程 PID，管理器启动时清理；运行中用 kqueue 监听退出；管道断开后 Chromium 是否自动退出，在 M0 验证 |
| Chrome for Testing 不自动更新 | 安全补丁要靠我们跟进 | 每月按 Stable 渠道更新；备选 ungoogled-chromium 的 arm64 构建 |
| 系统权限弹窗 | 摄像头、麦克风等权限按 Chromium 应用包授权，所有 Profile 共用一次授权 | 写进使用说明；V1 不做按 Profile 区分 |
| Dock 中多个相同图标 | 5 个 Profile 都显示为同一个 Chromium 图标 | V1 用窗口标题前缀和不同主题色区分；V2 再考虑每个 Profile 一个独立应用包 |
| Mihomo 单点故障 | 崩溃时全部 Profile 断网数秒 | 守护重启 + 槽位恢复（第四节）；断网优于泄漏，可接受 |
| 代理端口无认证 | 本机其他程序可借用运行中的槽位 | V1 接受，V2 加认证 |
| **已定：系统版本** | macOS 15 起，只编译 aarch64-apple-darwin | WebView 按 Safari 18 的能力开发，不写兼容代码 |
| **已定：CDP 客户端** | 自写管道传输层（按 `\0` 分隔的 JSON 消息），协议类型复用现成 crate | 传输层代码量小，不依赖第三方 crate 是否支持管道 |
| **已定：GeoIP** | DB-IP City Lite（mmdb，免注册，CC BY 4.0）+ 按经纬度离线查时区 | 许可要求在界面注明来源；精度不够时由用户手动覆盖 |
| **已定：出口回显** | Cloudflare `cdn-cgi/trace` 为主，ipify 备用，可配置 | 两个都失败即视为校验失败；测试环境用本地回显服务 |
