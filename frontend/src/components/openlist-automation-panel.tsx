import { type Dispatch, type SetStateAction, useEffect, useMemo, useRef, useState } from "react";
import { AlertCircle, CheckCircle2, CirclePause, Clock3, FolderInput, KeyRound, LoaderCircle, Plus, RefreshCw, Save, ScanLine, ShieldAlert, Trash2, X } from "lucide-react";
import { caseFold } from "unicode-case-folding";

import { Dialog } from "@/components/ui/dialog";
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
  updated_at: string;
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
  workflow: "auto_copy" | "qb_migration";
  version: number;
  source_qb_path: string;
  source_openlist_path: string;
  target_openlist_path: string;
  target_qb_path: string;
  attempts: number;
  openlist_task_ids: string[];
  copy_checkpoint: {
    path: string;
    size: number;
    operation: "copy_file" | "create_directory" | "review_existing" | "remove_file";
    phase: "prepared" | "uncertain";
    submitted_at: string | null;
    terminal_failure_verified: boolean;
  } | null;
  manual_resolution_allowed: boolean;
  copy_resolution_actions: string[];
  copy_lock_acquired: boolean;
  manifest_cursor: number;
  next_attempt_at: string | null;
  last_error: string | null;
  created_at: string;
  updated_at: string;
  completed_at: string | null;
};

type OpenListJobsResponse = {
  page: number;
  page_size: number;
  total: number;
  records: OpenListJob[];
};

type ClearOpenListJobsResponse = {
  cleared: number;
};

const JOB_STAGE_LABELS: Record<string, string> = {
  waiting_download: "等待下载完成",
  planning_manual_review: "自动复制规划需要处理",
  auto_copy_paused: "自动归档已暂停",
  copy_reconcile: "正在核对目标文件",
  copy_legacy_reconcile: "正在核对旧任务",
  copy_submitting: "正在安全提交复制",
  copying: "等待 OpenList 复制",
  copy_manual_review: "复制需要人工处理",
  manifest_required: "缺少可靠文件清单",
  copy_succeeded: "正在完成旧任务核验",
  copy_verified: "复制已确认",
  completed: "复制完成",
  cancelled: "复制已停止",
};

type CopyResolution = "recheck" | "cancel";

const AUTO_JOB_PAGE_SIZE = 20;
const AUTO_JOB_POLL_INTERVAL_MS = 10_000;

type PathErrorMap = Record<string, string>;

function normalizedPath(value: string): string | null {
  const trimmed = value.trim().replace(/\\/g, "/");
  if (!trimmed.startsWith("/") || trimmed.includes("\0")) return null;
  const parts = trimmed.split("/").filter(Boolean);
  if (parts.some((part) => part === "." || part === "..")) return null;
  return parts.length === 0 ? "/" : `/${parts.join("/")}`;
}

function pathsOverlap(left: string, right: string, caseInsensitive = false): boolean {
  if (caseInsensitive) {
    left = caseFold(left.normalize("NFC")).normalize("NFC");
    right = caseFold(right.normalize("NFC")).normalize("NFC");
  }
  return left === right || left === "/" || right === "/" || left.startsWith(`${right}/`) || right.startsWith(`${left}/`);
}

function validOpenListAddress(value: string): boolean {
  const address = value.trim();
  if (!address) return true;
  try {
    const url = new URL(address);
    return ["http:", "https:"].includes(url.protocol)
      && Boolean(url.hostname)
      && !url.username
      && !url.password
      && !url.search
      && !url.hash;
  } catch {
    return false;
  }
}

