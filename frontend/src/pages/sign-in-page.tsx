import { useEffect, useMemo, useState } from "react";
import { CalendarCheck, Edit, Loader2, Pause, Play, Plus, RefreshCw, Trash2, Zap } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate, statusBadge } from "@/lib/format";
import type { LightpandaProbeResult, SignInRecord, SignInTaskRecord, SignInTaskRequest, SiteRecord } from "@/types";

const selectClass =
  "flex h-11 w-full rounded-2xl border border-border bg-input px-4 py-2 text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring/30";

const SIGN_IN_INTERVAL_HOURS = [6, 8, 12, 16, 20, 24] as const;
type SignInIntervalHours = (typeof SIGN_IN_INTERVAL_HOURS)[number];

const emptyForm: SignInTaskRequest = {
  name: "",
  site_id: 0,
  cron_expression: intervalToCron(8),
  lightpanda_endpoint: null,
  lightpanda_token: "",
  lightpanda_region: null,
  browser: null,
  proxy: null,
  country: null,
};

function taskToForm(task: SignInTaskRecord): SignInTaskRequest {
  return {
    name: task.name,
    site_id: task.site_id,
    cron_expression: task.cron_expression,
    lightpanda_endpoint: task.lightpanda_endpoint,
    lightpanda_token: task.lightpanda_token,
    lightpanda_region: null,
    browser: null,
    proxy: null,
    country: null,
  };
}

function isNexusSite(site: SiteRecord) {
  const siteType = site.site_type.trim().toLowerCase();
  return siteType === "nexusphp" || siteType === "nexus_php";
}

function displayStatus(status: string | null | undefined) {
  if (!status) return "-";
  if (status === "success") return "成功";
  if (status === "already") return "已签到";
  if (status === "failed") return "失败";
  return status;
}

function intervalToCron(hours: SignInIntervalHours) {
  return `0 0 0/${hours} * * *`;
}

function cronToInterval(cron: string): SignInIntervalHours {
  const fields = cron.trim().split(/\s+/);
  const hourField = fields.length === 6 ? fields[2] : fields.length === 5 ? fields[1] : "";
  const match = hourField.match(/^0\/(\d+)$/);
  const hours = match ? Number(match[1]) : 8;
  return SIGN_IN_INTERVAL_HOURS.includes(hours as SignInIntervalHours) ? (hours as SignInIntervalHours) : 8;
}

