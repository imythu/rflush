import { Suspense, lazy, useEffect, useMemo, useState } from "react";
import { BarChart3, BellRing, Database, Download, HardDrive, History, LayoutDashboard, Menu, RefreshCw, Settings, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { api, defaultSettings } from "@/lib/api";
import { formatDate } from "@/lib/format";
import type { BootstrapResponse, DownloadRecord, GlobalConfig, JobInfo, RssSubscription } from "@/types";

type AppPage = "dashboard" | "tasks" | "settings" | "history" | "sites" | "downloaders" | "brush-tasks" | "stats";

const DashboardPage = lazy(() => import("@/pages/dashboard-page").then((module) => ({ default: module.DashboardPage })));
const HistoryPage = lazy(() => import("@/pages/history-page").then((module) => ({ default: module.HistoryPage })));
const SettingsPage = lazy(() => import("@/pages/settings-page").then((module) => ({ default: module.SettingsPage })));
const TasksPage = lazy(() => import("@/pages/tasks-page").then((module) => ({ default: module.TasksPage })));
const SitesPage = lazy(() => import("@/pages/sites-page").then((module) => ({ default: module.SitesPage })));
const DownloadersPage = lazy(() => import("@/pages/downloaders-page").then((module) => ({ default: module.DownloadersPage })));
const BrushTasksPage = lazy(() => import("@/pages/brush-tasks-page").then((module) => ({ default: module.BrushTasksPage })));
const StatsPage = lazy(() => import("@/pages/stats-page").then((module) => ({ default: module.StatsPage })));

const navItems: Array<{
  key: AppPage;
  label: string;
  description: string;
  icon: typeof LayoutDashboard;
  group: "rss" | "brush";
}> = [
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
    description: "限流、并发和日志配置",
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
    key: "sites",
    label: "站点管理",
    description: "PT站点配置、连接测试与上传下载统计",
    icon: Database,
    group: "brush",
  },
  {
    key: "downloaders",
    label: "下载器",
    description: "管理qBittorrent等下载客户端",
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
    description: "上传下载量统计与走势图",
    icon: BarChart3,
    group: "brush",
  },
];

