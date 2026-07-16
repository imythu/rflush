# 云母

[![GitHub Release](https://img.shields.io/github/v/release/imythu/rflush?style=flat-square)](https://github.com/imythu/rflush/releases/latest)
[![Docker Image](https://img.shields.io/github/v/release/imythu/rflush?style=flat-square&label=ghcr.io)](https://github.com/imythu/rflush/pkgs/container/rflush)

云母是一套面向 PT 使用场景的 Web 管理工具，包含自动追剧与电影订阅、PT 聚合资源搜索、RSS 种子下载、PT 刷流任务管理、站点账号数据缓存与账号数据总览导出。

当前仓库、二进制、Docker 镜像与数据库文件名仍沿用 `rflush`，对外展示的产品名为“云母”。

现在的运行方式不再依赖 `rss.yaml`。程序启动后会开启一个本地 Web 服务，通过页面管理：

- 全局下载配置
- RSS 任务的新增 / 删除 / 暂停 / 启动
- 任务批量启动 / 暂停 / 删除
- 删除任务时可选同步删除已下载种子文件
- 历史下载记录查看
- PT 站点配置与连接测试
- PT 站点上传 / 下载量缓存与账号数据总览
- 站点账号数据支持导出 PNG、复制到剪贴板，便于求邀或资历展示
- 下载器配置与连接测试
- TMDB 影视搜索、电影 / 剧集订阅与自动扫描
- 多 PT 站点聚合搜索、发布名解析、匹配评分与手动下载
- 质量配置、候选拒绝原因与媒体下载队列查看
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
- React 前端页面，适配桌面端与移动端
- PT 刷流任务支持 cron 调度和手动立即执行
- PT 刷流支持站点绑定、下载器绑定、选种规则和删种规则
- 免费种 / H&R 判定支持 RSS 扩展属性和站点详情增强两层来源
- PT 站点账号数据会写入 SQLite，程序启动后异步刷新一次，之后每小时自动刷新
- 站点总览按站点并发拉取数据，单个站点失败不会影响其它站点，失败项会保留错误状态
- 自动追剧支持电影、标准季集和动画绝对集目标，自动生成分层搜索词
- 资源决策按“解析 -> 身份硬门槛 -> 质量规则 -> 评分 -> 稳定排序”执行，错剧、错季、错集不能靠高分抵消
- PT 聚合搜索支持 NexusPHP API / Cookie HTML 与 M-Team，单站失败不会丢弃其它站点的结果
- 自动与手动下载共用持久化 outbox，使用 infohash 幂等和状态对账处理重试

## Get Started

### 方式一：直接运行二进制

从 GitHub Release 下载与你平台匹配的压缩包，解压后直接运行：

```bash
# Linux
./rflush
```

查看启动参数与帮助：

```bash
./rflush -h
./rflush --help
```

支持的启动参数：

- `-H, --host`：监听地址，默认 `127.0.0.1`（支持环境变量 `RFLUSH_HOST`）
- `-p, --port`：监听端口，默认 `3000`（支持环境变量 `RFLUSH_PORT`）
- `-d, --data-dir <DIR>`：数据目录（数据库和下载输出都写入该目录，支持环境变量 `RFLUSH_DATA_DIR`）

启动示例：

```bash
# 监听本机回环地址
./rflush -H 127.0.0.1 -p 8080

# 指定数据目录
./rflush -d ./runtime-data
```

默认行为（不传 `--data-dir`）：

- 监听 `http://127.0.0.1:3000`
- 数据库 `./data/rflush.db`
- RSS 下载输出目录仍在当前工作目录

然后在浏览器打开页面，完成以下配置：

1. 全局下载设置
2. RSS 任务
3. PT 站点
4. 下载器
5. 自动追剧的 TMDB、质量配置与订阅
6. 刷流任务
7. 站点账号数据总览与导出

### 方式二：使用 Docker

镜像发布到：

```text
ghcr.io/imythu/rflush
```

支持平台：

- Linux `amd64`
- Linux `arm64`

示例：

```bash
docker run --name rflush \
  -p 127.0.0.1:3000:3000 \
  -v $(pwd)/data:/data \
  ghcr.io/imythu/rflush:latest
```

镜像内默认环境变量：

- `RFLUSH_HOST=0.0.0.0`
- `RFLUSH_PORT=3000`
- `RFLUSH_DATA_DIR=/data`

因此上面的挂载会把数据库与下载输出统一写到宿主机的 `$(pwd)/data`。

也可以覆盖端口：

```bash
docker run --name rflush \
  -e RFLUSH_PORT=8080 \
  -p 127.0.0.1:8080:8080 \
  -v $(pwd)/data:/data \
  ghcr.io/imythu/rflush:latest
```

指定版本（版本号见 [Releases](https://github.com/imythu/rflush/releases)）：

```bash
docker run --name rflush \
  -p 127.0.0.1:3000:3000 \
  -v $(pwd)/data:/data \
  ghcr.io/imythu/rflush:<version>
```

上述示例只把端口发布到宿主机回环地址。rflush 当前不内置用户认证；如果使用 `-H 0.0.0.0`、其它非回环监听地址，或把容器端口暴露给局域网 / 公网，必须限制网络访问并置于带身份认证的反向代理后。程序检测到非回环监听时会输出安全警告。CORS 限制不能替代身份认证。

### 开发模式

```bash
# 终端 1：后端
cargo run

# 终端 2：前端
cd frontend
npm install
npm run dev
```

## 自动追剧与资源搜索

### 前置配置

自动追剧复用现有的 PT 站点和下载器配置，不需要维护第二套账号：

1. 在“站点管理”添加至少一个可搜索的 `NexusPHP` 或 `M-Team` 站点，并完成连接测试。
2. 在“下载器”添加并测试 qBittorrent。
3. 打开“自动追剧 -> 质量与设置”，填写 TMDB v3 API Key 或 v4 Read Access Token。默认语言为 `zh-CN`。
4. 使用内置“高清优先”质量配置，或按分辨率、来源、编码、最低匹配分和最低做种数新建配置。
5. 从 TMDB 搜索结果创建电影或剧集订阅，选择 PT 站点、质量配置、下载器和保存路径。

`扫描间隔` 决定每轮订阅扫描后的下次运行时间；`单次查询上限` 限制标题、别名和季集组合产生的搜索词数量；`搜索并发` 限制站点请求并发数。程序启动后会自动恢复过期的订阅和下载 lease，并启动订阅扫描及下载 worker。

TMDB token 与站点、下载器凭据保存在本地 SQLite 中，请保护数据目录和数据库备份。`GET /api/media/settings` 不会回传 token，只返回 `tmdb_token_configured`；站点和下载器列表同样只返回认证类型 / `configured` 标记，不返回 Cookie、Passkey、API Key 或密码。更新配置时省略密钥或传空字符串会保留原值，只有显式提交 `clear_tmdb_token=true`、`clear_auth_config=true` 或 `clear_password=true` 才会清除对应凭据。聚合搜索响应会清洗站点结果，不返回 Cookie、API Key、密码、授权头或长期签名下载 URL。

### 搜索、匹配与下载

“资源搜索”既可以直接输入关键词，也可以选择订阅的当前目标。选择目标时，后端复用自动扫描的完整流程：生成查询、并发搜索、解析发布名、检查媒体身份和季集、应用质量规则、评分并稳定排序。去重、评分和排序完成后，单次聚合搜索全局最多返回前 512 个候选。响应中的 `errors` 会逐站记录认证过期、限流、解析等错误；部分站点失败时，其它站点的候选仍会正常返回。

每个搜索候选会获得一个短期有效的 opaque `candidate_id`。手动入队只提交该 ID，后端从有界缓存恢复权威站点结果并重新检查标题、站点、质量配置和订阅当前目标，不接受客户端拼装的种子 ID、下载地址或匹配结论。关联订阅时，目标必须已经由 TMDB 物化为 `pending` 且已到播出时间；校验、outbox 入队和 `pending -> queued` 在同一 SQLite 事务完成。

候选的 `accepted` 只会在所有硬门槛和最低分都通过时为 `true`。手动下载被拒绝的候选必须填写覆盖原因，覆盖原因会随下载决策快照进入审计记录。

自动与手动下载都先写入 `media_downloads` outbox，再由后台 worker 取种、校验 torrent、计算 infohash 并提交 qBittorrent。自动扫描同样以 subscription owner/version/lease 做 CAS，并在同一事务写入 outbox 与 `pending -> queued`，过期扫描不能为已修改的游标创建旧任务。该链路是 **at-least-once（至少一次）投递 + infohash 幂等 + 状态对账**，不是 SQLite 与 qBittorrent 之间的 exactly-once 事务。进程在外部提交窗口中断时，记录可能进入 `reconciling` 或重试状态；worker 会按 infohash 查询下载器后再确认成功或重试。确认最终失败时，关联的 `queued` 目标会恢复为 `pending` 并重新进入选种周期；外部提交结果未知时则保守停留在对账流程。

### 当前限制

- 媒体类型限 TMDB `tv` 和 `movie`；不订阅人物搜索结果。
- PT 搜索适配器当前限 NexusPHP API / Cookie HTML 与 M-Team；下载器当前限 qBittorrent。
- scene/XEM 编号映射、每日剧日期模式、整季升级、更多下载器和通知渠道尚未实现。
- 搜索质量依赖站点返回字段和发布名；无法可靠解析的候选会展示解析错误，不会被自动下载。

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

- `data/rflush.db`：配置与历史数据库（未指定 `--data-dir` 时）
- 各 RSS 对应下载目录：默认在当前目录；指定 `--data-dir` 后写入该目录

### 使用流程

1. 打开 `http://<服务器IP>:3000`
2. 在“任务设置”中保存下载参数
3. 在“任务管理”中添加 RSS 任务，可选择是否自动启动
4. 在任务页执行单个或批量启动 / 暂停 / 删除
5. 在“下载历史”或任务弹窗中查看结果
6. 在“站点管理”和“下载器”中完成 PT 配置
7. 在“自动追剧”中配置 TMDB 与质量规则，再创建订阅或手动搜索资源
8. 在“刷流任务”中创建任务，可按计划执行或点击“立即执行一次”
9. 在“站点管理”的“总览”中查看账号数据，并按需复制或下载图片

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
  - Linux `amd64` / `arm64`
  - Windows `amd64` / `arm64`
- Docker 镜像：
  - Linux `amd64`
  - Linux `arm64`

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
默认: ./data/rflush.db
指定 --data-dir 后: <data-dir>/rflush.db
```

其中包含：

- `global_settings`：任务设置
- `rss_subscriptions`：RSS 任务
- `download_runs`：每次批量执行的概要
- `download_records`：每个种子的最终下载记录，包含种子文件是否已删除标记
- `sites`：PT 站点配置
- `site_stats`：PT 站点账号数据缓存，包含 UID、用户名、上传量、下载量、分享率与最近刷新状态
- `downloaders`：下载器配置
- `brush_tasks`：刷流任务配置
- `brush_task_torrents`：刷流任务下的种子记录
- `task_stats_snapshots`：刷流任务统计快照
- `torrent_traffic`：种子级流量快照
- `media_settings`：TMDB、自动扫描间隔和搜索并发设置
- `quality_profiles`：分辨率、来源、编码、最低分和做种数规则
- `subscriptions` / `subscription_sites`：影视订阅及其搜索站点
- `subscription_targets`：TMDB 已确认集、播出日期、元数据前沿及季终状态；未来集不会提前搜索
- `media_downloads`：自动与手动下载共用的持久化 outbox、lease、重试和对账状态

## 后端接口

主要 API：

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
- `POST /api/jobs/run-all`
- `POST /api/jobs/run/:id`
- `GET /api/sites`
- `POST /api/sites`
- `PUT /api/sites/:id`
- `DELETE /api/sites/:id`
- `POST /api/sites/:id/test`
- `GET /api/sites/:id/stats`
- `GET /api/sites/stats-overview`
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
- `GET /api/media/settings`
- `PUT /api/media/settings`
- `GET /api/media/tmdb/search?query=...&media_type=multi|tv|movie`
- `GET /api/media/tmdb/details?tmdb_id=...&media_type=tv|movie`
- `GET /api/media/tmdb/season?tmdb_id=...&season=...`
- `GET /api/media/quality-profiles`
- `POST /api/media/quality-profiles`
- `GET /api/media/quality-profiles/:id`
- `PUT /api/media/quality-profiles/:id`
- `DELETE /api/media/quality-profiles/:id`
- `GET /api/media/subscriptions`
- `POST /api/media/subscriptions`
- `GET /api/media/subscriptions/:id`
- `PUT /api/media/subscriptions/:id`
- `DELETE /api/media/subscriptions/:id`
- `POST /api/media/subscriptions/:id/run`
- `POST /api/media/subscriptions/:id/pause`
- `POST /api/media/subscriptions/:id/resume`
- `GET /api/media/subscriptions/:id/downloads`
- `POST /api/media/resources/search`
- `GET /api/media/downloads`
- `POST /api/media/downloads`
- `GET /api/media/downloads/:id`
- `GET /api/stats/overview`
- `GET /api/stats/trend`
