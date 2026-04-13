# rflush

基于 Web 界面的 RSS 种子下载器，同时包含 PT 刷流任务管理。

现在的运行方式不再依赖 `rss.yaml`。程序启动后会开启一个本地 Web 服务，通过页面管理：

- 全局下载配置
- RSS 任务的新增 / 删除 / 暂停 / 启动
- 任务批量启动 / 暂停 / 删除
- 删除任务时可选同步删除已下载种子文件
- 历史下载记录查看
- PT 站点配置与连接测试
- 下载器配置与连接测试
- 刷流任务的新增 / 编辑 / 删除 / 启动 / 停止 / 立即执行一次
- 刷流种子列表、删种状态、缓存状态查看

下载逻辑已拆为可复用模块：一次接收一组配置，执行完本轮下载后返回；多个任务可以并发运行，但限流仍按 **协议 + 域名(+端口)** 全局共享。

## 功能

- 多 RSS 订阅，每个订阅独立目录
- SQLite 持久化配置、下载历史和刷流状态，数据库位于 `data/rflush.db`
- 单次执行型下载引擎，可被后端重复并发调用
- 内置域名级 FIFO 限流器
- 遇到“请求过于频繁”自动冻结对应域名并等待恢复
- 历史记录保存重试次数，不保存每次重试细节
- React + shadcn 风格前端页面
- PT 刷流任务支持 cron 调度和手动立即执行
- PT 刷流支持站点绑定、下载器绑定、选种规则和删种规则
- 免费种 / H&R 判定支持 RSS 扩展属性和站点详情增强两层来源
- 站点详情增强带进程内缓存、TTL 清理和缓存统计

## Get Started

### 方式一：直接运行二进制

从 GitHub Release 下载与你平台匹配的压缩包，解压后直接运行：

```bash
# Linux / macOS
./rflush

# Windows
.\rflush.exe
```

程序默认监听：

- `http://127.0.0.1:3000`

首次启动会自动创建：

- `data/rflush.db`

然后在浏览器打开页面，完成以下配置：

1. 全局下载设置
2. RSS 任务
3. PT 站点
4. 下载器
5. 刷流任务

### 方式二：使用 Docker

镜像发布到：

```text
ghcr.io/<github-owner>/rflush
```

支持平台：

- Linux `amd64`
- Linux `arm64`
- Windows `amd64`

示例：

```bash
docker run --name rflush \
  -p 3000:3000 \
  -v $(pwd)/data:/data \
  ghcr.io/<github-owner>/rflush:latest
```

指定版本：

```bash
docker run --name rflush \
  -p 3000:3000 \
  -v $(pwd)/data:/data \
  ghcr.io/<github-owner>/rflush:2026-04-13
```

Windows 镜像标签示例：

```text
ghcr.io/<github-owner>/rflush:latest-windows-amd64
```

### 开发模式

```bash
# 终端 1：后端
cargo run

# 终端 2：前端
cd frontend
npm install
npm run dev
```

## PT 刷流说明

当前刷流链路支持：

- 站点管理：`NexusPHP`、`M-Team`
- 下载器：`qBittorrent`
- 选种条件：体积、做种人数、促销类型、是否跳过 H&R
- 删种条件：最小做种时间、H&R 最小做种时间、分享率、上传量、下载超时、平均上传速度、不活跃时间

免费种 / H&R 判定规则：

- 第一轮：只使用 RSS 已提供的属性做快速过滤
- 第二轮：对需要补充判定的候选种子请求站点详情页 / API，再做严格过滤

这意味着：

- RSS 没带 `free` / `H&R` 属性的种子，不会在第一轮被提前过滤掉
- `M-Team` 这类站点可以依赖详情增强补出免费种信息

刷流种子列表页面当前展示：

- `种子ID`：站点详情页 ID
- `信息Hash`：`.torrent` 文件中的真实 infohash
- `状态`：优先显示下载器中的实时状态；如果下载器里仍存在，不会继续显示为 `removed`

## 使用说明

### 直接运行

```bash
./rflush
```

默认监听：

- 后端 / 页面：`http://127.0.0.1:3000`

首次启动会自动创建：

- `data/rflush.db`：配置与历史数据库
- 各 RSS 对应下载目录：以订阅名称（清洗后）命名

### 使用流程