function validTargetName(value: string): boolean {
  const name = value.trim();
  return Boolean(name) && new TextEncoder().encode(name).length <= 100;
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
      if (entry.scope === other.scope && otherPath && pathsOverlap(path, otherPath, entry.scope === "openlist")) {
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
  const qbDownloaderIds = new Set(qbDownloaders.map((item) => item.id));
  const pathErrors = validatePaths(settings);
  const [jobs, setJobs] = useState<OpenListJob[]>([]);
  const [jobsLoading, setJobsLoading] = useState(true);
  const [jobPage, setJobPage] = useState(1);
  const [jobTotal, setJobTotal] = useState(0);
  const [jobPollNonce, setJobPollNonce] = useState(0);
  const [scanBusy, setScanBusy] = useState(false);
  const [clearBusy, setClearBusy] = useState(false);
  const [jobsError, setJobsError] = useState("");
  const [jobsNotice, setJobsNotice] = useState("");
  const [resolvingJobId, setResolvingJobId] = useState<number | null>(null);
  const [pendingResolutionJobId, setPendingResolutionJobId] = useState<number | null>(null);
  const [resolutionError, setResolutionError] = useState("");
  const [remoteTaskStoppedConfirmed, setRemoteTaskStoppedConfirmed] = useState(false);
  const jobPollGeneration = useRef(0);
  const jobPollController = useRef<AbortController | null>(null);
  const jobPageCount = Math.max(1, Math.ceil(jobTotal / AUTO_JOB_PAGE_SIZE));
  const pendingResolutionJob = pendingResolutionJobId == null
    ? null
    : jobs.find((job) => job.id === pendingResolutionJobId) ?? null;

  function stopJobPolling() {
    jobPollController.current?.abort();
    jobPollController.current = null;
    jobPollGeneration.current += 1;
  }

  function refreshJobs(page = jobPage) {
    stopJobPolling();
    if (page !== jobPage) {
      setJobs([]);
      setJobsLoading(true);
    }
    setJobPage(page);
    setJobPollNonce((current) => current + 1);
  }

  useEffect(() => {
    const generation = ++jobPollGeneration.current;
    let active = true;
    let timer: number | null = null;
    let controller: AbortController | null = null;

    async function poll(showLoading: boolean) {
      if (!active || generation !== jobPollGeneration.current) return;
      if (showLoading) setJobsLoading(true);
      let pageRedirected = false;
      controller = new AbortController();
      jobPollController.current = controller;
      try {
        const response = await api<OpenListJobsResponse>(
          `/api/media/openlist/jobs?page=${jobPage}&page_size=${AUTO_JOB_PAGE_SIZE}`,
          { signal: controller.signal },
        );
        if (!active || generation !== jobPollGeneration.current) return;
        const responsePageCount = Math.max(1, Math.ceil(response.total / AUTO_JOB_PAGE_SIZE));
        if (jobPage > responsePageCount) {
          pageRedirected = true;
          setJobTotal(response.total);
          setJobs([]);
          setJobsLoading(true);
          setJobPage(responsePageCount);
          return;
        }
        setJobs(response.records.filter((job) => job.workflow === "auto_copy"));
        setJobTotal(response.total);
        setJobsError("");
      } catch (error) {
        if (!active || generation !== jobPollGeneration.current) return;
        const requestError = error as Error;
        if (requestError.name !== "AbortError") {
          setJobsError(requestError.message || "加载自动复制任务失败");
        }
      } finally {
        if (jobPollController.current === controller) jobPollController.current = null;
        if (!active || generation !== jobPollGeneration.current) return;
        if (pageRedirected) return;
        if (showLoading) setJobsLoading(false);
        timer = window.setTimeout(() => void poll(false), AUTO_JOB_POLL_INTERVAL_MS);
      }
    }

    void poll(true);
    return () => {
      active = false;
      if (timer != null) window.clearTimeout(timer);
      controller?.abort();
      if (jobPollController.current === controller) jobPollController.current = null;
    };
  }, [jobPage, jobPollNonce]);

  useEffect(() => {
    if (
      pendingResolutionJobId != null
      && (!pendingResolutionJob || !pendingResolutionJob.copy_resolution_actions.includes("cancel"))
    ) {
      setPendingResolutionJobId(null);
    }
  }, [pendingResolutionJob, pendingResolutionJobId]);

  async function scanNow() {
    setScanBusy(true);
    try {
      const result = await api<{ discovered: number; processing_enabled: boolean }>("/api/media/openlist/scan", { method: "POST" });
      refreshJobs(1);
      setJobsNotice(
        result.processing_enabled
          ? result.discovered > 0 ? `发现 ${result.discovered} 个新任务，后台正在处理` : "扫描完成，暂无新任务"
          : result.discovered > 0 ? `已记录 ${result.discovered} 个新任务；自动复制关闭，暂不处理` : "扫描完成；自动复制关闭，任务不会处理",
      );
      setJobsError("");
    } catch (error) {
      setJobsError(error instanceof Error ? error.message : String(error));
    } finally {
      setScanBusy(false);
    }
  }

  async function clearJobs() {
    stopJobPolling();
    setClearBusy(true);
    setJobsError("");
    setJobsNotice("");
    setPendingResolutionJobId(null);
    setResolutionError("");
    setRemoteTaskStoppedConfirmed(false);
    try {
      const result = await api<ClearOpenListJobsResponse>("/api/media/openlist/jobs/clear-all", {
        method: "POST",
      });
      setJobs([]);
      setJobTotal(0);
      setJobPage(1);
      setJobsNotice(result.cleared > 0
        ? `已停止并清空 ${result.cleared} 个自动复制任务；已提交的 OpenList 远端操作无法撤回`
        : "暂无可清空的自动复制任务");
    } catch (error) {
      setJobsError(error instanceof Error ? error.message : String(error));
    } finally {
      setClearBusy(false);
      refreshJobs(1);
    }
  }

  async function resolveCopy(
    job: OpenListJob,
    resolution: CopyResolution,
    confirmTaskTerminated = false,
  ) {
    stopJobPolling();
    setResolvingJobId(job.id);
    setJobsError("");
    setJobsNotice("");
    setResolutionError("");
    try {
      const updated = await api<OpenListJob>(`/api/media/openlist/jobs/${job.id}/resolve-copy`, {
        method: "POST",
        body: JSON.stringify({
          resolution,
          expected_version: job.version,
          confirm_task_terminated: resolution === "cancel" && confirmTaskTerminated,
        }),
      });
      setJobs((current) => current.map((item) => item.id === updated.id ? updated : item));
      if (resolution === "cancel") setPendingResolutionJobId(null);
      setJobsNotice(resolution === "cancel"
        ? "复制任务已停止"
        : job.stage === "planning_manual_review"
          ? "已重新规划；条件满足后会继续自动复制"
          : job.copy_checkpoint?.operation === "review_existing"
            ? "已安排只读重新检查已有文件"
          : job.copy_checkpoint?.phase === "prepared"
            ? "已恢复核验；确认尚未提交后会继续复制"
            : "已安排只读重新检查");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (resolution === "cancel") setResolutionError(message);
      else setJobsError(message);
    } finally {
      setResolvingJobId(null);
      refreshJobs();
    }
  }
  const hasApiKey = Boolean(
    settings.api_key?.trim() || (settings.api_key_configured && !settings.clear_api_key),
  );
  const downloaderReferencesValid = settings.source_mappings.every(
    (mapping) => qbDownloaderIds.has(mapping.downloader_id),
  ) && settings.target_directories.every((target) => qbDownloaderIds.has(target.downloader_id));
  const selectedTargetExists = settings.target_directories.some((target) => target.id != null && target.id === settings.target_directory_id);
  const addressValid = validOpenListAddress(settings.address);
  const scanIntervalValid = Number.isInteger(settings.scan_interval_mins)
    && settings.scan_interval_mins >= 1
    && settings.scan_interval_mins <= 1_440;
  const submittedFieldsValid = Boolean(
    addressValid
      && scanIntervalValid
      && downloaderReferencesValid
      && settings.target_directories.every((target) => validTargetName(target.name))
      && Object.keys(pathErrors).length === 0,
  );
  const enablePrerequisitesValid = Boolean(
    settings.address.trim()
      && hasApiKey
      && settings.source_mappings.length > 0
      && settings.target_directories.length > 0
      && selectedTargetExists,
  );
  const configVersionAvailable = Boolean(settings.updated_at.trim());
  const valid = configVersionAvailable
    && submittedFieldsValid
    && (!settings.enabled || enablePrerequisitesValid);

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
    <>
      <Card>
        <CardHeader className="flex-row items-start justify-between gap-4">
        <div>
          <CardTitle className="flex items-center gap-2"><FolderInput className="size-5" />OpenList 自动复制</CardTitle>
          <CardDescription>追剧下载完成后复制到所选 OpenList 目录</CardDescription>
        </div>
        <label className="flex cursor-pointer items-center gap-2 text-sm font-medium">
          <input
            type="checkbox"
            className="size-4 accent-primary"
            checked={settings.enabled}
            onChange={(event) => setSettings((current) => ({ ...current, enabled: event.target.checked }))}
          />
          启用自动复制
        </label>
      </CardHeader>
      <CardContent className="flex flex-col gap-6">
        <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          <div className="flex flex-col gap-2 md:col-span-2 xl:col-span-2">
            <Label htmlFor="openlist-address">OpenList 地址</Label>
            <Input id="openlist-address" aria-invalid={!addressValid} aria-describedby="openlist-address-error" value={settings.address} placeholder="https://openlist.example.com" onChange={(event) => setSettings((current) => ({ ...current, address: event.target.value }))} />
            <FieldError id="openlist-address-error">{addressValid ? undefined : "请输入不含凭据、查询或片段的 HTTP(S) 地址"}</FieldError>
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="openlist-scan-interval">扫描间隔（分钟）</Label>
            <Input id="openlist-scan-interval" type="number" min={1} max={1440} aria-invalid={!scanIntervalValid} aria-describedby="openlist-scan-interval-error" value={settings.scan_interval_mins} onChange={(event) => setSettings((current) => ({ ...current, scan_interval_mins: Number(event.target.value) }))} />
            <FieldError id="openlist-scan-interval-error">{scanIntervalValid ? undefined : "请输入 1 到 1440 之间的整数"}</FieldError>
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
            {settings.enabled && !hasApiKey ? <p className="text-xs text-destructive">启用自动复制前必须配置 API Key</p> : null}
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
              <div className="flex flex-col gap-2"><Label htmlFor={`source-qb-${index}`}>下载器</Label><Select id={`source-qb-${index}`} value={String(mapping.downloader_id)} options={downloaderOptions} aria-invalid={!qbDownloaderIds.has(mapping.downloader_id)} aria-describedby={`source-qb-downloader-error-${index}`} onChange={(value) => updateSource(index, { downloader_id: Number(value) })} /><FieldError id={`source-qb-downloader-error-${index}`}>{qbDownloaderIds.has(mapping.downloader_id) ? undefined : "所选下载器已不可用"}</FieldError></div>
              <div className="flex flex-col gap-2"><Label htmlFor={`source-qb-path-${index}`}>qB 路径</Label><Input id={`source-qb-path-${index}`} aria-invalid={Boolean(pathErrors[`source-${index}-qb`])} aria-describedby={`source-qb-error-${index}`} value={mapping.qb_path} placeholder="/pt" onChange={(event) => updateSource(index, { qb_path: event.target.value })} /><FieldError id={`source-qb-error-${index}`}>{pathErrors[`source-${index}-qb`]}</FieldError></div>
              <div className="flex flex-col gap-2"><Label htmlFor={`source-openlist-path-${index}`}>OpenList 路径</Label><Input id={`source-openlist-path-${index}`} aria-invalid={Boolean(pathErrors[`source-${index}-openlist`])} aria-describedby={`source-openlist-error-${index}`} value={mapping.openlist_path} placeholder="/local/pt" onChange={(event) => updateSource(index, { openlist_path: event.target.value })} /><FieldError id={`source-openlist-error-${index}`}>{pathErrors[`source-${index}-openlist`]}</FieldError></div>
              <Button type="button" variant="destructive" className="mt-0 size-10 px-0 md:mt-7" title="删除映射" aria-label="删除映射" onClick={() => setSettings((current) => ({ ...current, source_mappings: current.source_mappings.filter((_, itemIndex) => itemIndex !== index) }))}><Trash2 /></Button>
            </div>
          ))}
        </section>

        <section className="flex flex-col gap-3" aria-labelledby="openlist-target-title">
          <div className="flex items-center justify-between gap-3">
            <div><h3 id="openlist-target-title" className="font-semibold">目标目录</h3><p className="mt-1 text-xs text-muted">OpenList 根目录用于自动复制；qB 字段只供独立的手动迁移使用</p></div>
            <Button type="button" variant="outline" disabled={qbDownloaders.length === 0} onClick={() => setSettings((current) => ({ ...current, target_directories: [...current.target_directories, { id: -Date.now(), name: "", downloader_id: qbDownloaders[0]?.id ?? 0, openlist_root: "", qb_root: "" }] }))}><Plus data-icon="inline-start" />新增目录</Button>
          </div>
          {settings.target_directories.map((target, index) => (
            <div key={target.id ?? `target-${index}`} className="grid gap-3 rounded-2xl border border-border bg-surface-container/45 p-4 md:grid-cols-2 xl:grid-cols-[0.7fr_0.8fr_1fr_1fr_40px] xl:items-start">
              <div className="flex flex-col gap-2"><Label htmlFor={`target-name-${index}`}>名称</Label><Input id={`target-name-${index}`} aria-invalid={!validTargetName(target.name)} aria-describedby={`target-name-error-${index}`} value={target.name} placeholder="媒体库" onChange={(event) => updateTarget(index, { name: event.target.value })} /><FieldError id={`target-name-error-${index}`}>{validTargetName(target.name) ? undefined : "名称不能为空且不能超过 100 字节"}</FieldError></div>
              <div className="flex flex-col gap-2"><Label htmlFor={`target-qb-${index}`}>手动迁移目标 qB</Label><Select id={`target-qb-${index}`} value={String(target.downloader_id)} options={downloaderOptions} aria-invalid={!qbDownloaderIds.has(target.downloader_id)} aria-describedby={`target-qb-downloader-error-${index}`} onChange={(value) => updateTarget(index, { downloader_id: Number(value) })} /><FieldError id={`target-qb-downloader-error-${index}`}>{qbDownloaderIds.has(target.downloader_id) ? undefined : "所选下载器已不可用"}</FieldError></div>
              <div className="flex flex-col gap-2"><Label htmlFor={`target-openlist-${index}`}>OpenList 根目录</Label><Input id={`target-openlist-${index}`} aria-invalid={Boolean(pathErrors[`target-${index}-openlist`])} aria-describedby={`target-openlist-error-${index}`} value={target.openlist_root} placeholder="/media" onChange={(event) => updateTarget(index, { openlist_root: event.target.value })} /><FieldError id={`target-openlist-error-${index}`}>{pathErrors[`target-${index}-openlist`]}</FieldError></div>
              <div className="flex flex-col gap-2"><Label htmlFor={`target-qb-root-${index}`}>手动迁移 qB 根目录</Label><Input id={`target-qb-root-${index}`} aria-invalid={Boolean(pathErrors[`target-${index}-qb`])} aria-describedby={`target-qb-error-${index}`} value={target.qb_root} placeholder="/downloads/media" onChange={(event) => updateTarget(index, { qb_root: event.target.value })} /><FieldError id={`target-qb-error-${index}`}>{pathErrors[`target-${index}-qb`]}</FieldError></div>
              <Button type="button" variant="destructive" className="size-10 px-0 xl:mt-7" title="删除目标目录" aria-label="删除目标目录" onClick={() => setSettings((current) => ({ ...current, target_directory_id: current.target_directory_id === target.id ? null : current.target_directory_id, target_directories: current.target_directories.filter((_, itemIndex) => itemIndex !== index) }))}><Trash2 /></Button>
            </div>
          ))}
          <div className="flex max-w-xl flex-col gap-2">
            <Label htmlFor="openlist-selected-target">自动复制目标</Label>
            <Select id="openlist-selected-target" disabled={settings.target_directories.length === 0} value={String(settings.target_directory_id ?? "")} options={settings.target_directories.filter((target): target is OpenListTargetDirectory & { id: number } => target.id != null).map((target) => ({ value: String(target.id), label: `${target.name || "未命名"} · ${target.openlist_root || "未设置路径"}` }))} onChange={(value) => setSettings((current) => ({ ...current, target_directory_id: Number(value) }))} />
            {settings.enabled && !selectedTargetExists && settings.target_directories.length > 0 ? <p className="text-xs text-destructive">请选择复制目标目录</p> : null}
          </div>
        </section>

        <section className="flex flex-col gap-3 border-t border-border pt-5" aria-labelledby="openlist-jobs-title">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <h3 id="openlist-jobs-title" className="font-semibold">自动复制任务</h3>
              <p className="mt-1 text-xs text-muted">这里只显示追剧触发的复制；手动 qB 迁移在“种子转移”中单独管理</p>
            </div>
            <div className="flex flex-wrap justify-end gap-2">
              <Button
                type="button"
                variant="destructive"
                disabled={jobsLoading || clearBusy || jobTotal === 0}
                onClick={() => void clearJobs()}
                title="停止并清空所有自动复制任务"
                aria-label="停止并清空所有自动复制任务"
              >
                {clearBusy ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <Trash2 data-icon="inline-start" />}
                {clearBusy ? "清空中" : "停止并清空"}
              </Button>
              <Button type="button" variant="outline" disabled={jobsLoading} onClick={() => refreshJobs()} title="刷新任务列表">
                {jobsLoading ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <RefreshCw data-icon="inline-start" />}
                刷新
              </Button>
              <Button type="button" disabled={scanBusy || clearBusy} onClick={() => void scanNow()} title="立即扫描并记录已提交的种子">
                {scanBusy ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <ScanLine data-icon="inline-start" />}
                {scanBusy ? "扫描中" : "立即扫描"}
              </Button>
            </div>
          </div>
          {!settings.enabled ? (
            <div className="rounded-2xl border border-border bg-surface-container/45 px-4 py-3 text-sm text-muted">自动复制当前关闭：不会启动新的复制；已经提交给 OpenList 的任务只会继续做只读核验。</div>
          ) : null}
          {jobsNotice ? (
            <div className="flex items-start gap-2 rounded-md border border-primary/25 bg-primary/5 px-4 py-3 text-sm" role="status" aria-live="polite">
              <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-primary" />
              <span>{jobsNotice}</span>
            </div>
          ) : null}
          {jobsError ? (
            <div className={cn("flex items-start gap-2 rounded-2xl border px-4 py-3 text-sm", jobsError.includes("失败") || jobsError.includes("disabled") ? "border-destructive/25 bg-destructive/5 text-destructive" : "border-border bg-surface-container/45 text-muted")} role="alert" aria-live="assertive">
              <AlertCircle className="mt-0.5 size-4 shrink-0" />
              <span>{jobsError}</span>
            </div>
          ) : null}
          {jobsLoading && jobs.length === 0 ? (
            <div className="flex min-h-24 items-center justify-center gap-2 text-sm text-muted"><LoaderCircle className="size-4 animate-spin" />加载任务</div>
          ) : jobs.length === 0 && !jobsError ? (
            <div className="rounded-2xl border border-border bg-surface-container/45 px-4 py-6 text-center text-sm text-muted">暂无自动复制任务</div>
          ) : jobs.length > 0 ? (
            <>
              <div className="hidden lg:block">
                <Table>
                  <TableHeader><TableRow><TableHead>种子</TableHead><TableHead>状态</TableHead><TableHead>路径</TableHead><TableHead>更新时间</TableHead><TableHead className="text-right">操作</TableHead></TableRow></TableHeader>
                  <TableBody>{jobs.map((job) => <OpenListJobRow key={job.id} job={job} resolving={resolvingJobId === job.id} onRecheck={() => void resolveCopy(job, "recheck")} onCancel={() => {
                    setResolutionError("");
                    setRemoteTaskStoppedConfirmed(false);
                    setPendingResolutionJobId(job.id);
                  }} />)}</TableBody>
                </Table>
              </div>
              <div className="grid gap-3 lg:hidden">
                {jobs.map((job) => <OpenListJobItem key={job.id} job={job} resolving={resolvingJobId === job.id} onRecheck={() => void resolveCopy(job, "recheck")} onCancel={() => {
                  setResolutionError("");
                  setRemoteTaskStoppedConfirmed(false);
                  setPendingResolutionJobId(job.id);
                }} />)}
              </div>
            </>
          ) : null}
          <div className="flex flex-wrap items-center justify-between gap-3">
            <span className="text-xs text-muted">共 {jobTotal} 个 · 第 {jobPage} / {jobPageCount} 页</span>
            <div className="flex gap-2">
              <Button
                type="button"
                variant="outline"
                disabled={jobsLoading || jobPage <= 1}
                onClick={() => refreshJobs(Math.max(1, jobPage - 1))}
              >
                上一页
              </Button>
              <Button
                type="button"
                variant="outline"
                disabled={jobsLoading || jobPage >= jobPageCount}
                onClick={() => refreshJobs(Math.min(jobPageCount, jobPage + 1))}
              >
                下一页
              </Button>
            </div>
          </div>
        </section>

        {!configVersionAvailable ? (
          <div className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive" role="alert">
            <AlertCircle className="mt-0.5 size-4 shrink-0" />
            <span>未取得配置版本，请刷新页面后再保存。</span>
          </div>
        ) : null}
        <div className="flex justify-end border-t border-border pt-4">
          <Button disabled={saving || !valid} onClick={() => onSave(valid)}>{saving ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <Save data-icon="inline-start" />}{saving ? "保存中" : "保存 OpenList 设置"}</Button>
        </div>
        </CardContent>
      </Card>
      <Dialog
        open={pendingResolutionJob != null}
        onClose={() => {
          if (resolvingJobId == null) {
            setPendingResolutionJobId(null);
            setResolutionError("");
          }
        }}
        title="停止自动复制任务"
        description={pendingResolutionJob?.stage === "planning_manual_review"
          ? "任务尚未提交 OpenList 操作，可以直接停止。"
          : "仅在你已确认，且服务端确认远端任务已终止或已从 OpenList 任务列表移除后释放目标锁；状态未知时会保留锁。"}
        panelClassName="max-w-lg"
      >
        <div className="flex flex-col gap-4 p-5 sm:p-6">
          {resolutionError ? (
            <div className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive" role="alert" aria-live="assertive">
              <AlertCircle className="mt-0.5 size-4 shrink-0" />
              <span className="min-w-0 break-words">{resolutionError}</span>
            </div>
          ) : null}
          <div className="flex items-start gap-3 rounded-md border border-destructive/30 bg-destructive/5 p-4 text-sm">
            <ShieldAlert className="mt-0.5 size-5 shrink-0 text-destructive" />
            <div className="min-w-0">
              <div className="break-words font-semibold">{pendingResolutionJob?.torrent_name}</div>
              <p className="mt-1 text-muted">
                {pendingResolutionJob?.stage === "planning_manual_review"
                  ? "停止只会取消这条尚未规划成功的自动复制任务，不会操作 OpenList 或 qB。"
                  : "停止只会释放本任务的复制锁，不会删除源文件或目标文件，也不会撤销已经由 OpenList 完成的复制。"}
              </p>
            </div>
          </div>
          {pendingResolutionJob?.stage !== "planning_manual_review" ? (
            <label className="flex cursor-pointer items-start gap-3 rounded-md border border-border px-4 py-3 text-sm">
              <input
                type="checkbox"
                className="mt-0.5 size-4 shrink-0 accent-primary"
                checked={remoteTaskStoppedConfirmed}
                onChange={(event) => setRemoteTaskStoppedConfirmed(event.target.checked)}
              />
              <span>我已在 OpenList 中确认相关复制/建目录任务不再运行，允许释放目标锁。</span>
            </label>
          ) : null}
          <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
            <Button variant="outline" disabled={resolvingJobId != null} onClick={() => {
              setPendingResolutionJobId(null);
              setResolutionError("");
            }}>返回</Button>
            <Button
              variant="destructive"
              disabled={(pendingResolutionJob != null && resolvingJobId === pendingResolutionJob.id)
                || (pendingResolutionJob?.stage !== "planning_manual_review" && !remoteTaskStoppedConfirmed)}
              onClick={() => pendingResolutionJob && void resolveCopy(
                pendingResolutionJob,
                "cancel",
                remoteTaskStoppedConfirmed,
              )}
            >
              {pendingResolutionJob != null && resolvingJobId === pendingResolutionJob.id ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : null}
              {pendingResolutionJob?.stage === "planning_manual_review" ? "停止任务" : "核验并停止"}
            </Button>
          </div>
        </div>
      </Dialog>
    </>
  );
}

