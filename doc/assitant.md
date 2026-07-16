# 项目目标

在当前项目基础上新增「自动追剧系统」以及「资源搜索下载系统」。

目标效果：

用户可以：
1. 从 TMDB API 搜索影视内容
2. 创建追剧订阅
3. 设置追剧规则（开始集数、质量要求等）
4. 系统自动搜索 PT 资源
5. 自动判断资源是否匹配目标影视
6. 自动选择最佳资源
7. 自动提交下载任务

最终效果类似：
- Sonarr（自动追剧）
- Prowlarr（搜索源聚合）
- qBittorrent 自动下载

但交互必须比 Sonarr 简单，面向普通用户。


---

# 核心设计原则

不要简单复制 Sonarr。

需要：
- 学习 Sonarr 的核心算法
- 提取其中成熟的资源匹配思想
- 使用当前项目已有技术栈重新设计

重点复刻：
- 搜索流程
- 资源解析
- 匹配算法
- 质量评分

不要复刻：
- Sonarr 复杂 UI
- Sonarr 大量高级配置
- 面向专业用户的交互


---

# 第一部分：搜索源 / Indexer 系统

参考项目：

https://github.com/JustLookAtNow/pt_mate


## 要求

下载该仓库到本地，深入分析源码。

复刻其中：

1. NexusPHP 网站接入方式
2. 馒头站点接入方式
3. 搜索请求封装方式
4. 登录状态管理方式
5. Cookie / Session 管理方式
6. 搜索结果解析方式


目标：

提供统一的视频资源搜索接口。


该模块定位类似：

Prowlarr


设计目标：

不同 PT 站：

```

NexusPHP
馒头
其他站点

````

统一转换成：

```rust
struct SearchResult {

    title: String,

    torrent_url: String,

    magnet: Option<String>,

    size: u64,

    seeders: u32,

    leechers: u32,

    publish_time: DateTime,

    source_site: String
}
````

上层业务不能感知具体 PT 网站。

要求：

* 最大程度兼容当前系统已有站点配置
* 不破坏已有功能
* 不重新设计站点配置体系
* 新增 Adapter 层完成兼容

---

# 第二部分：影视资源匹配引擎

参考项目：

[https://github.com/Sonarr/Sonarr](https://github.com/Sonarr/Sonarr)

必要时分析：

[https://github.com/guessit-io/guessit](https://github.com/guessit-io/guessit)

目标：

实现：

TMDB 影视信息

↓

PT 搜索结果

↓

判断是否是目标资源

---

# 需要深入分析 Sonarr 以下模块：

## 1. Release Parser

研究：

如何解析：

例如：

```
百日成王.S01E03.1080p.WEB-DL.H265.AAC
```

转换为：

```rust
struct ReleaseInfo {

    title:String,

    year:Option<u32>,

    season:Option<u32>,

    episodes:Vec<u32>,

    resolution:Option<String>,

    codec:Option<String>,

    source:Option<String>
}
```

需要支持：

* 欧美剧
* 国产剧
* 动漫
* 电影
* 季编号
* 集编号
* 动画绝对集数

---

## 2. Matching Engine

研究 Sonarr Decision Engine。

实现：

资源匹配评分系统。

例如：

```
标题匹配       40分
年份匹配       10分
季匹配         20分
集匹配         20分
质量匹配       10分

总分 >=80 自动接受
```

要求：

评分规则可扩展。

---

## 3. Quality Profile

参考 Sonarr Quality Profile。

实现：

用户可以设置：

例如：

```
优先：

1080p
WEB-DL
H265


接受：

720p


拒绝：

480p
```

系统自动评分。

---

## 4. Search Query Generator

实现：

根据 TMDB 数据生成 PT 搜索关键词。

例如：

TMDB:

```
百日成王
Season 1
Episode 3
```

生成：

```
百日成王 S01E03

百日成王 S1E3

百日成王 03
```

提高 PT 命中率。

---

# 第三部分：自动追剧逻辑

设计：

Subscription 模型。

例如：

```rust
struct Subscription {

    tmdb_id:String,

    media_type:String,

    season:u32,

    start_episode:u32,


    quality_profile:String

}
```

流程：

```
用户订阅

↓

生成追剧任务

↓

Scheduler 定时执行

↓

调用搜索接口

↓

解析资源

↓

Matching Engine评分

↓

选择最佳资源

↓

进入下载队列
```

---

# 第四部分：Agent 协作要求

## 设计阶段

必须使用 subagent。

创建：

### Agent A：架构设计

负责：

* 分析当前项目
* 分析 pt_mate
* 分析 Sonarr
* 输出整体架构

---

### Agent B：架构评审

要求：

必须新建 agent。

不能复用 Agent A。

使用 React 思想：

```
提出方案

↓

寻找漏洞

↓

提出反例

↓

修正方案

↓

重新评估
```

重点检查：

* 数据结构是否合理
* 模块边界是否合理
* 是否破坏已有代码
* 是否存在未来扩展问题

---

设计阶段输出：

必须包含：

1. 系统架构图
2. 模块划分
3. 数据模型
4. API设计
5. 调用流程
6. 风险列表
7. 修改文件列表

---

# 第五部分：实现阶段

根据设计方案：

使用多个 subagent 分工实现。

建议拆分：

Agent：

1.

PT Indexer Adapter

负责：

* pt_mate逻辑迁移
* PT搜索接口

2.

Media Metadata

负责：

* TMDB模型
* 影视信息管理

3.

Release Parser

负责：

* 文件名解析

4.

Matching Engine

负责：

* 评分算法

5.

Scheduler

负责：

* 自动追剧任务

6.

Download Adapter

负责：

* qBittorrent等下载器

---

# 实现完成后

必须重新创建新的 Review Agent。

不能复用开发 Agent。

Review Agent 使用 React 思想：

```
检查代码

↓

寻找设计缺陷

↓

寻找边界问题

↓

提出修改建议

↓

开发Agent修复

↓

再次Review
```

重点检查：

* Rust代码质量
* 生命周期问题
* 错误处理
* 并发安全
* 数据库设计
* 可扩展性
* 是否符合原设计

---

# 最终验收标准

必须实现：

## 搜索

✅ 支持多个 PT 搜索源

✅ 统一搜索接口

## 影视

✅ TMDB metadata

✅ 订阅管理

## 匹配

✅ 自动解析资源标题

✅ 自动判断季/集

✅ 自动质量评分

## 自动化

✅ 定时扫描

✅ 自动选择资源

✅ 自动下载

## 工程质量

✅ Rust idiomatic code

✅ 模块清晰

✅ 不破坏已有功能

✅ 新功能可独立扩展

开始执行前：

不要直接编码。

先完成：

1. 项目分析
2. pt_mate 分析
3. Sonarr 分析
4. 架构设计
5. 架构评审

通过评审后再进入实现阶段。
