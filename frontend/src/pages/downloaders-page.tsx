import { useEffect, useState } from "react";
import { ArrowRight, CheckSquare2, Edit, FolderInput, LoaderCircle, Plus, Search, TestTubeDiagonal, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate } from "@/lib/format";
import type { DownloaderRecord, DownloaderSpaceStats, DownloaderTestResult, TransferableTorrent } from "@/types";

type OpenListSettingsSummary = {
  enabled: boolean;
  target_directory_id: number | null;
  target_directories: Array<{ id?: number; name: string; openlist_root: string }>;
};

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`;
}

type FormData = {
  name: string;
  downloader_type: string;
  url: string;
  username: string;
  password: string;
};

const emptyForm: FormData = {
  name: "",
  downloader_type: "qbittorrent",
  url: "",
  username: "",
  password: "",
};

export function DownloadersPage() {
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [spaceStats, setSpaceStats] = useState<Record<number, DownloaderSpaceStats>>({});
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [form, setForm] = useState<FormData>(emptyForm);
  const [existingPasswordConfigured, setExistingPasswordConfigured] = useState(false);
  const [clearPassword, setClearPassword] = useState(false);
  const [saving, setSaving] = useState(false);
  const [submitError, setSubmitError] = useState("");

  const [deleteTarget, setDeleteTarget] = useState<DownloaderRecord | null>(null);
  const [deleting, setDeleting] = useState(false);

  const [testResult, setTestResult] = useState<DownloaderTestResult | null>(null);
  const [testing, setTesting] = useState<number | null>(null);
  const [transferSource, setTransferSource] = useState<DownloaderRecord | null>(null);
  const [transferTorrents, setTransferTorrents] = useState<TransferableTorrent[]>([]);
  const [selectedHashes, setSelectedHashes] = useState<Set<string>>(new Set());
  const [transferKeyword, setTransferKeyword] = useState("");
  const [transferLoading, setTransferLoading] = useState(false);
  const [transferring, setTransferring] = useState(false);
  const [transferError, setTransferError] = useState("");
  const [openListSettings, setOpenListSettings] = useState<OpenListSettingsSummary | null>(null);

  function loadDownloaders() {
    setLoading(true);
    api<DownloaderRecord[]>("/api/downloaders")
      .then(async (items) => {
        setDownloaders(items);
        const entries = await Promise.all(
          items.map(async (downloader) => {
            try {
              const stats = await api<DownloaderSpaceStats>(`/api/downloaders/${downloader.id}/space`);
              return [downloader.id, stats] as const;
            } catch {
              return null;
            }
          }),
        );
        const next: Record<number, DownloaderSpaceStats> = {};
        for (const entry of entries) {
          if (entry) {
            next[entry[0]] = entry[1];
          }
        }
        setSpaceStats(next);
        setMessage("");
      })
      .catch((error: Error) => {
        setDownloaders([]);
        setSpaceStats({});
        setMessage(error.message || "加载下载器失败");
      })
      .finally(() => setLoading(false));
  }

  useEffect(() => {
    loadDownloaders();
  }, []);

  function openAdd() {
    setEditingId(null);
    setForm(emptyForm);
    setExistingPasswordConfigured(false);
    setClearPassword(false);
    setSubmitError("");
    setDialogOpen(true);
  }

  function openEdit(d: DownloaderRecord) {
    setEditingId(d.id);
    setForm({
      name: d.name,
      downloader_type: d.downloader_type,
      url: d.url,
      username: d.username,
      password: "",
    });
    setExistingPasswordConfigured(d.password_configured);
    setClearPassword(false);
    setSubmitError("");
    setDialogOpen(true);
  }

  function closeDialog() {
    setDialogOpen(false);
    setEditingId(null);
    setForm(emptyForm);
    setExistingPasswordConfigured(false);
    setClearPassword(false);
    setSubmitError("");
  }

  async function handleSave() {
    setSaving(true);
    setSubmitError("");
    try {
      const body = JSON.stringify(
        editingId !== null ? { ...form, clear_password: clearPassword } : form,
      );
      if (editingId !== null) {
        await api(`/api/downloaders/${editingId}`, { method: "PUT", body });
      } else {
        await api("/api/downloaders", { method: "POST", body });
      }
      closeDialog();
      setMessage(editingId !== null ? "下载器已更新" : "下载器已创建");
      loadDownloaders();
    } catch (error) {
      setSubmitError((error as Error).message || "保存下载器失败");
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete() {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await api(`/api/downloaders/${deleteTarget.id}`, { method: "DELETE" });
      setDeleteTarget(null);
      setMessage("下载器已删除");
      loadDownloaders();
    } catch (error) {
      setMessage((error as Error).message || "删除下载器失败");
    } finally {
      setDeleting(false);
    }
  }

  async function handleTest(id: number) {
    setTesting(id);
    setTestResult(null);
    try {
      const result = await api<DownloaderTestResult>(`/api/downloaders/${id}/test`, {
        method: "POST",
      });
      setTestResult(result);
    } catch (error) {
      setTestResult({ success: false, message: (error as Error).message || "请求失败", version: null, free_space: null });
    } finally {
      setTesting(null);
    }
  }

  async function openTransfer(downloader: DownloaderRecord) {
    setTransferSource(downloader);
    setTransferTorrents([]);
    setSelectedHashes(new Set());
    setTransferKeyword("");
    setTransferError("");
    setTransferLoading(true);
    try {
      const [torrents, settings] = await Promise.all([
        api<TransferableTorrent[]>(`/api/downloaders/${downloader.id}/torrents`),
        api<OpenListSettingsSummary>("/api/media/openlist/settings"),
      ]);
      setTransferTorrents(torrents);
      setOpenListSettings(settings);
    } catch (error) {
      setTransferError((error as Error).message || "加载种子失败");
    } finally {
      setTransferLoading(false);
    }
  }

  function closeTransfer() {
    if (transferring) return;
    setTransferSource(null);
    setTransferTorrents([]);
    setSelectedHashes(new Set());
    setTransferError("");
  }

  function toggleTorrent(hash: string) {
    setSelectedHashes((current) => {
      const next = new Set(current);
      if (next.has(hash)) next.delete(hash);
      else next.add(hash);
      return next;
    });
  }

  async function submitTransfer() {
    if (!transferSource || selectedHashes.size === 0) return;
    setTransferring(true);
    setTransferError("");
    try {
      const result = await api<{ created: number; skipped: number }>(`/api/downloaders/${transferSource.id}/openlist-transfer`, {
        method: "POST",
        body: JSON.stringify({ hashes: [...selectedHashes] }),
      });
      await api("/api/media/openlist/scan", { method: "POST" }).catch(() => undefined);
      setTransferSource(null);
      setTransferTorrents([]);
      setSelectedHashes(new Set());
      setMessage(`已创建 ${result.created} 个转移任务${result.skipped ? `，跳过 ${result.skipped} 个进行中的任务` : ""}`);
    } catch (error) {
      setTransferError((error as Error).message || "创建转移任务失败");
    } finally {
      setTransferring(false);
    }
  }

  const normalizedKeyword = transferKeyword.trim().toLowerCase();
  const visibleTorrents = transferTorrents.filter((torrent) =>
    !normalizedKeyword
      || torrent.name.toLowerCase().includes(normalizedKeyword)
      || torrent.hash.toLowerCase().includes(normalizedKeyword),
  );
  const visibleAllSelected = visibleTorrents.length > 0 && visibleTorrents.every((torrent) => selectedHashes.has(torrent.hash));
  const selectedTarget = openListSettings?.target_directories.find((target) => target.id === openListSettings.target_directory_id);

  function toggleVisibleTorrents() {
    setSelectedHashes((current) => {
      const next = new Set(current);
      visibleTorrents.forEach((torrent) => {
        if (visibleAllSelected) next.delete(torrent.hash);
        else next.add(torrent.hash);
      });
      return next;
    });
  }

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-semibold">下载器管理</h2>
          <p className="mt-1 text-sm text-muted">管理已配置的下载器实例</p>
        </div>
        <Button onClick={openAdd}>
          <Plus className="mr-2 h-4 w-4" />
          添加下载器
        </Button>
      </div>

      {/* Test result banner */}
      {testResult && (
        <div
          className={`rounded-2xl border p-4 text-sm ${
            testResult.success
              ? "border-emerald-200 bg-emerald-50 text-emerald-800"
              : "border-red-200 bg-red-50 text-red-800"
          }`}
        >
          <div className="flex items-center justify-between gap-4">
            <div className="space-y-1">
              <div className="font-medium">{testResult.success ? "连接成功" : "连接失败"}</div>
              <div>{testResult.message}</div>
              {testResult.version && <div>版本：{testResult.version}</div>}
              {testResult.free_space !== null && (
                <div>可用空间：{formatBytes(testResult.free_space)}</div>
              )}
            </div>
            <Button variant="outline" onClick={() => setTestResult(null)}>
              关闭
            </Button>
          </div>
        </div>
      )}

      {/* Table (desktop) */}
      <Card className="rounded-2xl">
        <CardHeader>
          <CardTitle>下载器列表</CardTitle>
        </CardHeader>
        <CardContent>
          {message ? (
            <div className="mb-4 rounded-2xl border border-border bg-surface-container/70 px-4 py-3 text-sm">
              <div className="flex items-start justify-between gap-3">
                <span>{message}</span>
                <button type="button" className="text-muted hover:text-foreground" onClick={() => setMessage("")}>
                  关闭
                </button>
              </div>
            </div>
          ) : null}

          {loading ? (
            <div className="text-sm text-muted">加载中...</div>
          ) : downloaders.length === 0 ? (
            <div className="text-sm text-muted">暂无下载器，请点击右上角添加。</div>
          ) : (
            <>
              {/* Desktop table */}
              <div className="hidden xl:block">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>名称</TableHead>
                      <TableHead>类型</TableHead>
                      <TableHead>URL</TableHead>
                      <TableHead>创建时间</TableHead>
                      <TableHead>空间状态</TableHead>
                      <TableHead className="text-right">操作</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {downloaders.map((d) => (
                      <TableRow key={d.id}>
                        <TableCell>
                          <div className="font-medium">{d.name}</div>
                          <div className="text-xs text-muted">#{d.id}</div>
                        </TableCell>
                        <TableCell>
                          <span className="rounded-full bg-violet-100 px-3 py-1 text-xs font-medium text-violet-700">
                            {d.downloader_type}
                          </span>
                        </TableCell>
                        <TableCell className="max-w-xs truncate text-sm text-muted">
                          {d.url}
                        </TableCell>
                        <TableCell className="text-sm text-muted">
                          {formatDate(d.created_at)}
                        </TableCell>
                        <TableCell className="text-sm text-muted">
                          {spaceStats[d.id] ? (
                            <div className="space-y-1">
                              <div>当前空闲：{formatBytes(spaceStats[d.id].free_space)}</div>
                              <div>未完成剩余：{formatBytes(spaceStats[d.id].pending_download_bytes)}</div>
                              <div>预测可用：{formatBytes(spaceStats[d.id].effective_free_space)}</div>
                            </div>
                          ) : (
                            "加载中..."
                          )}
                        </TableCell>
                        <TableCell className="text-right">
                          <div className="flex items-center justify-end gap-2">
                            <Button
                              variant="outline"
                              disabled={testing === d.id}
                              onClick={() => handleTest(d.id)}
                            >
                              <TestTubeDiagonal className="mr-2 h-4 w-4" />
                              {testing === d.id ? "测试中..." : "测试连接"}
                            </Button>
                            <Button variant="outline" onClick={() => void openTransfer(d)}>
                              <FolderInput className="mr-2 h-4 w-4" />
                              转移种子
                            </Button>
                            <Button variant="outline" onClick={() => openEdit(d)}>
                              <Edit className="mr-2 h-4 w-4" />
                              编辑
                            </Button>
                            <Button variant="destructive" onClick={() => setDeleteTarget(d)}>
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

              {/* Mobile cards */}
              <div className="grid gap-3 xl:hidden">
                {downloaders.map((d) => (
                  <div
                    key={d.id}
                    className="rounded-[20px] border border-border bg-surface-container/70 p-3.5 shadow-sm"
                  >
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="min-w-0 flex-1">
                        <div className="font-medium text-sm truncate">{d.name}</div>
                        <div className="mt-0.5 text-[11px] text-muted">#{d.id}</div>
                      </div>
                      <span className="shrink-0 rounded-full bg-violet-100 px-2.5 py-0.5 text-[11px] font-medium text-violet-700">
                        {d.downloader_type}
                      </span>
                    </div>
                    <div className="mt-2.5 grid gap-1.5 text-[11px] text-muted sm:grid-cols-2">
                      <div className="truncate">URL: {d.url}</div>
                      <div>创建时间: {formatDate(d.created_at)}</div>
                    </div>
                    {spaceStats[d.id] ? (
                      <div className="mt-2.5 grid gap-1.5 text-[11px] text-muted sm:grid-cols-2">
                        <div>空闲: {formatBytes(spaceStats[d.id].free_space)}</div>
                        <div>未完成剩余: {formatBytes(spaceStats[d.id].pending_download_bytes)}</div>
                        <div>预测可用: {formatBytes(spaceStats[d.id].effective_free_space)}</div>
                        <div>未完成: {spaceStats[d.id].incomplete_count} / 总数: {spaceStats[d.id].torrent_count}</div>
                      </div>
                    ) : null}
                    <div className="mt-3 flex flex-wrap gap-2">
                      <Button
                        variant="outline"
                        className="h-7 text-[11px] px-2.5"
                        disabled={testing === d.id}
                        onClick={() => handleTest(d.id)}
                      >
                        <TestTubeDiagonal className="mr-1.5 h-3.5 w-3.5" />
                        {testing === d.id ? "测试中..." : "测试连接"}
                      </Button>
                      <Button variant="outline" className="h-7 text-[11px] px-2.5" onClick={() => openEdit(d)}>
                        <Edit className="mr-1.5 h-3.5 w-3.5" />
                        编辑
                      </Button>
                      <Button variant="outline" className="h-7 px-2.5 text-[11px]" onClick={() => void openTransfer(d)}>
                        <FolderInput className="mr-1.5 h-3.5 w-3.5" />
                        转移种子
                      </Button>
                      <Button variant="destructive" className="h-7 text-[11px] px-2.5" onClick={() => setDeleteTarget(d)}>
                        <Trash2 className="mr-1.5 h-3.5 w-3.5" />
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

      {/* Add / Edit dialog */}
      <Dialog
        open={dialogOpen}
        onClose={closeDialog}
        title={editingId !== null ? "编辑下载器" : "添加下载器"}
        description={editingId !== null ? "修改下载器配置信息。" : "填写下载器连接信息。"}
        escMode="double"
      >
        <div className="space-y-4 p-4 sm:p-6">
          {submitError ? (
            <div className="rounded-2xl border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
              {submitError}
            </div>
          ) : null}

          <div className="space-y-2">
            <Label htmlFor="dl-name">名称</Label>
            <Input
              id="dl-name"
              value={form.name}
              onChange={(e) => setForm((prev) => ({ ...prev, name: e.target.value }))}
              placeholder="例如：我的 qBittorrent"
            />
          </div>

          <div className="space-y-2">
            <Label>类型</Label>
            <Select
              value={form.downloader_type}
              onChange={(val) => setForm((prev) => ({ ...prev, downloader_type: val }))}
              options={[{ value: "qbittorrent", label: "qBittorrent" }]}
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="dl-url">URL</Label>
            <Input
              id="dl-url"
              value={form.url}
              onChange={(e) => setForm((prev) => ({ ...prev, url: e.target.value }))}
              placeholder="例如：http://127.0.0.1:8080"
            />
          </div>

          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="dl-user">用户名</Label>
              <Input
                id="dl-user"
                value={form.username}
                onChange={(e) => setForm((prev) => ({ ...prev, username: e.target.value }))}
                placeholder="可选"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="dl-pass">密码</Label>
              <Input
                id="dl-pass"
                type="password"
                value={form.password}
                onChange={(e) => {
                  setForm((prev) => ({ ...prev, password: e.target.value }));
                  if (e.target.value) setClearPassword(false);
                }}
                placeholder={
                  editingId !== null && existingPasswordConfigured
                    ? "留空以保留已保存密码"
                    : "可选"
                }
                disabled={clearPassword}
              />
            </div>
          </div>

          {editingId !== null && existingPasswordConfigured ? (
            <Label className="flex cursor-pointer items-center gap-2 text-sm">
              <input
                type="checkbox"
                className="size-4 accent-primary"
                checked={clearPassword}
                onChange={(event) => {
                  const checked = event.target.checked;
                  setClearPassword(checked);
                  if (checked) {
                    setForm((prev) => ({ ...prev, password: "" }));
                  }
                }}
              />
              清除已保存密码
            </Label>
          ) : null}

          <div className="flex justify-end gap-2 border-t border-border pt-4">
            <Button variant="outline" onClick={closeDialog}>
              取消
            </Button>
            <Button disabled={saving || !form.name || !form.url} onClick={handleSave}>
              {saving ? "保存中..." : "保存"}
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={transferSource !== null}
        onClose={closeTransfer}
        title={transferSource ? `从 ${transferSource.name} 转移种子` : "转移种子"}
        description="选择已完成的 qBittorrent 任务，复制到当前 OpenList 目标并恢复做种。"
        panelClassName="max-w-6xl"
      >
        <div className="flex min-h-[32rem] flex-col p-4 sm:p-6">
          <div className="mb-4 flex flex-col gap-3 border-b border-border pb-4 lg:flex-row lg:items-center lg:justify-between">
            <div className="min-w-0 text-sm">
              <div className="flex items-center gap-2 font-medium">
                <FolderInput className="h-4 w-4 text-primary" />
                目标：{selectedTarget ? `${selectedTarget.name} · ${selectedTarget.openlist_root}` : "未配置"}
              </div>
              <div className="mt-1 text-xs text-muted">已选择 {selectedHashes.size} 个，共 {transferTorrents.length} 个可转移任务</div>
            </div>
            <div className="relative w-full lg:w-80">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted" />
              <Input
                value={transferKeyword}
                onChange={(event) => setTransferKeyword(event.target.value)}
                placeholder="搜索名称或 info hash"
                aria-label="搜索可转移种子"
                className="pl-9"
              />
            </div>
          </div>

          {transferError ? <div className="mb-4 rounded-lg border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">{transferError}</div> : null}
          {transferLoading ? (
            <div className="flex flex-1 items-center justify-center gap-2 text-sm text-muted"><LoaderCircle className="h-4 w-4 animate-spin" />加载种子列表</div>
          ) : visibleTorrents.length === 0 ? (
            <div className="flex flex-1 items-center justify-center text-sm text-muted">{transferTorrents.length ? "没有匹配的种子" : "没有已完成的种子"}</div>
          ) : (
            <div className="min-h-0 flex-1 overflow-auto rounded-lg border border-border">
              <Table>
                <TableHeader className="sticky top-0 z-10 bg-card">
                  <TableRow>
                    <TableHead className="w-12">
                      <button type="button" onClick={toggleVisibleTorrents} aria-label={visibleAllSelected ? "取消全选当前结果" : "全选当前结果"} className="flex size-8 items-center justify-center rounded-md text-muted transition-colors hover:bg-accent hover:text-foreground">
                        <CheckSquare2 className={`h-4 w-4 ${visibleAllSelected ? "text-primary" : ""}`} />
                      </button>
                    </TableHead>
                    <TableHead>种子</TableHead><TableHead>大小</TableHead><TableHead>保存路径</TableHead><TableHead>分类 / 标签</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {visibleTorrents.map((torrent) => {
                    const checked = selectedHashes.has(torrent.hash);
                    return (
                      <TableRow key={torrent.hash} className={checked ? "bg-primary/5" : undefined}>
                        <TableCell>
                          <input type="checkbox" className="size-4 cursor-pointer accent-primary" checked={checked} onChange={() => toggleTorrent(torrent.hash)} aria-label={`选择 ${torrent.name}`} />
                        </TableCell>
                        <TableCell><div className="max-w-md truncate font-medium" title={torrent.name}>{torrent.name}</div><div className="mt-1 font-mono text-xs text-muted">{torrent.hash.slice(0, 12)}</div></TableCell>
                        <TableCell className="whitespace-nowrap">{formatBytes(torrent.size)}</TableCell>
                        <TableCell><div className="max-w-xs truncate text-xs text-muted" title={torrent.save_path}>{torrent.save_path}</div></TableCell>
                        <TableCell className="text-xs text-muted">{[torrent.category, torrent.tags].filter(Boolean).join(" · ") || "-"}</TableCell>
                      </TableRow>
                    );
                  })}
                </TableBody>
              </Table>
            </div>
          )}

          <div className="mt-4 flex flex-col-reverse gap-2 border-t border-border pt-4 sm:flex-row sm:items-center sm:justify-end">
            <Button variant="outline" onClick={closeTransfer} disabled={transferring}>取消</Button>
            <Button onClick={() => void submitTransfer()} disabled={transferring || selectedHashes.size === 0 || !openListSettings?.enabled || !selectedTarget}>
              {transferring ? <LoaderCircle className="mr-2 h-4 w-4 animate-spin" /> : <ArrowRight className="mr-2 h-4 w-4" />}
              {transferring ? "创建中" : `转移 ${selectedHashes.size} 个种子`}
            </Button>
          </div>
        </div>
      </Dialog>

      {/* Delete confirmation dialog */}
      <Dialog
        open={deleteTarget !== null}
        onClose={() => setDeleteTarget(null)}
        title="确认删除"
        description={`确定要删除下载器「${deleteTarget?.name ?? ""}」吗？此操作不可撤销。`}
      >
        <div className="flex justify-end gap-2 p-4 sm:p-6">
          <Button variant="outline" onClick={() => setDeleteTarget(null)}>
            取消
          </Button>
          <Button variant="destructive" disabled={deleting} onClick={handleDelete}>
            {deleting ? "删除中..." : "确认删除"}
          </Button>
        </div>
      </Dialog>
    </div>
  );
}