function OpenListJobRow({ job, resolving, onRecheck, onCancel }: { job: OpenListJob; resolving: boolean; onRecheck: () => void; onCancel: () => void }) {
  return (
    <TableRow>
      <TableCell><JobIdentity job={job} /></TableCell>
      <TableCell><JobStatus job={job} /></TableCell>
      <TableCell><JobPaths job={job} /></TableCell>
      <TableCell className="text-xs text-muted">{new Date(job.updated_at).toLocaleString()}</TableCell>
      <TableCell><JobActions job={job} resolving={resolving} onRecheck={onRecheck} onCancel={onCancel} /></TableCell>
    </TableRow>
  );
}

function OpenListJobItem({ job, resolving, onRecheck, onCancel }: { job: OpenListJob; resolving: boolean; onRecheck: () => void; onCancel: () => void }) {
  return (
    <article className="min-w-0 rounded-2xl border border-border bg-surface-container/45 p-4">
      <div className="flex min-w-0 flex-col gap-3 sm:flex-row sm:items-start sm:justify-between"><JobIdentity job={job} /><JobStatus job={job} /></div>
      <div className="mt-3"><JobPaths job={job} /></div>
      <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
        <span className="text-xs text-muted">更新于 {new Date(job.updated_at).toLocaleString()}</span>
        <JobActions job={job} resolving={resolving} onRecheck={onRecheck} onCancel={onCancel} />
      </div>
    </article>
  );
}

