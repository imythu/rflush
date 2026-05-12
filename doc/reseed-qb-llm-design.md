# qB 扫描自动辅种与 LLM 候选优化设计

## 背景

自动辅种的目标是基于 qBittorrent 中已经完成的任务，自动在 PT 站点查找同资源种子，并将匹配成功的候选添加回下载器做种。该功能不以本地目录扫描作为第一数据源，而是优先扫描 qB 的任务列表，把 qB 中已有的完成任务作为“源资源索引”。

LLM 在该流程中的定位是辅助搜索与候选排序：解析脏标题、生成搜索词、理解站点搜索结果、对候选做语义排序。最终是否自动添加与恢复做种，仍由 `.torrent` 文件列表匹配和 qB 校验结果决定。

## 目标

- 扫描 qB 中已完成任务，建立辅种源索引。
- 基于 qB 任务名、体积、分类、标签等信息生成站点搜索关键词。
- 通过 LLM 优化标题归一化、搜索词生成和候选排序。
- 下载候选 `.torrent` 后解析文件列表，和 qB 源任务做强匹配。
- 匹配通过后添加到 qB，执行校验，并在校验成功后进入做种。
- 记录每个源任务、候选、加种和校验结果，便于审计与重试。

## 非目标

- LLM 不作为最终匹配裁判。
- 第一版不要求支持磁盘全量文件扫描。
- 第一版不要求自动改名、移动文件或创建硬链接。
- 第一版不要求跨下载器辅种。
- 第一版不要求支持所有站点，建议先支持 M-Team 与 qBittorrent 的闭环。

## 总体流程

```text
qB 扫描
  -> 过滤已完成源任务
  -> 建立 reseed source index
  -> 规则解析标题
  -> 必要时调用 LLM 归一化与生成搜索词
  -> 站点搜索
  -> 规则粗筛候选
  -> 候选太多或置信度不足时调用 LLM 排序
  -> 下载前 N 个候选 .torrent
  -> 解析候选文件列表与 info hash
  -> 获取 qB 源任务文件列表
  -> 文件列表、大小、结构强匹配
  -> 添加候选 torrent 到 qB
  -> recheck
  -> 校验 100% 后 resume
  -> 写入结果与日志
```

## 模块设计

### reseed 模块

新增模块：

```text
src/reseed/mod.rs
src/reseed/scheduler.rs
src/reseed/indexer.rs
src/reseed/llm.rs
src/reseed/matcher.rs
src/reseed/torrent.rs
```

职责划分：

- `scheduler.rs`：定时触发辅种任务，控制任务并发与生命周期。
- `indexer.rs`：扫描 qB 任务，建立源资源索引。
- `llm.rs`：封装 LLM 请求、JSON schema、缓存和错误处理。
- `matcher.rs`：候选粗筛、文件列表强匹配、评分合并。
- `torrent.rs`：解析 `.torrent` 文件，提取 info hash、文件列表、总大小。
- `mod.rs`：定义请求、记录、状态、错误类型。

### 下载器扩展

当前下载器抽象已有：

```text
add_torrent
list_torrents
delete_torrent
get_free_space
```

自动辅种建议补充：

```text
list_torrent_files(hash) -> Vec<TorrentFile>
recheck_torrent(hash)
resume_torrent(hash)
pause_torrent(hash)
```

qBittorrent 对应接口：

```text
GET  /api/v2/torrents/files?hash=...
POST /api/v2/torrents/recheck
POST /api/v2/torrents/resume
POST /api/v2/torrents/pause
```

`TorrentFile` 建议字段：

```rust
pub struct TorrentFile {
    pub name: String,
    pub size: i64,
    pub progress: f64,
    pub priority: i32,
}
```

### 站点扩展

当前站点适配器只有用户信息和种子属性能力。自动辅种需要新增：

```text
search_torrents(query, page, limit) -> Vec<TorrentSearchResult>
download_torrent(torrent_id) -> Vec<u8>
```

`TorrentSearchResult` 建议字段：

```rust
pub struct TorrentSearchResult {
    pub torrent_id: String,
    pub title: String,
    pub size_bytes: Option<u64>,
    pub seeders: Option<i32>,
    pub leechers: Option<i32>,
    pub detail_url: Option<String>,
    pub download_url: Option<String>,
    pub discount: Option<String>,
    pub published_at: Option<String>,
}
```

第一版优先实现 M-Team。NexusPHP 由于站点变体多，建议在 M-Team 跑通后再做。

## 数据模型

### reseed_tasks

保存自动辅种任务配置。

