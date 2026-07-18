import { type Dispatch, type SetStateAction, useCallback, useEffect, useMemo, useState } from "react";
import { AlertCircle, CheckCircle2, Clock3, FolderInput, KeyRound, LoaderCircle, Plus, RefreshCw, Save, ScanLine, Trash2, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";

export type OpenListSourceMapping = {
  id?: number;
  downloader_id: number;
  qb_path: string;
  openlist_path: string;
};

export type OpenListTargetDirectory = {
  id?: number;
  name: string;
  downloader_id: number;
  openlist_root: string;
  qb_root: string;
};

export type OpenListAutomationSettings = {
  address: string;
  api_key: string | null;
  api_key_configured: boolean;
  enabled: boolean;
  scan_interval_mins: number;
  source_mappings: OpenListSourceMapping[];
  target_directories: OpenListTargetDirectory[];
  target_directory_id: number | null;
  clear_api_key?: boolean;
};

export type OpenListDownloader = {
  id: number;
  name: string;
  downloader_type: string;
};

type OpenListJob = {
  id: number;
  media_download_id: number;
  downloader_id: number | null;
  infohash: string;
  torrent_name: string;
  stage: string;
  source_qb_path: string;
  source_openlist_path: string;
  target_openlist_path: string;
  target_qb_path: string;
  attempts: number;
  next_attempt_at: string | null;
  last_error: string | null;
  created_at: string;
  updated_at: string;
  completed_at: string | null;
};

const JOB_STAGE_LABELS: Record<string, string> = {
  waiting_download: "等待下载完成",
  copying: "正在复制",
  copy_succeeded: "复制已完成",
  torrent_exported: "种子已导出",
  source_qb_removed: "原任务已移除",
  target_qb_submitted: "已提交目标 qB",
  target_qb_starting: "等待目标 qB 正常做种",
  source_removed: "源文件已清理",
  completed: "已完成",
  cancelled: "已跳过",
};

type PathErrorMap = Record<string, string>;

function normalizedPath(value: string): string | null {
  const trimmed = value.trim().replace(/\\/g, "/");
  if (!trimmed.startsWith("/") || trimmed.includes("\0")) return null;
  const parts = trimmed.split("/").filter(Boolean);
  if (parts.some((part) => part === "." || part === "..")) return null;
  return parts.length === 0 ? "/" : `/${parts.join("/")}`;
}

function pathsOverlap(left: string, right: string): boolean {
  return left === right || left === "/" || right === "/" || left.startsWith(`${right}/`) || right.startsWith(`${left}/`);
}

function validatePaths(settings: OpenListAutomationSettings): PathErrorMap {
  const errors: PathErrorMap = {};
  const entries: Array<{ key: string; path: string; scope: string }> = [];

  settings.source_mappings.forEach((mapping, index) => {
    entries.push({ key: `source-${index}-qb`, path: mapping.qb_path, scope: `qb:${mapping.downloader_id}` });
    entries.push({ key: `source-${index}-openlist`, path: mapping.openlist_path, scope: "openlist" });
  });
  settings.target_directories.forEach((target, index) => {
    entries.push({ key: `target-${index}-qb`, path: target.qb_root, scope: `qb:${target.downloader_id}` });
    entries.push({ key: `target-${index}-openlist`, path: target.openlist_root, scope: "openlist" });
  });

  entries.forEach((entry) => {
    if (!normalizedPath(entry.path)) errors[entry.key] = "请输入不含 . 或 .. 的绝对路径";
  });
  entries.forEach((entry, index) => {
    const path = normalizedPath(entry.path);
    if (!path) return;
    entries.slice(index + 1).forEach((other) => {
      const otherPath = normalizedPath(other.path);
      if (entry.scope === other.scope && otherPath && pathsOverlap(path, otherPath)) {
        errors[entry.key] = "路径不能相同或互为父子目录";
        errors[other.key] = "路径不能相同或互为父子目录";
      }
    });
  });
  return errors;
}

function FieldError({ id, children }: { id: string; children?: string }) {
  return children ? <p id={id} className="text-xs text-destructive">{children}</p> : null;
}

export function OpenListAutomationPanel({
  settings,
  setSettings,
  downloaders,
  saving,
  onSave,
}: {
  settings: OpenListAutomationSettings;
  setSettings: Dispatch<SetStateAction<OpenListAutomationSettings>>;
  downloaders: OpenListDownloader[];
  saving: boolean;
  onSave: (valid: boolean) => void;
}) {
  const qbDownloaders = useMemo(
    () => downloaders.filter((item) => ["qbittorrent", "qb"].includes(item.downloader_type.toLowerCase())),
    [downloaders],
  );
  const downloaderOptions = qbDownloaders.map((item) => ({ value: String(item.id), label: item.name }));
  const pathErrors = validatePaths(settings);
  const [jobs, setJobs] = useState<OpenListJob[]>([]);
  const [jobsLoading, setJobsLoading] = useState(true);
  const [scanBusy, setScanBusy] = useState(false);
  const [jobsError, setJobsError] = useState("");

  const loadJobs = useCallback(async () => {
    setJobsLoading(true);
    try {
      setJobs(await api<OpenListJob[]>("/api/media/openlist/jobs"));
      setJobsError("");
    } catch (error) {
      setJobsError(error instanceof Error ? error.message : String(error));
    } finally {
      setJobsLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadJobs();
    const timer = window.setInterval(() => void loadJobs(), 10_000);
    return () => window.clearInterval(timer);
  }, [loadJobs]);

  async function scanNow() {
    setScanBusy(true);
    try {
      const result = await api<{ discovered: number; processing_enabled: boolean }>("/api/media/openlist/scan", { method: "POST" });
      await loadJobs();
      setJobsError(
        result.processing_enabled
          ? result.discovered > 0 ? `发现 ${result.discovered} 个新任务，后台正在处理` : "扫描完成，暂无新任务"
          : result.discovered > 0 ? `已记录 ${result.discovered} 个新任务；自动归档关闭，暂不处理` : "扫描完成；自动归档关闭，任务不会处理",
      );
    } catch (error) {
      setJobsError(error instanceof Error ? error.message : String(error));
    } finally {
      setScanBusy(false);
    }
  }
  const selectedTargetExists = settings.target_directories.some((target) => target.id != null && target.id === settings.target_directory_id);
  const configurationValid = Boolean(
    settings.address.trim()
      && settings.scan_interval_mins >= 1
      && settings.source_mappings.length > 0
      && settings.target_directories.length > 0
      && selectedTargetExists
      && settings.target_directories.every((target) => target.name.trim())
      && Object.keys(pathErrors).length === 0,
  );
  const valid = !settings.enabled || configurationValid;

  function updateSource(index: number, patch: Partial<OpenListSourceMapping>) {
    setSettings((current) => ({
      ...current,
      source_mappings: current.source_mappings.map((item, itemIndex) => itemIndex === index ? { ...item, ...patch } : item),
    }));
  }

  function updateTarget(index: number, patch: Partial<OpenListTargetDirectory>) {
    setSettings((current) => ({
      ...current,
      target_directories: current.target_directories.map((item, itemIndex) => itemIndex === index ? { ...item, ...patch } : item),
    }));
  }

  return (
    <Card>
      <CardHeader className="flex-row items-start justify-between gap-4">
        <div>
          <CardTitle className="flex items-center gap-2"><FolderInput className="size-5" />OpenList 自动归档</CardTitle>
          <CardDescription>复制追剧文件，并在目标目录恢复做种</CardDescription>
        </div>
        <label className="flex cursor-pointer items-center gap-2 text-sm font-medium">
          <input
            type="checkbox"
            className="size-4 accent-primary"
            checked={settings.enabled}
            onChange={(event) => setSettings((current) => ({ ...current, enabled: event.target.checked }))}
          />
          启用自动归档
        </label>
      </CardHeader>
      <CardContent className="flex flex-col gap-6">
        <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          <div className="flex flex-col gap-2 md:col-span-2 xl:col-span-2">
            <Label htmlFor="openlist-address">OpenList 地址</Label>
            <Input id="openlist-address" value={settings.address} placeholder="https://openlist.example.com" onChange={(event) => setSettings((current) => ({ ...current, address: event.target.value }))} />
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="openlist-scan-interval">扫描间隔（分钟）</Label>
            <Input id="openlist-scan-interval" type="number" min={1} value={settings.scan_interval_mins} onChange={(event) => setSettings((current) => ({ ...current, scan_interval_mins: Number(event.target.value) }))} />
          </div>
          <div className="flex flex-col gap-2 md:col-span-2 xl:col-span-3">
            <div className="flex items-center justify-between gap-3">
              <Label htmlFor="openlist-api-key">API Key</Label>
              {settings.api_key_configured && !settings.clear_api_key ? <span className="inline-flex items-center gap-1 text-xs font-medium text-primary"><CheckCircle2 className="size-3.5" />已配置</span> : null}
            </div>
            <div className="flex gap-2">
              <div className="relative flex-1">
                <KeyRound className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted" />
                <Input id="openlist-api-key" className="pl-10" type="password" autoComplete="off" value={settings.api_key ?? ""} placeholder={settings.api_key_configured && !settings.clear_api_key ? "留空以保留现有密钥" : "输入 API Key"} onChange={(event) => setSettings((current) => ({ ...current, api_key: event.target.value, clear_api_key: false }))} />
              </div>
              {settings.api_key_configured ? <Button type="button" variant="outline" className={cn("size-10 px-0", settings.clear_api_key && "border-destructive text-destructive")} title={settings.clear_api_key ? "取消清除 API Key" : "清除 API Key"} aria-label={settings.clear_api_key ? "取消清除 API Key" : "清除 API Key"} onClick={() => setSettings((current) => ({ ...current, api_key: null, clear_api_key: !current.clear_api_key }))}>{settings.clear_api_key ? <X /> : <Trash2 />}</Button> : null}
            </div>
          </div>
        </div>

        <section className="flex flex-col gap-3" aria-labelledby="openlist-source-title">
          <div className="flex items-center justify-between gap-3">
            <div><h3 id="openlist-source-title" className="font-semibold">源目录映射</h3><p className="mt-1 text-xs text-muted">配置下载器路径与 OpenList 中同一目录的对应关系</p></div>
            <Button type="button" variant="outline" disabled={qbDownloaders.length === 0} onClick={() => setSettings((current) => ({ ...current, source_mappings: [...current.source_mappings, { downloader_id: qbDownloaders[0]?.id ?? 0, qb_path: "", openlist_path: "" }] }))}><Plus data-icon="inline-start" />新增映射</Button>
          </div>
          {qbDownloaders.length === 0 ? <p className="rounded-2xl border border-border bg-surface-container/45 p-4 text-sm text-muted">请先添加 qBittorrent 下载器。</p> : null}
          {settings.source_mappings.map((mapping, index) => (
            <div key={mapping.id ?? `source-${index}`} className="grid gap-3 rounded-2xl border border-border bg-surface-container/45 p-4 md:grid-cols-[minmax(140px,0.8fr)_minmax(0,1fr)_minmax(0,1fr)_40px] md:items-start">
              <div className="flex flex-col gap-2"><Label htmlFor={`source-qb-${index}`}>下载器</Label><Select id={`source-qb-${index}`} value={String(mapping.downloader_id)} options={downloaderOptions} onChange={(value) => updateSource(index, { downloader_id: Number(value) })} /></div>
              <div className="flex flex-col gap-2"><Label htmlFor={`source-qb-path-${index}`}>qB 路径</Label><Input id={`source-qb-path-${index}`} aria-invalid={Boolean(pathErrors[`source-${index}-qb`])} aria-describedby={`source-qb-error-${index}`} value={mapping.qb_path} placeholder="/pt" onChange={(event) => updateSource(index, { qb_path: event.target.value })} /><FieldError id={`source-qb-error-${index}`}>{pathErrors[`source-${index}-qb`]}</FieldError></div>
              <div className="flex flex-col gap-2"><Label htmlFor={`source-openlist-path-${index}`}>OpenList 路径</Label><Input id={`source-openlist-path-${index}`} aria-invalid={Boolean(pathErrors[`source-${index}-openlist`])} aria-describedby={`source-openlist-error-${index}`} value={mapping.openlist_path} placeholder="/local/pt" onChange={(event) => updateSource(index, { openlist_path: event.target.value })} /><FieldError id={`source-openlist-error-${index}`}>{pathErrors[`source-${index}-openlist`]}</FieldError></div>
              <Button type="button" variant="destructive" className="mt-0 size-10 px-0 md:mt-7" title="删除映射" aria-label="删除映射" onClick={() => setSettings((current) => ({ ...current, source_mappings: current.source_mappings.filter((_, itemIndex) => itemIndex !== index) }))}><Trash2 /></Button>
            </div>
          ))}
        </section>

        <section className="flex flex-col gap-3" aria-labelledby="openlist-target-title">
          <div className="flex items-center justify-between gap-3">
            <div><h3 id="openlist-target-title" className="font-semibold">目标目录</h3><p className="mt-1 text-xs text-muted">分类目录会自动创建在所选目标根目录下</p></div>
            <Button type="button" variant="outline" disabled={qbDownloaders.length === 0} onClick={() => setSettings((current) => ({ ...current, target_directories: [...current.target_directories, { id: -Date.now(), name: "", downloader_id: qbDownloaders[0]?.id ?? 0, openlist_root: "", qb_root: "" }] }))}><Plus data-icon="inline-start" />新增目录</Button>
          </div>
          {settings.target_directories.map((target, index) => (
            <div key={target.id ?? `target-${index}`} className="grid gap-3 rounded-2xl border border-border bg-surface-container/45 p-4 md:grid-cols-2 xl:grid-cols-[0.7fr_0.8fr_1fr_1fr_40px] xl:items-start">
              <div className="flex flex-col gap-2"><Label htmlFor={`target-name-${index}`}>名称</Label><Input id={`target-name-${index}`} value={target.name} placeholder="媒体库" onChange={(event) => updateTarget(index, { name: event.target.value })} /></div>
              <div className="flex flex-col gap-2"><Label htmlFor={`target-qb-${index}`}>目标下载器</Label><Select id={`target-qb-${index}`} value={String(target.downloader_id)} options={downloaderOptions} onChange={(value) => updateTarget(index, { downloader_id: Number(value) })} /></div>
              <div className="flex flex-col gap-2"><Label htmlFor={`target-openlist-${index}`}>OpenList 根目录</Label><Input id={`target-openlist-${index}`} aria-invalid={Boolean(pathErrors[`target-${index}-openlist`])} aria-describedby={`target-openlist-error-${index}`} value={target.openlist_root} placeholder="/media" onChange={(event) => updateTarget(index, { openlist_root: event.target.value })} /><FieldError id={`target-openlist-error-${index}`}>{pathErrors[`target-${index}-openlist`]}</FieldError></div>
              <div className="flex flex-col gap-2"><Label htmlFor={`target-qb-root-${index}`}>qB 可见根目录</Label><Input id={`target-qb-root-${index}`} aria-invalid={Boolean(pathErrors[`target-${index}-qb`])} aria-describedby={`target-qb-error-${index}`} value={target.qb_root} placeholder="/downloads/media" onChange={(event) => updateTarget(index, { qb_root: event.target.value })} /><FieldError id={`target-qb-error-${index}`}>{pathErrors[`target-${index}-qb`]}</FieldError></div>
              <Button type="button" variant="destructive" className="size-10 px-0 xl:mt-7" title="删除目标目录" aria-label="删除目标目录" onClick={() => setSettings((current) => ({ ...current, target_directory_id: current.target_directory_id === target.id ? null : current.target_directory_id, target_directories: current.target_directories.filter((_, itemIndex) => itemIndex !== index) }))}><Trash2 /></Button>
            </div>
          ))}
          <div className="flex max-w-xl flex-col gap-2">
            <Label htmlFor="openlist-selected-target">当前复制目标</Label>
            <Select id="openlist-selected-target" disabled={settings.target_directories.length === 0} value={String(settings.target_directory_id ?? "")} options={settings.target_directories.filter((target): target is OpenListTargetDirectory & { id: number } => target.id != null).map((target) => ({ value: String(target.id), label: `${target.name || "未命名"} · ${qbDownloaders.find((item) => item.id === target.downloader_id)?.name ?? "未知下载器"} · ${target.openlist_root || "未设置路径"}` }))} onChange={(value) => setSettings((current) => ({ ...current, target_directory_id: Number(value) }))} />
            {!selectedTargetExists && settings.target_directories.length > 0 ? <p className="text-xs text-destructive">请选择复制目标目录</p> : null}
          </div>
        </section>

        <section className="flex flex-col gap-3 border-t border-border pt-5" aria-labelledby="openlist-jobs-title">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <h3 id="openlist-jobs-title" className="font-semibold">归档任务</h3>
              <p className="mt-1 text-xs text-muted">记录追剧和资源搜索添加的种子，以及复制与恢复做种进度</p>
            </div>
            <div className="flex gap-2">
              <Button type="button" variant="outline" disabled={jobsLoading} onClick={() => void loadJobs()} title="刷新任务列表">
                {jobsLoading ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <RefreshCw data-icon="inline-start" />}
                刷新
              </Button>
              <Button type="button" disabled={scanBusy} onClick={() => void scanNow()} title="立即扫描并记录已提交的种子">
                {scanBusy ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <ScanLine data-icon="inline-start" />}
                {scanBusy ? "扫描中" : "立即扫描"}
              </Button>
            </div>
          </div>
          {!settings.enabled ? (
            <div className="rounded-2xl border border-border bg-surface-container/45 px-4 py-3 text-sm text-muted">自动归档当前关闭：系统仍会记录种子，开启并保存后才会复制和恢复做种。</div>
          ) : null}
          {jobsError ? (
            <div className={cn("flex items-start gap-2 rounded-2xl border px-4 py-3 text-sm", jobsError.includes("失败") || jobsError.includes("disabled") ? "border-destructive/25 bg-destructive/5 text-destructive" : "border-border bg-surface-container/45 text-muted")} role="status">
              <AlertCircle className="mt-0.5 size-4 shrink-0" />
              <span>{jobsError}</span>
            </div>
          ) : null}
          {jobsLoading && jobs.length === 0 ? (
            <div className="flex min-h-24 items-center justify-center gap-2 text-sm text-muted"><LoaderCircle className="size-4 animate-spin" />加载任务</div>
          ) : jobs.length === 0 ? (
            <div className="rounded-2xl border border-border bg-surface-container/45 px-4 py-6 text-center text-sm text-muted">暂无归档任务</div>
          ) : (
            <>
              <div className="hidden lg:block">
                <Table>
                  <TableHeader><TableRow><TableHead>种子</TableHead><TableHead>状态</TableHead><TableHead>路径</TableHead><TableHead>更新时间</TableHead></TableRow></TableHeader>
                  <TableBody>{jobs.map((job) => <OpenListJobRow key={job.id} job={job} />)}</TableBody>
                </Table>
              </div>
              <div className="grid gap-3 lg:hidden">
                {jobs.map((job) => <OpenListJobItem key={job.id} job={job} />)}
              </div>
            </>
          )}
        </section>

        <div className="flex justify-end border-t border-border pt-4">
          <Button disabled={saving || !valid} onClick={() => onSave(valid)}>{saving ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <Save data-icon="inline-start" />}{saving ? "保存中" : "保存 OpenList 设置"}</Button>
        </div>
      </CardContent>
    </Card>
  );
}

function OpenListJobRow({ job }: { job: OpenListJob }) {
  return (
    <TableRow>
      <TableCell><JobIdentity job={job} /></TableCell>
      <TableCell><JobStatus job={job} /></TableCell>
      <TableCell><JobPaths job={job} /></TableCell>
      <TableCell className="text-xs text-muted">{new Date(job.updated_at).toLocaleString()}</TableCell>
    </TableRow>
  );
}

function OpenListJobItem({ job }: { job: OpenListJob }) {
  return (
    <article className="rounded-2xl border border-border bg-surface-container/45 p-4">
      <div className="flex items-start justify-between gap-3"><JobIdentity job={job} /><JobStatus job={job} /></div>
      <div className="mt-3"><JobPaths job={job} /></div>
      <div className="mt-3 text-xs text-muted">更新于 {new Date(job.updated_at).toLocaleString()}</div>
    </article>
  );
}

function JobIdentity({ job }: { job: OpenListJob }) {
  return <div className="max-w-72"><div className="line-clamp-2 text-sm font-semibold" title={job.torrent_name}>{job.torrent_name}</div><div className="mt-1 font-mono text-xs text-muted">{job.infohash.slice(0, 12)} · #{job.id}</div></div>;
}

function JobStatus({ job }: { job: OpenListJob }) {
  const complete = job.stage === "completed";
  const Icon = complete ? CheckCircle2 : Clock3;
  return <div className="max-w-64 text-xs"><span className={cn("inline-flex items-center gap-1 font-medium", complete ? "text-primary" : "text-foreground")}><Icon className="size-3.5" />{JOB_STAGE_LABELS[job.stage] ?? job.stage}</span>{job.attempts > 0 ? <div className="mt-1 text-muted">已重试 {job.attempts} 次</div> : null}{job.last_error ? <div className="mt-1 line-clamp-3 text-destructive" title={job.last_error}>{job.last_error}</div> : null}</div>;
}

function JobPaths({ job }: { job: OpenListJob }) {
  const source = job.source_qb_path || job.source_openlist_path;
  const target = job.target_qb_path || job.target_openlist_path;
  return <div className="max-w-xl text-xs"><div className="truncate text-muted" title={source || "等待识别"}>源：{source || "等待识别"}</div><div className="mt-1 truncate text-muted" title={target || "等待规划"}>目标：{target || "等待规划"}</div></div>;
}