function JobIdentity({ job }: { job: OpenListJob }) {
  return <div className="min-w-0 max-w-72"><div className="line-clamp-2 break-words text-sm font-semibold" title={job.torrent_name}>{job.torrent_name}</div><div className="mt-1 font-mono text-xs text-muted">{job.infohash.slice(0, 12)} · #{job.id}</div></div>;
}

function JobStatus({ job }: { job: OpenListJob }) {
  const complete = job.stage === "completed";
  const paused = job.stage === "auto_copy_paused";
  const needsAttention = job.manual_resolution_allowed && !paused;
  const stopped = paused || job.stage === "cancelled";
  const Icon = complete ? CheckCircle2 : needsAttention ? AlertCircle : paused ? CirclePause : Clock3;
  return <div className="min-w-0 max-w-64 text-xs" aria-live="polite"><span className={cn("inline-flex items-center gap-1 font-medium", complete ? "text-primary" : needsAttention ? "text-destructive" : stopped ? "text-muted" : "text-foreground")}><Icon className="size-3.5 shrink-0" />{JOB_STAGE_LABELS[job.stage] ?? job.stage}</span>{job.copy_checkpoint ? <div className="mt-1 max-w-64 truncate text-muted" title={job.copy_checkpoint.path}>核验：{job.copy_checkpoint.path}</div> : null}{job.attempts > 0 ? <div className="mt-1 text-muted">连续失败 {job.attempts} 次</div> : null}{job.last_error ? <div className={cn("mt-1 line-clamp-3 break-words", paused ? "text-muted" : "text-destructive")} title={job.last_error}>{job.last_error}</div> : null}</div>;
}

