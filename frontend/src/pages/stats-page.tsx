import { useEffect, useRef, useState } from "react";
import {
  Activity,
  ArrowUpDown,
  BarChart3,
  HardDrive,
  RefreshCw,
  TrendingUp,
} from "lucide-react";
import {
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { api } from "@/lib/api";
import type {
  DownloaderRecord,
  DownloaderSpeedSnapshot,
  StatsOverview,
  TaskOverview,
  TaskStatsSnapshot,
} from "@/types";

/* ---------- helpers ---------- */

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`;
}

function formatTime(isoString: string): string {
  const d = new Date(isoString);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function ratio(up: number, down: number): string {
  if (down === 0) return up > 0 ? "∞" : "N/A";
  return (up / down).toFixed(2);
}

/* ---------- constants ---------- */

const TIME_RANGES = [
  { label: "1h", hours: 1 },
  { label: "6h", hours: 6 },
  { label: "12h", hours: 12 },
  { label: "24h", hours: 24 },
  { label: "7d", hours: 168 },
] as const;

const COLORS = {
  upload: "#10b981",
  download: "#0ea5e9",
  torrent: "#8b5cf6",
  grid: "#e5e7eb",
} as const;

/* ---------- component ---------- */

export function StatsPage() {
  const [overview, setOverview] = useState<StatsOverview | null>(null);
  const [selectedTaskId, setSelectedTaskId] = useState<number | -1>(-1);
  const [hours, setHours] = useState(24);
  const [snapshots, setSnapshots] = useState<TaskStatsSnapshot[]>([]);
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [selectedDownloaderId, setSelectedDownloaderId] = useState<number | -1>(-1);
  const [downloaderSnapshots, setDownloaderSnapshots] = useState<DownloaderSpeedSnapshot[]>([]);
  const [downloaderTrendLoading, setDownloaderTrendLoading] = useState(false);
  const [loading, setLoading] = useState(true);
  const [trendLoading, setTrendLoading] = useState(false);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Fetch overview
  const fetchOverview = async () => {
    try {
      const data = await api<StatsOverview>("/api/stats/overview");
      setOverview(data);
    } catch {
      /* silently ignore – overview card will stay empty */
    }
  };

  const fetchDownloaders = async () => {
    try {
      const data = await api<DownloaderRecord[]>("/api/downloaders");
      setDownloaders(data);
    } catch {
      setDownloaders([]);
    }
  };

  // Fetch trend data for selected task(s)
  const fetchTrend = async (taskId: number | -1, h: number) => {
    setTrendLoading(true);
    try {
      if (taskId === -1) {
        // Combined: fetch all tasks and merge by recorded_at
        if (!overview || overview.tasks.length === 0) {
          setSnapshots([]);
          return;
        }
        const allData = await Promise.all(
          overview.tasks.map((t) =>
            api<TaskStatsSnapshot[]>(
              `/api/stats/trend?task_id=${t.task_id}&hours=${h}`,
            ),
          ),
        );
        // Merge: sum up values per timestamp
        const map = new Map<
          string,
          {
            total_uploaded: number;
            total_downloaded: number;
            torrent_count: number;
          }
        >();
        for (const arr of allData) {
          for (const s of arr) {
            const existing = map.get(s.recorded_at);
            if (existing) {
              existing.total_uploaded += s.total_uploaded;
              existing.total_downloaded += s.total_downloaded;
              existing.torrent_count += s.torrent_count;
            } else {
              map.set(s.recorded_at, {
                total_uploaded: s.total_uploaded,
                total_downloaded: s.total_downloaded,
                torrent_count: s.torrent_count,
              });
            }
          }
        }
        const merged: TaskStatsSnapshot[] = Array.from(map.entries())
          .sort(
            ([a], [b]) => new Date(a).getTime() - new Date(b).getTime(),
          )
          .map(([recorded_at, v], i) => ({
            id: i,
            task_id: -1,
            recorded_at,
            ...v,
          }));
        setSnapshots(merged);
      } else {
        const data = await api<TaskStatsSnapshot[]>(
          `/api/stats/trend?task_id=${taskId}&hours=${h}`,
        );
        setSnapshots(data);
      }
    } catch {
      setSnapshots([]);
    } finally {
      setTrendLoading(false);
    }
  };

  const fetchDownloaderTrend = async (downloaderId: number | -1, h: number) => {
    setDownloaderTrendLoading(true);
    try {
      if (downloaderId === -1) {
        if (downloaders.length === 0) {
          setDownloaderSnapshots([]);
          return;
        }
        const allData = await Promise.all(
          downloaders.map((downloader) =>
            api<DownloaderSpeedSnapshot[]>(
              `/api/stats/downloader-speed-trend?downloader_id=${downloader.id}&hours=${h}`,
            ),
          ),
        );
        const map = new Map<string, { upload_speed: number; download_speed: number }>();
        for (const arr of allData) {
          for (const snapshot of arr) {
            const existing = map.get(snapshot.recorded_at);
            if (existing) {
              existing.upload_speed += snapshot.upload_speed;
              existing.download_speed += snapshot.download_speed;
            } else {
              map.set(snapshot.recorded_at, {
                upload_speed: snapshot.upload_speed,
                download_speed: snapshot.download_speed,
              });
            }
          }
        }
        const merged: DownloaderSpeedSnapshot[] = Array.from(map.entries())
          .sort(([a], [b]) => new Date(a).getTime() - new Date(b).getTime())
          .map(([recorded_at, value], index) => ({
            id: index,
            downloader_id: -1,
            upload_speed: value.upload_speed,
            download_speed: value.download_speed,
            recorded_at,
          }));
        setDownloaderSnapshots(merged);
      } else {
        const data = await api<DownloaderSpeedSnapshot[]>(
          `/api/stats/downloader-speed-trend?downloader_id=${downloaderId}&hours=${h}`,
        );
        setDownloaderSnapshots(data);
      }
    } catch {
      setDownloaderSnapshots([]);
    } finally {
      setDownloaderTrendLoading(false);
    }
  };

  // Initial load & auto-refresh
  useEffect(() => {
    const init = async () => {
      setLoading(true);
      await Promise.all([fetchOverview(), fetchDownloaders()]);
      setLoading(false);
    };
    void init();

    timerRef.current = setInterval(() => {
      void fetchOverview();
    }, 30_000);

    return () => {
      if (timerRef.current) clearInterval(timerRef.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Re-fetch trend when task/hours/overview changes
  useEffect(() => {
    if (overview) {
      void fetchTrend(selectedTaskId, hours);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedTaskId, hours, overview]);

  useEffect(() => {
    if (downloaders.length > 0 || selectedDownloaderId === -1) {
      void fetchDownloaderTrend(selectedDownloaderId, hours);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedDownloaderId, hours, downloaders]);

  /* ---------- chart data ---------- */

  const transferData = snapshots.map((s) => ({
    time: formatTime(s.recorded_at),
    upload: s.total_uploaded,
    download: s.total_downloaded,
  }));

  const torrentData = snapshots.map((s) => ({
    time: formatTime(s.recorded_at),
    count: s.torrent_count,
  }));

  const downloaderSpeedData = downloaderSnapshots.map((s) => ({
    time: formatTime(s.recorded_at),
    uploadSpeed: s.upload_speed,
    downloadSpeed: s.download_speed,
  }));

  /* ---------- render ---------- */

  if (loading) {
    return (
      <div className="flex items-center justify-center py-24 text-sm text-muted">
        <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
        加载统计数据…
      </div>
    );
  }

  const tasks = overview?.tasks ?? [];

  return (
    <div className="grid gap-6">
      {/* ===== Overview Section ===== */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <BarChart3 className="h-5 w-5" />
            任务概览
          </CardTitle>
          <CardDescription>
            各刷流任务的实时汇总数据，每 30 秒自动刷新。
          </CardDescription>
        </CardHeader>
        <CardContent>
          {tasks.length === 0 ? (
            <div className="rounded-2xl border border-dashed border-border bg-surface-container/60 p-5 text-sm text-muted">
              暂无任务统计数据。
            </div>
          ) : (
            <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
              {tasks.map((t) => (
                <TaskOverviewCard key={t.task_id} task={t} />
              ))}
            </div>
          )}
        </CardContent>
      </Card>

      {/* ===== Trend Chart Section ===== */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <TrendingUp className="h-5 w-5" />
            趋势图表
          </CardTitle>
          <CardDescription>
            查看上传/下载量和种子数的变化趋势。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {/* Controls */}
          <div className="flex flex-wrap items-center gap-3">
            {/* Task selector */}
            <select
              className="rounded-lg border border-border bg-surface-container/70 px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/40"
              value={selectedTaskId}
              onChange={(e) => setSelectedTaskId(Number(e.target.value))}
            >
              <option value={-1}>全部任务</option>
              {tasks.map((t) => (
                <option key={t.task_id} value={t.task_id}>
                  {t.task_name}
                </option>
              ))}
            </select>

            {/* Time range buttons */}
            <div className="flex gap-1">
              {TIME_RANGES.map((r) => (
                <Button
                  key={r.label}
                  variant={hours === r.hours ? "default" : "secondary"}
                  className="h-8 px-3 text-xs"
                  onClick={() => setHours(r.hours)}
                >
                  {r.label}
                </Button>
              ))}
            </div>

            {/* Manual refresh */}
            <Button
              variant="outline"
              className="h-8 px-3 text-xs"
              onClick={() => void fetchTrend(selectedTaskId, hours)}
              disabled={trendLoading}
            >
              <RefreshCw
                className={`mr-1 h-4 w-4 ${trendLoading ? "animate-spin" : ""}`}
              />
              刷新
            </Button>
          </div>

          {trendLoading && snapshots.length === 0 ? (
            <div className="flex items-center justify-center py-16 text-sm text-muted">
              <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
              加载趋势数据…
            </div>
          ) : snapshots.length === 0 ? (
            <div className="rounded-2xl border border-dashed border-border bg-surface-container/60 p-5 text-center text-sm text-muted">
              所选时间范围内无数据。
            </div>
          ) : (
            <div className="grid gap-6 xl:grid-cols-2">
              {/* Upload/Download trend */}
              <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
                <div className="mb-3 flex items-center gap-2 text-sm font-semibold">
                  <ArrowUpDown className="h-4 w-4" />
                  上传 / 下载趋势
                </div>
                <ResponsiveContainer width="100%" height={300}>
                  <LineChart data={transferData}>
                    <CartesianGrid
                      strokeDasharray="3 3"
                      stroke={COLORS.grid}
                    />
                    <XAxis
                      dataKey="time"
                      tick={{ fontSize: 12 }}
                      interval="preserveStartEnd"
                    />
                    <YAxis
                      tick={{ fontSize: 12 }}
                      tickFormatter={(v: number) => formatBytes(v)}
                      width={80}
                    />
                    <Tooltip
                      formatter={(value, name) => [
                        formatBytes(Number(value)),
                        name === "upload" ? "上传" : "下载",
                      ]}
                      labelFormatter={(label) => `时间: ${label}`}
                    />
                    <Legend
                      formatter={(value: string) =>
                        value === "upload" ? "上传" : "下载"
                      }
                    />
                    <Line
                      type="monotone"
                      dataKey="upload"
                      stroke={COLORS.upload}
                      strokeWidth={2}
                      dot={false}
                      activeDot={{ r: 4 }}
                    />
                    <Line
                      type="monotone"
                      dataKey="download"
                      stroke={COLORS.download}
                      strokeWidth={2}
                      dot={false}
                      activeDot={{ r: 4 }}
                    />
                  </LineChart>
                </ResponsiveContainer>
              </div>

              {/* Torrent count trend */}
              <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
                <div className="mb-3 flex items-center gap-2 text-sm font-semibold">
                  <HardDrive className="h-4 w-4" />
                  种子数趋势
                </div>
                <ResponsiveContainer width="100%" height={300}>
                  <LineChart data={torrentData}>
                    <CartesianGrid
                      strokeDasharray="3 3"
                      stroke={COLORS.grid}
                    />
                    <XAxis
                      dataKey="time"
                      tick={{ fontSize: 12 }}
                      interval="preserveStartEnd"
                    />
                    <YAxis tick={{ fontSize: 12 }} width={50} />
                    <Tooltip
                      formatter={(value) => [
                        String(value),
                        "种子数",
                      ]}
                      labelFormatter={(label) => `时间: ${label}`}
                    />
                    <Legend
                      formatter={() => "种子数"}
                    />
                    <Line
                      type="monotone"
                      dataKey="count"
                      stroke={COLORS.torrent}
                      strokeWidth={2}
                      dot={false}
                      activeDot={{ r: 4 }}
                    />
                  </LineChart>
                </ResponsiveContainer>
              </div>
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Activity className="h-5 w-5" />
            qB 上传 / 下载速度趋势
          </CardTitle>
          <CardDescription>
            查看下载器实时总上传速度和下载速度变化。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex flex-wrap items-center gap-3">
            <select
              className="rounded-lg border border-border bg-surface-container/70 px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/40"
              value={selectedDownloaderId}
              onChange={(e) => setSelectedDownloaderId(Number(e.target.value))}
            >
              <option value={-1}>全部下载器</option>
              {downloaders.map((downloader) => (
                <option key={downloader.id} value={downloader.id}>
                  {downloader.name}
                </option>
              ))}
            </select>

            <div className="flex gap-1">
              {TIME_RANGES.map((r) => (
                <Button
                  key={`downloader-${r.label}`}
                  variant={hours === r.hours ? "default" : "secondary"}
                  className="h-8 px-3 text-xs"
                  onClick={() => setHours(r.hours)}
                >
                  {r.label}
                </Button>
              ))}
            </div>

            <Button
              variant="outline"
              className="h-8 px-3 text-xs"
              onClick={() => void fetchDownloaderTrend(selectedDownloaderId, hours)}
              disabled={downloaderTrendLoading}
            >
              <RefreshCw
                className={`mr-1 h-4 w-4 ${downloaderTrendLoading ? "animate-spin" : ""}`}
              />
              刷新
            </Button>
          </div>

          {downloaderTrendLoading && downloaderSnapshots.length === 0 ? (
            <div className="flex items-center justify-center py-16 text-sm text-muted">
              <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
              加载下载器速度数据…
            </div>
          ) : downloaderSnapshots.length === 0 ? (
            <div className="rounded-2xl border border-dashed border-border bg-surface-container/60 p-5 text-center text-sm text-muted">
              所选时间范围内无下载器速度数据。
            </div>
          ) : (
            <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
              <ResponsiveContainer width="100%" height={320}>
                <LineChart data={downloaderSpeedData}>
                  <CartesianGrid strokeDasharray="3 3" stroke={COLORS.grid} />
                  <XAxis dataKey="time" tick={{ fontSize: 12 }} interval="preserveStartEnd" />
                  <YAxis
                    tick={{ fontSize: 12 }}
                    tickFormatter={(v: number) => `${(v / 1024).toFixed(0)} KB/s`}
                    width={80}
                  />
                  <Tooltip
                    formatter={(value, name) => [
                      `${(Number(value) / 1024).toFixed(1)} KB/s`,
                      name === "uploadSpeed" ? "上传速度" : "下载速度",
                    ]}
                    labelFormatter={(label) => `时间: ${label}`}
                  />
                  <Legend
                    formatter={(value: string) =>
                      value === "uploadSpeed" ? "上传速度" : "下载速度"
                    }
                  />
                  <Line
                    type="monotone"
                    dataKey="uploadSpeed"
                    stroke={COLORS.upload}
                    strokeWidth={2}
                    dot={false}
                    activeDot={{ r: 4 }}
                  />
                  <Line
                    type="monotone"
                    dataKey="downloadSpeed"
                    stroke={COLORS.download}
                    strokeWidth={2}
                    dot={false}
                    activeDot={{ r: 4 }}
                  />
                </LineChart>
              </ResponsiveContainer>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

/* ---------- sub-components ---------- */

function TaskOverviewCard({ task }: { task: TaskOverview }) {
  return (
    <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
      <div className="flex items-center justify-between gap-2">
        <div className="text-sm font-semibold truncate">{task.task_name}</div>
        <span
          className={`shrink-0 rounded-full px-2.5 py-0.5 text-xs font-medium ${
            task.enabled
              ? "bg-emerald-500/15 text-emerald-600"
              : "bg-neutral-500/15 text-neutral-500"
          }`}
        >
          {task.enabled ? "启用" : "停用"}
        </span>
      </div>

      <div className="mt-3 grid grid-cols-2 gap-3">
        <MetricItem
          icon={<TrendingUp className="h-3.5 w-3.5 text-emerald-500" />}
          label="上传"
          value={formatBytes(task.total_uploaded)}
        />
        <MetricItem
          icon={<TrendingUp className="h-3.5 w-3.5 text-sky-500" />}
          label="下载"
          value={formatBytes(task.total_downloaded)}
        />
        <MetricItem
          icon={<HardDrive className="h-3.5 w-3.5 text-violet-500" />}
          label="种子数"
          value={task.torrent_count.toString()}
        />
        <MetricItem
          icon={<Activity className="h-3.5 w-3.5 text-amber-500" />}
          label="分享率"
          value={ratio(task.total_uploaded, task.total_downloaded)}
        />
      </div>
    </div>
  );
}

function MetricItem({
  icon,
  label,
  value,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
}) {
  return (
    <div className="space-y-0.5">
      <div className="flex items-center gap-1 text-xs text-muted">
        {icon}
        {label}
      </div>
      <div className="text-sm font-semibold tracking-tight">{value}</div>
    </div>
  );
}
