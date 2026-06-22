import { useEffect, useMemo, useState } from "react";
import { Edit, Loader2, Plus, Tag, Trash2, Zap } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate } from "@/lib/format";
import type { DownloaderRecord, TagMatchCriteria, TagRuleRecord, TagRuleRequest } from "@/types";

const MATCH_TYPES = [
  { value: "prefix", label: "前缀匹配" },
  { value: "suffix", label: "后缀匹配" },
  { value: "contains", label: "包含匹配" },
  { value: "exact", label: "完全匹配" },
  { value: "regex", label: "正则匹配" },
] as const;

const emptyRule: TagMatchCriteria = { match_type: "contains", pattern: "" };

const emptyForm: TagRuleRequest = {
  name: "",
  tag_name: "",
  match_rules: [{ ...emptyRule }],
  enabled: true,
  downloader_ids: null,
};

function ruleToForm(rule: TagRuleRecord): TagRuleRequest {
  let matchRules: TagMatchCriteria[] = [];
  try {
    matchRules = JSON.parse(rule.match_rules);
  } catch {
    matchRules = [{ ...emptyRule }];
  }
  let downloaderIds: number[] | null = null;
  if (rule.downloader_ids) {
    try {
      downloaderIds = JSON.parse(rule.downloader_ids);
    } catch {
      downloaderIds = null;
    }
  }
  return {
    name: rule.name,
    tag_name: rule.tag_name,
    match_rules: matchRules,
    enabled: rule.enabled,
    downloader_ids: downloaderIds,
  };
}

function matchTypeLabel(type: string) {
  return MATCH_TYPES.find((m) => m.value === type)?.label ?? type;
}

