import { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, Eye, Pause, Play, Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate } from "@/lib/format";
import type { DownloadRecord, RssSubscription, TaskRecordsResponse } from "@/types";

type TaskForm = {
  name: string;
  url: string;
  autoStart: boolean;
};

export function TasksPage({
  tasks,
  form,
  setForm,
  selectedIds,
  setSelectedIds,
  deleteFiles,
  setDeleteFiles,
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
  selectedIds: number[];
  setSelectedIds: React.Dispatch<React.SetStateAction<number[]>>;
  deleteFiles: boolean;
  setDeleteFiles: React.Dispatch<React.SetStateAction<boolean>>;
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
        <Card>
          <CardHeader>
            <CardTitle>任务管理</CardTitle>
            <CardDescription>RSS 订阅现在直接作为任务管理，支持自动启动、暂停、批量操作和按任务查看历史。</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid gap-3 xl:grid-cols-[220px_minmax(0,1fr)_auto]">
              <Input
                placeholder="任务名称"
                value={form.name}
                onChange={(event) => setForm((prev) => ({ ...prev, name: event.target.value }))}
              />
              <Input
                placeholder="RSS 地址"
                value={form.url}
                onChange={(event) => setForm((prev) => ({ ...prev, url: event.target.value }))}
              />
              <Button className="w-full xl:w-auto" onClick={() => void onAddTask()}>
                <Plus className="mr-2 h-4 w-4" />
                添加任务
              </Button>
            </div>

            <label className="flex items-center gap-3 text-sm text-muted">
              <input
                type="checkbox"
                className="h-4 w-4 rounded border border-border accent-[hsl(var(--primary))]"
                checked={form.autoStart}
                onChange={(event) => setForm((prev) => ({ ...prev, autoStart: event.target.checked }))}
              />
              添加后自动启动
            </label>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>批量操作</CardTitle>
            <CardDescription>支持对全部任务或当前勾选任务执行启动、暂停、删除。</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex flex-wrap gap-2">
              <Button onClick={() => void onStartAll()}>
                <Play className="mr-2 h-4 w-4" />
                启动全部
              </Button>
              <Button variant="secondary" onClick={() => void onPauseAll()}>
                <Pause className="mr-2 h-4 w-4" />
                暂停全部
              </Button>
              <Button variant="destructive" onClick={() => void onDeleteAll()}>
                <Trash2 className="mr-2 h-4 w-4" />
                删除全部
              </Button>
            </div>

            <div className="flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
              <div className="flex flex-wrap gap-2">
                <Button variant="outline" disabled={selectedIds.length === 0} onClick={() => void onStartSelected()}>
                  <Play className="mr-2 h-4 w-4" />
                  启动所选
                </Button>
                <Button variant="outline" disabled={selectedIds.length === 0} onClick={() => void onPauseSelected()}>
                  <Pause className="mr-2 h-4 w-4" />
                  暂停所选
                </Button>
                <Button variant="destructive" disabled={selectedIds.length === 0} onClick={() => void onDeleteSelected()}>
                  <Trash2 className="mr-2 h-4 w-4" />
                  删除所选
                </Button>
              </div>

              <label className="flex items-center gap-3 text-sm text-muted">
                <input
                  type="checkbox"
                  className="h-4 w-4 rounded border border-border accent-[hsl(var(--primary))]"
                  checked={deleteFiles}
                  onChange={(event) => setDeleteFiles(event.target.checked)}
                />
                删除任务时同时删除已下载种子文件
              </label>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>任务列表</CardTitle>
            <CardDescription>可单个操作，也可勾选多项后批量处理；任务记录支持弹窗分页查看。</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="hidden xl:block">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>
                      <input
                        type="checkbox"
                        className="h-4 w-4 rounded border border-border accent-[hsl(var(--primary))]"
                        checked={allSelected}
                        onChange={(event) => toggleAll(event.target.checked)}
                      />
                    </TableHead>
                    <TableHead>任务</TableHead>
                    <TableHead>状态</TableHead>
                    <TableHead>地址</TableHead>
                    <TableHead>更新时间</TableHead>
                    <TableHead>操作</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {tasks.map((task) => {
                    return (
                      <TableRow key={task.id}>
                        <TableCell>
                          <input
                            type="checkbox"
                            className="h-4 w-4 rounded border border-border accent-[hsl(var(--primary))]"
                            checked={selectedIds.includes(task.id)}
                            onChange={(event) => toggleSelection(task.id, event.target.checked)}
                          />
                        </TableCell>
                        <TableCell>
                          <div className="font-medium">{task.name}</div>
                          <div className="text-xs text-muted">#{task.id}</div>
                        </TableCell>
                        <TableCell>
                          <div className="flex flex-wrap gap-2">
                            <span className={`rounded-full px-3 py-1 text-xs font-medium ${task.enabled ? "bg-emerald-100 text-emerald-700" : "bg-amber-100 text-amber-700"}`}>
                              {task.enabled ? "已启用" : "已暂停"}
                            </span>
                          </div>
                        </TableCell>
                        <TableCell className="max-w-[420px] truncate">{task.url}</TableCell>
                        <TableCell>{formatDate(task.updated_at)}</TableCell>
                        <TableCell>
                          <div className="flex flex-wrap gap-2">
                            <Button variant="secondary" onClick={() => void onStartTask(task.id)}>
                              <Play className="mr-2 h-4 w-4" />
                              启动
                            </Button>
                            <Button variant="outline" onClick={() => void onPauseTask(task.id)}>
                              <Pause className="mr-2 h-4 w-4" />
                              暂停
                            </Button>
                            <Button variant="outline" onClick={() => { setSelectedTask(task); setPage(1); }}>
                              <Eye className="mr-2 h-4 w-4" />
                              记录
                            </Button>
                            <Button variant="destructive" onClick={() => void onDeleteTask(task.id)}>
                              <Trash2 className="mr-2 h-4 w-4" />
                              删除
                            </Button>
                          </div>
                        </TableCell>
                      </TableRow>
                    );
                  })}
                </TableBody>
              </Table>
            </div>

            <div className="grid gap-3 xl:hidden">
              {tasks.map((task) => {
                return (
                  <div key={task.id} className="rounded-2xl border border-border bg-surface-container/70 p-4">
                    <div className="flex items-start justify-between gap-3">
                      <div className="flex items-start gap-3">
                        <input
                          type="checkbox"
                          className="mt-1 h-4 w-4 rounded border border-border accent-[hsl(var(--primary))]"
                          checked={selectedIds.includes(task.id)}
                          onChange={(event) => toggleSelection(task.id, event.target.checked)}
                        />
                        <div>
                          <div className="font-semibold">{task.name}</div>
                          <div className="text-xs text-muted">#{task.id}</div>
                        </div>
                      </div>
                      <div className="flex flex-wrap justify-end gap-2">
                        <span className={`rounded-full px-3 py-1 text-xs font-medium ${task.enabled ? "bg-emerald-100 text-emerald-700" : "bg-amber-100 text-amber-700"}`}>
                          {task.enabled ? "已启用" : "已暂停"}
                        </span>
                      </div>
                    </div>
                    <div className="mt-3 break-all text-sm text-muted">{task.url}</div>
                    <div className="mt-3 text-xs text-muted">更新时间：{formatDate(task.updated_at)}</div>
                    <div className="mt-4 grid gap-2 sm:grid-cols-2">
                      <Button variant="secondary" onClick={() => void onStartTask(task.id)}>
                        <Play className="mr-2 h-4 w-4" />
                        启动
                      </Button>
                      <Button variant="outline" onClick={() => void onPauseTask(task.id)}>
                        <Pause className="mr-2 h-4 w-4" />
                        暂停
                      </Button>
                      <Button variant="outline" onClick={() => { setSelectedTask(task); setPage(1); }}>
                        <Eye className="mr-2 h-4 w-4" />
                        查看记录
                      </Button>
                      <Button variant="destructive" onClick={() => void onDeleteTask(task.id)}>
                        <Trash2 className="mr-2 h-4 w-4" />
                        删除
                      </Button>
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
                      <div>种子文件：{record.file_deleted ? "已删除" : "保留/未知"}</div>
                      <div className="break-all">保存路径：{record.saved_path ?? "-"}</div>
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