function JobPaths({ job }: { job: OpenListJob }) {
  const source = job.source_openlist_path || job.source_qb_path;
  const target = job.target_openlist_path || job.target_qb_path;
  return <div className="min-w-0 max-w-xl text-xs"><div className="truncate text-muted" title={source || "等待识别"}>源：{source || "等待识别"}</div><div className="mt-1 truncate text-muted" title={target || "等待规划"}>目标：{target || "等待规划"}</div></div>;
}

function JobActions({ job, resolving, onRecheck, onCancel }: { job: OpenListJob; resolving: boolean; onRecheck: () => void; onCancel: () => void }) {
  if (!job.manual_resolution_allowed) return <div />;
  const canRecheck = job.copy_resolution_actions.includes("recheck");
  const canCancel = job.copy_resolution_actions.includes("cancel");
  const recheckLabel = job.stage === "planning_manual_review"
    ? "重新规划"
    : job.copy_checkpoint?.operation !== "review_existing" &&
        job.copy_checkpoint?.phase === "prepared" &&
        (job.copy_checkpoint.operation === "copy_file" ||
          job.copy_checkpoint.operation === "create_directory")
      ? "继续复制"
      : "重新检查";
  return (
    <div className="flex flex-wrap justify-end gap-2">
      {canRecheck ? <Button type="button" variant="outline" className="h-8 px-3 text-xs" disabled={resolving} onClick={onRecheck}>{resolving ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <RefreshCw data-icon="inline-start" />}{recheckLabel}</Button> : null}
      {canCancel ? <Button type="button" variant="destructive" className="h-8 px-3 text-xs" disabled={resolving} onClick={onCancel}><X data-icon="inline-start" />停止</Button> : null}
    </div>
  );
}
