# offideskOllama

Ollama 在 Linux 上的全能桌面图形化管理器（GTK4 原生版）。

> 显示名：**O-deskUI .wokk.**（中文名：**Ollama管理器WokK**）
> 二进制 / 包名：`offideskollama`

## 这是什么

offideskOllama 不是聊天工具，而是 Ollama 的本地图形控制台。模型库、生成、嵌入、运行时、服务、设置都是平级的功能模块，聊天只是其中之一。所有模型都在你本机，本应用不提供任何云端能力，也不上传你的数据。

## 功能

- **模型管理**：列表、拉取 / 推送、删除、查看体积与量化；内置种子索引 + 用户叠加层
- **流式聊天**：逐 token 渲染、思考过程可见、如实展示 token 数与速度
- **生成 / 嵌入**：单次补全、文本嵌入，本机完成
- **已加载模型**：keep-alive 倒计时、显存占用
- **系统托盘**：静默启动（`--tray`）、菜单常驻
- **云库**：检索官方库、空状态给出搜索出口
- **设置**：主题跟随系统、自启动、上下文长度等
- **首次引导 / 看门狗**：新装机引导；卡死时抓取调用栈（黑匣子）

## 技术栈

- 语言：Rust
- UI：纯 GTK4 原生（`gtk4` 0.8，启用 `v4_6`），**不依赖 libadwaita**
- 网络：`reqwest` 0.12（JSON + 流式）、`tokio`
- 其他：`pulldown-cmark`（Markdown）、`ksni`（托盘）、`chrono`、`libc` + `backtrace`（看门狗）

> 为什么不用 libadwaita：libadwaita 控件不跟随 Cinnamon / Mint-Y 系统主题，会导致界面与桌面环境脱节；纯 GTK4 由系统 GTK 主题统一绘制，原生贴合桌面。

## 目录结构

```
offideskOllama/
├── gtk-app/
│   ├── Cargo.toml
│   ├── Cargo.lock
│   ├── src/                # 全部源码
│   │   ├── main.rs         # 入口、窗口、轮询、托盘、看门狗
│   │   ├── config.rs       # 配置读写
│   │   ├── engine.rs       # Ollama 进程管理
│   │   ├── ollama/         # API：chat/pull/create/library/models/catalog...
│   │   ├── registry.rs     # 注册中心鉴权
│   │   ├── rt.rs           # 流式运行时
│   │   ├── service.rs      # 业务编排
│   │   ├── tray.rs         # 系统托盘
│   │   ├── ui/             # 界面：sidebar/overview/discover/chat/settings...
│   │   └── watch.rs        # 看门狗
│   ├── packaging/          # 桌面文件（构建用）
│   └── pkg/                # .deb 打包骨架（DEBIAN 脚本、desktop）
└── README.md
```

## 构建

依赖系统 GTK4 开发库：

```bash
# Debian / Ubuntu / Linux Mint
sudo apt install libgtk-4-dev pkg-config
```

```bash
cd gtk-app
cargo build --release
# 产物：target/release/offideskollama
```

## 运行

```bash
./target/release/offideskollama          # 正常启动
./target/release/offideskollama --tray   # 静默启动到系统托盘
```

需本机已安装并运行 Ollama（`ollama serve`）。

## 打包 .deb

`pkg/` 为 .deb 骨架（DEBIAN 脚本 + desktop）。将编译产物放入 `pkg/usr/bin/`，再执行 `dpkg-deb -b pkg offideskollama_<版本>_amd64.deb`。

## 许可证

[MIT](LICENSE)
