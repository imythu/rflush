import { useEffect, useMemo, useState } from "react";
import { AlertCircle, ArrowRight, CheckCircle2, CheckSquare2, Clock3, FolderInput, LoaderCircle, RefreshCw, Search } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate } from "@/lib/format";
import type { DownloaderRecord, TransferableTorrent } from "@/types";

type OpenListSettingsSummary = {
  enabled: boolean;
  target_directory_id: number | null;
  target_directories: Array<{ id?: number; name: string; openlist_root: string }>;
};

type TransferJob = {
  id: number;
  version: number;
  downloader_id: number | null;
  infohash: string;
  torrent_name: string;
  stage: string;
  attempts: number;
  copy_lock_acquired: boolean;
  last_error: string | null;
  created_at: string;
  updated_at: string;
};

type TransferJobsResponse = {
  page: number;
  page_size: number;
  total: number;
  records: TransferJob[];
};

const STAGES: Record<string, { label: string; progress: number }> = {
  waiting_download: { label: "检查源任务", progress: 5 },
  copy_reconcile: { label: "核对目标文件", progress: 15 },
  copy_legacy_reconcile: { label: "核对目标文件", progress: 15 },
  copy_submitting: { label: "提交复制", progress: 25 },
  copying: { label: "OpenList 复制中", progress: 40 },
  copy_manual_review: { label: "需要人工确认", progress: 40 },
  copy_succeeded: { label: "复制已完成", progress: 55 },
  torrent_exported: { label: "种子已导出", progress: 65 },
  source_qb_removed: { label: "源 qB 已移除", progress: 75 },
  target_qb_submitted: { label: "已提交目标 qB", progress: 85 },
  target_qb_starting: { label: "等待恢复做种", progress: 92 },
  source_removed: { label: "源文件已清理", progress: 98 },
  completed: { label: "转移完成", progress: 100 },
  cancelled: { label: "已取消", progress: 100 },
  manifest_required: { label: "需要人工处理", progress: 40 },
};