```sql
CREATE TABLE IF NOT EXISTS reseed_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    cron_expression TEXT NOT NULL,
    site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
    downloader_id INTEGER NOT NULL REFERENCES downloaders(id) ON DELETE CASCADE,
    source_tag_filter TEXT,
    exclude_tags TEXT,
    target_tag TEXT NOT NULL DEFAULT 'reseed',
    only_completed INTEGER NOT NULL DEFAULT 1,
    add_paused INTEGER NOT NULL DEFAULT 1,
    auto_recheck INTEGER NOT NULL DEFAULT 1,
    auto_resume INTEGER NOT NULL DEFAULT 0,
    max_candidates_per_source INTEGER NOT NULL DEFAULT 5,
    max_size_diff_percent REAL NOT NULL DEFAULT 0.5,
    min_rule_score REAL NOT NULL DEFAULT 0.70,
    min_llm_score REAL NOT NULL DEFAULT 0.70,
    min_final_score REAL NOT NULL DEFAULT 0.80,
    llm_enabled INTEGER NOT NULL DEFAULT 1,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### reseed_source_items

保存从 qB 扫描得到的源资源索引。

```sql
CREATE TABLE IF NOT EXISTS reseed_source_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES reseed_tasks(id) ON DELETE CASCADE,
    downloader_id INTEGER NOT NULL REFERENCES downloaders(id) ON DELETE CASCADE,
    source_hash TEXT NOT NULL,
    source_name TEXT NOT NULL,
    source_size_bytes INTEGER NOT NULL,
    source_save_path TEXT NOT NULL,
    source_category TEXT,
    source_tags TEXT,
    source_state TEXT,
    source_completion_on INTEGER,
    normalized_title TEXT,
    media_kind TEXT,
    year INTEGER,
    season INTEGER,
    episode INTEGER,
    resolution TEXT,
    source_type TEXT,
    release_type TEXT,
    video_codec TEXT,
    release_group TEXT,
    llm_confidence REAL,
    llm_metadata_json TEXT,
    search_queries_json TEXT,
    indexed_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(task_id, source_hash)
);
```

### reseed_candidates

保存站点搜索结果与候选排序结果。

```sql
CREATE TABLE IF NOT EXISTS reseed_candidates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_item_id INTEGER NOT NULL REFERENCES reseed_source_items(id) ON DELETE CASCADE,
    site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
    torrent_id TEXT NOT NULL,
    title TEXT NOT NULL,
    size_bytes INTEGER,
    seeders INTEGER,
    leechers INTEGER,
    detail_url TEXT,
    download_url TEXT,
    discount TEXT,
    rule_score REAL NOT NULL DEFAULT 0,
    llm_score REAL,
    final_score REAL NOT NULL DEFAULT 0,
    decision TEXT NOT NULL DEFAULT 'pending',
    reason TEXT,
    torrent_hash TEXT,
    torrent_file_tree_json TEXT,
    checked_at TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(source_item_id, site_id, torrent_id)
);
```

### reseed_records

保存加种、校验、恢复做种结果。

```sql
CREATE TABLE IF NOT EXISTS reseed_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES reseed_tasks(id) ON DELETE CASCADE,
    source_item_id INTEGER NOT NULL REFERENCES reseed_source_items(id) ON DELETE CASCADE,
    candidate_id INTEGER REFERENCES reseed_candidates(id) ON DELETE SET NULL,
    added_hash TEXT,
    status TEXT NOT NULL,
    verify_progress REAL,
    message TEXT,
    added_at TEXT,
    checked_at TEXT,
    finished_at TEXT
);
```

状态建议：

```text
indexed
searched
no_candidate
candidate_ranked
torrent_downloaded
matched
rejected
added
rechecking
verified
resumed
failed
skipped
```

## qB 扫描与源索引

扫描 qB 时只处理满足条件的任务：

- `downloaded >= size` 或 `completion_on > 0`
- 非错误状态
- 不包含排除标签，如 `reseed`、`brush`
- 如果配置了源标签过滤，则必须包含对应标签
- qB 中没有已存在的目标站点辅种 hash

索引 key：

```text
task_id + source_hash
```

如果 qB 任务名、大小、保存路径未变化，不重复执行 LLM 解析和搜索。

## LLM 找候选设计

LLM 参与两个阶段：

1. 源任务归一化与搜索词生成。
2. 站点候选排序。

### 源任务归一化

输入：

```json
{
  "name": "The.Matrix.1999.2160p.UHD.BluRay.REMUX.HEVC.DV.HDR.TrueHD.Atmos.7.1-FraMeSToR",
  "size_bytes": 83648221184,
  "save_path": "/downloads/movies/The.Matrix.1999.2160p.UHD.BluRay.REMUX.HEVC.DV.HDR.TrueHD.Atmos.7.1-FraMeSToR",
  "category": "movie",
  "tags": "uhd,hd",
  "state": "uploading"
}
```

输出必须是 JSON：

```json
{
  "kind": "movie",
  "canonical_title": "The Matrix",
  "original_title": null,
  "year": 1999,
  "season": null,
  "episode": null,
  "pack_type": null,
  "resolution": "2160p",
  "source": "UHD BluRay",
  "release_type": "REMUX",
  "video_codec": "HEVC",
  "dynamic_range": ["DV", "HDR"],
  "audio": ["TrueHD Atmos 7.1"],
  "release_group": "FraMeSToR",
  "edition": null,
  "confidence": 0.96,
  "search_queries": [
    {
      "query": "The Matrix 1999 2160p UHD BluRay REMUX FraMeSToR",
      "strictness": "high",
      "reason": "包含标题、年份、分辨率、来源、类型和发布组"
    },
    {
      "query": "The Matrix 1999 2160p REMUX",
      "strictness": "medium",
      "reason": "去掉音轨和动态范围，扩大搜索范围"
    },
    {
      "query": "黑客帝国 1999 4K",
      "strictness": "low",
      "reason": "中文别名兜底"
    }
  ]
}
```

规则：

- 无法确定的字段填 `null`。
- 不输出最终匹配结论。
- 不猜测不存在的信息。
- `confidence < 0.6` 时，该源任务可进入人工确认或低优先级队列。

### 候选排序

输入：

```json
{
  "source": {
    "name": "The.Matrix.1999.2160p.UHD.BluRay.REMUX.HEVC.DV.HDR.TrueHD.Atmos.7.1-FraMeSToR",
    "size_bytes": 83648221184,
    "metadata": {
      "kind": "movie",
      "canonical_title": "The Matrix",
      "year": 1999,
      "resolution": "2160p",
      "source": "UHD BluRay",
      "release_type": "REMUX",
      "release_group": "FraMeSToR"
    }
  },
  "candidates": [
    {
      "torrent_id": "1001",
      "title": "The Matrix 1999 2160p UHD BluRay REMUX HEVC DV HDR TrueHD Atmos 7.1-FraMeSToR",
      "size_bytes": 83648000000,
      "seeders": 42
    },
    {
      "torrent_id": "1002",
      "title": "The Matrix 1999 2160p UHD BluRay x265 10bit HDR",
      "size_bytes": 23500000000,
      "seeders": 120
    }
  ]
}
```

输出：

```json
[
  {
    "torrent_id": "1001",
    "llm_score": 0.98,
    "decision": "strong_candidate",
    "reason": "标题、年份、2160p、UHD BluRay、REMUX、HEVC、DV/HDR、音轨和发布组均一致，大小接近"
  },
  {
    "torrent_id": "1002",
    "llm_score": 0.30,
    "decision": "reject",
    "reason": "同电影同分辨率，但不是 REMUX，大小差异显著，可能是压制版"
  }
]
```

候选决策：

```text
strong_candidate
weak_candidate
reject
needs_manual_review
```

LLM 的排序只决定是否值得下载 `.torrent` 进一步检查，不直接决定是否加种。

## 规则评分

规则分建议占主导：

```text
final_score = rule_score * 0.7 + llm_score * 0.3
```

规则分参考项：

```text
标题相似度      20
年份一致        10
季/集一致       20
分辨率一致      10
来源一致        10
发布类型一致    10
大小误差        20
```

硬拒绝条件：

- 电影年份不一致。
- 剧集季集类型不一致，如完整季对单集。
- `REMUX` 对 `Encode`。
- `UHD BluRay` 对 `WEB-DL`。
- `2160p` 对 `1080p`。
- 大小误差超过配置阈值。

## 强匹配

候选进入强匹配前，需要下载 `.torrent` 文件并解析：

- info hash
- 总大小
- 文件列表
- 每个文件路径
- 每个文件大小
- piece length

qB 源任务需要通过 `list_torrent_files(hash)` 获取文件列表。

匹配分级：

### exact

文件数量、相对路径和大小完全一致。可自动添加。

### size_path_relaxed

文件大小完全一致，但根目录或路径前缀不同。可以在保存路径可推导时自动添加；否则进入人工确认。

### size_only

主文件大小一致，但文件名或结构不同。第一版不自动添加。

### mismatch

大小、文件数量或关键结构不一致。拒绝。

自动添加建议只允许：

```text
exact
size_path_relaxed 且保存路径可明确推导
```

## 添加与校验

匹配通过后：

1. 使用候选 `.torrent` 调用 `add_torrent`。
2. `save_path` 指向源 qB 任务的保存路径或其父目录。
3. `tags` 添加任务目标标签，例如 `reseed,mteam`。
4. 第一版建议 `paused = true`。
5. 添加成功后调用 `recheck_torrent`。
6. 周期性查询 qB 任务状态和进度。
7. 校验 100% 后，如果 `auto_resume = true`，调用 `resume_torrent`。
8. 校验不足 100% 或进入错误状态时，保持暂停并记录失败。

注意：即使 LLM 和文件列表匹配都通过，最终仍以 qB 校验为准。

## 缓存策略

LLM 调用必须缓存，避免每次扫描重复消耗。

源解析缓存 key：

```text
downloader_id + source_hash + source_name + source_size_bytes
```

候选排序缓存 key：

```text
source_item_id + site_id + sorted(candidate torrent ids + titles + sizes)
```

缓存失效条件：

- qB 源任务名称变化。
- qB 源任务大小变化。
- 站点搜索结果变化。
- LLM schema 版本变化。
- 用户手动要求重新分析。

## 失败处理

常见失败与处理：

| 场景 | 处理 |
| --- | --- |
| qB 连接失败 | 任务失败，等待下次调度 |
| 站点搜索失败 | 记录失败，可重试 |
| LLM 调用失败 | 降级到规则搜索 |
| 无候选 | 记录 `no_candidate` |
| 候选过多 | 规则粗筛后调用 LLM 排序 |
| torrent 下载失败 | 跳过该候选 |
| 文件列表不匹配 | 记录 reject reason |
| qB 已有相同 hash | 跳过 |
| qB 校验不足 100% | 暂停，记录 `verify_failed` |
| qB resume 失败 | 保留 verified 状态，提示手动恢复 |

## 前端页面

新增“自动辅种”页面，建议放在刷流分组下。

页面能力：

- 辅种任务列表。
- 新建/编辑任务。
- 立即执行一次。
- 启动/停止任务。
- 源 qB 任务索引列表。
- 候选列表与 LLM 排序结果。
- 匹配详情，包括规则分、LLM 分、最终分、拒绝原因。
- 加种与校验记录。
- 人工确认入口，用于 `needs_manual_review` 候选。

任务表单字段：

- 站点。
- 下载器。
- cron。
- 源标签过滤。
- 排除标签。
- 目标标签。
- 最大候选数量。
- 大小误差阈值。
- 是否启用 LLM。
- 是否暂停添加。
- 是否自动校验。
- 是否校验成功后自动恢复。

## API 草案

```text
GET    /api/reseed-tasks
POST   /api/reseed-tasks
GET    /api/reseed-tasks/{id}
PUT    /api/reseed-tasks/{id}
DELETE /api/reseed-tasks/{id}
POST   /api/reseed-tasks/{id}/start
POST   /api/reseed-tasks/{id}/stop
POST   /api/reseed-tasks/{id}/run

