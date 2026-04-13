import { useEffect, useState } from "react";
import { Edit, Eye, Pause, Play, Plus, Trash2, Zap } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate } from "@/lib/format";
import { cn } from "@/lib/utils";
import type { BrushCacheStats, BrushTaskRecord, BrushTaskRequest, BrushTorrentRecord, DownloaderRecord, SiteRecord } from "@/types";

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`;
}

const emptyForm: BrushTaskRequest = {
  name: "",
  cron_expression: "",
  site_id: null,
  downloader_id: 0,
  tag: "",
  rss_url: "",
  seed_volume_gb: null,
  save_dir: null,
  active_time_windows: null,
  promotion: "all",
  skip_hit_and_run: false,
  max_concurrent: 100,
  download_speed_limit: null,
  upload_speed_limit: null,
  size_ranges: null,
  seeder_ranges: null,
  delete_mode: "or",
  min_seed_time_hours: null,
  hr_min_seed_time_hours: null,
  target_ratio: null,
  max_upload_gb: null,
  download_timeout_hours: null,
  min_avg_upload_speed_kbs: null,
  max_inactive_hours: null,
  min_disk_space_gb: null,
};

function taskToForm(task: BrushTaskRecord): BrushTaskRequest {
  return {
    name: task.name,
    cron_expression: task.cron_expression,
    site_id: task.site_id,
    downloader_id: task.downloader_id,
    tag: task.tag,
    rss_url: task.rss_url,
    seed_volume_gb: task.seed_volume_gb,
    save_dir: task.save_dir,
    active_time_windows: task.active_time_windows,
    promotion: task.promotion,
    skip_hit_and_run: task.skip_hit_and_run,
    max_concurrent: task.max_concurrent,
    download_speed_limit: task.download_speed_limit,
    upload_speed_limit: task.upload_speed_limit,
    size_ranges: task.size_ranges,
    seeder_ranges: task.seeder_ranges,
    delete_mode: task.delete_mode,
    min_seed_time_hours: task.min_seed_time_hours,
    hr_min_seed_time_hours: task.hr_min_seed_time_hours,
    target_ratio: task.target_ratio,
    max_upload_gb: task.max_upload_gb,
    download_timeout_hours: task.download_timeout_hours,
    min_avg_upload_speed_kbs: task.min_avg_upload_speed_kbs,
    max_inactive_hours: task.max_inactive_hours,
    min_disk_space_gb: task.min_disk_space_gb,
  };
}

const selectClass =
  "flex h-11 w-full rounded-2xl border border-border bg-input px-4 py-2 text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring/30";

const checkboxClass = "h-4 w-4 rounded border border-border accent-[hsl(var(--primary))]";

export function BrushTasksPage() {
  const [tasks, setTasks] = useState<BrushTaskRecord[]>([]);
  const [sites, setSites] = useState<SiteRecord[]>([]);
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [cacheStats, setCacheStats] = useState<BrushCacheStats | null>(null);
  const [form, setForm] = useState<BrushTaskRequest>({ ...emptyForm });
  const [editingId, setEditingId] = useState<number | null>(null);
  const [formOpen, setFormOpen] = useState(false);
  const [torrentsOpen, setTorrentsOpen] = useState(false);
  const [torrentsTask, setTorrentsTask] = useState<BrushTaskRecord | null>(null);
  const [torrents, setTorrents] = useState<BrushTorrentRecord[]>([]);
  const [loadingTorrents, setLoadingTorrents] = useState(false);
  const [deleteConfirmId, setDeleteConfirmId] = useState<number | null>(null);
  const [submitError, setSubmitError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  function reload() {
    api<BrushTaskRecord[]>("/api/brush-tasks").then(setTasks);
    api<BrushCacheStats>("/api/brush-tasks/cache-stats").then(setCacheStats);
  }

  useEffect(() => {
    reload();
    api<SiteRecord[]>("/api/sites").then(setSites);
    api<DownloaderRecord[]>("/api/downloaders").then(setDownloaders);
  }, []);

  function openAdd() {
    setForm({ ...emptyForm, downloader_id: downloaders[0]?.id ?? 0 });
    setEditingId(null);
    setSubmitError("");
    setFormOpen(true);
  }

  function openEdit(task: BrushTaskRecord) {
    setForm(taskToForm(task));
    setEditingId(task.id);
    setSubmitError("");
    setFormOpen(true);
  }

  function closeForm() {
    setFormOpen(false);
    setEditingId(null);
    setSubmitError("");
  }

  async function handleSubmit() {
    setSubmitting(true);
    setSubmitError("");
    try {
      if (editingId !== null) {
        await api(`/api/brush-tasks/${editingId}`, { method: "PUT", body: JSON.stringify(form) });
      } else {
        await api("/api/brush-tasks", { method: "POST", body: JSON.stringify(form) });
      }
      closeForm();
      reload();
    } catch (error) {
      setSubmitError((error as Error).message || "提交失败");
    } finally {
      setSubmitting(false);
    }
  }

  async function handleDelete(id: number) {
    await api(`/api/brush-tasks/${id}`, { method: "DELETE" });
    setDeleteConfirmId(null);
    reload();
  }

  async function handleStart(id: number) {
    await api(`/api/brush-tasks/${id}/start`, { method: "POST" });
    reload();
  }

  async function handleStop(id: number) {
    await api(`/api/brush-tasks/${id}/stop`, { method: "POST" });
    reload();
  }

  async function handleRunOnce(id: number) {
    await api(`/api/brush-tasks/${id}/run`, { method: "POST" });
    reload();
  }

  function openTorrents(task: BrushTaskRecord) {
    setTorrentsTask(task);
    setTorrentsOpen(true);
    setLoadingTorrents(true);
    api<BrushTorrentRecord[]>(`/api/brush-tasks/${task.id}/torrents`)
      .then((data) => setTorrents([...data].sort((a, b) => b.added_at.localeCompare(a.added_at))))
      .finally(() => setLoadingTorrents(false));
  }

  function closeTorrents() {
    setTorrentsOpen(false);
    setTorrentsTask(null);
    setTorrents([]);
  }

  function setField<K extends keyof BrushTaskRequest>(key: K, value: BrushTaskRequest[K]) {
    setForm((prev) => ({ ...prev, [key]: value }));
  }

  function numOrNull(value: string): number | null {
    const n = Number(value);
    return value === "" || Number.isNaN(n) ? null : n;
  }

  return (
    <>
      <div className="grid gap-4 xl:gap-6">
        <Card>
          <CardHeader>
            <CardTitle>详情增强缓存</CardTitle>
            <CardDescription>显示免费种/H&R 详情增强的缓存体量和累计命中情况。</CardDescription>
          </CardHeader>
          <CardContent>
            {cacheStats ? (
              <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-6">
                <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
                  <div className="text-xs text-muted">缓存条目</div>
                  <div className="mt-1 text-2xl font-semibold">{cacheStats.cached_entry_count}</div>
                </div>
                <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
                  <div className="text-xs text-muted">站点桶数</div>
                  <div className="mt-1 text-2xl font-semibold">{cacheStats.site_bucket_count}</div>
                </div>
                <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
                  <div className="text-xs text-muted">累计命中</div>
                  <div className="mt-1 text-2xl font-semibold">{cacheStats.total_cache_hits}</div>
                </div>
                <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
                  <div className="text-xs text-muted">累计成功抓取</div>
                  <div className="mt-1 text-2xl font-semibold">{cacheStats.total_fetch_successes}</div>
                </div>
                <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
                  <div className="text-xs text-muted">TTL</div>
                  <div className="mt-1 text-2xl font-semibold">{cacheStats.ttl_secs}s</div>
                </div>
                <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
                  <div className="text-xs text-muted">并发上限</div>
                  <div className="mt-1 text-2xl font-semibold">{cacheStats.max_concurrency}</div>
                </div>
              </div>
            ) : (
              <div className="py-6 text-sm text-muted">缓存状态加载中...</div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <CardTitle>刷流任务管理</CardTitle>
                <CardDescription>管理 PT 刷流任务，配置选种与删种规则，查看种子状态。</CardDescription>
              </div>
              <Button className="w-full sm:w-auto" onClick={openAdd}>
                <Plus className="mr-2 h-4 w-4" />
                添加任务
              </Button>
            </div>
          </CardHeader>
          <CardContent>
            {tasks.length === 0 ? (
              <div className="py-12 text-center text-sm text-muted">暂无刷流任务，点击上方按钮添加。</div>
            ) : (
              <div className="grid gap-3">
                {tasks.map((task) => (
                  <div key={task.id} className="rounded-2xl border border-border bg-surface-container/70 p-4">
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="min-w-0">
                        <div className="flex items-center gap-2">
                          <span className="font-semibold">{task.name}</span>
                          <span
                            className={`rounded-full px-3 py-1 text-xs font-medium ${
                              task.enabled ? "bg-emerald-100 text-emerald-700" : "bg-amber-100 text-amber-700"
                            }`}
                          >
                            {task.enabled ? "运行中" : "已停止"}
                          </span>
                        </div>
                        <div className="mt-1 text-xs text-muted">#{task.id}</div>
                      </div>
                      <div className="flex flex-wrap gap-2">
                        {task.enabled ? (
                          <Button variant="outline" onClick={() => void handleStop(task.id)}>
                            <Pause className="mr-2 h-4 w-4" />
                            停止
                          </Button>
                        ) : (
                          <Button variant="secondary" onClick={() => void handleStart(task.id)}>
                            <Play className="mr-2 h-4 w-4" />
                            启动
                          </Button>
                        )}
                        <Button variant="outline" onClick={() => void handleRunOnce(task.id)}>
                          <Zap className="mr-2 h-4 w-4" />
                          立即执行一次
                        </Button>
                        <Button variant="outline" onClick={() => openEdit(task)}>
                          <Edit className="mr-2 h-4 w-4" />
                          编辑
                        </Button>
                        <Button variant="outline" onClick={() => openTorrents(task)}>
                          <Eye className="mr-2 h-4 w-4" />
                          查看种子
                        </Button>
                        <Button variant="destructive" onClick={() => setDeleteConfirmId(task.id)}>
                          <Trash2 className="mr-2 h-4 w-4" />
                          删除
                        </Button>
                      </div>
                    </div>

                    <div className="mt-3 grid gap-2 text-sm text-muted sm:grid-cols-2 xl:grid-cols-4">
                      <div>
                        <span className="font-medium text-foreground">Cron：</span>
                        {task.cron_expression}
                      </div>
                      <div>
                        <span className="font-medium text-foreground">站点：</span>
                        {sites.find((site) => site.id === task.site_id)?.name ?? (task.site_id ? `#${task.site_id}` : "自动匹配")}
                      </div>
                      <div>
                        <span className="font-medium text-foreground">标签：</span>
                        {task.tag}
                      </div>
                      <div className="sm:col-span-2 truncate">
                        <span className="font-medium text-foreground">RSS：</span>
                        {task.rss_url}
                      </div>
                    </div>

                    <div className="mt-2 text-xs text-muted">
                      创建：{formatDate(task.created_at)} · 更新：{formatDate(task.updated_at)}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      {/* 添加/编辑任务对话框 */}
      <Dialog
        open={formOpen}
        onClose={closeForm}
        title={editingId !== null ? "编辑刷流任务" : "添加刷流任务"}
        description="配置任务的选种规则、删种策略和其他参数。"
      >
        <div className="space-y-6 p-4 sm:p-6">
          {submitError ? (
            <div className="rounded-2xl border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
              {submitError}
            </div>
          ) : null}

          {/* 基本设置 */}
          <section>
            <h4 className="mb-3 text-sm font-semibold">基本设置</h4>
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label>任务名称</Label>
                <Input
                  placeholder="我的刷流任务"
                  value={form.name}
                  onChange={(e) => setField("name", e.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label>Cron 表达式</Label>
                <Input
                  placeholder="0 */5 * * * *"
                  value={form.cron_expression}
                  onChange={(e) => setField("cron_expression", e.target.value)}
                />
                <p className="text-xs text-muted">例如：0 */5 * * * *（每 5 分钟）</p>
              </div>
              <div className="space-y-2">
                <Label>站点</Label>
                <select
                  className={selectClass}
                  value={form.site_id ?? ""}
                  onChange={(e) => setField("site_id", e.target.value === "" ? null : Number(e.target.value))}
                >
                  <option value="">按 RSS/详情页域名自动匹配</option>
                  {sites.map((site) => (
                    <option key={site.id} value={site.id}>
                      {site.name} ({site.site_type})
                    </option>
                  ))}
                </select>
              </div>
              <div className="space-y-2">
                <Label>下载器</Label>
                <select
                  className={selectClass}
                  value={form.downloader_id}
                  onChange={(e) => setField("downloader_id", Number(e.target.value))}
                >
                  {downloaders.length === 0 ? (
                    <option value={0}>无可用下载器</option>
                  ) : (
                    downloaders.map((d) => (
                      <option key={d.id} value={d.id}>
                        {d.name} ({d.downloader_type})
                      </option>
                    ))
                  )}
                </select>
              </div>
              <div className="space-y-2">
                <Label>标签</Label>
                <Input
                  placeholder="brush"
                  value={form.tag}
                  onChange={(e) => setField("tag", e.target.value)}
                />
              </div>
              <div className="space-y-2 sm:col-span-2">
                <Label>RSS 地址</Label>
                <Input
                  placeholder="https://example.com/rss"
                  value={form.rss_url}
                  onChange={(e) => setField("rss_url", e.target.value)}
                />
              </div>
            </div>
          </section>

          {/* 可选设置 */}
          <section>
            <h4 className="mb-3 text-sm font-semibold">可选设置</h4>
            <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
              <div className="space-y-2">
                <Label>做种体积上限 (GB)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.seed_volume_gb ?? ""}
                  onChange={(e) => setField("seed_volume_gb", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>保存目录</Label>
                <Input
                  placeholder="默认目录"
                  value={form.save_dir ?? ""}
                  onChange={(e) => setField("save_dir", e.target.value || null)}
                />
              </div>
              <div className="space-y-2">
                <Label>活动时间窗口</Label>
                <Input
                  placeholder='["00:00-09:00"]'
                  value={form.active_time_windows ?? ""}
                  onChange={(e) => setField("active_time_windows", e.target.value || null)}
                />
                <p className="text-xs text-muted">JSON 数组格式，例如：[&quot;00:00-09:00&quot;]</p>
              </div>
            </div>
          </section>

          {/* 选种规则 */}
          <section>
            <h4 className="mb-3 text-sm font-semibold">选种规则</h4>
            <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
              <div className="space-y-2">
                <Label>促销类型</Label>
                <select
                  className={selectClass}
                  value={form.promotion ?? "all"}
                  onChange={(e) => setField("promotion", e.target.value)}
                >
                  <option value="all">全部</option>
                  <option value="free">免费</option>
                  <option value="normal">普通</option>
                </select>
              </div>
              <div className="space-y-2">
                <Label>最大并发数</Label>
                <Input
                  type="number"
                  min={1}
                  value={form.max_concurrent ?? 100}
                  onChange={(e) => setField("max_concurrent", Number(e.target.value) || 100)}
                />
              </div>
              <div className="space-y-2">
                <Label>下载限速 (KB/s)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.download_speed_limit ?? ""}
                  onChange={(e) => setField("download_speed_limit", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>上传限速 (KB/s)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.upload_speed_limit ?? ""}
                  onChange={(e) => setField("upload_speed_limit", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>体积范围</Label>
                <Input
                  placeholder='["0-10","10-50"]'
                  value={form.size_ranges ?? ""}
                  onChange={(e) => setField("size_ranges", e.target.value || null)}
                />
                <p className="text-xs text-muted">JSON 数组，单位 GB</p>
              </div>
              <div className="space-y-2">
                <Label>做种人数范围</Label>
                <Input
                  placeholder='["1-100"]'
                  value={form.seeder_ranges ?? ""}
                  onChange={(e) => setField("seeder_ranges", e.target.value || null)}
                />
                <p className="text-xs text-muted">JSON 数组</p>
              </div>
              <div className="flex items-end pb-2">
                <label className="flex items-center gap-3 text-sm text-muted">
                  <input
                    type="checkbox"
                    className={checkboxClass}
                    checked={form.skip_hit_and_run ?? false}
                    onChange={(e) => setField("skip_hit_and_run", e.target.checked)}
                  />
                  跳过 Hit and Run
                </label>
              </div>
            </div>
          </section>

          {/* 删种规则 */}
          <section>
            <h4 className="mb-3 text-sm font-semibold">删种规则</h4>
            <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
              <div className="space-y-2">
                <Label>删除模式</Label>
                <select
                  className={selectClass}
                  value={form.delete_mode ?? "or"}
                  onChange={(e) => setField("delete_mode", e.target.value)}
                >
                  <option value="or">或（满足任一条件即删除）</option>
                  <option value="and">与（满足所有条件才删除）</option>
                </select>
              </div>
              <div className="space-y-2">
                <Label>最小做种时间 (小时)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.min_seed_time_hours ?? ""}
                  onChange={(e) => setField("min_seed_time_hours", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>HR 最小做种时间 (小时)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.hr_min_seed_time_hours ?? ""}
                  onChange={(e) => setField("hr_min_seed_time_hours", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>目标分享率</Label>
                <Input
                  type="number"
                  step="0.1"
                  placeholder="不限"
                  value={form.target_ratio ?? ""}
                  onChange={(e) => setField("target_ratio", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>最大上传量 (GB)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.max_upload_gb ?? ""}
                  onChange={(e) => setField("max_upload_gb", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>下载超时 (小时)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.download_timeout_hours ?? ""}
                  onChange={(e) => setField("download_timeout_hours", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>最低平均上传速度 (KB/s)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.min_avg_upload_speed_kbs ?? ""}
                  onChange={(e) => setField("min_avg_upload_speed_kbs", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>最大不活跃时间 (小时)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.max_inactive_hours ?? ""}
                  onChange={(e) => setField("max_inactive_hours", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>最小磁盘空间 (GB)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.min_disk_space_gb ?? ""}
                  onChange={(e) => setField("min_disk_space_gb", numOrNull(e.target.value))}
                />
              </div>
            </div>
          </section>

          {/* 操作按钮 */}
          <div className="flex gap-3 border-t border-border pt-4">
            <Button disabled={submitting} onClick={() => void handleSubmit()}>
              {submitting ? "提交中..." : editingId !== null ? "保存修改" : "创建任务"}
            </Button>
            <Button variant="outline" disabled={submitting} onClick={closeForm}>
              取消
            </Button>
          </div>
        </div>
      </Dialog>

      {/* 查看种子对话框 */}
      <Dialog
        open={torrentsOpen}
        onClose={closeTorrents}
        title={torrentsTask ? `${torrentsTask.name} 的种子列表` : "种子列表"}
        description="当前任务中所有种子的状态信息。"
      >
        <div className="space-y-4 p-4 sm:p-6">
          {loadingTorrents ? (
            <div className="text-sm text-muted">加载中...</div>
          ) : torrents.length === 0 ? (
            <div className="py-8 text-center text-sm text-muted">暂无种子记录。</div>
          ) : (
            <div className="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>名称</TableHead>
                    <TableHead>种子ID</TableHead>
                    <TableHead>大小</TableHead>
                    <TableHead>状态</TableHead>
                    <TableHead>HR</TableHead>
                    <TableHead>添加时间</TableHead>
                    <TableHead>移除原因</TableHead>
                    <TableHead>信息Hash</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {torrents.map((t) => (
                    <TableRow key={t.id}>
                      <TableCell>
                        <div className="max-w-[240px] truncate font-medium">{t.torrent_name}</div>
                      </TableCell>
                      <TableCell className="font-mono text-xs">
                        {t.torrent_id ? (
                          <a
                            href={t.torrent_link ?? "#"}
                            target="_blank"
                            rel="noopener noreferrer"
                            className={cn(
                              "hover:underline",
                              t.torrent_link ? "text-blue-500 hover:text-blue-700" : "pointer-events-none text-muted",
                            )}
                          >
                            {t.torrent_id}
                          </a>
                        ) : (
                          <span className="text-muted">-</span>
                        )}
                      </TableCell>
                      <TableCell>{t.size_bytes != null ? formatBytes(t.size_bytes) : "-"}</TableCell>
                      <TableCell>
                        <span
                          className={`rounded-full px-3 py-1 text-xs font-medium ${
                            t.status === "seeding"
                              ? "bg-emerald-100 text-emerald-700"
                              : t.status === "downloading"
                                ? "bg-sky-100 text-sky-700"
                                : t.status === "removed"
                                  ? "bg-red-100 text-red-700"
                                  : "bg-violet-100 text-violet-700"
                          }`}
                        >
                          {t.status}
                        </span>
                      </TableCell>
                      <TableCell>
                        {t.is_hr ? (
                          <span className="rounded-full bg-amber-100 px-3 py-1 text-xs font-medium text-amber-700">
                            HR
                          </span>
                        ) : (
                          <span className="text-xs text-muted">否</span>
                        )}
                      </TableCell>
                      <TableCell className="text-xs">{formatDate(t.added_at)}</TableCell>
                      <TableCell className="text-xs text-muted">{t.remove_reason ?? "-"}</TableCell>
                      <TableCell className="font-mono text-xs">{t.torrent_hash || "-"}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}
        </div>
      </Dialog>

      {/* 删除确认对话框 */}
      <Dialog
        open={deleteConfirmId !== null}
        onClose={() => setDeleteConfirmId(null)}
        title="确认删除"
        description="删除后任务配置和相关种子记录将无法恢复。"
      >
        <div className="space-y-4 p-4 sm:p-6">
          <p className="text-sm text-muted">确定要删除该刷流任务吗？此操作不可撤销。</p>
          <div className="flex gap-3">
            <Button variant="destructive" onClick={() => deleteConfirmId !== null && void handleDelete(deleteConfirmId)}>
              <Trash2 className="mr-2 h-4 w-4" />
              确认删除
            </Button>
            <Button variant="outline" onClick={() => setDeleteConfirmId(null)}>
              取消
            </Button>
          </div>
        </div>
      </Dialog>
    </>
  );
}