export function SignInPage() {
  const [tasks, setTasks] = useState<SignInTaskRecord[]>([]);
  const [sites, setSites] = useState<SiteRecord[]>([]);
  const [records, setRecords] = useState<SignInRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");
  const [formOpen, setFormOpen] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [form, setForm] = useState<SignInTaskRequest>({ ...emptyForm });
  const [intervalHours, setIntervalHours] = useState<SignInIntervalHours>(8);
  const [submitError, setSubmitError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [probing, setProbing] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<SignInTaskRecord | null>(null);
  const [deleting, setDeleting] = useState(false);

  const nexusSites = useMemo(() => sites.filter(isNexusSite), [sites]);
  const siteNameById = useMemo(
    () => new Map(sites.map((site) => [site.id, `${site.name} (${site.site_type})`])),
    [sites],
  );

  function loadData() {
    setLoading(true);
    Promise.all([
      api<SignInTaskRecord[]>("/api/sign-in-tasks"),
      api<SiteRecord[]>("/api/sites"),
      api<SignInRecord[]>("/api/sign-in-records?limit=100"),
    ])
      .then(([nextTasks, nextSites, nextRecords]) => {
        setTasks(nextTasks);
        setSites(nextSites);
        setRecords(nextRecords);
      })
      .catch((error: Error) => setMessage(error.message || "加载自动签到数据失败"))
      .finally(() => setLoading(false));
  }

  useEffect(() => {
    loadData();
  }, []);

  function setField<K extends keyof SignInTaskRequest>(key: K, value: SignInTaskRequest[K]) {
    setForm((prev) => ({ ...prev, [key]: value }));
  }

  function openAdd() {
    setEditingId(null);
    setForm({ ...emptyForm, site_id: nexusSites[0]?.id ?? 0 });
    setIntervalHours(8);
    setSubmitError("");
    setFormOpen(true);
  }

  function openEdit(task: SignInTaskRecord) {
    setEditingId(task.id);
    setForm(taskToForm(task));
    setIntervalHours(cronToInterval(task.cron_expression));
    setSubmitError("");
    setFormOpen(true);
  }

  function copyFromTask(taskId: number) {
    const source = tasks.find((task) => task.id === taskId);
    if (!source) return;
    setForm((prev) => ({
      ...prev,
      cron_expression: source.cron_expression,
      lightpanda_endpoint: source.lightpanda_endpoint,
      lightpanda_token: source.lightpanda_token,
      lightpanda_region: null,
      browser: null,
      proxy: null,
      country: null,
    }));
    setIntervalHours(cronToInterval(source.cron_expression));
  }

  function closeForm() {
    setFormOpen(false);
    setEditingId(null);
    setSubmitError("");
  }

  async function handleSubmit() {
    if (!form.name.trim()) {
      setSubmitError("名称不能为空");
      return;
    }
    if (!form.site_id) {
      setSubmitError("请选择 NexusPHP 站点");
      return;
    }
    if (!form.lightpanda_token.trim() && !form.lightpanda_endpoint?.trim()) {
      setSubmitError("Lightpanda token 或自定义 endpoint 至少填写一个");
      return;
    }

    const body: SignInTaskRequest = {
      ...form,
      name: form.name.trim(),
      cron_expression: intervalToCron(intervalHours),
      lightpanda_endpoint: form.lightpanda_endpoint?.trim() || null,
      lightpanda_token: form.lightpanda_token.trim(),
      lightpanda_region: "euwest",
      browser: "lightpanda",
      proxy: "fast_dc",
      country: null,
    };

    setSubmitting(true);
    setSubmitError("");
    try {
      if (editingId !== null) {
        await api(`/api/sign-in-tasks/${editingId}`, { method: "PUT", body: JSON.stringify(body) });
      } else {
        await api("/api/sign-in-tasks", { method: "POST", body: JSON.stringify(body) });
      }
      closeForm();
      setMessage(editingId !== null ? "自动签到任务已更新" : "自动签到任务已创建");
      loadData();
    } catch (error) {
      setSubmitError((error as Error).message || "保存自动签到任务失败");
    } finally {
      setSubmitting(false);
    }
  }

  async function probeEndpoint() {
    if (!form.lightpanda_endpoint?.trim() && !form.lightpanda_token.trim()) {
      setSubmitError("Lightpanda endpoint 不能为空");
      return;
    }

    setProbing(true);
    setSubmitError("");
    try {
      const result = await api<LightpandaProbeResult>("/api/sign-in-probe-1-1-1-1", {
        method: "POST",
        body: JSON.stringify({
          ...form,
          cron_expression: intervalToCron(intervalHours),
          lightpanda_endpoint: form.lightpanda_endpoint?.trim() || null,
          lightpanda_token: form.lightpanda_token.trim(),
          lightpanda_region: "euwest",
          browser: "lightpanda",
          proxy: "fast_dc",
          country: null,
        }),
      });
      if (!result.success) {
        throw new Error(result.message);
      }
      setSubmitError(`测试成功：已打开 ${result.url}${result.title ? `，标题：${result.title}` : ""}`);
    } catch (error) {
      setSubmitError((error as Error).message || "Lightpanda 测试失败");
    } finally {
      setProbing(false);
    }
  }

  async function runAction(action: Promise<unknown>, success: string) {
    try {
      await action;
      setMessage(success);
      loadData();
    } catch (error) {
      setMessage((error as Error).message || "操作失败");
    }
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await api(`/api/sign-in-tasks/${deleteTarget.id}`, { method: "DELETE" });
      setDeleteTarget(null);
      setMessage("自动签到任务已删除");
      loadData();
    } catch (error) {
      setMessage((error as Error).message || "删除自动签到任务失败");
    } finally {
      setDeleting(false);
    }
  }

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          {message ? (
            <div className="rounded-2xl border border-border bg-surface-container/70 px-4 py-3 text-sm">
              <div className="flex items-start justify-between gap-3">
                <span>{message}</span>
                <button type="button" className="text-muted hover:text-foreground" onClick={() => setMessage("")}>
                  关闭
                </button>
              </div>
            </div>
          ) : null}
          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <CardTitle className="flex items-center gap-2">
                <CalendarCheck className="h-5 w-5" />
                自动签到
              </CardTitle>
              <CardDescription>管理 NexusPHP 站点签到任务、调度状态和最近执行结果。</CardDescription>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button variant="outline" onClick={loadData}>
                <RefreshCw className="mr-2 h-4 w-4" />
                刷新
              </Button>
              <Button onClick={openAdd}>
                <Plus className="mr-2 h-4 w-4" />
                添加任务
              </Button>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          {loading ? (
            <div className="flex items-center justify-center py-12 text-muted">
              <Loader2 className="mr-2 h-5 w-5 animate-spin" />
              加载中...
            </div>
          ) : tasks.length === 0 ? (
            <div className="py-12 text-center text-sm text-muted">暂无自动签到任务，点击上方按钮添加。</div>
          ) : (
            <div className="grid gap-3">
              {tasks.map((task) => (
                <div key={task.id} className="rounded-2xl border border-border bg-surface-container/70 p-4">
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-semibold">{task.name}</span>
                        <span
                          className={`rounded-full px-3 py-1 text-xs font-medium ${
                            task.enabled ? "bg-emerald-100 text-emerald-700" : "bg-amber-100 text-amber-700"
                          }`}
                        >
                          {task.enabled ? "已启用" : "已停用"}
                        </span>
                        <span className={`rounded-full px-3 py-1 text-xs font-medium ${statusBadge(task.last_status ?? "")}`}>
                          {displayStatus(task.last_status)}
                        </span>
                      </div>
                      <div className="mt-1 text-xs text-muted">#{task.id}</div>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      <Button variant="outline" onClick={() => void runAction(api(`/api/sign-in-tasks/${task.id}/run`, { method: "POST" }), "已触发运行一次")}>
                        <Zap className="mr-2 h-4 w-4" />
                        运行一次
                      </Button>
                      {task.enabled ? (
                        <Button variant="outline" onClick={() => void runAction(api(`/api/sign-in-tasks/${task.id}/stop`, { method: "POST" }), "自动签到任务已停用")}>
                          <Pause className="mr-2 h-4 w-4" />
                          停用
                        </Button>
                      ) : (
                        <Button variant="secondary" onClick={() => void runAction(api(`/api/sign-in-tasks/${task.id}/start`, { method: "POST" }), "自动签到任务已启用")}>
                          <Play className="mr-2 h-4 w-4" />
                          启用
                        </Button>
                      )}
                      <Button variant="outline" onClick={() => openEdit(task)}>
                        <Edit className="mr-2 h-4 w-4" />
                        编辑
                      </Button>
                      <Button variant="destructive" onClick={() => setDeleteTarget(task)}>
                        <Trash2 className="mr-2 h-4 w-4" />
                        删除
                      </Button>
                    </div>
                  </div>

                  <div className="mt-3 grid gap-2 text-sm text-muted sm:grid-cols-2 xl:grid-cols-4">
                    <div>
                      <span className="font-medium text-foreground">站点：</span>
                      {siteNameById.get(task.site_id) ?? `#${task.site_id}`}
                    </div>
                    <div>
                      <span className="font-medium text-foreground">间隔：</span>
                      每 {cronToInterval(task.cron_expression)} 小时
                    </div>
                    <div>
                      <span className="font-medium text-foreground">最近时间：</span>
                      {formatDate(task.last_run_at)}
                    </div>
                    <div className="sm:col-span-2 xl:col-span-4">
                      <span className="font-medium text-foreground">最近消息：</span>
                      {task.last_message || "-"}
                    </div>
                  </div>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>最近记录</CardTitle>
          <CardDescription>展示最近 100 条自动签到执行记录。</CardDescription>
        </CardHeader>
        <CardContent>
          {records.length === 0 ? (
            <div className="py-8 text-center text-sm text-muted">暂无签到记录。</div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>任务</TableHead>
                  <TableHead>站点</TableHead>
                  <TableHead>状态</TableHead>
                  <TableHead>消息</TableHead>
                  <TableHead>开始时间</TableHead>
                  <TableHead>结束时间</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {records.map((record) => (
                  <TableRow key={record.id}>
                    <TableCell>#{record.task_id}</TableCell>
                    <TableCell>{record.site_name || siteNameById.get(record.site_id) || `#${record.site_id}`}</TableCell>
                    <TableCell>
                      <span className={`rounded-full px-3 py-1 text-xs font-medium ${statusBadge(record.status)}`}>
                        {displayStatus(record.status)}
                      </span>
                    </TableCell>
                    <TableCell className="max-w-[360px] truncate text-muted" title={record.message}>
                      {record.message || "-"}
                    </TableCell>
                    <TableCell className="text-muted">{formatDate(record.started_at)}</TableCell>
                    <TableCell className="text-muted">{formatDate(record.finished_at)}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      <Dialog
        open={formOpen}
        onClose={closeForm}
        title={editingId !== null ? "编辑自动签到任务" : "添加自动签到任务"}
        description="配置 NexusPHP 站点、执行间隔和 Lightpanda 云端浏览器地址。"
        escMode="double"
      >
        <div className="space-y-6 p-4 sm:p-6">
          {submitError ? (
            <div className="rounded-2xl border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
              {submitError}
            </div>
          ) : null}

          <div className="grid gap-4 sm:grid-cols-2">
            {editingId === null && tasks.length > 0 ? (
              <div className="space-y-2 sm:col-span-2">
                <Label>从已有任务复制</Label>
                <select
                  className={selectClass}
                  defaultValue=""
                  onChange={(event) => {
                    if (event.target.value) {
                      copyFromTask(Number(event.target.value));
                      event.target.value = "";
                    }
                  }}
                >
                  <option value="">选择已有任务复制配置</option>
                  {tasks.map((task) => (
                    <option key={task.id} value={task.id}>
                      {task.name} · 每 {cronToInterval(task.cron_expression)} 小时
                    </option>
                  ))}
                </select>
              </div>
            ) : null}

            <div className="space-y-2">
              <Label>名称</Label>
              <Input value={form.name} onChange={(event) => setField("name", event.target.value)} placeholder="每日签到" />
            </div>
            <div className="space-y-2">
              <Label>站点</Label>
              <select
                className={selectClass}
                value={form.site_id || ""}
                onChange={(event) => setField("site_id", event.target.value === "" ? 0 : Number(event.target.value))}
              >
                {nexusSites.length === 0 ? <option value="">请先添加 NexusPHP 站点</option> : null}
                {nexusSites.map((site) => (
                  <option key={site.id} value={site.id}>
                    {site.name} ({site.site_type})
                  </option>
                ))}
              </select>
            </div>
            <div className="space-y-2">
              <Label>执行间隔</Label>
              <select
                className={selectClass}
                value={intervalHours}
                onChange={(event) => setIntervalHours(Number(event.target.value) as SignInIntervalHours)}
              >
                {SIGN_IN_INTERVAL_HOURS.map((hours) => (
                  <option key={hours} value={hours}>
                    每 {hours} 小时
                  </option>
                ))}
              </select>
            </div>
            <div className="space-y-2">
              <Label>Lightpanda token</Label>
              <Input
                value={form.lightpanda_token}
                onChange={(event) => setField("lightpanda_token", event.target.value)}
                placeholder="Lightpanda token"
              />
            </div>
            <div className="space-y-2 sm:col-span-2">
              <Label>Lightpanda endpoint</Label>
              <div className="flex flex-col gap-2 sm:flex-row">
                <Input
                  value={form.lightpanda_endpoint ?? ""}
                  onChange={(event) => setField("lightpanda_endpoint", event.target.value || null)}
                  placeholder="wss://euwest.cloud.lightpanda.io/ws?token=..."
                />
                <Button type="button" variant="outline" disabled={probing} onClick={() => void probeEndpoint()}>
                  {probing ? "测试中..." : "测试 1.1.1.1"}
                </Button>
              </div>
            </div>
          </div>

          <div className="flex gap-3 border-t border-border pt-4">
            <Button disabled={submitting} onClick={() => void handleSubmit()}>
              {submitting ? "提交中..." : editingId !== null ? "保存修改" : "创建任务"}
            </Button>
            <Button variant="outline" disabled={submitting} onClick={closeForm}>
              取消
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={deleteTarget !== null}
        onClose={() => setDeleteTarget(null)}
        title="确认删除"
        description={`确定要删除自动签到任务「${deleteTarget?.name ?? ""}」吗？此操作不可撤销。`}
      >
        <div className="flex justify-end gap-2 pt-2">
          <Button variant="secondary" onClick={() => setDeleteTarget(null)}>
            取消
          </Button>
          <Button variant="destructive" onClick={() => void confirmDelete()} disabled={deleting}>
            {deleting ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : <Trash2 className="mr-2 h-4 w-4" />}
            删除
          </Button>
        </div>
      </Dialog>
    </div>
  );
}
