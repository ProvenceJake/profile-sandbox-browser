# 多 Profile 沙盒浏览器

面向 macOS 15+ 和 Apple Silicon。产品设计采用 Rust 管理器、单个 Mihomo 内核、五个代理槽位和独立的 Chromium 用户数据目录。

当前预发布版本 `v0.1.0-m0.1` 提供可复跑的本机技术验证。桌面管理器和安装包仍在规划中。[开发规划](docs/plan-v1.md) · [实测结果与剩余验收项](docs/m0.md)

## 运行本机验证

需要 Node.js 22 或以上、Chrome 或 Chrome for Testing，以及 Mihomo。脚本默认使用「应用程序」中的 Google Chrome 和 Clash Verge 随附的稳定版 Mihomo。其他安装位置可用 `CHROME_BIN` 和 `MIHOMO_BIN` 指向对应的可执行文件。

```sh
node probes/m0.mjs
```

一次运行会建立五个本地模拟节点、独立的 Mihomo 进程和五个无头 Chrome Profile。测试使用同一网页来源和不同的存储标记，检查路由、长连接切换、存储隔离、地区环境和代理中断行为。断言失败时命令返回非零退出码。

## 磁盘与版本

验证脚本仅使用 Node 标准库并复用已安装的运行时。每次运行在系统临时目录中创建独立工作目录，正常完成、断言失败或收到中断请求后都会关闭测试进程并移除临时 Profile、代理配置和缓存。若整机断电或脚本自身被强制杀死，可能留下本次临时目录。

Git 仓库保存源码、测试页面和文档。阶段版本通过带注释的 Git 标签和 GitHub 预发布记录定位。
