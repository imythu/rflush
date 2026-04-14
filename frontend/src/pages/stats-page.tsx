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

type TimeWindow = {
  start: number;
  end: number;
};

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

function formatAxisTime(value: string | number, hours: number): string {
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) {
    return String(value);
  }

  const mm = String(d.getMinutes()).padStart(2, "0");
  const hh = String(d.getHours()).padStart(2, "0");
  const month = d.getMonth() + 1;
  const day = d.getDate();

  if (hours <= 6) {
    return `${hh}:${mm}`;
  }
  if (hours <= 12) {
    return `${hh}:00`;
  }
  if (hours <= 24) {
    return `${hh}:00`;
  }
  if (hours <= 24 * 7) {
    return `${month}/${day}`;
  }
  return `${month}/${day}`;
}

function formatTooltipTime(value: string | number, hours: number): string {
  return formatAxisTime(typeof value === "number" ? value : Number(value) || value, hours);
}

function formatSpeed(bytesPerSec: number): string {
  return `${formatBytes(bytesPerSec)}/s`;
}

function getTimeAxisProps(hours: number) {
  if (hours <= 1) {
    return { tickCount: 7, minTickGap: 24 };
  }
  if (hours <= 6) {
    return { tickCount: 7, minTickGap: 28 };
  }
  if (hours <= 12) {
    return { tickCount: 7, minTickGap: 32 };
  }
  if (hours <= 24) {
    return { tickCount: 9, minTickGap: 36 };
  }
  return { tickCount: 8, minTickGap: 42 };
}

function buildTimeTicks(window: TimeWindow, hours: number): number[] {
  const stepMs =
    hours <= 1
      ? 10 * 60_000
      : hours <= 6
        ? 30 * 60_000
        : hours <= 24
          ? 60 * 60_000
          : 24 * 60 * 60_000;

  const ticks: number[] = [];
  const first = Math.ceil(window.start / stepMs) * stepMs;
  for (let value = first; value <= window.end; value += stepMs) {
    ticks.push(value);
  }
  return ticks.length > 0 ? ticks : [window.start, window.end];
}

function minuteBucket(isoString: string): string {
  const timestamp = new Date(isoString).getTime();
  if (Number.isNaN(timestamp)) {
    return isoString;
  }
  return new Date(Math.floor(timestamp / 60_000) * 60_000).toISOString();
}

function sortSnapshots(snapshots: TaskStatsSnapshot[]): TaskStatsSnapshot[] {
  return [...snapshots].sort(
    (a, b) =>
      new Date(a.recorded_at).getTime() - new Date(b.recorded_at).getTime(),
  );
}

function toTransferDeltaSnapshots(
  snapshots: TaskStatsSnapshot[],
): TaskStatsSnapshot[] {
  const sorted = sortSnapshots(snapshots);
  let previous: TaskStatsSnapshot | null = null;
  return sorted.map((snapshot) => {
    const total_uploaded =
      previous == null
        ? 0
        : Math.max(0, snapshot.total_uploaded - previous.total_uploaded);
    const total_downloaded =
      previous == null
        ? 0
        : Math.max(0, snapshot.total_downloaded - previous.total_downloaded);
    previous = snapshot;
    return {
      ...snapshot,
      total_uploaded,
      total_downloaded,
    };
  });
}

function toTransferGrowthSnapshots(
  snapshots: TaskStatsSnapshot[],
): TaskStatsSnapshot[] {
  let uploadGrowth = 0;
  let downloadGrowth = 0;
  return toTransferDeltaSnapshots(snapshots).map((snapshot) => {
    uploadGrowth += snapshot.total_uploaded;
    downloadGrowth += snapshot.total_downloaded;
    return {
      ...snapshot,
      total_uploaded: uploadGrowth,
      total_downloaded: downloadGrowth,
    };
  });
}