GET    /api/reseed-tasks/{id}/sources
GET    /api/reseed-sources/{id}/candidates
POST   /api/reseed-sources/{id}/refresh
POST   /api/reseed-candidates/{id}/confirm
POST   /api/reseed-candidates/{id}/reject
GET    /api/reseed-records
```

## 分阶段落地

### Phase 1：最小闭环

- qB 扫描已完成任务。
- M-Team 搜索与下载 `.torrent`。
- 规则解析标题与基础搜索词。
- 文件列表强匹配。
- 暂停加种。
- 结果入库与前端展示。

### Phase 2：LLM 优化

- LLM 源任务归一化。
- LLM 搜索词生成。
- LLM 候选排序。
- 缓存与降级策略。
- 前端展示 LLM 分数和解释。

### Phase 3：自动校验与恢复

- qB `list_torrent_files`。
- qB `recheck`。
- qB `resume`。
- 校验状态轮询。
- 校验成功后自动恢复做种。

### Phase 4：增强能力

- 人工确认工作流。
- NexusPHP 站点支持。
- 多站点并行搜索。
- 更细的剧集和合集匹配。
- 硬链接辅助模式。
- 辅种收益统计。

## 安全与保守策略

- 默认 `add_paused = true`。
- 默认 `auto_resume = false`，直到强匹配和校验流程稳定。
- LLM 不能直接触发自动加种。
- 文件列表不匹配时不自动添加。
- 校验不足 100% 不自动恢复。
- 所有自动决策都写入 `reason`，便于回溯。

## 结论

扫描 qB 做自动辅种是当前项目更自然的起点，因为已有下载器快照采集和 qB 任务信息。LLM 能显著提升搜索召回和候选排序质量，但它必须被限制在“找候选”的环节。最终自动化闭环应由规则评分、`.torrent` 文件列表强匹配和 qB 校验共同兜底。