const TORRENT_PAGE_SIZE = 50;
const JOB_PAGE_SIZE = 20;

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(2)} ${units[index]}`;
}

export function TorrentTransferPage() {
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [settings, setSettings] = useState<OpenListSettingsSummary | null>(null);
  const [downloaderId, setDownloaderId] = useState("");
  const [torrents, setTorrents] = useState<TransferableTorrent[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [keyword, setKeyword] = useState("");
  const [torrentPage, setTorrentPage] = useState(1);
  const [torrentLoading, setTorrentLoading] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [jobs, setJobs] = useState<TransferJob[]>([]);
  const [jobPage, setJobPage] = useState(1);
  const [jobTotal, setJobTotal] = useState(0);
  const [jobsLoading, setJobsLoading] = useState(true);
  const [resolvingJobId, setResolvingJobId] = useState<number | null>(null);

  const selectedTarget = settings?.target_directories.find((target) => target.id === settings.target_directory_id);
  const filteredTorrents = useMemo(() => {
    const query = keyword.trim().toLowerCase();
    return torrents.filter((torrent) => !query || torrent.name.toLowerCase().includes(query) || torrent.hash.toLowerCase().includes(query));
  }, [keyword, torrents]);
  const torrentPageCount = Math.max(1, Math.ceil(filteredTorrents.length / TORRENT_PAGE_SIZE));
  const pagedTorrents = filteredTorrents.slice((torrentPage - 1) * TORRENT_PAGE_SIZE, torrentPage * TORRENT_PAGE_SIZE);
  const pageSelected = pagedTorrents.length > 0 && pagedTorrents.every((torrent) => selected.has(torrent.hash));
  const jobPageCount = Math.max(1, Math.ceil(jobTotal / JOB_PAGE_SIZE));
  const blockingJobs = jobs.filter((job) => job.stage === "copy_manual_review" && job.copy_lock_acquired);

  async function loadJobs(page = jobPage, silent = false) {
    if (!silent) setJobsLoading(true);
    try {
      const response = await api<TransferJobsResponse>(`/api/media/openlist/manual-jobs?page=${page}&page_size=${JOB_PAGE_SIZE}`);
      setJobs(response.records);
      setJobTotal(response.total);
    } catch (loadError) {
      setError((loadError as Error).message || "加载转移任务失败");
    } finally {
      if (!silent) setJobsLoading(false);
    }
  }

  useEffect(() => {
    Promise.all([
      api<DownloaderRecord[]>("/api/downloaders"),
      api<OpenListSettingsSummary>("/api/media/openlist/settings"),
    ])
      .then(([items, loadedSettings]) => {
        const qbDownloaders = items.filter((item) => item.downloader_type === "qbittorrent" || item.downloader_type === "qb");
        setDownloaders(qbDownloaders);
        setSettings(loadedSettings);
        if (qbDownloaders.length === 1) setDownloaderId(String(qbDownloaders[0].id));
      })
      .catch((loadError: Error) => setError(loadError.message || "加载转移配置失败"));
  }, []);

  useEffect(() => {
    void loadJobs(jobPage);
    const timer = window.setInterval(() => void loadJobs(jobPage, true), 5_000);
    return () => window.clearInterval(timer);
  }, [jobPage]);

  useEffect(() => {
    setSelected(new Set());
    setKeyword("");
    setTorrentPage(1);
    setTorrents([]);
    if (!downloaderId) return;
    setTorrentLoading(true);
    setError("");
    api<TransferableTorrent[]>(`/api/downloaders/${downloaderId}/torrents`)
      .then(setTorrents)
      .catch((loadError: Error) => setError(loadError.message || "加载种子失败"))
      .finally(() => setTorrentLoading(false));
  }, [downloaderId]);

  function toggle(hash: string) {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(hash)) next.delete(hash); else next.add(hash);
      return next;
    });
  }

  function togglePage() {
    setSelected((current) => {
      const next = new Set(current);
      pagedTorrents.forEach((torrent) => pageSelected ? next.delete(torrent.hash) : next.add(torrent.hash));
      return next;
    });
  }

  async function submit() {
    if (!downloaderId || selected.size === 0) return;
    setSubmitting(true);
    setError("");
    setNotice("");
    try {
      const result = await api<{ created: number; skipped: number }>(`/api/downloaders/${downloaderId}/openlist-transfer`, {
        method: "POST",
        body: JSON.stringify({ hashes: [...selected] }),
      });
      await api("/api/media/openlist/scan", { method: "POST" }).catch(() => undefined);
      setSelected(new Set());
      setJobPage(1);
      await loadJobs(1);
      setNotice(`已创建 ${result.created} 个转移任务${result.skipped ? `，跳过 ${result.skipped} 个进行中的任务` : ""}`);
    } catch (submitError) {
      setError((submitError as Error).message || "创建转移任务失败");
    } finally {
      setSubmitting(false);
    }
  }

  async function resolveJob(job: TransferJob, resolution: "recheck" | "force_retry") {
    const confirmed = resolution === "force_retry"
      ? window.confirm("仅在确认 OpenList 中没有正在运行的旧复制任务后继续。是否强制重新提交复制？")
      : true;
    if (!confirmed) return;
    setResolvingJobId(job.id);
    setError("");
    try {
      await api(`/api/media/openlist/jobs/${job.id}/resolve-copy`, {
        method: "POST",
        body: JSON.stringify({
          resolution,
          expected_version: job.version,
          confirm_task_terminated: resolution === "force_retry",
        }),
      });
      await loadJobs(jobPage, true);
      setNotice(resolution === "force_retry" ? "已重新提交任务" : "已安排重新检查");
    } catch (resolveError) {
      setError((resolveError as Error).message || "处理任务失败");
    } finally {
      setResolvingJobId(null);
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-xl font-semibold">种子转移</h2>
        <p className="mt-1 text-sm text-muted">选择已完成的 qBittorrent 任务，转移到 OpenList 并恢复做种</p>
      </div>

      {error ? <div className="rounded-lg border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">{error}</div> : null}
      {notice ? <div className="rounded-lg border border-primary/30 bg-primary/10 px-4 py-3 text-sm text-foreground">{notice}</div> : null}

      <Card className="rounded-2xl">
        <CardHeader><CardTitle>提交转移</CardTitle></CardHeader>
        <CardContent className="space-y-4">
          <div className="grid gap-3 lg:grid-cols-[18rem_minmax(0,1fr)_20rem] lg:items-end">
            <div>
              <div className="mb-2 text-sm font-medium">来源下载器</div>
              <Select value={downloaderId} onChange={setDownloaderId} options={[{ value: "", label: "选择 qBittorrent" }, ...downloaders.map((item) => ({ value: String(item.id), label: item.name }))]} />
            </div>
            <div className="min-w-0 rounded-lg border border-border bg-surface-container/45 px-4 py-3 text-sm">
              <div className="flex items-center gap-2 font-medium"><FolderInput className="h-4 w-4 text-primary" />OpenList 目标</div>
              <div className="mt-1 truncate text-xs text-muted" title={selectedTarget?.openlist_root}>{selectedTarget ? `${selectedTarget.name} · ${selectedTarget.openlist_root}` : "未配置目标目录"}</div>
            </div>
            <div className="relative">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted" />
              <Input value={keyword} onChange={(event) => { setKeyword(event.target.value); setTorrentPage(1); }} placeholder="搜索名称或 info hash" aria-label="搜索可转移种子" className="pl-9" />
            </div>
          </div>

          <div className="flex h-[min(34rem,60dvh)] min-h-80 flex-col overflow-hidden rounded-lg border border-border">
            <div className="min-h-0 flex-1 overflow-auto">
              {torrentLoading ? <div className="flex h-full items-center justify-center gap-2 text-sm text-muted"><LoaderCircle className="h-4 w-4 animate-spin" />加载种子列表</div> : !downloaderId ? <div className="flex h-full items-center justify-center text-sm text-muted">请先选择来源下载器</div> : pagedTorrents.length === 0 ? <div className="flex h-full items-center justify-center text-sm text-muted">没有可转移的已完成种子</div> : (
                <Table>
                  <TableHeader className="sticky top-0 z-10 bg-card"><TableRow><TableHead className="w-12"><button type="button" onClick={togglePage} aria-label={pageSelected ? "取消全选当前页" : "全选当前页"} className="flex size-8 items-center justify-center rounded-md text-muted transition-colors hover:bg-accent hover:text-foreground"><CheckSquare2 className={`h-4 w-4 ${pageSelected ? "text-primary" : ""}`} /></button></TableHead><TableHead>种子</TableHead><TableHead>大小</TableHead><TableHead>保存路径</TableHead><TableHead>分类 / 标签</TableHead></TableRow></TableHeader>
                  <TableBody>{pagedTorrents.map((torrent) => <TableRow key={torrent.hash} className={selected.has(torrent.hash) ? "bg-primary/5" : undefined}><TableCell><input type="checkbox" className="size-4 cursor-pointer accent-primary" checked={selected.has(torrent.hash)} onChange={() => toggle(torrent.hash)} aria-label={`选择 ${torrent.name}`} /></TableCell><TableCell><div className="max-w-md truncate font-medium" title={torrent.name}>{torrent.name}</div><div className="mt-1 font-mono text-xs text-muted">{torrent.hash.slice(0, 12)}</div></TableCell><TableCell className="whitespace-nowrap">{formatBytes(torrent.size)}</TableCell><TableCell><div className="max-w-xs truncate text-xs text-muted" title={torrent.save_path}>{torrent.save_path}</div></TableCell><TableCell className="text-xs text-muted">{[torrent.category, torrent.tags].filter(Boolean).join(" · ") || "-"}</TableCell></TableRow>)}</TableBody>
                </Table>
              )}
            </div>
            <div className="flex shrink-0 flex-col gap-3 border-t border-border bg-card px-3 py-3 sm:flex-row sm:items-center sm:justify-between">
              <div className="flex items-center justify-between gap-2 sm:justify-start"><Button variant="outline" className="h-9 px-3" disabled={torrentPage <= 1} onClick={() => setTorrentPage((page) => page - 1)}>上一页</Button><span className="whitespace-nowrap text-xs text-muted">第 {torrentPage} / {torrentPageCount} 页 · 共 {filteredTorrents.length} 个</span><Button variant="outline" className="h-9 px-3" disabled={torrentPage >= torrentPageCount} onClick={() => setTorrentPage((page) => page + 1)}>下一页</Button></div>
              <Button onClick={() => void submit()} disabled={submitting || selected.size === 0 || !settings?.enabled || !selectedTarget}>{submitting ? <LoaderCircle className="mr-2 h-4 w-4 animate-spin" /> : <ArrowRight className="mr-2 h-4 w-4" />}{submitting ? "创建中" : `转移 ${selected.size} 个种子`}</Button>
            </div>
          </div>
        </CardContent>
      </Card>

      <Card className="rounded-2xl">
        <CardHeader><div className="flex items-center justify-between gap-3"><div><CardTitle>全部转移任务</CardTitle><div className="mt-1 text-xs text-muted">共 {jobTotal} 条，每 5 秒自动刷新</div></div><Button variant="outline" className="size-9 px-0" onClick={() => void loadJobs()} aria-label="刷新转移任务" title="刷新"><RefreshCw className="h-4 w-4" /></Button></div></CardHeader>
        <CardContent>
          {blockingJobs.map((job) => <div key={`blocker-${job.id}`} className="mb-4 flex flex-col gap-3 rounded-lg border border-destructive/35 bg-destructive/10 px-4 py-3 lg:flex-row lg:items-center lg:justify-between">
            <div className="min-w-0">
              <div className="flex items-center gap-2 text-sm font-semibold text-destructive"><AlertCircle className="h-4 w-4" />目标目录被任务 #{job.id} 锁定</div>
              <div className="mt-1 truncate text-sm" title={job.torrent_name}>{job.torrent_name}</div>
              {job.last_error ? <div className="mt-1 line-clamp-2 text-xs text-muted" title={job.last_error}>{job.last_error}</div> : null}
            </div>
            <div className="flex shrink-0 flex-wrap gap-2">
              <Button variant="outline" disabled={resolvingJobId === job.id} onClick={() => void resolveJob(job, "recheck")}>重新检查</Button>
              <Button disabled={resolvingJobId === job.id} onClick={() => void resolveJob(job, "force_retry")}>{resolvingJobId === job.id ? <LoaderCircle className="mr-2 h-4 w-4 animate-spin" /> : null}确认并重试</Button>
            </div>
          </div>)}
          {jobsLoading ? <div className="py-10 text-center text-sm text-muted">加载任务中...</div> : jobs.length === 0 ? <div className="py-10 text-center text-sm text-muted">暂无转移任务</div> : <div className="divide-y divide-border rounded-lg border border-border">{jobs.map((job) => {
            const stage = job.stage === "copying" && !job.copy_lock_acquired
              ? { label: "等待目标目录解锁", progress: 25 }
              : STAGES[job.stage] ?? { label: job.stage, progress: 0 };
            const failed = job.stage === "cancelled" || job.stage === "copy_manual_review" || job.stage === "manifest_required";
            const complete = job.stage === "completed";
            const StatusIcon = complete ? CheckCircle2 : failed ? AlertCircle : Clock3;
            return <div key={job.id} className="grid gap-3 px-4 py-3 lg:grid-cols-[minmax(0,1fr)_12rem_9rem_9rem_auto] lg:items-center">
              <div className="min-w-0"><div className="truncate text-sm font-medium" title={job.torrent_name}>{job.torrent_name}</div><div className="mt-1 text-xs text-muted">{downloaders.find((item) => item.id === job.downloader_id)?.name ?? `下载器 #${job.downloader_id ?? "-"}`} · {job.infohash.slice(0, 12)}</div>{job.last_error ? <div className="mt-1 line-clamp-2 text-xs text-destructive" title={job.last_error}>{job.last_error}</div> : null}</div>
              <div><div className="h-1.5 overflow-hidden rounded-full bg-surface-container-high"><div className={`h-full rounded-full transition-[width] duration-300 ${failed ? "bg-destructive" : "bg-primary"}`} style={{ width: `${stage.progress}%` }} /></div><div className="mt-1 text-right text-[11px] text-muted">{stage.progress}%</div></div>
              <div className={`flex items-center gap-1.5 text-xs font-medium ${complete ? "text-primary" : failed ? "text-destructive" : "text-foreground"}`}><StatusIcon className={`h-4 w-4 ${!complete && !failed ? "animate-pulse" : ""}`} />{stage.label}</div>
              <div className="text-xs text-muted">{formatDate(job.updated_at)}</div>
              {job.stage === "copy_manual_review" ? <div className="flex flex-wrap gap-2 lg:justify-end">
                <Button variant="outline" className="h-8 px-3 text-xs" disabled={resolvingJobId === job.id} onClick={() => void resolveJob(job, "recheck")}>重新检查</Button>
                <Button className="h-8 px-3 text-xs" disabled={resolvingJobId === job.id} onClick={() => void resolveJob(job, "force_retry")}>{resolvingJobId === job.id ? <LoaderCircle className="mr-1.5 h-3.5 w-3.5 animate-spin" /> : null}确认并重试</Button>
              </div> : <div />}
            </div>;
          })}</div>}
          <div className="mt-4 flex items-center justify-between gap-3"><span className="text-xs text-muted">第 {jobPage} / {jobPageCount} 页</span><div className="flex gap-2"><Button variant="outline" disabled={jobPage <= 1} onClick={() => setJobPage((page) => page - 1)}>上一页</Button><Button variant="outline" disabled={jobPage >= jobPageCount} onClick={() => setJobPage((page) => page + 1)}>下一页</Button></div></div>
        </CardContent>
      </Card>
    </div>
  );
}