function mergeTransferSnapshotsByMinute(
  snapshotGroups: TaskStatsSnapshot[][],
): TaskStatsSnapshot[] {
  const map = new Map<
    string,
    {
      total_uploaded: number;
      total_downloaded: number;
      torrent_count: number;
    }
  >();

  for (const snapshots of snapshotGroups) {
    for (const snapshot of toTransferDeltaSnapshots(snapshots)) {
      const bucket = minuteBucket(snapshot.recorded_at);
      const existing = map.get(bucket);
      if (existing) {
        existing.total_uploaded += snapshot.total_uploaded;
        existing.total_downloaded += snapshot.total_downloaded;
        existing.torrent_count = Math.max(
          existing.torrent_count,
          snapshot.torrent_count,
        );
      } else {
        map.set(bucket, {
          total_uploaded: snapshot.total_uploaded,
          total_downloaded: snapshot.total_downloaded,
          torrent_count: snapshot.torrent_count,
        });
      }
    }
  }

  return Array.from(map.entries())
    .sort(([a], [b]) => new Date(a).getTime() - new Date(b).getTime())
    .map(([recorded_at, value], index) => ({
      id: index,
      task_id: -1,
      recorded_at,
      ...value,
    }))
    .reduce<TaskStatsSnapshot[]>((acc, snapshot, index) => {
      const previous = acc[index - 1];
      acc.push({
        ...snapshot,
        total_uploaded:
          (previous?.total_uploaded ?? 0) + snapshot.total_uploaded,
        total_downloaded:
          (previous?.total_downloaded ?? 0) + snapshot.total_downloaded,
      });
      return acc;
    }, []);
}

function mergeTorrentSnapshotsByMinute(
  snapshotGroups: TaskStatsSnapshot[][],
): TaskStatsSnapshot[] {
  const map = new Map<
    string,
    {
      total_uploaded: number;
      total_downloaded: number;
      torrent_count: number;
    }
  >();

  for (const snapshots of snapshotGroups) {
    const perMinute = new Map<string, TaskStatsSnapshot>();
    for (const snapshot of sortSnapshots(snapshots)) {
      perMinute.set(minuteBucket(snapshot.recorded_at), snapshot);
    }
    for (const [bucket, snapshot] of perMinute) {
      const existing = map.get(bucket);
      if (existing) {
        existing.torrent_count += snapshot.torrent_count;
      } else {
        map.set(bucket, {
          total_uploaded: 0,
          total_downloaded: 0,
          torrent_count: snapshot.torrent_count,
        });
      }
    }
  }

  return Array.from(map.entries())
    .sort(([a], [b]) => new Date(a).getTime() - new Date(b).getTime())
    .map(([recorded_at, value], index) => ({
      id: index,
      task_id: -1,
      recorded_at,
      ...value,
    }));
}

function ratio(up: number, down: number): string {
  if (down === 0) return up > 0 ? "∞" : "N/A";
  return (up / down).toFixed(2);
}

function withinWindow(timestamp: number, window: TimeWindow): boolean {
  return timestamp >= window.start && timestamp <= window.end;
}

/* ---------- constants ---------- */

const TIME_RANGES = [
  { label: "1h", hours: 1 },
  { label: "6h", hours: 6 },
  { label: "12h", hours: 12 },
  { label: "24h", hours: 24 },
  { label: "7d", hours: 168 },
] as const;

