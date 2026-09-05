# 云母已迁移至 Kirara

> **本仓库已停止维护。** 后续开发、问题反馈、版本发布和文档更新均移至新仓库。

## 新仓库

**[imythu/kirara — 云母 Kirara](https://github.com/imythu/kirara)**

- [项目说明与安装方式](https://github.com/imythu/kirara#readme)
- [从 rflush 迁移到 Kirara](https://github.com/imythu/kirara/blob/master/doc/migration-from-rflush.md)
- [新版本下载](https://github.com/imythu/kirara/releases)
- [问题反馈](https://github.com/imythu/kirara/issues)

中文产品名仍为“云母”。新程序、镜像和环境变量分别使用 `kirara`、`ghcr.io/imythu/kirara` 和 `KIRARA_*`；已有 `rflush.db` 数据目录可继续使用，具体步骤请阅读迁移指南。

迁移前请先停止旧服务并备份整个数据目录。不要同时运行新旧服务处理同一组任务。

本仓库保留历史提交、Issues、Pull Requests 和 [旧版发布记录](https://github.com/imythu/rflush/releases)，供查阅使用，不再接受维护更新。新仓库保留此前的 Git 提交历史。
