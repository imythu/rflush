import { Suspense, lazy, useEffect, useRef, useState, type SVGProps } from "react";
import {
  BarChart3,
  ChevronDown,
  Database,
  Download,
  FileText,
  HardDrive,
  FolderInput,
  LayoutDashboard,
  Menu,
  CalendarCheck,
  Settings,
  Tag,
  Tv,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { API_BASE, APP_VERSION, api, defaultSettings } from "@/lib/api";
import type { GlobalConfig } from "@/types";

const MAX_LOG_LINES = 500;
const LOG_FLUSH_INTERVAL_MS = 250;
const LOG_LEVEL_PRIORITY = {
  trace: 10,
  debug: 20,
  info: 30,
  warn: 40,
  error: 50,
} as const;

type LogLevel = keyof typeof LOG_LEVEL_PRIORITY;
const LOG_LEVELS: LogLevel[] = ["trace", "debug", "info", "warn", "error"];

type AppPage =
  | "system-overview"
  | "media"
  | "sites"
  | "downloaders"
  | "torrent-transfer"
  | "brush-tasks"
  | "sign-in"
  | "tag-rules"
  | "stats"
  | "system-settings";

type NavGroup = "brush" | "system";

const SitesPage = lazy(() => import("@/pages/sites-page").then((module) => ({ default: module.SitesPage })));
const DownloadersPage = lazy(() => import("@/pages/downloaders-page").then((module) => ({ default: module.DownloadersPage })));
const TorrentTransferPage = lazy(() => import("@/pages/torrent-transfer-page").then((module) => ({ default: module.TorrentTransferPage })));
const BrushTasksPage = lazy(() => import("@/pages/brush-tasks-page").then((module) => ({ default: module.BrushTasksPage })));
const SignInPage = lazy(() => import("@/pages/sign-in-page").then((module) => ({ default: module.SignInPage })));
const TagRulesPage = lazy(() => import("@/pages/tag-rules-page").then((module) => ({ default: module.TagRulesPage })));
const StatsPage = lazy(() => import("@/pages/stats-page").then((module) => ({ default: module.StatsPage })));
const SystemSettingsPage = lazy(() =>
  import("@/pages/system-settings-page").then((module) => ({ default: module.SystemSettingsPage })),
);
const SystemOverviewPage = lazy(() =>
  import("@/pages/system-overview-page").then((module) => ({ default: module.SystemOverviewPage })),
);
const MediaPage = lazy(() => import("@/pages/media-page").then((module) => ({ default: module.MediaPage })));

const navItems: Array<{
  key: AppPage;
  label: string;
  description: string;
  icon: typeof LayoutDashboard;
  group: NavGroup;
}> = [
  {
    key: "media",
    label: "自动追剧",
    description: "TMDB 订阅、PT 聚合搜索与自动下载",
    icon: Tv,
    group: "brush",
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
    description: "管理下载客户端与空间状态",
    icon: HardDrive,
    group: "brush",
  },
  {
    key: "torrent-transfer",
    label: "种子转移",
    description: "选择 qBittorrent 种子并跟踪 OpenList 转移进度",
    icon: FolderInput,
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
    key: "sign-in",
    label: "自动签到",
    description: "NexusPHP 站点自动签到任务与执行记录",
    icon: CalendarCheck,
    group: "brush",
  },
  {
    key: "tag-rules",
    label: "标签规则",
    description: "根据 Tracker URL 自动匹配并管理种子标签",
    icon: Tag,
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
    "system-overview",
    "media",
    "sites",
    "downloaders",
    "torrent-transfer",
    "brush-tasks",
    "sign-in",
    "tag-rules",
    "stats",
    "system-settings",
  ];
  if (valid.includes(raw as AppPage)) {
    return raw as AppPage;
  }
  return raw === "" ? "system-overview" : "brush-tasks";
}

function setHash(page: AppPage) {
  const next = page === "system-overview" ? "#/" : `#/${page}`;
  if (window.location.hash !== next) {
    window.location.hash = next;
  }
}

function getLogsStreamUrl() {
  return `${API_BASE}/api/system/logs/stream`;
}

function extractLogLevel(line: string): LogLevel | null {
  const normalized = line.toLowerCase();
  if (normalized.includes(" trace ")) return "trace";
  if (normalized.includes(" debug ")) return "debug";
  if (normalized.includes(" info ")) return "info";
  if (normalized.includes(" warn ")) return "warn";
  if (normalized.includes(" error ")) return "error";
  return null;
}

function getEffectiveLogLevel(settings: GlobalConfig): LogLevel {
  const level = settings.log_level?.trim().toLowerCase();
  if (level && level in LOG_LEVEL_PRIORITY) {
    return level as LogLevel;
  }
  return "info";
}

export default function App() {
  const [page, setPage] = useState<AppPage>(readPageFromHash());
  const [menuOpen, setMenuOpen] = useState(false);
  const [settings, setSettings] = useState<GlobalConfig>(defaultSettings);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState("");
  const [currentTime, setCurrentTime] = useState(() => new Date());
  const [automationOpen, setAutomationOpen] = useState(true);
  const [logsOpen, setLogsOpen] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  const [logsConnected, setLogsConnected] = useState(false);
  const [logLevelFilter, setLogLevelFilter] = useState<LogLevel>("trace");
  const [logKeywordFilter, setLogKeywordFilter] = useState("");
  const logsViewportRef = useRef<HTMLDivElement | null>(null);
  const pendingLogsRef = useRef<string[]>([]);

  const currentNav =
    page === "system-overview"
      ? { key: "system-overview" as AppPage, label: "系统总览", description: "CPU、内存使用率实时监控与历史趋势", icon: LayoutDashboard, group: "system" as NavGroup }
      : navItems.find((item) => item.key === page) ?? navItems[0];
  const effectiveLogLevel = getEffectiveLogLevel(settings);
  const selectableLogLevels = LOG_LEVELS.filter(
    (level) => LOG_LEVEL_PRIORITY[level] >= LOG_LEVEL_PRIORITY[effectiveLogLevel],
  );

  useEffect(() => {
    if (logsOpen) {
      setLogLevelFilter(effectiveLogLevel);
    }
  }, [logsOpen, effectiveLogLevel]);

  async function loadSettings() {
    setSettings(await api<GlobalConfig>("/api/settings"));
  }

  useEffect(() => {
    const onHashChange = () => setPage(readPageFromHash());
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  useEffect(() => {
    loadSettings()
      .catch((error: Error) => setMessage(error.message))
      .finally(() => setLoading(false));
  }, [page]);

  useEffect(() => {
    const timer = window.setInterval(() => setCurrentTime(new Date()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!menuOpen) return;

    const previousBodyOverflow = document.body.style.overflow;
    const previousHtmlOverflow = document.documentElement.style.overflow;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setMenuOpen(false);
      }
    };
    const closeOnDesktop = () => {
      if (window.innerWidth >= 1024) {
        setMenuOpen(false);
      }
    };

    document.body.style.overflow = "hidden";
    document.documentElement.style.overflow = "hidden";
    window.addEventListener("keydown", closeOnEscape);
    window.addEventListener("resize", closeOnDesktop);

    return () => {
      document.body.style.overflow = previousBodyOverflow;
      document.documentElement.style.overflow = previousHtmlOverflow;
      window.removeEventListener("keydown", closeOnEscape);
      window.removeEventListener("resize", closeOnDesktop);
    };
  }, [menuOpen]);

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
    const lineLevel = extractLogLevel(line);
    if (lineLevel && LOG_LEVEL_PRIORITY[lineLevel] < LOG_LEVEL_PRIORITY[logLevelFilter]) {
      return false;
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

  if (loading) {
    return <div className="p-8 text-sm text-muted">加载中...</div>;
  }

  const sidebar = (
    <aside className={cn(
      "relative flex h-full min-h-0 w-full flex-col gap-4 overflow-hidden rounded-[30px] border border-border bg-card/90 p-3 shadow-card backdrop-blur-xl lg:p-4",
    )}>
      <div className="pointer-events-none absolute -left-12 -top-16 h-40 w-40 rounded-full bg-blossom/15 blur-3xl" />
      <div className="pointer-events-none absolute right-4 top-4 h-20 w-20 rounded-full border border-primary/10" />

      <div className="relative shrink-0 overflow-hidden rounded-[22px] border border-border bg-surface/70 p-3 shadow-[inset_0_1px_0_rgba(255,255,255,0.92)] lg:p-2.5">
        <div className="flex items-center gap-3">
          <button
            type="button"
            onClick={() => navigate("system-overview")}
            className="flex items-center gap-3 min-w-0 text-left transition-opacity hover:opacity-80"
          >
            <div className="flex h-12 w-12 shrink-0 items-center justify-center rounded-xl bg-gradient-to-br from-primary via-[#9a7bff] to-blossom p-1.5 shadow-glow">
              <img src="/yunmu-icon.svg" alt="云母" className="h-full w-full rounded-lg" />
            </div>
            <div className="min-w-0">
              <p className="text-[10px] font-bold uppercase tracking-[0.2em] text-primary">YUNMU</p>
              <h1 className="mt-0.5 truncate text-xl font-black tracking-tight text-foreground">云母</h1>
            </div>
          </button>
          <div className="ml-auto flex shrink-0 items-center gap-1.5">
            <a
              href="https://github.com/imythu/rflush"
              target="_blank"
              rel="noopener noreferrer"
              className="rounded-full border border-border bg-card/80 p-2 text-muted transition hover:border-primary/30 hover:text-primary"
              aria-label="GitHub 源码"
            >
              <GithubIcon className="h-4 w-4" />
            </a>
            <button
              type="button"
              className="rounded-full border border-border bg-card/80 p-2 text-muted transition hover:border-primary/30 hover:text-primary lg:hidden"
              onClick={() => setMenuOpen(false)}
              aria-label="关闭菜单"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>
      </div>

      <div className="sidebar-scroll relative min-h-0 flex-1">
        <div className="flex flex-col gap-4 pb-1 pr-1">
          <NavSection
            index="01."
            title="PT 自动化"
            open={automationOpen}
            onToggle={() => setAutomationOpen((open) => !open)}
            items={navItems.filter((item) => item.group === "brush")}
            page={page}
            navigate={navigate}
          />

          <div className="rounded-[24px] border border-border bg-surface-container/70 p-2 lg:p-1.5">
            <div className="mb-2 flex items-center justify-between px-2">
              <span className="whitespace-nowrap text-[10px] font-black tracking-[0.18em] text-primary">02. 系统</span>
              <span className="ml-3 h-px flex-1 bg-border" />
            </div>
            <div className="space-y-1.5">
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
                      title={item.label}
                      className={cn(
                        "group/item relative flex w-full items-center gap-3 rounded-2xl px-3 py-3 text-left transition-all duration-200",
                        active ? "bg-primary text-primary-foreground shadow-glow" : "hover:bg-accent",
                      )}
                    >
                      <Icon className={cn("h-5 w-5 shrink-0", active ? "text-primary-foreground" : "text-primary")} />
                      <div className="min-w-0">
                        <span className="block whitespace-nowrap text-sm font-semibold">{item.label}</span>
                        <span className={cn("mt-1 block line-clamp-1 text-[11px] leading-relaxed", active ? "text-primary-foreground/85" : "text-muted")}>
                          {item.description}
                        </span>
                      </div>
                    </button>
                  );
                })}
            </div>
          </div>
        </div>
      </div>
    </aside>
  );

  return (
    <main className="min-h-[100dvh] bg-background pb-24 text-foreground sm:px-4 sm:py-4 lg:px-6 lg:py-6 lg:pb-0">
      {/* Mobile Floating Dock */}
      <div className="mobile-dock fixed left-1/2 z-50 w-[92%] max-w-[440px] -translate-x-1/2 lg:hidden">
        <div className="rounded-[26px] border border-white/20 bg-card/80 p-2 shadow-2xl backdrop-blur-2xl flex items-center justify-between">
          <DockItem icon={Tv} active={page === "media"} onClick={() => navigate("media")} label="追剧" />
          <DockItem icon={BarChart3} active={page === "stats"} onClick={() => navigate("stats")} label="统计" />
          <DockItem icon={Download} active={page === "brush-tasks"} onClick={() => navigate("brush-tasks")} label="刷流" />
          <DockItem icon={Database} active={page === "sites"} onClick={() => navigate("sites")} label="站点" />
          <button
            type="button"
            onClick={() => setMenuOpen(true)}
            className="flex h-12 w-12 items-center justify-center rounded-2xl text-muted hover:bg-accent transition-colors"
            aria-label="打开全部菜单"
            aria-expanded={menuOpen}
            aria-controls="mobile-navigation"
          >
            <Menu className="h-6 w-6" />
          </button>
        </div>
      </div>

      <div className="app-shell-grid mx-auto grid max-w-[1720px] gap-4 lg:grid-cols-[320px_minmax(0,1fr)] lg:gap-6">
        <div className="hidden h-full min-h-0 lg:block">
          {sidebar}
        </div>

        {menuOpen ? (
          <div
            className="mobile-viewport fixed inset-0 z-[60] overflow-hidden bg-black/40 backdrop-blur-sm lg:hidden"
            onClick={() => setMenuOpen(false)}
          >
            <div
              id="mobile-navigation"
              className="mobile-menu-frame h-full w-[85vw] max-w-[320px]"
              role="dialog"
              aria-modal="true"
              aria-label="主菜单"
              onClick={(event) => event.stopPropagation()}
            >
              <div className="h-full min-h-0 animate-in slide-in-from-left duration-300">
                {sidebar}
              </div>
            </div>
          </div>
        ) : null}

        <section className="min-h-0 min-w-0 overflow-y-auto no-scrollbar">
          <header className="relative overflow-hidden rounded-[22px] border border-border bg-card/88 px-3 py-3 shadow-card backdrop-blur-xl sm:px-4 lg:rounded-[30px] lg:px-6 lg:py-5">
            <div className="pointer-events-none absolute inset-y-0 right-0 hidden w-64 bg-[radial-gradient(circle_at_top_right,rgba(255,125,168,0.18),transparent_56%),linear-gradient(135deg,transparent_40%,rgba(125,92,255,0.08))] lg:block" />
            <div className="pointer-events-none absolute bottom-4 right-8 hidden h-px w-40 bg-gradient-to-r from-transparent via-primary/30 to-transparent lg:block" />
            <div className="relative flex items-center justify-between gap-3 lg:items-start">
              <div className="flex min-w-0 items-center gap-2 lg:items-start lg:gap-3">
                <Button
                  variant="outline"
                  className="h-9 px-3 lg:hidden"
                  onClick={() => setMenuOpen(true)}
                  aria-label="打开菜单"
                  aria-expanded={menuOpen}
                  aria-controls="mobile-navigation"
                >
                  <Menu className="h-4 w-4" />
                </Button>
                <div className="min-w-0">
                  <div className={cn("hidden flex-wrap items-center gap-2 lg:flex", page === "media" && "lg:hidden")}>
                    <span className="rounded-full border border-primary/15 bg-secondary px-3 py-1 text-xs font-bold text-secondary-foreground">
                      {currentNav.label}
                    </span>
                  </div>
                  <h2 className={cn("truncate text-base font-black tracking-tight sm:text-lg lg:mt-2 lg:text-3xl", page === "media" && "lg:mt-0 lg:text-xl")}>{page === "media" ? "自动追剧" : currentNav.description}</h2>
                  {page !== "media" ? <p className="mt-1 hidden text-sm leading-6 text-muted lg:block">实时后端日志、系统时间和当前模块状态集中在右侧，页面主体保持高密度操作。</p> : null}
                </div>
              </div>

              <div className="flex shrink-0 items-center justify-end gap-2">
                <div className="hidden rounded-full border border-border bg-surface-container/80 px-3 py-2 text-sm font-medium text-muted lg:block">
                  {currentTime.toLocaleString()}
                </div>
                <Button variant="outline" className="h-9 px-3 lg:h-10 lg:px-5" onClick={() => setLogsOpen(true)}>
                  <FileText className="h-4 w-4 lg:mr-2" />
                  <span className="hidden lg:inline">实时日志</span>
                </Button>
              </div>
            </div>
          </header>

          {message ? (
            <div className="mt-4 rounded-[22px] border border-border bg-card/90 px-4 py-3 text-sm shadow-card backdrop-blur">
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
              {page === "system-overview" ? <SystemOverviewPage /> : null}
              {page === "media" ? <MediaPage /> : null}
              {page === "sites" ? <SitesPage /> : null}
              {page === "downloaders" ? <DownloadersPage /> : null}
              {page === "torrent-transfer" ? <TorrentTransferPage /> : null}
              {page === "brush-tasks" ? <BrushTasksPage /> : null}
              {page === "sign-in" ? <SignInPage /> : null}
              {page === "tag-rules" ? <TagRulesPage /> : null}
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
          <div className="grid gap-3 sm:grid-cols-[220px_minmax(0,1fr)]">
            <div className="space-y-2">
              <div className="flex items-center gap-2">
                <Select
                  className="flex-1"
                  value={logLevelFilter}
                  onChange={(val) => setLogLevelFilter(val as LogLevel)}
                  options={selectableLogLevels.map((level) => ({
                    value: level,
                    label: level.toUpperCase(),
                  }))}
                />
                <button
                  type="button"
                  className="inline-flex h-8 w-8 items-center justify-center rounded-full border border-border bg-surface-container text-xs font-semibold text-muted transition hover:text-foreground"
                  title={`当前系统日志级别是 ${effectiveLogLevel.toUpperCase()}。低于该级别的日志已被后端过滤，所以这里只能选择该级别及以上。`}
                  aria-label="查看日志级别筛选说明"
                >
                  ?
                </button>
              </div>
              <p className="text-xs leading-5 text-muted">
                当前系统日志级别：{effectiveLogLevel.toUpperCase()}。筛选项只显示该级别及以上。
              </p>
            </div>
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
  index,
  title,
  open,
  onToggle,
  items,
  page,
  navigate,
}: {
  index: string;
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
    <div className="rounded-[24px] border border-border bg-surface-container/70 p-2 lg:p-1.5 overflow-hidden">
      <button type="button" onClick={onToggle} className="flex w-full items-center justify-between rounded-2xl px-2 py-1.5 text-left transition-colors hover:bg-accent/30">
        <div className="flex items-center gap-2">
          <span className="text-[10px] font-black tracking-[0.18em] text-primary">{index}</span>
          <span className="text-[10px] font-black tracking-[0.18em] text-primary whitespace-nowrap">{title}</span>
        </div>
        <ChevronDown className={cn("h-3.5 w-3.5 text-primary transition-transform", open ? "rotate-180" : "")} />
      </button>
      {open ? (
        <nav className="mt-2 flex flex-col gap-1.5">
          {items.map((item) => {
            const Icon = item.icon;
            const active = item.key === page;
            return (
              <button
                key={item.key}
                type="button"
                onClick={() => navigate(item.key)}
                className={cn(
                  "group/item relative flex w-full items-center gap-3 rounded-2xl px-2.5 py-2.5 text-left transition-all duration-200",
                  active ? "bg-primary text-primary-foreground shadow-glow" : "hover:bg-accent",
                )}
              >
                <span
                  className={cn(
                    "flex h-9 w-9 shrink-0 items-center justify-center rounded-2xl border transition-colors",
                    active
                      ? "border-white/20 bg-white/15 text-primary-foreground"
                      : "border-primary/10 bg-card/70 text-primary group-hover/item:border-primary/30",
                  )}
                >
                  <Icon className="h-4 w-4" />
                </span>
                <div className="min-w-0">
                  <span className="block text-sm font-semibold whitespace-nowrap">{item.label}</span>
                  <span className={cn("mt-1 block text-[11px] leading-relaxed line-clamp-1", active ? "text-primary-foreground/85" : "text-muted")}>
                    {item.description}
                  </span>
                </div>
              </button>
            );
          })}
        </nav>
      ) : null}
    </div>
  );
}

function DockItem({ 
  icon: Icon, 
  active, 
  onClick, 
  label 
}: { 
  icon: typeof LayoutDashboard; 
  active: boolean; 
  onClick: () => void; 
  label: string 
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex flex-col items-center justify-center h-12 w-12 rounded-2xl transition-all duration-300",
        active ? "bg-primary text-primary-foreground shadow-glow scale-110" : "text-muted hover:bg-accent/50"
      )}
    >
      <Icon className="h-5 w-5" />
      <span className={cn("text-[9px] font-bold mt-1", active ? "block" : "hidden")}>{label}</span>
    </button>
  );
}

function GithubIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" {...props}>
      <path d="M12 0C5.37 0 0 5.37 0 12c0 5.303 3.438 9.8 8.205 11.387.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.84 1.237 1.84 1.237 1.07 1.834 2.807 1.304 3.492.997.108-.775.418-1.305.762-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.468-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.3 1.23a11.52 11.52 0 0 1 3.003-.404c1.02.005 2.047.138 3.003.404 2.29-1.552 3.297-1.23 3.297-1.23.653 1.652.242 2.873.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.61-2.807 5.625-5.479 5.921.43.372.823 1.102.823 2.222 0 1.606-.015 2.898-.015 3.293 0 .322.216.694.825.576C20.565 21.796 24 17.3 24 12c0-6.63-5.37-12-12-12z" />
    </svg>
  );
}
