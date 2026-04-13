import { Suspense, lazy, useEffect, useRef, useState } from "react";
import {
  BarChart3,
  ChevronDown,
  Database,
  Download,
  FileText,
  HardDrive,
  History,
  LayoutDashboard,
  Menu,
  Settings,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { API_BASE, api, defaultSettings } from "@/lib/api";
import type { DownloadRecord, GlobalConfig, RssSubscription } from "@/types";

const MAX_LOG_LINES = 500;
const LOG_FLUSH_INTERVAL_MS = 250;
const LOG_LEVELS = ["all", "trace", "debug", "info", "warn", "error"] as const;
const LOG_LEVEL_PRIORITY: Record<Exclude<LogLevelFilter, "all">, number> = {
  trace: 10,
  debug: 20,
  info: 30,
  warn: 40,
  error: 50,
};

type LogLevelFilter = (typeof LOG_LEVELS)[number];

type AppPage =
  | "dashboard"
  | "tasks"
  | "settings"
  | "history"
  | "sites"
  | "downloaders"
  | "brush-tasks"
  | "stats"
  | "system-settings";

type NavGroup = "brush" | "rss" | "system";

const DashboardPage = lazy(() => import("@/pages/dashboard-page").then((module) => ({ default: module.DashboardPage })));
const HistoryPage = lazy(() => import("@/pages/history-page").then((module) => ({ default: module.HistoryPage })));
const SettingsPage = lazy(() => import("@/pages/settings-page").then((module) => ({ default: module.SettingsPage })));
const TasksPage = lazy(() => import("@/pages/tasks-page").then((module) => ({ default: module.TasksPage })));
const SitesPage = lazy(() => import("@/pages/sites-page").then((module) => ({ default: module.SitesPage })));
const DownloadersPage = lazy(() => import("@/pages/downloaders-page").then((module) => ({ default: module.DownloadersPage })));
const BrushTasksPage = lazy(() => import("@/pages/brush-tasks-page").then((module) => ({ default: module.BrushTasksPage })));
const StatsPage = lazy(() => import("@/pages/stats-page").then((module) => ({ default: module.StatsPage })));
const SystemSettingsPage = lazy(() =>
  import("@/pages/system-settings-page").then((module) => ({ default: module.SystemSettingsPage })),
);

const navItems: Array<{
  key: AppPage;
  label: string;
  description: string;
  icon: typeof LayoutDashboard;
  group: NavGroup;
}> = [
  {
    key: "sites",
    label: "站点管理",
    description: "PT站点配置、连接测试与上传下载统计",
    icon: Database,
    group: "brush",
  },
  {
    key: "downloaders",
    label: "下载器",
    description: "管理下载客户端与空间状态",
    icon: HardDrive,
    group: "brush",
  },
  {
    key: "brush-tasks",
    label: "刷流任务",
    description: "自动刷流任务配置、选种与删种规则",
    icon: Download,
    group: "brush",
  },
  {
    key: "stats",
    label: "数据统计",
    description: "上传下载量、种子数与下载器趋势",
    icon: BarChart3,
    group: "brush",
  },
  {
    key: "dashboard",
    label: "概览",
    description: "任务总览与快速操作",
    icon: LayoutDashboard,
    group: "rss",
  },
  {
    key: "tasks",
    label: "任务管理",
    description: "新增、暂停、批量处理和按任务看历史",
    icon: LayoutDashboard,
    group: "rss",
  },
  {
    key: "settings",
    label: "任务设置",
    description: "限流、并发和下载控制",
    icon: Settings,
    group: "rss",
  },
  {
    key: "history",
    label: "下载历史",
    description: "全部历史记录与种子删除标记",
    icon: History,
    group: "rss",
  },
  {
    key: "system-settings",
    label: "系统设置",
    description: "全局日志级别与系统运行设置",
    icon: Settings,
    group: "system",
  },
];

function readPageFromHash(): AppPage {
  const raw = window.location.hash.replace(/^#\/?/, "");
  const valid: AppPage[] = [
    "dashboard",
    "tasks",
    "settings",
    "history",
    "sites",
    "downloaders",
    "brush-tasks",
    "stats",
    "system-settings",
  ];
  if (valid.includes(raw as AppPage)) {
    return raw as AppPage;
  }
  return "brush-tasks";
}

function setHash(page: AppPage) {
  const next = page === "dashboard" ? "#/" : `#/${page}`;
  if (window.location.hash !== next) {
    window.location.hash = next;
  }
}

function getLogsStreamUrl() {
  return `${API_BASE}/api/system/logs/stream`;
}

function extractLogLevel(line: string): Exclude<LogLevelFilter, "all"> | null {
  const normalized = line.toLowerCase();
  if (normalized.includes(" trace ")) return "trace";
  if (normalized.includes(" debug ")) return "debug";
  if (normalized.includes(" info ")) return "info";
  if (normalized.includes(" warn ")) return "warn";
  if (normalized.includes(" error ")) return "error";
  return null;
}

export default function App() {
  const [page, setPage] = useState<AppPage>(readPageFromHash());
  const [menuOpen, setMenuOpen] = useState(false);
  const [settings, setSettings] = useState<GlobalConfig>(defaultSettings);
  const [tasks, setTasks] = useState<RssSubscription[]>([]);
  const [history, setHistory] = useState<DownloadRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState("");
  const [taskForm, setTaskForm] = useState({ name: "", url: "", autoStart: true });
  const [selectedIds, setSelectedIds] = useState<number[]>([]);
  const [deleteFiles, setDeleteFiles] = useState(false);
  const [currentTime, setCurrentTime] = useState(() => new Date());
  const [groupOpen, setGroupOpen] = useState<Record<"brush" | "rss", boolean>>({
    brush: true,
    rss: false,
  });
  const [logsOpen, setLogsOpen] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  const [logsConnected, setLogsConnected] = useState(false);
  const [logLevelFilter, setLogLevelFilter] = useState<LogLevelFilter>("all");
  const [logKeywordFilter, setLogKeywordFilter] = useState("");
  const logsViewportRef = useRef<HTMLDivElement | null>(null);
  const pendingLogsRef = useRef<string[]>([]);

  const currentNav = navItems.find((item) => item.key === page) ?? navItems[0];

  async function loadPageData(targetPage: AppPage) {
    switch (targetPage) {
      case "dashboard": {
        const [rss, history] = await Promise.all([
          api<RssSubscription[]>("/api/rss"),
          api<DownloadRecord[]>("/api/history"),
        ]);
        setTasks(rss);
        setHistory(history);
        break;
      }
      case "tasks": {
        const rss = await api<RssSubscription[]>("/api/rss");
        setTasks(rss);
        break;
      }
      case "history": {
        const history = await api<DownloadRecord[]>("/api/history");
        setHistory(history);
        break;
      }
      case "settings":
      case "system-settings": {
        const nextSettings = await api<GlobalConfig>("/api/settings");
        setSettings(nextSettings);
        break;
      }
      default:
        break;
    }
  }

  useEffect(() => {
    const onHashChange = () => setPage(readPageFromHash());
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  useEffect(() => {
    loadPageData(page)
      .catch((error: Error) => setMessage(error.message))
      .finally(() => setLoading(false));
  }, [page]);

  useEffect(() => {
    const timer = window.setInterval(() => setCurrentTime(new Date()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!logsOpen) return;

    let closed = false;
    let source: EventSource | null = null;
    let flushTimer: number | null = null;
    setLogs([]);
    pendingLogsRef.current = [];

    const enqueueLog = (line: string) => {
      pendingLogsRef.current.push(line);
    };

    const flushLogs = () => {
      if (pendingLogsRef.current.length === 0) {
        return;
      }

      const pending = pendingLogsRef.current;
      pendingLogsRef.current = [];
      setLogs((prev) => {
        const next = prev.concat(pending);
        return next.length > MAX_LOG_LINES ? next.slice(next.length - MAX_LOG_LINES) : next;
      });
    };

    flushTimer = window.setInterval(flushLogs, LOG_FLUSH_INTERVAL_MS);

    source = new EventSource(getLogsStreamUrl());
    source.onopen = () => {
      if (!closed) {
        setLogsConnected(true);
      }
    };
    source.onmessage = () => undefined;
    source.addEventListener("log", (event) => {
      if (closed) return;
      const message = event as MessageEvent<string>;
      try {
        const payload = JSON.parse(message.data) as { encoded_line?: string };
        if (typeof payload.encoded_line === "string") {
          enqueueLog(decodeURIComponent(payload.encoded_line));
        }
      } catch {
        enqueueLog(message.data);
      }
    });
    source.onerror = () => {
      if (!closed) {
        setLogsConnected(false);
      }
    };

    return () => {
      closed = true;
      setLogsConnected(false);
      if (flushTimer !== null) {
        window.clearInterval(flushTimer);
      }
      flushLogs();
      pendingLogsRef.current = [];
      source?.close();
    };
  }, [logsOpen]);

  const filteredLogs = logs.filter((line) => {
    if (logLevelFilter !== "all") {
      const lineLevel = extractLogLevel(line);
      if (!lineLevel) {
        return false;
      }
      if (LOG_LEVEL_PRIORITY[lineLevel] < LOG_LEVEL_PRIORITY[logLevelFilter]) {
        return false;
      }
    }

    if (logKeywordFilter.trim() !== "") {
      return line.toLowerCase().includes(logKeywordFilter.trim().toLowerCase());
    }

    return true;
  });

  useEffect(() => {
    if (!logsOpen) return;
    logsViewportRef.current?.scrollTo({
      top: logsViewportRef.current.scrollHeight,
      behavior: "smooth",
    });
  }, [logs, logsOpen]);

  function navigate(nextPage: AppPage) {
    setPage(nextPage);
    setHash(nextPage);
    setMenuOpen(false);
  }

  async function refreshWithMessage(action: Promise<unknown>, success: string) {
    try {
      await action;
      await loadPageData(page);
      setMessage(success);
    } catch (error) {
      setMessage((error as Error).message);
    }
  }

  async function saveSettings() {
    setSaving(true);
    try {
      const saved = await api<GlobalConfig>("/api/settings", {
        method: "PUT",
        body: JSON.stringify(settings),
      });
      setSettings(saved);
      setMessage("设置已保存");
    } catch (error) {
      setMessage((error as Error).message);
    } finally {
      setSaving(false);
    }
  }

  async function addTask() {
    await refreshWithMessage(
      api<RssSubscription>("/api/rss", {
        method: "POST",
        body: JSON.stringify({
          name: taskForm.name,
          url: taskForm.url,
          auto_start: taskForm.autoStart,
        }),
      }),
      taskForm.autoStart ? "任务已添加并自动启动" : "任务已添加",
    );
    setTaskForm({ name: "", url: "", autoStart: true });
  }

  async function startTask(id: number) {
    await refreshWithMessage(api(`/api/tasks/${id}/start`, { method: "POST" }), "任务已启动");
  }

  async function pauseTask(id: number) {
    await refreshWithMessage(api(`/api/tasks/${id}/pause`, { method: "POST" }), "任务已暂停");
  }

  async function deleteTask(id: number) {
    await refreshWithMessage(
      api(`/api/tasks/${id}/delete`, {
        method: "POST",
        body: JSON.stringify({ ids: [id], delete_files: deleteFiles }),
      }),
      deleteFiles ? "任务与种子文件已删除" : "任务已删除",
    );
  }

  async function startSelected() {
    await refreshWithMessage(
      api("/api/tasks/start", {
        method: "POST",
        body: JSON.stringify({ ids: selectedIds }),
      }),
      "所选任务已启动",
    );
  }

  async function pauseSelected() {
    await refreshWithMessage(
      api("/api/tasks/pause", {
        method: "POST",
        body: JSON.stringify({ ids: selectedIds }),
      }),
      "所选任务已暂停",
    );
  }

  async function deleteSelected() {
    await refreshWithMessage(
      api("/api/tasks/delete", {
        method: "POST",
        body: JSON.stringify({ ids: selectedIds, delete_files: deleteFiles }),
      }),
      deleteFiles ? "所选任务和种子文件已删除" : "所选任务已删除",
    );
    setSelectedIds([]);
  }

  async function startAll() {
    await refreshWithMessage(api("/api/tasks/start-all", { method: "POST" }), "全部任务已启动");
  }

  async function pauseAll() {
    await refreshWithMessage(api("/api/tasks/pause-all", { method: "POST" }), "全部任务已暂停");
  }

  async function deleteAll() {
    await refreshWithMessage(
      api("/api/tasks/delete-all", {
        method: "POST",
        body: JSON.stringify({ ids: [], delete_files: deleteFiles }),
      }),
      deleteFiles ? "全部任务和种子文件已删除" : "全部任务已删除",
    );
    setSelectedIds([]);
  }

  function toggleGroup(group: "brush" | "rss") {
    setGroupOpen((prev) => ({ ...prev, [group]: !prev[group] }));
  }

  if (loading) {
    return <div className="p-8 text-sm text-muted">加载中...</div>;
  }

  const sidebar = (
    <aside className="flex h-full w-full flex-col gap-4 rounded-[28px] border border-border/80 bg-card/95 p-4 shadow-card lg:p-5">
      <div className="px-2 pt-1">
        <p className="text-xs font-semibold uppercase tracking-[0.22em] text-primary">rflush</p>
        <h1 className="mt-2 text-2xl font-semibold tracking-tight text-foreground">控制台</h1>
        <p className="mt-1 text-sm leading-6 text-muted">PT 刷流优先展开，RSS 下载和系统配置统一收纳。</p>
      </div>

      <NavSection
        title="PT 刷流"
        open={groupOpen.brush}
        onToggle={() => toggleGroup("brush")}
        items={navItems.filter((item) => item.group === "brush")}
        page={page}
        navigate={navigate}
      />

      <NavSection
        title="RSS 下载"
        open={groupOpen.rss}
        onToggle={() => toggleGroup("rss")}
        items={navItems.filter((item) => item.group === "rss")}
        page={page}
        navigate={navigate}
      />

      <div className="rounded-2xl bg-surface-container px-4 py-3">
        {navItems
          .filter((item) => item.group === "system")
          .map((item) => {
            const Icon = item.icon;
            const active = item.key === page;
            return (
              <button
                key={item.key}
                type="button"
                onClick={() => navigate(item.key)}
                className={cn(
                  "flex w-full items-start gap-3 rounded-2xl px-4 py-3 text-left transition-all",
                  active ? "bg-primary text-primary-foreground shadow-md" : "hover:bg-accent",
                )}
              >
                <Icon className={cn("mt-0.5 h-5 w-5 shrink-0", active ? "text-primary-foreground" : "text-primary")} />
                <span className="min-w-0">
                  <span className="block text-sm font-semibold">{item.label}</span>
                  <span className={cn("mt-1 block text-xs leading-5", active ? "text-primary-foreground/85" : "text-muted")}>
                    {item.description}
                  </span>
                </span>
              </button>
            );
          })}
      </div>

    </aside>
  );

  return (
    <main className="min-h-screen bg-background px-3 py-3 text-foreground sm:px-4 sm:py-4 lg:px-6 lg:py-6">
      <div className="mx-auto grid max-w-[1600px] gap-4 lg:grid-cols-[300px_minmax(0,1fr)] lg:gap-6">
        <div className="hidden lg:block">{sidebar}</div>

        {menuOpen ? (
          <div className="fixed inset-0 z-40 bg-black/35 lg:hidden" onClick={() => setMenuOpen(false)}>
            <div className="h-full w-[88vw] max-w-[340px] p-3" onClick={(event) => event.stopPropagation()}>
              {sidebar}
            </div>
          </div>
        ) : null}

        <section className="min-w-0">
          <header className="rounded-[28px] border border-border/80 bg-card/90 px-4 py-4 shadow-card sm:px-6 sm:py-5">
            <div className="flex flex-col gap-4 lg:flex-row lg:items-center lg:justify-between">
              <div className="flex items-start gap-3">
                <Button variant="outline" className="lg:hidden" onClick={() => setMenuOpen(true)} aria-label="打开菜单">
                  <Menu className="h-4 w-4" />
                </Button>
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="rounded-full bg-secondary px-3 py-1 text-xs font-medium text-secondary-foreground">
                      {currentNav.label}
                    </span>
                  </div>
                  <h2 className="mt-2 text-2xl font-semibold tracking-tight sm:text-3xl">{currentNav.description}</h2>
                  <p className="mt-1 text-sm leading-6 text-muted">右上角可直接查看实时后端日志；系统时间由前端本地持续刷新。</p>
                </div>
              </div>

              <div className="flex flex-wrap items-center justify-end gap-2">
                <div className="rounded-full border border-border bg-surface-container px-3 py-2 text-sm text-muted">
                  {currentTime.toLocaleString()}
                </div>
                <Button variant="outline" onClick={() => setLogsOpen(true)}>
                  <FileText className="mr-2 h-4 w-4" />
                  实时日志
                </Button>
              </div>
            </div>
          </header>

          {message ? (
            <div className="mt-4 rounded-2xl border border-border bg-card px-4 py-3 text-sm shadow-card">
              <div className="flex items-start justify-between gap-3">
                <span>{message}</span>
                <button
                  type="button"
                  className="rounded-full p-1 text-muted transition hover:bg-accent hover:text-foreground"
                  onClick={() => setMessage("")}
                >
                  <X className="h-4 w-4" />
                </button>
              </div>
            </div>
          ) : null}

          <Suspense fallback={<div className="mt-4 rounded-2xl border border-border bg-card px-4 py-6 text-sm text-muted shadow-card">页面加载中...</div>}>
            <div className="mt-4">
              {page === "dashboard" ? (
                <DashboardPage
                  rss={tasks}
                  history={history}
                  onRunAll={startAll}
                  onGoRss={() => navigate("tasks")}
                  onGoHistory={() => navigate("history")}
                  onRunOne={startTask}
                />
              ) : null}

              {page === "tasks" ? (
                <TasksPage
                  tasks={tasks}
                  form={taskForm}
                  setForm={setTaskForm}
                  selectedIds={selectedIds}
                  setSelectedIds={setSelectedIds}
                  deleteFiles={deleteFiles}
                  setDeleteFiles={setDeleteFiles}
                  onAddTask={addTask}
                  onStartTask={startTask}
                  onPauseTask={pauseTask}
                  onDeleteTask={deleteTask}
                  onStartSelected={startSelected}
                  onPauseSelected={pauseSelected}
                  onDeleteSelected={deleteSelected}
                  onStartAll={startAll}
                  onPauseAll={pauseAll}
                  onDeleteAll={deleteAll}
                />
              ) : null}

              {page === "settings" ? (
                <SettingsPage settings={settings} setSettings={setSettings} saving={saving} onSave={saveSettings} />
              ) : null}

              {page === "history" ? <HistoryPage history={history} /> : null}
              {page === "sites" ? <SitesPage /> : null}
              {page === "downloaders" ? <DownloadersPage /> : null}
              {page === "brush-tasks" ? <BrushTasksPage /> : null}
              {page === "stats" ? <StatsPage /> : null}
              {page === "system-settings" ? (
                <SystemSettingsPage settings={settings} setSettings={setSettings} saving={saving} onSave={saveSettings} />
              ) : null}
            </div>
          </Suspense>
        </section>
      </div>

      <Dialog open={logsOpen} onClose={() => setLogsOpen(false)} title="实时日志" description="查看后端程序的最近日志和实时输出。">
        <div className="space-y-4 p-4 sm:p-6">
          <div className="flex items-center justify-between gap-3">
            <span
              className={cn(
                "rounded-full px-3 py-1 text-xs font-medium",
                logsConnected ? "bg-emerald-100 text-emerald-700" : "bg-amber-100 text-amber-700",
              )}
            >
              {logsConnected ? "已连接" : "连接中"}
            </span>
            <div className="flex items-center gap-2">
              <span className="text-xs text-muted">最多保留 {MAX_LOG_LINES} 行</span>
              <Button
                variant="outline"
                onClick={() => {
                  pendingLogsRef.current = [];
                  setLogs([]);
                }}
              >
              清空视图
              </Button>
            </div>
          </div>
          <div className="grid gap-3 sm:grid-cols-[180px_minmax(0,1fr)]">
            <select
              className="flex h-11 w-full rounded-2xl border border-border bg-input px-4 py-2 text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring/30"
              value={logLevelFilter}
              onChange={(event) => setLogLevelFilter(event.target.value as LogLevelFilter)}
            >
              <option value="all">全部级别</option>
              <option value="trace">TRACE</option>
              <option value="debug">DEBUG</option>
              <option value="info">INFO</option>
              <option value="warn">WARN</option>
              <option value="error">ERROR</option>
            </select>
            <Input
              value={logKeywordFilter}
              onChange={(event) => setLogKeywordFilter(event.target.value)}
              placeholder="按关键词筛选日志"
            />
          </div>
          <div
            ref={logsViewportRef}
            className="h-[60vh] overflow-auto rounded-2xl border border-border bg-slate-950 p-4 font-mono text-xs leading-6 text-slate-100"
          >
            {filteredLogs.length === 0 ? (
              <div className="text-slate-400">{logs.length === 0 ? "暂无日志输出。" : "没有匹配当前筛选条件的日志。"}</div>
            ) : (
              filteredLogs.map((line, index) => (
                <div key={`${index}-${line.slice(0, 24)}`} className="whitespace-pre-wrap break-all">
                  {line}
                </div>
              ))
            )}
          </div>
        </div>
      </Dialog>
    </main>
  );
}

function NavSection({
  title,
  open,
  onToggle,
  items,
  page,
  navigate,
}: {
  title: string;
  open: boolean;
  onToggle: () => void;
  items: Array<{
    key: AppPage;
    label: string;
    description: string;
    icon: typeof LayoutDashboard;
  }>;
  page: AppPage;
  navigate: (page: AppPage) => void;
}) {
  return (
    <div className="rounded-2xl bg-surface-container px-4 py-3">
      <button type="button" onClick={onToggle} className="flex w-full items-center justify-between text-left">
        <div className="text-xs font-semibold uppercase tracking-[0.2em] text-primary">{title}</div>
        <ChevronDown className={cn("h-4 w-4 text-primary transition-transform", open ? "rotate-180" : "")} />
      </button>
      {open ? (
        <nav className="mt-3 flex flex-col gap-2">
          {items.map((item) => {
            const Icon = item.icon;
            const active = item.key === page;
            return (
              <button
                key={item.key}
                type="button"
                onClick={() => navigate(item.key)}
                className={cn(
                  "flex w-full items-start gap-3 rounded-2xl px-4 py-3 text-left transition-all",
                  active ? "bg-primary text-primary-foreground shadow-md" : "hover:bg-accent",
                )}
              >
                <Icon className={cn("mt-0.5 h-5 w-5 shrink-0", active ? "text-primary-foreground" : "text-primary")} />
                <span className="min-w-0">
                  <span className="block text-sm font-semibold">{item.label}</span>
                  <span className={cn("mt-1 block text-xs leading-5", active ? "text-primary-foreground/85" : "text-muted")}>
                    {item.description}
                  </span>
                </span>
              </button>
            );
          })}
        </nav>
      ) : null}
    </div>
  );
}
