import { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, Eye, Pause, Play, Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate, statusBadge } from "@/lib/format";
import type { DownloadRecord, DownloaderRecord, RssSubscription, TaskRecordsResponse } from "@/types";

type TaskForm = {
  name: string;
  url: string;
  autoStart: boolean;
  downloaderId: number;
};

export function TasksPage({
  tasks,
  form,
  setForm,
  downloaders,
  selectedIds,
  setSelectedIds,
  onAddTask,
  onStartTask,
  onPauseTask,
  onDeleteTask,
  onStartSelected,
  onPauseSelected,
  onDeleteSelected,
  onStartAll,
  onPauseAll,
  onDeleteAll,
}: {
  tasks: RssSubscription[];
  form: TaskForm;
  setForm: React.Dispatch<React.SetStateAction<TaskForm>>;
  downloaders: DownloaderRecord[];
  selectedIds: number[];
  setSelectedIds: React.Dispatch<React.SetStateAction<number[]>>;
  onAddTask: () => Promise<void>;
  onStartTask: (id: number) => Promise<void>;
  onPauseTask: (id: number) => Promise<void>;
  onDeleteTask: (id: number) => Promise<void>;
  onStartSelected: () => Promise<void>;
  onPauseSelected: () => Promise<void>;
  onDeleteSelected: () => Promise<void>;
  onStartAll: () => Promise<void>;
  onPauseAll: () => Promise<void>;
  onDeleteAll: () => Promise<void>;
}) {
  const [selectedTask, setSelectedTask] = useState<RssSubscription | null>(null);
  const [details, setDetails] = useState<TaskRecordsResponse | null>(null);
  const [loadingDetails, setLoadingDetails] = useState(false);
  const [page, setPage] = useState(1);

  useEffect(() => {
    setSelectedIds((prev) => prev.filter((id) => tasks.some((task) => task.id === id)));
  }, [setSelectedIds, tasks]);

  useEffect(() => {
    if (!selectedTask) {
      setDetails(null);
      setPage(1);
      return;
    }

    setLoadingDetails(true);
    api<TaskRecordsResponse>(`/api/tasks/${selectedTask.id}/records?page=${page}&page_size=10`)
      .then(setDetails)
      .finally(() => setLoadingDetails(false));
  }, [page, selectedTask]);

  const allSelected = tasks.length > 0 && selectedIds.length === tasks.length;
  const totalPages = details ? Math.max(1, Math.ceil(details.total_records / details.page_size)) : 1;

  function toggleSelection(id: number, checked: boolean) {
    setSelectedIds((prev) => {
      if (checked) {
        return prev.includes(id) ? prev : [...prev, id];
      }
      return prev.filter((item) => item !== id);
    });
  }

  function toggleAll(checked: boolean) {
    setSelectedIds(checked ? tasks.map((task) => task.id) : []);
  }

  return (
    <>
      <div className="grid gap-4 xl:gap-6">
        <Card className="rounded-[22px] border-border bg-surface-container/30 shadow-sm overflow-hidden">
          <CardHeader className="pb-3">
            <CardTitle className="text-lg">任务管理</CardTitle>
            <CardDescription className="text-[11px]">配置 RSS 订阅任务及其自动化策略。</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-[200px_1fr_200px_auto]">
              <Input
                className="h-10 rounded-xl bg-background/50 border-border/50 text-[13px]"
                placeholder="任务名称"
                value={form.name}
                onChange={(event) => setForm((prev) => ({ ...prev, name: event.target.value }))}
              />
              <Input
                className="h-10 rounded-xl bg-background/50 border-border/50 text-[13px]"
                placeholder="RSS 地址"
                value={form.url}
                onChange={(event) => setForm((prev) => ({ ...prev, url: event.target.value }))}
              />
              <Select
                className="h-10"
                value={String(form.downloaderId)}
                onChange={(val) => setForm((prev) => ({ ...prev, downloaderId: Number(val) }))}
                options={[
                  { value: "0", label: "仅下载种子文件" },
                  ...downloaders.map((dl) => ({ value: String(dl.id), label: dl.name })),
                ]}
              />
              <Button className="h-10 rounded-xl px-6 font-semibold shadow-glow sm:col-span-2 lg:col-span-1" onClick={() => void onAddTask()}>
                <Plus className="mr-2 h-4 w-4" />
                添加任务
              </Button>
            </div>

            <div className="flex items-center justify-between px-1">
              <label className="flex items-center gap-3 cursor-pointer group">
                <input
                  type="checkbox"
                  className="h-4 w-4 rounded border border-border accent-[hsl(var(--primary))] transition-all group-hover:scale-110"
                  checked={form.autoStart}
                  onChange={(event) => setForm((prev) => ({ ...prev, autoStart: event.target.checked }))}
                />
                <span className="text-xs text-muted-foreground group-hover:text-foreground transition-colors font-medium">添加后自动启动任务</span>
              </label>
            </div>
          </CardContent>
        </Card>

        <Card className="rounded-[22px] border-border bg-surface-container/30 shadow-sm overflow-hidden">
          <CardHeader className="pb-3">
            <CardTitle className="text-lg">批量操作</CardTitle>
            <CardDescription className="text-[11px]">对全部或勾选任务执行统一操作。</CardDescription>
          </CardHeader>
          <CardContent className="space-y-5">
            <div className="space-y-2">
              <div className="text-[10px] font-black uppercase tracking-wider text-primary/70 px-1">全量控制</div>
              <div className="flex flex-wrap gap-2">
                <Button size="sm" className="h-8 rounded-xl text-xs" onClick={() => void onStartAll()}>
                  <Play className="mr-2 h-3.5 w-3.5" />
                  全部启动
                </Button>
                <Button size="sm" variant="secondary" className="h-8 rounded-xl text-xs" onClick={() => void onPauseAll()}>
                  <Pause className="mr-2 h-3.5 w-3.5" />
                  全部暂停
                </Button>
                <Button size="sm" variant="destructive" className="h-8 rounded-xl text-xs bg-destructive/10 text-destructive hover:bg-destructive/20 border-none" onClick={() => void onDeleteAll()}>
                  <Trash2 className="mr-2 h-3.5 w-3.5" />
                  全部删除
                </Button>
              </div>
            </div>

            <div className="space-y-3 pt-2 border-t border-border/30">
              <div className="text-[10px] font-black uppercase tracking-wider text-primary/70 px-1">勾选控制 ({selectedIds.length})</div>
              <div className="flex flex-wrap gap-2">
                <Button 
                  size="sm" 
                  variant="outline" 
                  className="h-8 rounded-xl text-xs border-border/50 bg-background/50" 
                  disabled={selectedIds.length === 0} 
                  onClick={() => void onStartSelected()}
                >
                  <Play className="mr-2 h-3.5 w-3.5" />
                  启动所选
                </Button>
                <Button 
                  size="sm" 
                  variant="outline" 
                  className="h-8 rounded-xl text-xs border-border/50 bg-background/50" 
                  disabled={selectedIds.length === 0} 
                  onClick={() => void onPauseSelected()}
                >
                  <Pause className="mr-2 h-3.5 w-3.5" />
                  暂停所选
                </Button>
                <Button 
                  size="sm" 
                  variant="destructive" 
                  className="h-8 rounded-xl text-xs bg-destructive/10 text-destructive hover:bg-destructive/20 border-none" 
                  disabled={selectedIds.length === 0} 
                  onClick={() => void onDeleteSelected()}
                >
                  <Trash2 className="mr-2 h-3.5 w-3.5" />
                  删除所选
                </Button>
              </div>
            </div>
          </CardContent>
        </Card>

        <Card className="rounded-[22px] border-border bg-surface-container/30 shadow-sm overflow-hidden">
          <CardHeader className="pb-3 flex flex-row items-center justify-between space-y-0">
            <div>
              <CardTitle className="text-lg">任务列表</CardTitle>
              <CardDescription className="text-[10px]">管理当前已配置的 RSS 订阅任务。</CardDescription>
            </div>
            <label className="flex items-center gap-2 cursor-pointer group bg-background/40 px-3 py-1.5 rounded-full border border-border/50 transition-all hover:bg-background/60">
              <input
                type="checkbox"
                className="h-3.5 w-3.5 rounded border border-border accent-[hsl(var(--primary))] transition-all group-hover:scale-110"
                checked={allSelected}
                onChange={(event) => toggleAll(event.target.checked)}
              />
              <span className="text-[10px] font-bold text-primary group-hover:text-primary/80 transition-colors">全选</span>
            </label>
          </CardHeader>
          <CardContent>
            <div className="grid gap-3 grid-cols-1 sm:grid-cols-2 lg:grid-cols-3">
              {tasks.map((task) => {
                return (
                  <div key={task.id} className="rounded-[20px] border border-border bg-surface-container/30 p-3.5 shadow-sm transition-all hover:bg-surface-container/50">
                    <div className="flex items-start justify-between gap-3">
                      <div className="flex items-start gap-2.5 min-w-0">
                        <input
                          type="checkbox"
                          className="mt-1 h-3.5 w-3.5 rounded border border-border accent-[hsl(var(--primary))] shrink-0"
                          checked={selectedIds.includes(task.id)}
                          onChange={(event) => toggleSelection(task.id, event.target.checked)}
                        />
                        <div className="min-w-0">
                          <div className="text-[13px] font-semibold truncate text-foreground">{task.name}</div>
                          <div className="text-[10px] text-muted-foreground mt-0.5">#{task.id}</div>
                        </div>
                      </div>
                      <span className={`shrink-0 rounded-full px-2 py-0.5 text-[10px] font-medium ${task.enabled ? "bg-emerald-500/10 text-emerald-500" : "bg-amber-500/10 text-amber-500"}`}>
                        {task.enabled ? "已启用" : "已暂停"}
                      </span>
                    </div>
                    
                    <div className="mt-2.5 break-all text-[11px] text-muted-foreground line-clamp-1">{task.url}</div>
                    {task.downloader_id ? (
                      <div className="mt-1.5 text-[10px] text-primary/80">
                        下载到：{downloaders.find((dl) => dl.id === task.downloader_id)?.name ?? `QB #${task.downloader_id}`}
                      </div>
                    ) : null}
                    <div className="mt-1.5 text-[10px] text-muted-foreground">更新：{formatDate(task.updated_at)}</div>
                    
                    <div className="mt-3.5 flex flex-wrap gap-1.5">
                      <button 
                        onClick={() => void onStartTask(task.id)}
                        className="h-7 px-2.5 rounded-lg text-[10px] font-medium bg-emerald-500/10 text-emerald-500 hover:bg-emerald-500/20 transition-colors flex items-center"
                      >
                        <Play className="mr-1 h-3 w-3" />
                        启动
                      </button>
                      <button 
                        onClick={() => void onPauseTask(task.id)}
                        className="h-7 px-2.5 rounded-lg text-[10px] font-medium bg-surface-container-highest text-foreground hover:bg-surface-container-highest/80 transition-colors flex items-center"
                      >
                        <Pause className="mr-1 h-3 w-3" />
                        暂停
                      </button>
                      <button 
                        onClick={() => { setSelectedTask(task); setPage(1); }}
                        className="h-7 px-2.5 rounded-lg text-[10px] font-medium border border-border text-foreground hover:bg-accent transition-colors flex items-center"
                      >
                        <Eye className="mr-1 h-3 w-3" />
                        记录
                      </button>
                      <button 
                        onClick={() => void onDeleteTask(task.id)}
                        className="h-7 px-2.5 rounded-lg text-[10px] font-medium bg-destructive/10 text-destructive hover:bg-destructive/20 transition-colors flex items-center"
                      >
                        <Trash2 className="mr-1 h-3 w-3" />
                        删除
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          </CardContent>
        </Card>
      </div>

      <Dialog
        open={selectedTask !== null}
        onClose={() => setSelectedTask(null)}
        title={selectedTask ? `${selectedTask.name} 的任务记录` : "任务记录"}
        description={selectedTask ? "历史记录仅可查看，不可删除。" : undefined}
      >
        <div className="space-y-4 p-4 sm:p-6">
          {loadingDetails ? <div className="text-sm text-muted">加载中...</div> : null}

          {details ? (
            <>
              <div className="grid gap-3 sm:grid-cols-3">
                <Metric label="历史总数" value={details.total_records} />
                <Metric label="当前页" value={details.page} />
                <Metric label="每页" value={details.page_size} />
              </div>

              <div className="grid gap-3">
                {details.records.map((record: DownloadRecord) => (
                  <div key={record.id} className="rounded-2xl border border-border bg-surface-container/70 p-4">
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="min-w-0">
                        <div className="font-semibold">{record.title}</div>
                        <div className="mt-1 text-xs text-muted">
                          {record.rss_name} · {formatDate(record.finished_at)}
                        </div>
                      </div>
                      <span className={`rounded-full px-3 py-1 text-xs font-medium ${statusBadge(record.final_status)}`}>
                        {record.final_status}
                      </span>
                    </div>
                    <div className="mt-3 grid gap-2 text-xs text-muted sm:grid-cols-2">
                      <div>重试次数：{record.retry_count}</div>
                      <div>刷新次数：{record.refresh_count}</div>
                    </div>
                  </div>
                ))}
              </div>

              <div className="flex flex-col gap-3 border-t border-border pt-4 sm:flex-row sm:items-center sm:justify-between">
                <div className="text-sm text-muted">
                  第 {details.page} / {totalPages} 页，共 {details.total_records} 条
                </div>
                <div className="flex gap-2">
                  <Button variant="outline" disabled={details.page <= 1} onClick={() => setPage((prev) => Math.max(1, prev - 1))}>
                    <ChevronLeft className="mr-2 h-4 w-4" />
                    上一页
                  </Button>
                  <Button
                    variant="outline"
                    disabled={details.page >= totalPages}
                    onClick={() => setPage((prev) => Math.min(totalPages, prev + 1))}
                  >
                    下一页
                    <ChevronRight className="ml-2 h-4 w-4" />
                  </Button>
                </div>
              </div>
            </>
          ) : null}
        </div>
      </Dialog>
    </>
  );
}

function Metric({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
      <div className="text-sm text-muted">{label}</div>
      <div className="mt-2 text-2xl font-semibold">{value}</div>
    </div>
  );
}