export function TagRulesPage() {
  const [rules, setRules] = useState<TagRuleRecord[]>([]);
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");
  const [formOpen, setFormOpen] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [form, setForm] = useState<TagRuleRequest>({ ...emptyForm, match_rules: [{ ...emptyRule }] });
  const [submitError, setSubmitError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<TagRuleRecord | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [tagCounts, setTagCounts] = useState<Record<number, number>>({});
  const [tagCountsLoading, setTagCountsLoading] = useState<Record<number, boolean>>({});

  const downloaderNameById = useMemo(
    () => new Map(downloaders.map((d) => [d.id, d.name])),
    [downloaders],
  );

  function loadData() {
    setLoading(true);
    setTagCounts({});
    Promise.all([
      api<TagRuleRecord[]>("/api/tag-rules"),
      api<DownloaderRecord[]>("/api/downloaders"),
    ])
      .then(([rulesData, downloadersData]) => {
        setRules(rulesData);
        setDownloaders(downloadersData);
        // 逐个查询标签种子数
        for (const rule of rulesData) {
          fetchTagCount(rule.id);
        }
      })
      .catch((err: Error) => setMessage(err.message))
      .finally(() => setLoading(false));
  }

  async function fetchTagCount(ruleId: number) {
    setTagCountsLoading((prev) => ({ ...prev, [ruleId]: true }));
    try {
      const res = await api<{ count: number }>(`/api/tag-rules/${ruleId}/tag-count`);
      setTagCounts((prev) => ({ ...prev, [ruleId]: res.count }));
    } catch {
      // 静默失败
    } finally {
      setTagCountsLoading((prev) => ({ ...prev, [ruleId]: false }));
    }
  }

  useEffect(() => {
    loadData();
  }, []);

  function openCreate() {
    setEditingId(null);
    setForm({ ...emptyForm, match_rules: [{ ...emptyRule }] });
    setSubmitError("");
    setFormOpen(true);
  }

  function openEdit(rule: TagRuleRecord) {
    setEditingId(rule.id);
    setForm(ruleToForm(rule));
    setSubmitError("");
    setFormOpen(true);
  }

  async function handleSubmit() {
    setSubmitting(true);
    setSubmitError("");
    try {
      const payload: TagRuleRequest = {
        ...form,
        match_rules: form.match_rules.filter((r) => r.pattern.trim() !== ""),
      };
      if (payload.match_rules.length === 0) {
        setSubmitError("至少需要一条匹配规则");
        setSubmitting(false);
        return;
      }
      if (editingId) {
        await api(`/api/tag-rules/${editingId}`, {
          method: "PUT",
          body: JSON.stringify(payload),
        });
        setMessage("标签规则已更新");
      } else {
        await api("/api/tag-rules", {
          method: "POST",
          body: JSON.stringify(payload),
        });
        setMessage("标签规则已创建");
      }
      setFormOpen(false);
      loadData();
    } catch (err) {
      setSubmitError((err as Error).message);
    } finally {
      setSubmitting(false);
    }
  }

  async function handleDelete() {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await api(`/api/tag-rules/${deleteTarget.id}`, { method: "DELETE" });
      setMessage("标签规则已删除");
      setDeleteTarget(null);
      loadData();
    } catch (err) {
      setMessage((err as Error).message);
    } finally {
      setDeleting(false);
    }
  }

  async function handleScan() {
    setScanning(true);
    try {
      await api("/api/tag-rules/scan", { method: "POST" });
      setMessage("扫描完成");
    } catch (err) {
      setMessage((err as Error).message);
    } finally {
      setScanning(false);
    }
  }

  function updateMatchRule(index: number, field: keyof TagMatchCriteria, value: string) {
    setForm((prev) => {
      const next = [...prev.match_rules];
      next[index] = { ...next[index], [field]: value };
      return { ...prev, match_rules: next };
    });
  }

  function addMatchRule() {
    setForm((prev) => ({
      ...prev,
      match_rules: [...prev.match_rules, { ...emptyRule }],
    }));
  }

  function removeMatchRule(index: number) {
    setForm((prev) => ({
      ...prev,
      match_rules: prev.match_rules.filter((_, i) => i !== index),
    }));
  }

  function toggleDownloader(id: number) {
    setForm((prev) => {
      const current = prev.downloader_ids ?? [];
      const next = current.includes(id) ? current.filter((x) => x !== id) : [...current, id];
      return { ...prev, downloader_ids: next.length === 0 ? null : next };
    });
  }

  function setAllDownloaders() {
    setForm((prev) => ({ ...prev, downloader_ids: null }));
  }

  if (loading) {
    return (
      <Card>
        <CardContent className="flex items-center justify-center py-12">
          <Loader2 className="h-5 w-5 animate-spin text-primary" />
          <span className="ml-2 text-sm text-muted">加载中...</span>
        </CardContent>
      </Card>
    );
  }

  const isAllDownloaders = form.downloader_ids === null || form.downloader_ids === undefined;

  return (
    <div className="space-y-4">
      {message ? (
        <div className="rounded-2xl border border-border bg-card/90 px-4 py-3 text-sm shadow-card">
          <div className="flex items-center justify-between">
            <span>{message}</span>
            <button
              type="button"
              className="rounded-full p-1 text-muted hover:bg-accent hover:text-foreground"
              onClick={() => setMessage("")}
            >
              ×
            </button>
          </div>
        </div>
      ) : null}

      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <div>
              <CardTitle className="flex items-center gap-2">
                <Tag className="h-5 w-5 text-primary" />
                标签规则
              </CardTitle>
              <CardDescription>
                根据种子的 Tracker URL 自动匹配并添加标签，每分钟扫描一次。
              </CardDescription>
            </div>
            <div className="flex items-center gap-2">
              <Button variant="outline" onClick={handleScan} disabled={scanning} className="gap-2">
                {scanning ? <Loader2 className="h-4 w-4 animate-spin" /> : <Zap className="h-4 w-4" />}
                立即扫描
              </Button>
              <Button onClick={openCreate} className="gap-2">
                <Plus className="h-4 w-4" />
                新增规则
              </Button>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          {rules.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-12 text-muted">
              <Tag className="mb-3 h-8 w-8 opacity-40" />
              <p className="text-sm">暂无标签规则，点击右上角「新增规则」创建。</p>
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>名称</TableHead>
                  <TableHead>标签名</TableHead>
                  <TableHead>种子数</TableHead>
                  <TableHead>匹配规则</TableHead>
                  <TableHead>生效实例</TableHead>
                  <TableHead>状态</TableHead>
                  <TableHead>更新时间</TableHead>
                  <TableHead className="text-right">操作</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rules.map((rule) => {
                  let criteria: TagMatchCriteria[] = [];
                  try {
                    criteria = JSON.parse(rule.match_rules);
                  } catch {
                    /* ignore */
                  }
                  let downloaderIds: number[] | null = null;
                  if (rule.downloader_ids) {
                    try {
                      downloaderIds = JSON.parse(rule.downloader_ids);
                    } catch {
                      /* ignore */
                    }
                  }
                  return (
                    <TableRow key={rule.id}>
                      <TableCell className="font-medium">{rule.name}</TableCell>
                      <TableCell>
                        <span className="inline-flex items-center gap-1 rounded-full bg-primary/10 px-2.5 py-0.5 text-xs font-semibold text-primary">
                          {rule.tag_name}
                        </span>
                      </TableCell>
                      <TableCell>
                        {tagCountsLoading[rule.id] ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin text-muted" />
                        ) : (
                          <span className="text-sm tabular-nums">
                            {tagCounts[rule.id] ?? "-"}
                          </span>
                        )}
                      </TableCell>
                      <TableCell>
                        <div className="flex flex-wrap gap-1">
                          {criteria.map((c, i) => (
                            <span
                              key={i}
                              className="inline-flex items-center gap-1 rounded-full bg-surface-container px-2 py-0.5 text-xs text-foreground"
                            >
                              <span className="text-muted">{matchTypeLabel(c.match_type)}</span>
                              <span className="max-w-[140px] truncate font-mono">{c.pattern}</span>
                            </span>
                          ))}
                        </div>
                      </TableCell>
                      <TableCell>
                        <span className="text-xs text-muted">
                          {downloaderIds === null
                            ? "所有实例"
                            : downloaderIds.map((id) => downloaderNameById.get(id) ?? `#${id}`).join(", ")}
                        </span>
                      </TableCell>
                      <TableCell>
                        <span
                          className={`inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium ${
                            rule.enabled ? "bg-emerald-100 text-emerald-700" : "bg-slate-100 text-slate-500"
                          }`}
                        >
                          {rule.enabled ? "启用" : "禁用"}
                        </span>
                      </TableCell>
                      <TableCell className="text-xs text-muted">{formatDate(rule.updated_at)}</TableCell>
                      <TableCell className="text-right">
                        <div className="flex items-center justify-end gap-1">
                          <button
                            type="button"
                            onClick={() => openEdit(rule)}
                            title="编辑"
                            className="rounded-lg p-2 text-muted transition hover:bg-accent hover:text-foreground"
                          >
                            <Edit className="h-4 w-4" />
                          </button>
                          <button
                            type="button"
                            onClick={() => setDeleteTarget(rule)}
                            title="删除"
                            className="rounded-lg p-2 text-muted transition hover:bg-destructive/10 hover:text-destructive"
                          >
                            <Trash2 className="h-4 w-4" />
                          </button>
                        </div>
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      {/* 新增/编辑对话框 */}
      <Dialog
        open={formOpen}
        onClose={() => setFormOpen(false)}
        title={editingId ? "编辑标签规则" : "新增标签规则"}
        description="设置 Tracker 匹配规则，匹配成功的种子将自动添加对应标签。"
      >
        <div className="space-y-5 p-4 sm:p-6">
          {submitError ? (
            <div className="rounded-xl border border-destructive/30 bg-destructive/5 px-4 py-2 text-sm text-destructive">
              {submitError}
            </div>
          ) : null}

          {/* 基本信息 */}
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-2">
              <Label>规则名称</Label>
              <Input
                value={form.name}
                onChange={(e) => setForm((prev) => ({ ...prev, name: e.target.value }))}
                placeholder="例如：馒头站种子"
              />
            </div>
            <div className="space-y-2">
              <Label>标签名</Label>
              <Input
                value={form.tag_name}
                onChange={(e) => setForm((prev) => ({ ...prev, tag_name: e.target.value }))}
                placeholder="例如：mteam"
              />
            </div>
          </div>

          {/* 启用状态 */}
          <div className="flex items-center gap-3">
            <button
              type="button"
              role="switch"
              aria-checked={form.enabled ?? true}
              onClick={() => setForm((prev) => ({ ...prev, enabled: !(prev.enabled ?? true) }))}
              className={`relative inline-flex h-6 w-11 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors ${
                form.enabled ?? true ? "bg-primary" : "bg-border"
              }`}
            >
              <span
                className={`pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow-sm ring-0 transition-transform ${
                  form.enabled ?? true ? "translate-x-5" : "translate-x-0"
                }`}
              />
            </button>
            <Label>启用</Label>
          </div>

          {/* 匹配规则 */}
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <Label>匹配规则（任一匹配即生效）</Label>
              <button
                type="button"
                onClick={addMatchRule}
                className="inline-flex items-center gap-1 rounded-full border border-border bg-card/80 px-3 py-1.5 text-xs font-semibold text-foreground transition hover:border-primary/35 hover:bg-accent"
              >
                <Plus className="h-3 w-3" />
                添加
              </button>
            </div>
            <p className="text-xs leading-5 text-muted">
              匹配目标为 tracker 的<b>域名</b>（如 <code className="rounded bg-surface-container px-1">kp.m-team.xyz</code>），不含路径和参数。
            </p>
            {form.match_rules.map((rule, index) => (
              <div key={index} className="flex items-center gap-2">
                <Select
                  className="w-[140px] shrink-0"
                  value={rule.match_type}
                  onChange={(val) => updateMatchRule(index, "match_type", val as TagMatchCriteria["match_type"])}
                  options={MATCH_TYPES}
                />
                <Input
                  className="flex-1"
                  value={rule.pattern}
                  onChange={(e) => updateMatchRule(index, "pattern", e.target.value)}
                  placeholder={rule.match_type === "regex" ? "正则表达式，如 m-team\\.xyz" : "域名关键词，如 m-team.xyz"}
                />
                {form.match_rules.length > 1 ? (
                  <button
                    type="button"
                    onClick={() => removeMatchRule(index)}
                    className="shrink-0 rounded-lg p-2 text-muted transition hover:bg-destructive/10 hover:text-destructive"
                    title="删除此规则"
                  >
                    <Trash2 className="h-4 w-4" />
                  </button>
                ) : null}
              </div>
            ))}
          </div>

          {/* 生效实例 */}
          <div className="space-y-3">
            <Label>生效下载器实例</Label>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                onClick={setAllDownloaders}
                className={`rounded-full px-3 py-1.5 text-xs font-medium transition-colors ${
                  isAllDownloaders
                    ? "bg-primary text-primary-foreground shadow-glow"
                    : "bg-surface-container text-muted hover:bg-accent"
                }`}
              >
                所有实例
              </button>
              {downloaders.map((d) => {
                const selected = !isAllDownloaders && (form.downloader_ids ?? []).includes(d.id);
                return (
                  <button
                    key={d.id}
                    type="button"
                    onClick={() => toggleDownloader(d.id)}
                    className={`rounded-full px-3 py-1.5 text-xs font-medium transition-colors ${
                      selected
                        ? "bg-primary text-primary-foreground shadow-glow"
                        : "bg-surface-container text-muted hover:bg-accent"
                    }`}
                  >
                    {d.name}
                  </button>
                );
              })}
            </div>
          </div>

          {/* 提交 */}
          <div className="flex items-center justify-end gap-3 pt-2">
            <Button variant="outline" onClick={() => setFormOpen(false)}>
              取消
            </Button>
            <Button onClick={handleSubmit} disabled={submitting} className="gap-2">
              {submitting ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
              {editingId ? "保存" : "创建"}
            </Button>
          </div>
        </div>
      </Dialog>

      {/* 删除确认 */}
      <Dialog
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        title="确认删除"
        description={`确定要删除标签规则「${deleteTarget?.name ?? ""}」吗？此操作不可撤销。`}
      >
        <div className="flex items-center justify-end gap-3 p-4 sm:p-6">
          <Button variant="outline" onClick={() => setDeleteTarget(null)}>
            取消
          </Button>
          <Button variant="destructive" onClick={handleDelete} disabled={deleting} className="gap-2">
            {deleting ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
            删除
          </Button>
        </div>
      </Dialog>
    </div>
  );
}