function readPageFromHash(): AppPage {
  const raw = window.location.hash.replace(/^#\/?/, "");
  const valid: AppPage[] = ["dashboard", "tasks", "settings", "history", "sites", "downloaders", "brush-tasks", "stats"];
  if (valid.includes(raw as AppPage)) {
    return raw as AppPage;
  }
  return "dashboard";
}

function setHash(page: AppPage) {
  const next = page === "dashboard" ? "#/" : `#/${page}`;
  if (window.location.hash !== next) {
    window.location.hash = next;
  }
}

export default function App() {
  const [page, setPage] = useState<AppPage>(readPageFromHash());
  const [menuOpen, setMenuOpen] = useState(false);
  const [settings, setSettings] = useState<GlobalConfig>(defaultSettings);
  const [tasks, setTasks] = useState<RssSubscription[]>([]);
  const [history, setHistory] = useState<DownloadRecord[]>([]);
  const [jobs, setJobs] = useState<JobInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState("");
  const [taskForm, setTaskForm] = useState({ name: "", url: "", autoStart: true });
  const [selectedIds, setSelectedIds] = useState<number[]>([]);
  const [deleteFiles, setDeleteFiles] = useState(false);
  const [refreshIntervalMs, setRefreshIntervalMsState] = useState<number>(() => {
    const saved = localStorage.getItem("rflush-refresh-interval");
    return saved !== null ? Number(saved) : 10000;
  });

  function setRefreshInterval(ms: number) {
    localStorage.setItem("rflush-refresh-interval", String(ms));
    setRefreshIntervalMsState(ms);
  }

  const runningJobs = useMemo(
    () => jobs.filter((job) => job.status === "queued" || job.status === "running"),
    [jobs],
  );

  const currentNav = navItems.find((item) => item.key === page) ?? navItems[0];

  async function loadBootstrap() {
    const data = await api<BootstrapResponse>("/api/bootstrap");
    setSettings(data.settings);
    setTasks(data.rss);
    setHistory(data.history);
    setJobs(data.jobs);
  }

  useEffect(() => {
    const onHashChange = () => setPage(readPageFromHash());
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  useEffect(() => {
    loadBootstrap()
      .catch((error: Error) => setMessage(error.message))
      .finally(() => setLoading(false));

    if (refreshIntervalMs === 0) return;
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") {
        loadBootstrap().catch(() => undefined);
      }
    }, refreshIntervalMs);
    return () => window.clearInterval(timer);
  }, [refreshIntervalMs]);

  function navigate(nextPage: AppPage) {
    setPage(nextPage);
    setHash(nextPage);
    setMenuOpen(false);
  }

  async function refreshWithMessage(action: Promise<unknown>, success: string) {
    try {
      await action;
      await loadBootstrap();
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
      setMessage("任务设置已保存");
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

  if (loading) {
    return <div className="p-8 text-sm text-muted">加载中...</div>;
  }

  const sidebar = (
    <aside className="flex h-full w-full flex-col gap-4 rounded-[28px] border border-border/80 bg-card/95 p-4 shadow-card lg:p-5">
      <div className="px-2 pt-1">
        <p className="text-xs font-semibold uppercase tracking-[0.22em] text-primary">rflush</p>
        <h1 className="mt-2 text-2xl font-semibold tracking-tight text-foreground">控制台</h1>
        <p className="mt-1 text-sm leading-6 text-muted">自适应任务工作台，覆盖桌面、平板与手机。</p>
      </div>

      <div className="rounded-2xl bg-surface-container px-4 py-3">
        <div className="text-xs font-semibold uppercase tracking-[0.2em] text-primary">rss种子下载</div>
        <nav className="mt-3 flex flex-col gap-2">
          {navItems.filter(i => i.group === "rss").map((item) => {
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
      </div>

      <div className="rounded-2xl bg-surface-container px-4 py-3">
        <div className="text-xs font-semibold uppercase tracking-[0.2em] text-primary">PT 刷流</div>
        <nav className="mt-3 flex flex-col gap-2">
          {navItems.filter(i => i.group === "brush").map((item) => {
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
      </div>

      <div className="rounded-2xl bg-surface-container px-4 py-3 text-sm text-muted">
        <div className="font-medium text-foreground">当前时间</div>
        <div className="mt-1">{formatDate(new Date().toISOString())}</div>
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
                    {runningJobs.length > 0 ? (
                      <span className="inline-flex items-center gap-1 rounded-full bg-sky-100 px-3 py-1 text-xs font-medium text-sky-700">
                        <BellRing className="h-3.5 w-3.5" />
                        运行中 {runningJobs.length}
                      </span>
                    ) : null}
                  </div>
                  <h2 className="mt-2 text-2xl font-semibold tracking-tight sm:text-3xl">{currentNav.description}</h2>
                  <p className="mt-1 text-sm leading-6 text-muted">左侧统一归类到“rss种子下载”，所有页面保持响应式布局。</p>
                </div>
              </div>

              <div className="flex items-center gap-2">
                <select
                  value={refreshIntervalMs}
                  onChange={(e) => setRefreshInterval(Number(e.target.value))}
                  className="rounded-xl border border-border bg-card px-3 py-2 text-sm text-foreground shadow-sm transition hover:bg-accent focus:outline-none"
                >
                  <option value={3000}>每 3 秒</option>
                  <option value={5000}>每 5 秒</option>
                  <option value={10000}>每 10 秒</option>
                  <option value={60000}>每 60 秒</option>
                  <option value={0}>不刷新</option>
                </select>
                <Button variant="outline" onClick={() => void loadBootstrap()}>
                  <RefreshCw className="mr-2 h-4 w-4" />
                  刷新数据
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
                  runningJobs={runningJobs}
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
                  jobs={jobs}
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
            </div>
          </Suspense>
        </section>
      </div>
    </main>
  );
}