1. 打开 `http://127.0.0.1:3000`
2. 在“任务设置”中保存下载参数
3. 在“任务管理”中添加 RSS 任务，可选择是否自动启动
4. 在任务页执行单个或批量启动 / 暂停 / 删除
5. 在“下载历史”或任务弹窗中查看结果
6. 在“站点管理”和“下载器”中完成 PT 配置
7. 在“刷流任务”中创建任务，可按计划执行或点击“立即执行一次”

## 前后端开发模式

开发时前后端分开启动：

### 后端

```bash
cargo run
```

### 前端

```bash
cd frontend
npm install
npm run dev
```

前端开发服务器默认是 `http://127.0.0.1:5173`，会请求后端 `http://127.0.0.1:3000/api/*`。

## 打包

前端开发阶段与后端分离；发布阶段前端构建产物会放进 `frontend/dist`，然后由 Rust 可执行文件内置并对外提供。

本地手动构建：

```bash
cd frontend
npm install
npm run build

cd ..
cargo build --release
```

GitHub Release 发布产物：

- 二进制：
  - Linux `amd64`
  - Linux `arm64`
  - macOS `amd64`
  - macOS `arm64`
  - Windows `amd64`
- Docker 镜像：
  - Linux `amd64`
  - Linux `arm64`
  - Windows `amd64`

发布流程：

1. 先构建前端
2. 再构建各平台后端
3. 前端资源内嵌到最终单文件二进制
4. 发布 GitHub Release
5. 推送 GHCR 镜像

## 合并种子文件

把当前目录下所有一级子目录中的 `.torrent` 文件复制到 `merge/`：

```bash
# Windows
.\merge.ps1

# Linux / macOS
chmod +x merge.sh
./merge.sh
```

脚本会：

- 自动跳过 `merge/`、`src/`、`target/`、`frontend/`、`data/` 等非下载目录
- 只复制 `.torrent` 文件
- 同名文件自动跳过，不覆盖已有文件

## 数据库

SQLite 数据库位于：

```text
./data/rflush.db
```

其中包含：

- `global_settings`：任务设置
- `rss_subscriptions`：RSS 任务
- `download_runs`：每次批量执行的概要
- `download_records`：每个种子的最终下载记录，包含种子文件是否已删除标记
- `sites`：PT 站点配置
- `downloaders`：下载器配置
- `brush_tasks`：刷流任务配置
- `brush_task_torrents`：刷流任务下的种子记录
- `task_stats_snapshots`：刷流任务统计快照
- `torrent_traffic`：种子级流量快照

## 后端接口

主要 API：

- `GET /api/bootstrap`
- `GET /api/settings`
- `PUT /api/settings`
- `GET /api/rss`
- `POST /api/rss`
- `DELETE /api/rss/:id`
- `POST /api/tasks/:id/start`
- `POST /api/tasks/:id/pause`
- `POST /api/tasks/:id/delete`
- `GET /api/tasks/:id/records`
- `POST /api/tasks/start`
- `POST /api/tasks/pause`
- `POST /api/tasks/delete`
- `POST /api/tasks/start-all`
- `POST /api/tasks/pause-all`
- `POST /api/tasks/delete-all`
- `GET /api/history`
- `GET /api/jobs`
- `POST /api/jobs/run-all`
- `POST /api/jobs/run/:id`
- `GET /api/sites`
- `POST /api/sites`
- `PUT /api/sites/:id`
- `DELETE /api/sites/:id`
- `POST /api/sites/:id/test`
- `GET /api/sites/:id/stats`
- `GET /api/downloaders`
- `POST /api/downloaders`
- `PUT /api/downloaders/:id`
- `DELETE /api/downloaders/:id`
- `POST /api/downloaders/:id/test`
- `GET /api/brush-tasks`
- `POST /api/brush-tasks`
- `GET /api/brush-tasks/:id`
- `PUT /api/brush-tasks/:id`
- `DELETE /api/brush-tasks/:id`
- `POST /api/brush-tasks/:id/start`
- `POST /api/brush-tasks/:id/stop`
- `POST /api/brush-tasks/:id/run`
- `GET /api/brush-tasks/:id/torrents`
- `GET /api/brush-tasks/cache-stats`
- `GET /api/stats/overview`
- `GET /api/stats/trend`