const REFRESH_OPTIONS = [
  { label: "不刷新", value: 0 },
  { label: "3s", value: 3 },
  { label: "5s", value: 5 },
  { label: "10s", value: 10 },
  { label: "60s", value: 60 },
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
  const [selectedTransferTaskId, setSelectedTransferTaskId] = useState<number | -1>(-1);
  const [transferLineFilter, setTransferLineFilter] = useState<"both" | "upload" | "download">("both");
  const [transferTrendHours, setTransferTrendHours] = useState(24);
  const [transferRefreshSecs, setTransferRefreshSecs] = useState(0);
  const [transferSnapshots, setTransferSnapshots] = useState<TaskStatsSnapshot[]>([]);
  const [transferTimeWindow, setTransferTimeWindow] = useState<TimeWindow | null>(null);
  const [selectedTorrentTaskId, setSelectedTorrentTaskId] = useState<number | -1>(-1);
  const [torrentTrendHours, setTorrentTrendHours] = useState(24);
  const [torrentRefreshSecs, setTorrentRefreshSecs] = useState(0);
  const [torrentSnapshots, setTorrentSnapshots] = useState<TaskStatsSnapshot[]>([]);
  const [torrentTimeWindow, setTorrentTimeWindow] = useState<TimeWindow | null>(null);
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [selectedDownloaderId, setSelectedDownloaderId] = useState<number | -1>(-1);
  const [downloaderLineFilter, setDownloaderLineFilter] = useState<"both" | "upload" | "download">("both");
  const [downloaderTrendHours, setDownloaderTrendHours] = useState(24);
  const [downloaderRefreshSecs, setDownloaderRefreshSecs] = useState(0);
  const [downloaderSnapshots, setDownloaderSnapshots] = useState<DownloaderSpeedSnapshot[]>([]);
  const [downloaderTimeWindow, setDownloaderTimeWindow] = useState<TimeWindow | null>(null);
  const [downloaderTrendLoading, setDownloaderTrendLoading] = useState(false);
  const [loading, setLoading] = useState(true);
  const [transferTrendLoading, setTransferTrendLoading] = useState(false);
  const [torrentTrendLoading, setTorrentTrendLoading] = useState(false);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const transferRefreshRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const torrentRefreshRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const downloaderRefreshRef = useRef<ReturnType<typeof setInterval> | null>(null);

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
  const fetchTaskTrend = async (
    taskId: number | -1,
    h: number,
    mode: "transfer" | "torrent",
    setWindow: React.Dispatch<React.SetStateAction<TimeWindow | null>>,
    setData: React.Dispatch<React.SetStateAction<TaskStatsSnapshot[]>>,
    setLoadingState: React.Dispatch<React.SetStateAction<boolean>>,
  ) => {
    setLoadingState(true);
    try {
      const end = Date.now();
      const visibleStart = end - h * 60 * 60_000;
      const fetchStart = visibleStart - 2 * 60_000;
      setWindow({ start: visibleStart, end });
      const since = new Date(fetchStart).toISOString();
      const until = new Date(end).toISOString();

      if (taskId === -1) {
        if (!overview || overview.tasks.length === 0) {
          setData([]);
          return;
        }
        const allData = await Promise.all(
          overview.tasks.map((t) =>
            api<TaskStatsSnapshot[]>(
              `/api/stats/trend?task_id=${t.task_id}&hours=${h}&since=${encodeURIComponent(since)}&until=${encodeURIComponent(until)}`,
            ),
          ),
        );
        const merged =
          mode === "transfer"
            ? mergeTransferSnapshotsByMinute(allData)
            : mergeTorrentSnapshotsByMinute(allData);
        setData(merged);
      } else {
        const data = await api<TaskStatsSnapshot[]>(
          `/api/stats/trend?task_id=${taskId}&hours=${h}&since=${encodeURIComponent(since)}&until=${encodeURIComponent(until)}`,
        );
        setData(
          mode === "transfer"
            ? toTransferGrowthSnapshots(data)
            : sortSnapshots(data),
        );
      }
    } catch {
      setData([]);
    } finally {
      setLoadingState(false);
    }
  };

  const fetchDownloaderTrend = async (downloaderId: number | -1, h: number) => {
    setDownloaderTrendLoading(true);
    try {
      const end = Date.now();
      const visibleStart = end - h * 60 * 60_000;
      const fetchStart = visibleStart - 2 * 60_000;
      setDownloaderTimeWindow({ start: visibleStart, end });
      const since = new Date(fetchStart).toISOString();
      const until = new Date(end).toISOString();

      if (downloaderId === -1) {
        if (downloaders.length === 0) {
          setDownloaderSnapshots([]);
          return;
        }
        const allData = await Promise.all(
          downloaders.map((downloader) =>
            api<DownloaderSpeedSnapshot[]>(
              `/api/stats/downloader-speed-trend?downloader_id=${downloader.id}&hours=${h}&since=${encodeURIComponent(since)}&until=${encodeURIComponent(until)}`,
            ),
          ),
        );
        const map = new Map<string, { upload_speed: number; download_speed: number }>();
        for (const arr of allData) {
          for (const snapshot of arr) {
            const bucket = minuteBucket(snapshot.recorded_at);
            const existing = map.get(bucket);
            if (existing) {
              existing.upload_speed += snapshot.upload_speed;
              existing.download_speed += snapshot.download_speed;
            } else {
              map.set(bucket, {
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
          `/api/stats/downloader-speed-trend?downloader_id=${downloaderId}&hours=${h}&since=${encodeURIComponent(since)}&until=${encodeURIComponent(until)}`,
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

  useEffect(() => {
    if (overview) {
      void fetchTaskTrend(selectedTransferTaskId, transferTrendHours, "transfer", setTransferTimeWindow, setTransferSnapshots, setTransferTrendLoading);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedTransferTaskId, transferTrendHours, overview]);

  useEffect(() => {
    if (overview) {
      void fetchTaskTrend(selectedTorrentTaskId, torrentTrendHours, "torrent", setTorrentTimeWindow, setTorrentSnapshots, setTorrentTrendLoading);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedTorrentTaskId, torrentTrendHours, overview]);

  useEffect(() => {
    if (downloaders.length > 0 || selectedDownloaderId === -1) {
      void fetchDownloaderTrend(selectedDownloaderId, downloaderTrendHours);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedDownloaderId, downloaderTrendHours, downloaders]);

  useEffect(() => {
    if (transferRefreshRef.current) clearInterval(transferRefreshRef.current);
    if (transferRefreshSecs > 0) {
      transferRefreshRef.current = setInterval(() => {
        void fetchTaskTrend(selectedTransferTaskId, transferTrendHours, "transfer", setTransferTimeWindow, setTransferSnapshots, setTransferTrendLoading);
      }, transferRefreshSecs * 1000);
    }
    return () => {
      if (transferRefreshRef.current) clearInterval(transferRefreshRef.current);
    };
  }, [selectedTransferTaskId, transferTrendHours, transferRefreshSecs, overview]);

  useEffect(() => {
    if (torrentRefreshRef.current) clearInterval(torrentRefreshRef.current);
    if (torrentRefreshSecs > 0) {
      torrentRefreshRef.current = setInterval(() => {
        void fetchTaskTrend(selectedTorrentTaskId, torrentTrendHours, "torrent", setTorrentTimeWindow, setTorrentSnapshots, setTorrentTrendLoading);
      }, torrentRefreshSecs * 1000);
    }
    return () => {
      if (torrentRefreshRef.current) clearInterval(torrentRefreshRef.current);
    };
  }, [selectedTorrentTaskId, torrentTrendHours, torrentRefreshSecs, overview]);

  useEffect(() => {
    if (downloaderRefreshRef.current) clearInterval(downloaderRefreshRef.current);
    if (downloaderRefreshSecs > 0) {
      downloaderRefreshRef.current = setInterval(() => {
        void fetchDownloaderTrend(selectedDownloaderId, downloaderTrendHours);
      }, downloaderRefreshSecs * 1000);
    }
    return () => {
      if (downloaderRefreshRef.current) clearInterval(downloaderRefreshRef.current);
    };
  }, [selectedDownloaderId, downloaderTrendHours, downloaderRefreshSecs, downloaders]);

  /* ---------- chart data ---------- */

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
  const transferAxisProps = getTimeAxisProps(transferTrendHours);
  const torrentAxisProps = getTimeAxisProps(torrentTrendHours);
  const downloaderAxisProps = getTimeAxisProps(downloaderTrendHours);
  const currentTransferWindow = transferTimeWindow ?? {
    start: Date.now() - transferTrendHours * 60 * 60_000,
    end: Date.now(),
  };
  const currentTorrentWindow = torrentTimeWindow ?? {
    start: Date.now() - torrentTrendHours * 60 * 60_000,
    end: Date.now(),
  };
  const currentDownloaderWindow = downloaderTimeWindow ?? {
    start: Date.now() - downloaderTrendHours * 60 * 60_000,
    end: Date.now(),
  };
  const transferTicks = buildTimeTicks(currentTransferWindow, transferTrendHours);
  const torrentTicks = buildTimeTicks(currentTorrentWindow, torrentTrendHours);
  const downloaderTicks = buildTimeTicks(currentDownloaderWindow, downloaderTrendHours);
  const transferData = transferSnapshots
    .map((s) => ({
      recordedAt: s.recorded_at,
      timestamp: new Date(s.recorded_at).getTime(),
      upload: s.total_uploaded,
      download: s.total_downloaded,
    }))
    .filter((item) => withinWindow(item.timestamp, currentTransferWindow));
  const torrentData = torrentSnapshots
    .map((s) => ({
      recordedAt: s.recorded_at,
      timestamp: new Date(s.recorded_at).getTime(),
      count: s.torrent_count,
    }))
    .filter((item) => withinWindow(item.timestamp, currentTorrentWindow));
  const downloaderSpeedData = downloaderSnapshots
    .map((s) => ({
      recordedAt: s.recorded_at,
      timestamp: new Date(s.recorded_at).getTime(),
      uploadSpeed: s.upload_speed,
      downloadSpeed: s.download_speed,
    }))
    .filter((item) => withinWindow(item.timestamp, currentDownloaderWindow));

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
            查看区间上传/下载增量，以及种子数变化趋势。
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {/* Controls */}
          <div className="grid gap-6">
            <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
              <div className="mb-3 flex items-center gap-2 text-sm font-semibold">
                <ArrowUpDown className="h-4 w-4" />
                上传 / 下载区间增量
              </div>
              <div className="mb-4 flex flex-wrap items-center gap-3">
                <select
                  className="rounded-lg border border-border bg-surface-container/70 px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/40"
                  value={selectedTransferTaskId}
                  onChange={(e) => setSelectedTransferTaskId(Number(e.target.value))}
                >
                  <option value={-1}>全部任务</option>
                  {tasks.map((t) => (
                    <option key={t.task_id} value={t.task_id}>
                      {t.task_name}
                    </option>
                  ))}
                </select>
                <div className="flex gap-1">
                  {TIME_RANGES.map((r) => (
                    <Button
                      key={`transfer-${r.label}`}
                      variant={transferTrendHours === r.hours ? "default" : "secondary"}
                      className="h-8 px-3 text-xs"
                      onClick={() => setTransferTrendHours(r.hours)}
                    >
                      {r.label}
                    </Button>
                  ))}
                </div>
                <div className="flex gap-1">
                  {(["both", "upload", "download"] as const).map((f) => (
                    <Button
                      key={`transfer-filter-${f}`}
                      variant={transferLineFilter === f ? "default" : "secondary"}
                      className="h-8 px-3 text-xs"
                      onClick={() => setTransferLineFilter(f)}
                    >
                      {f === "both" ? "全部" : f === "upload" ? "上传" : "下载"}
                    </Button>
                  ))}
                </div>
                <select
                  className="rounded-lg border border-border bg-surface-container/70 px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/40"
                  value={transferRefreshSecs}
                  onChange={(e) => setTransferRefreshSecs(Number(e.target.value))}
                >
                  {REFRESH_OPTIONS.map((option) => (
                    <option key={`transfer-refresh-${option.value}`} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
                <Button
                  variant="outline"
                  className="h-8 px-3 text-xs"
                  onClick={() =>
                    void fetchTaskTrend(selectedTransferTaskId, transferTrendHours, "transfer", setTransferTimeWindow, setTransferSnapshots, setTransferTrendLoading)
                  }
                  disabled={transferTrendLoading}
                >
                  <RefreshCw className={`mr-1 h-4 w-4 ${transferTrendLoading ? "animate-spin" : ""}`} />
                  刷新
                </Button>
              </div>
              {transferTrendLoading && transferData.length === 0 ? (
                <div className="flex items-center justify-center py-16 text-sm text-muted">
                  <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
                  加载趋势数据…
                </div>
              ) : transferData.length === 0 ? (
                <div className="rounded-2xl border border-dashed border-border bg-surface-container/60 p-5 text-center text-sm text-muted">
                  所选时间范围内无数据。
                </div>
              ) : (
                <ResponsiveContainer width="100%" height={320}>
                  <LineChart data={transferData}>
                    <CartesianGrid
                      strokeDasharray="3 3"
                      stroke={COLORS.grid}
                    />
                    <XAxis
                      dataKey="timestamp"
                      type="number"
                      scale="time"
                      domain={[currentTransferWindow.start, currentTransferWindow.end]}
                      ticks={transferTicks}
                      tick={{ fontSize: 12 }}
                      tickCount={transferAxisProps.tickCount}
                      minTickGap={transferAxisProps.minTickGap}
                      tickFormatter={(value: number) => formatAxisTime(value, transferTrendHours)}
                    />
                    <YAxis
                      tick={{ fontSize: 12 }}
                      tickFormatter={(v: number) => formatBytes(v)}
                      width={96}
                    />
                    <Tooltip
                      formatter={(value, name) => [
                        formatBytes(Number(value)),
                        name === "upload" ? "上传" : "下载",
                      ]}
                      labelFormatter={(label) => `时间: ${formatTooltipTime(label, transferTrendHours)}`}
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
                      hide={transferLineFilter === "download"}
                    />
                    <Line
                      type="monotone"
                      dataKey="download"
                      stroke={COLORS.download}
                      strokeWidth={2}
                      dot={false}
                      activeDot={{ r: 4 }}
                      hide={transferLineFilter === "upload"}
                    />
                  </LineChart>
                </ResponsiveContainer>
              )}
            </div>

            <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
              <div className="mb-3 flex items-center gap-2 text-sm font-semibold">
                <HardDrive className="h-4 w-4" />
                种子数历史图
              </div>
              <div className="mb-4 flex flex-wrap items-center gap-3">
                <select
                  className="rounded-lg border border-border bg-surface-container/70 px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/40"
                  value={selectedTorrentTaskId}
                  onChange={(e) => setSelectedTorrentTaskId(Number(e.target.value))}
                >
                  <option value={-1}>全部任务</option>
                  {tasks.map((t) => (
                    <option key={t.task_id} value={t.task_id}>
                      {t.task_name}
                    </option>
                  ))}
                </select>
                <div className="flex gap-1">
                  {TIME_RANGES.map((r) => (
                    <Button
                      key={`torrent-${r.label}`}
                      variant={torrentTrendHours === r.hours ? "default" : "secondary"}
                      className="h-8 px-3 text-xs"
                      onClick={() => setTorrentTrendHours(r.hours)}
                    >
                      {r.label}
                    </Button>
                  ))}
                </div>
                <select
                  className="rounded-lg border border-border bg-surface-container/70 px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/40"
                  value={torrentRefreshSecs}
                  onChange={(e) => setTorrentRefreshSecs(Number(e.target.value))}
                >
                  {REFRESH_OPTIONS.map((option) => (
                    <option key={`torrent-refresh-${option.value}`} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
                <Button
                  variant="outline"
                  className="h-8 px-3 text-xs"
                  onClick={() =>
                    void fetchTaskTrend(selectedTorrentTaskId, torrentTrendHours, "torrent", setTorrentTimeWindow, setTorrentSnapshots, setTorrentTrendLoading)
                  }
                  disabled={torrentTrendLoading}
                >
                  <RefreshCw className={`mr-1 h-4 w-4 ${torrentTrendLoading ? "animate-spin" : ""}`} />
                  刷新
                </Button>
              </div>
              {torrentTrendLoading && torrentData.length === 0 ? (
                <div className="flex items-center justify-center py-16 text-sm text-muted">
                  <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
                  加载种子数数据…
                </div>
              ) : torrentData.length === 0 ? (
                <div className="rounded-2xl border border-dashed border-border bg-surface-container/60 p-5 text-center text-sm text-muted">
                  所选时间范围内无数据。
                </div>
              ) : (
                <ResponsiveContainer width="100%" height={320}>
                  <LineChart data={torrentData}>
                    <CartesianGrid
                      strokeDasharray="3 3"
                      stroke={COLORS.grid}
                    />
                    <XAxis
                      dataKey="timestamp"
                      type="number"
                      scale="time"
                      domain={[currentTorrentWindow.start, currentTorrentWindow.end]}
                      ticks={torrentTicks}
                      tick={{ fontSize: 12 }}
                      tickCount={torrentAxisProps.tickCount}
                      minTickGap={torrentAxisProps.minTickGap}
                      tickFormatter={(value: number) => formatAxisTime(value, torrentTrendHours)}
                    />
                    <YAxis tick={{ fontSize: 12 }} width={50} />
                    <Tooltip
                      formatter={(value) => [
                        String(value),
                        "种子数",
                      ]}
                      labelFormatter={(label) => `时间: ${formatTooltipTime(label, torrentTrendHours)}`}
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
              )}
            </div>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Activity className="h-5 w-5" />
            下载器上传 / 下载速度趋势
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
                  variant={downloaderTrendHours === r.hours ? "default" : "secondary"}
                  className="h-8 px-3 text-xs"
                  onClick={() => setDownloaderTrendHours(r.hours)}
                >
                  {r.label}
                </Button>
              ))}
            </div>
            <div className="flex gap-1">
              {(["both", "upload", "download"] as const).map((f) => (
                <Button
                  key={`downloader-filter-${f}`}
                  variant={downloaderLineFilter === f ? "default" : "secondary"}
                  className="h-8 px-3 text-xs"
                  onClick={() => setDownloaderLineFilter(f)}
                >
                  {f === "both" ? "全部" : f === "upload" ? "上传" : "下载"}
                </Button>
              ))}
            </div>
            <select
              className="rounded-lg border border-border bg-surface-container/70 px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/40"
              value={downloaderRefreshSecs}
              onChange={(e) => setDownloaderRefreshSecs(Number(e.target.value))}
            >
              {REFRESH_OPTIONS.map((option) => (
                <option key={`downloader-refresh-${option.value}`} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>

            <Button
              variant="outline"
              className="h-8 px-3 text-xs"
              onClick={() => void fetchDownloaderTrend(selectedDownloaderId, downloaderTrendHours)}
              disabled={downloaderTrendLoading}
            >
              <RefreshCw
                className={`mr-1 h-4 w-4 ${downloaderTrendLoading ? "animate-spin" : ""}`}
              />
              刷新
            </Button>
          </div>

          {downloaderTrendLoading && downloaderSpeedData.length === 0 ? (
            <div className="flex items-center justify-center py-16 text-sm text-muted">
              <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
              加载下载器速度数据…
            </div>
          ) : downloaderSpeedData.length === 0 ? (
            <div className="rounded-2xl border border-dashed border-border bg-surface-container/60 p-5 text-center text-sm text-muted">
              所选时间范围内无下载器速度数据。
            </div>
          ) : (
            <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
              <ResponsiveContainer width="100%" height={320}>
                <LineChart data={downloaderSpeedData}>
                  <CartesianGrid strokeDasharray="3 3" stroke={COLORS.grid} />
                  <XAxis
                    dataKey="timestamp"
                    type="number"
                    scale="time"
                    domain={[currentDownloaderWindow.start, currentDownloaderWindow.end]}
                    ticks={downloaderTicks}
                    tick={{ fontSize: 12 }}
                    tickCount={downloaderAxisProps.tickCount}
                    minTickGap={downloaderAxisProps.minTickGap}
                    tickFormatter={(value: number) => formatAxisTime(value, downloaderTrendHours)}
                  />
                  <YAxis
                    tick={{ fontSize: 12 }}
                    tickFormatter={(v: number) => formatSpeed(v)}
                    width={96}
                  />
                  <Tooltip
                    formatter={(value, name) => [
                      formatSpeed(Number(value)),
                      name === "uploadSpeed" ? "上传速度" : "下载速度",
                    ]}
                    labelFormatter={(label) => `时间: ${formatTooltipTime(label, downloaderTrendHours)}`}
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
                    hide={downloaderLineFilter === "download"}
                  />
                  <Line
                    type="monotone"
                    dataKey="downloadSpeed"
                    stroke={COLORS.download}
                    strokeWidth={2}
                    dot={false}
                    activeDot={{ r: 4 }}
                    hide={downloaderLineFilter === "upload"}
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
