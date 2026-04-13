import { useEffect, useState } from "react";
import {
  Globe,
  Plus,
  Pencil,
  Trash2,
  Activity,
  BarChart3,
  Loader2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate } from "@/lib/format";
import type { SiteRecord, SiteTestResult, UserStats } from "@/types";

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`;
}

type AuthType = "cookie" | "passkey" | "cookie_passkey" | "api_key";

interface SiteForm {
  name: string;
  site_type: "nexusphp" | "mteam";
  base_url: string;
  auth_type: AuthType;
  cookie: string;
  passkey: string;
  api_key: string;
}

const emptySiteForm: SiteForm = {
  name: "",
  site_type: "nexusphp",
  base_url: "",
  auth_type: "cookie",
  cookie: "",
  passkey: "",
  api_key: "",
};

function buildAuthConfig(form: SiteForm): object {
  if (form.site_type === "mteam") {
    return { auth_type: "api_key", api_key: form.api_key };
  }
  switch (form.auth_type) {
    case "cookie":
      return { auth_type: "cookie", cookie: form.cookie };
    case "passkey":
      return { auth_type: "passkey", passkey: form.passkey };
    case "cookie_passkey":
      return {
        auth_type: "cookie_passkey",
        cookie: form.cookie,
        passkey: form.passkey,
      };
    default:
      return { auth_type: form.auth_type };
  }
}

function parseAuthConfig(
  siteType: string,
  raw: string,
): Partial<SiteForm> {
  try {
    const obj = JSON.parse(raw);
    const authType: AuthType = obj.auth_type ?? "cookie";
    return {
      auth_type: authType,
      cookie: obj.cookie ?? "",
      passkey: obj.passkey ?? "",
      api_key: obj.api_key ?? "",
      site_type: siteType as SiteForm["site_type"],
    };
  } catch {
    return {};
  }
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function SitesPage() {
  const [sites, setSites] = useState<SiteRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");

  // form dialog
  const [formOpen, setFormOpen] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [form, setForm] = useState<SiteForm>(emptySiteForm);
  const [submitting, setSubmitting] = useState(false);

  // delete confirmation
  const [deleteTarget, setDeleteTarget] = useState<SiteRecord | null>(null);
  const [deleting, setDeleting] = useState(false);

  // test connection
  const [testResult, setTestResult] = useState<SiteTestResult | null>(null);
  const [testOpen, setTestOpen] = useState(false);
  const [testing, setTesting] = useState(false);

  // stats
  const [stats, setStats] = useState<UserStats | null>(null);
  const [statsOpen, setStatsOpen] = useState(false);
  const [statsLoading, setStatsLoading] = useState(false);

  /* ---- data loading ---- */

  function loadSites() {
    setLoading(true);
    api<SiteRecord[]>("/api/sites")
      .then((data) => {
        setSites(data);
        setMessage("");
      })
      .catch((error: Error) => {
        setSites([]);
        setMessage(error.message || "加载站点失败");
      })
      .finally(() => setLoading(false));
  }

  useEffect(() => {
    loadSites();
  }, []);

  /* ---- form helpers ---- */

  function patch(partial: Partial<SiteForm>) {
    setForm((prev) => ({ ...prev, ...partial }));
  }

  function openAdd() {
    setEditingId(null);
    setForm(emptySiteForm);
    setFormOpen(true);
  }

  function openEdit(site: SiteRecord) {
    setEditingId(site.id);
    const parsed = parseAuthConfig(site.site_type, site.auth_config);
    setForm({
      ...emptySiteForm,
      name: site.name,
      site_type: (site.site_type as SiteForm["site_type"]) || "nexusphp",
      base_url: site.base_url,
      ...parsed,
    });
    setFormOpen(true);
  }

  function handleSubmit() {
    setSubmitting(true);
    const body = {
      name: form.name,
      site_type: form.site_type,
      base_url: form.base_url,
      auth_config: buildAuthConfig(form),
    };
    const req =
      editingId != null
        ? api<{ ok: true }>(`/api/sites/${editingId}`, {
            method: "PUT",
            body: JSON.stringify(body),
          })
        : api<{ id: number }>("/api/sites", {
            method: "POST",
            body: JSON.stringify(body),
          });
    req
      .then(() => {
        setFormOpen(false);
        setMessage(editingId != null ? "站点已更新" : "站点已创建");
        loadSites();
      })
      .catch((error: Error) => setMessage(error.message || "保存站点失败"))
      .finally(() => setSubmitting(false));
  }

  /* ---- delete ---- */

  function confirmDelete() {
    if (!deleteTarget) return;
    setDeleting(true);
    api<{ ok: true }>(`/api/sites/${deleteTarget.id}`, { method: "DELETE" })
      .then(() => {
        setDeleteTarget(null);
        setMessage("站点已删除");
        loadSites();
      })
      .catch((error: Error) => setMessage(error.message || "删除站点失败"))
      .finally(() => setDeleting(false));
  }

  /* ---- test connection ---- */

  function handleTest(site: SiteRecord) {
    setTesting(true);
    setTestResult(null);
    setTestOpen(true);
    api<SiteTestResult>(`/api/sites/${site.id}/test`, { method: "POST" })
      .then(setTestResult)
      .catch((err) =>
        setTestResult({ success: false, message: String(err), user_stats: null }),
      )
      .finally(() => setTesting(false));
  }

  /* ---- stats ---- */

  function handleStats(site: SiteRecord) {
    setStatsLoading(true);
    setStats(null);
    setStatsOpen(true);
    api<UserStats>(`/api/sites/${site.id}/stats`)
      .then(setStats)
      .catch((error: Error) => {
        setStats(null);
        setMessage(error.message || "加载站点统计失败");
      })
      .finally(() => setStatsLoading(false));
  }

  /* ---- auth fields ---- */

  function renderAuthFields() {
    if (form.site_type === "mteam") {
      return (
        <div className="space-y-2">
          <Label>API Key</Label>
          <Input
            value={form.api_key}
            onChange={(e) => patch({ api_key: e.target.value })}
            placeholder="输入 API Key"
          />
        </div>
      );
    }

    return (
      <>
        <div className="space-y-2">
          <Label>认证方式</Label>
          <select
            className="h-10 w-full rounded-full border border-border bg-card px-4 text-sm"
            value={form.auth_type}
            onChange={(e) => patch({ auth_type: e.target.value as AuthType })}
          >
            <option value="cookie">Cookie</option>
            <option value="passkey">Passkey</option>
            <option value="cookie_passkey">Cookie + Passkey</option>
          </select>
        </div>

        {(form.auth_type === "cookie" ||
          form.auth_type === "cookie_passkey") && (
          <div className="space-y-2">
            <Label>Cookie</Label>
            <Input
              value={form.cookie}
              onChange={(e) => patch({ cookie: e.target.value })}
              placeholder="输入 Cookie"
            />
          </div>
        )}

        {(form.auth_type === "passkey" ||
          form.auth_type === "cookie_passkey") && (
          <div className="space-y-2">
            <Label>Passkey</Label>
            <Input
              value={form.passkey}
              onChange={(e) => patch({ passkey: e.target.value })}
              placeholder="输入 Passkey"
            />
          </div>
        )}
      </>
    );
  }

  /* ---- render ---- */

  return (
    <div className="space-y-6">
      <Card className="rounded-2xl">
        <CardHeader>
          <div className="flex items-center justify-between">
            <div>
              <CardTitle className="flex items-center gap-2">
                <Globe className="h-5 w-5" />
                站点管理
              </CardTitle>
              <CardDescription>管理 PT 站点连接配置</CardDescription>
            </div>
            <Button onClick={openAdd}>
              <Plus className="mr-2 h-4 w-4" />
              添加站点
            </Button>
          </div>
        </CardHeader>

        <CardContent className="space-y-4">
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

          {loading ? (
            <div className="flex items-center justify-center py-12 text-muted">
              <Loader2 className="mr-2 h-5 w-5 animate-spin" />
              加载中…
            </div>
          ) : sites.length === 0 ? (
            <p className="py-12 text-center text-muted">暂无站点，请添加</p>
          ) : (
            <>
              {/* ---- desktop table ---- */}
              <div className="hidden xl:block">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>名称</TableHead>
                      <TableHead>类型</TableHead>
                      <TableHead>基础URL</TableHead>
                      <TableHead>创建时间</TableHead>
                      <TableHead className="text-right">操作</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {sites.map((site) => (
                      <TableRow key={site.id}>
                        <TableCell className="font-medium">
                          {site.name}
                        </TableCell>
                        <TableCell>
                          <span className="rounded-full bg-violet-100 px-2 py-0.5 text-xs text-violet-700">
                            {site.site_type}
                          </span>
                        </TableCell>
                        <TableCell className="max-w-[260px] truncate text-muted">
                          {site.base_url}
                        </TableCell>
                        <TableCell className="text-muted">
                          {formatDate(site.created_at)}
                        </TableCell>
                        <TableCell>
                          <div className="flex justify-end gap-2">
                            <Button
                              variant="outline"
                              onClick={() => handleTest(site)}
                            >
                              <Activity className="mr-2 h-4 w-4" />
                              测试连接
                            </Button>
                            <Button
                              variant="outline"
                              onClick={() => handleStats(site)}
                            >
                              <BarChart3 className="mr-2 h-4 w-4" />
                              查看统计
                            </Button>
                            <Button
                              variant="secondary"
                              onClick={() => openEdit(site)}
                            >
                              <Pencil className="mr-2 h-4 w-4" />
                              编辑
                            </Button>
                            <Button
                              variant="destructive"
                              onClick={() => setDeleteTarget(site)}
                            >
                              <Trash2 className="mr-2 h-4 w-4" />
                              删除
                            </Button>
                          </div>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>

              {/* ---- mobile cards ---- */}
              <div className="grid gap-3 xl:hidden">
                {sites.map((site) => (
                  <div
                    key={site.id}
                    className="rounded-2xl border border-border bg-surface-container/70 p-4"
                  >
                    <div className="flex items-center justify-between">
                      <span className="font-medium">{site.name}</span>
                      <span className="rounded-full bg-violet-100 px-2 py-0.5 text-xs text-violet-700">
                        {site.site_type}
                      </span>
                    </div>
                    <p className="mt-1 truncate text-sm text-muted">
                      {site.base_url}
                    </p>
                    <p className="mt-1 text-xs text-muted">
                      {formatDate(site.created_at)}
                    </p>
                    <div className="mt-3 flex flex-wrap gap-2">
                      <Button
                        variant="outline"
                        onClick={() => handleTest(site)}
                      >
                        <Activity className="mr-2 h-4 w-4" />
                        测试连接
                      </Button>
                      <Button
                        variant="outline"
                        onClick={() => handleStats(site)}
                      >
                        <BarChart3 className="mr-2 h-4 w-4" />
                        查看统计
                      </Button>
                      <Button
                        variant="secondary"
                        onClick={() => openEdit(site)}
                      >
                        <Pencil className="mr-2 h-4 w-4" />
                        编辑
                      </Button>
                      <Button
                        variant="destructive"
                        onClick={() => setDeleteTarget(site)}
                      >
                        <Trash2 className="mr-2 h-4 w-4" />
                        删除
                      </Button>
                    </div>
                  </div>
                ))}
              </div>
            </>
          )}
        </CardContent>
      </Card>

      {/* ---- add / edit dialog ---- */}
      <Dialog
        open={formOpen}
        onClose={() => setFormOpen(false)}
        title={editingId != null ? "编辑站点" : "添加站点"}
        description={
          editingId != null
            ? "修改站点连接配置"
            : "填写站点信息以添加新的 PT 站点"
        }
      >
        <div className="space-y-4">
          <div className="space-y-2">
            <Label>名称</Label>
            <Input
              value={form.name}
              onChange={(e) => patch({ name: e.target.value })}
              placeholder="站点名称"
            />
          </div>

          <div className="space-y-2">
            <Label>站点类型</Label>
            <select
              className="h-10 w-full rounded-full border border-border bg-card px-4 text-sm"
              value={form.site_type}
              onChange={(e) => {
                const v = e.target.value as SiteForm["site_type"];
                patch({
                  site_type: v,
                  auth_type: v === "mteam" ? "api_key" : "cookie",
                });
              }}
            >
              <option value="nexusphp">NexusPHP</option>
              <option value="mteam">M-Team</option>
            </select>
          </div>

          <div className="space-y-2">
            <Label>基础 URL</Label>
            <Input
              value={form.base_url}
              onChange={(e) => patch({ base_url: e.target.value })}
              placeholder="https://example.com"
            />
          </div>

          {renderAuthFields()}

          <div className="flex justify-end gap-2 pt-2">
            <Button variant="secondary" onClick={() => setFormOpen(false)}>
              取消
            </Button>
            <Button onClick={handleSubmit} disabled={submitting}>
              {submitting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {editingId != null ? "保存" : "添加"}
            </Button>
          </div>
        </div>
      </Dialog>

      {/* ---- delete confirmation ---- */}
      <Dialog
        open={deleteTarget != null}
        onClose={() => setDeleteTarget(null)}
        title="确认删除"
        description={`确定要删除站点「${deleteTarget?.name ?? ""}」吗？此操作不可撤销。`}
      >
        <div className="flex justify-end gap-2 pt-2">
          <Button variant="secondary" onClick={() => setDeleteTarget(null)}>
            取消
          </Button>
          <Button
            variant="destructive"
            onClick={confirmDelete}
            disabled={deleting}
          >
            {deleting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            删除
          </Button>
        </div>
      </Dialog>

      {/* ---- test result dialog ---- */}
      <Dialog
        open={testOpen}
        onClose={() => setTestOpen(false)}
        title="测试连接"
        description="站点连接测试结果"
      >
        {testing ? (
          <div className="flex items-center justify-center py-8 text-muted">
            <Loader2 className="mr-2 h-5 w-5 animate-spin" />
            测试中…
          </div>
        ) : testResult ? (
          <div className="space-y-4">
            <div className="flex items-center gap-2">
              <span
                className={`inline-block h-3 w-3 rounded-full ${testResult.success ? "bg-emerald-500" : "bg-red-500"}`}
              />
              <span className="font-medium">
                {testResult.success ? "连接成功" : "连接失败"}
              </span>
            </div>
            <p className="text-sm text-muted">{testResult.message}</p>

            {testResult.user_stats && (
              <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
                <p className="mb-3 text-sm font-medium">用户信息</p>
                <div className="grid grid-cols-2 gap-3 text-sm">
                  <div>
                    <span className="text-muted">用户名</span>
                    <p className="font-medium">
                      {testResult.user_stats.username}
                    </p>
                  </div>
                  <div>
                    <span className="text-muted">上传量</span>
                    <p className="font-medium">
                      {formatBytes(testResult.user_stats.uploaded)}
                    </p>
                  </div>
                  <div>
                    <span className="text-muted">下载量</span>
                    <p className="font-medium">
                      {formatBytes(testResult.user_stats.downloaded)}
                    </p>
                  </div>
                  <div>
                    <span className="text-muted">分享率</span>
                    <p className="font-medium">
                      {testResult.user_stats.ratio?.toFixed(3) ?? "-"}
                    </p>
                  </div>
                </div>
              </div>
            )}

            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setTestOpen(false)}>
                关闭
              </Button>
            </div>
          </div>
        ) : null}
      </Dialog>

      {/* ---- stats dialog ---- */}
      <Dialog
        open={statsOpen}
        onClose={() => setStatsOpen(false)}
        title="站点统计"
        description="当前用户数据概览"
      >
        {statsLoading ? (
          <div className="flex items-center justify-center py-8 text-muted">
            <Loader2 className="mr-2 h-5 w-5 animate-spin" />
            加载中…
          </div>
        ) : stats ? (
          <div className="space-y-4">
            <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
              <div className="grid grid-cols-2 gap-4 text-sm sm:grid-cols-3">
                <div>
                  <span className="text-muted">用户名</span>
                  <p className="text-lg font-semibold">{stats.username}</p>
                </div>
                <div>
                  <span className="text-muted">上传量</span>
                  <p className="text-lg font-semibold">
                    {formatBytes(stats.uploaded)}
                  </p>
                </div>
                <div>
                  <span className="text-muted">下载量</span>
                  <p className="text-lg font-semibold">
                    {formatBytes(stats.downloaded)}
                  </p>
                </div>
                <div>
                  <span className="text-muted">分享率</span>
                  <p className="text-lg font-semibold">
                    {stats.ratio?.toFixed(3) ?? "-"}
                  </p>
                </div>
                <div>
                  <span className="text-muted">魔力值</span>
                  <p className="text-lg font-semibold">
                    {stats.bonus?.toLocaleString() ?? "-"}
                  </p>
                </div>
                <div>
                  <span className="text-muted">做种 / 下载</span>
                  <p className="text-lg font-semibold">
                    {stats.seeding_count ?? "-"} / {stats.leeching_count ?? "-"}
                  </p>
                </div>
              </div>
            </div>

            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setStatsOpen(false)}>
                关闭
              </Button>
            </div>
          </div>
        ) : (
          <p className="py-8 text-center text-muted">暂无统计数据</p>
        )}
      </Dialog>
    </div>
  );
}
