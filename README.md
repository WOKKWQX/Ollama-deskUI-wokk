# offideskOllama

## 最新正式版：V4.0.0（GTK4 原生版）

基于 **GTK4**（纯原生，不依赖 libadwaita）的 Ollama 图形控制台，系统原生 UI，自动适配 **Cinnamon / GNOME / Ubuntu** 深浅主题。

- **模型管理**：列表（参数量/大小）、搜索过滤、详情、复制、删除、拉取、推送、Modelfile 创建（右键菜单）
- **云库**：内置 458 条真实模型目录（243 个官方家族 × 参数尺寸档，体积以 registry 实测为准）+ 白盒评分推荐（任务/硬件适配/官方真实热度三维）+ 官方库在线检索
- **聊天**：多轮流式对话 + 参数（system 提示）
- **单次生成 / 已加载模型（可停止释放显存）/ 文本嵌入**
- **系统托盘**：关闭窗口最小化到托盘，托盘「退出」才真正终止（XDG StatusNotifierItem）
- **顶栏**：Ollama 启停按钮、连接状态灯 + 版本号、设置（服务地址/主题/托盘开关）、更新日志
- **首次使用引导**：分步介绍原理 / Ollama 逻辑 / 技术层 / 界面导览
- **UI**：GTK4 原生控件，贴合 Cinnamon / GNOME 系统 UI

> GTK4 版安装：`sudo dpkg -i offideskollama_4.0.0_amd64.deb`（依赖 `libgtk-4-1`）

---

下面为历史 **Tauri 2 (Rust) + Vue 3** 版（0.5.0）的说明（功能完整，界面为自绘 WebView，供参考）。

基于 **Tauri 2 (Rust) + Vue 3** 的 Linux Ollama 图形化控制台，打包为 `.deb`。

## 特性

- **模型管理**：列表（参数量 / 大小 / 修改时间）、搜索、详情（`ollama show`）、复制（`cp`）、删除（`rm`）
- **拉取 / 推送**：实时进度条（`total` / `completed`），支持 `ollama pull` / `push`
- **Modelfile 创建**：在界面直接编辑 Modelfile 创建模型（`ollama create`）
- **流式聊天**：Markdown 渲染、代码高亮、复制按钮，参数面板（system / temperature / top_p / top_k / num_ctx / seed）
- **单次生成**：非对话补全（`ollama generate`），适合测试
- **已加载模型面板**：`ollama ps`，显示显存占用，可停止释放内存（`stop`）
- **文本嵌入**：`ollama embed`，显示向量维度与样本
- **实时状态**：顶栏连接状态灯 + Ollama 版本号；断线提示引导，重试自动恢复；已加载模型每 8s 自动刷新
- **首次使用引导**：首次打开的分步教程（功能原理 / Ollama 使用逻辑 / 技术层 / 界面导览），可跳过或分步浏览
- **简约 UI**：单窗口，零 UI 框架依赖，**自动跟随系统深浅主题**（兼容 GNOME / Cinnamon 的 prefers-color-scheme）

## 环境要求

| 依赖 | 说明 |
|------|------|
| Node.js ≥ 20 | 前端构建 |
| Rust stable | Tauri 后端编译 |
| Tauri 系统库 | `libwebkit2gtk-4.1-dev` 等（下方安装命令） |
| Ollama | 运行于 `localhost:11434`（应用会检测，无硬依赖） |

## 安装系统依赖（Ubuntu / Linux Mint）

```bash
sudo apt install -y \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
  build-essential pkg-config libssl-dev
```

## 安装 Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

## 安装前端依赖与 Tauri CLI

```bash
npm install
# 可选：全局安装 tauri-cli（或使用 npm 依赖中的 @tauri-apps/cli，通过 npm run tauri 调用）
cargo install tauri-cli --locked
```

## 开发运行

```bash
npm run tauri dev
```

首次编译会拉取并编译全部 Rust 依赖，约 5–15 分钟。

## 构建 .deb 安装包

```bash
npm run tauri build -- --bundles deb
```

产物位于：

```
src-tauri/target/release/bundle/deb/ollama-deskui_*.deb
```

## 安装 .deb

```bash
sudo dpkg -i src-tauri/target/release/bundle/deb/ollama-deskui_*.deb
```

安装后可从应用菜单启动 **Ollama DeskUI**。

## 使用指南

1. **连接**：应用启动自动检测 `localhost:11434`。顶栏绿灯表示已连接并显示版本号；红灯则表示 Ollama 未运行，请先执行 `ollama serve`。
2. **拉取模型**：左侧「＋ 拉取」，输入如 `llama3.2:1b`。
3. **聊天**：点选左侧模型，输入消息（Enter 发送 / Shift+Enter 换行），展开「参数设置」可调 system / temperature 等。
4. **查看已加载**：切到「已加载」Tab，可停止正在占内存的模型。
5. **模型操作**：列表项悬停显示 ⓘ(详情) ⧉(复制) ⇪(推送) ✕(删除)。

## 项目结构

```
src/                      Vue3 前端
├── api/ollama.ts         Tauri invoke 封装 + 事件监听
├── stores/               models / chat / ui 状态
├── components/           布局 + 4 个 Tab + 6 个 Modal + ChatBubble
├── styles/theme.css      深色主题
└── utils/markdown.ts     markdown-it + highlight.js
src-tauri/                Rust 后端
├── src/ollama/           models / pull / create / chat 模块
├── src/commands.rs       Tauri command 层
└── tauri.conf.json       窗口 + deb bundle 配置
```

## 架构说明

- **通信**：前端经 `invoke` 调用 Rust command；流式输出（聊天 / 拉取 / 推送 / 创建）由 Rust 解析 NDJSON 后经 Tauri `event` 逐段推送前端。
- **原因**：避免浏览器直接访问 Ollama 的 CORS 问题，错误集中处理在 Rust 端，且符合「核心逻辑与 UI 分离」原则。
- **无硬依赖**：deb 不声明 `Depends: ollama`，因为 Ollama 通常非 deb 安装；应用内置连接检测与引导。

## 版本发布规范

每次**正式更新**（发新版本）必须完整执行以下四步：

1. **更新版本号**：`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、`package.json` 三处同步升至新版本号（如 `0.2.0`）。
2. **更新日志板**：在 `src/components/modals/UpdateLogModal.vue` 的 `LOG` 数组**最前面**追加新版本条目（含版本号、日期、本次更新项），历史版本自动保留。
3. **新建交付文件夹**：在 `软件包正式版交付文件夹-所有格式/` 下按新版本号新建文件夹（如 `0.2.0/`），放入对应 `.deb` 包。
4. **重新打包**：执行 `npm run tauri build` 生成新版本 `.deb`。

> **约定**：任何功能性变更（含更换应用图标、新增面板等）都计为一次正式更新，须走上述流程。
