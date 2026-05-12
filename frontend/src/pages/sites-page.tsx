import { useEffect, useState } from "react";
import {
  Globe,
  Plus,
  Pencil,
  Trash2,
  Activity,
  ListChecks,
  Loader2,
  Trophy,
  UploadCloud,
  DownloadCloud,
  Gauge,
  ShieldCheck,
  Copy,
  Download as DownloadIcon,
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
import { Select } from "@/components/ui/select";
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
import type { SiteRecord, SiteStatsRecord, SiteTestResult } from "@/types";

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

function formatBytesCompact(bytes: number): { value: string; unit: string } {
  if (bytes === 0) return { value: "0", unit: "B" };
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), sizes.length - 1);
  return {
    value: (bytes / Math.pow(k, i)).toFixed(bytes >= Math.pow(k, 4) ? 2 : 1),
    unit: sizes[i],
  };
}

function formatRatio(uploaded: number, downloaded: number): string {
  if (downloaded <= 0) {
    return uploaded > 0 ? "∞" : "-";
  }
  return (uploaded / downloaded).toFixed(3);
}

function dateStamp(date: Date): string {
  const pad = (value: number) => value.toString().padStart(2, "0");
  return `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-${pad(date.getHours())}${pad(date.getMinutes())}`;
}

function truncateCanvasText(ctx: CanvasRenderingContext2D, text: string, maxWidth: number): string {
  if (ctx.measureText(text).width <= maxWidth) return text;
  let next = text;
  while (next.length > 0 && ctx.measureText(`${next}...`).width > maxWidth) {
    next = next.slice(0, -1);
  }
  return `${next}...`;
}

function roundedRectPath(ctx: CanvasRenderingContext2D, x: number, y: number, width: number, height: number, radius: number) {
  const r = Math.min(radius, width / 2, height / 2);
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.lineTo(x + width - r, y);
  ctx.quadraticCurveTo(x + width, y, x + width, y + r);
  ctx.lineTo(x + width, y + height - r);
  ctx.quadraticCurveTo(x + width, y + height, x + width - r, y + height);
  ctx.lineTo(x + r, y + height);
  ctx.quadraticCurveTo(x, y + height, x, y + height - r);
  ctx.lineTo(x, y + r);
  ctx.quadraticCurveTo(x, y, x + r, y);
  ctx.closePath();
}

function drawRoundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  radius: number,
  fill: string | CanvasGradient,
  stroke?: string,
) {
  roundedRectPath(ctx, x, y, width, height, radius);
  ctx.fillStyle = fill;
  ctx.fill();
  if (stroke) {
    ctx.strokeStyle = stroke;
    ctx.lineWidth = 1;
    ctx.stroke();
  }
}

function drawText(
  ctx: CanvasRenderingContext2D,
  text: string,
  x: number,
  y: number,
  options: {
    font: string;
    color: string;
    maxWidth?: number;
    align?: CanvasTextAlign;
  },
) {
  ctx.font = options.font;
  ctx.fillStyle = options.color;
  ctx.textAlign = options.align ?? "left";
  ctx.textBaseline = "alphabetic";
  ctx.fillText(options.maxWidth ? truncateCanvasText(ctx, text, options.maxWidth) : text, x, y);
}

function renderOverviewProofImage({
  rows,
  generatedAt,
}: {
  rows: SiteOverviewRow[];
  generatedAt: Date;
}): Promise<Blob> {
  const successfulRows = rows.filter((row) => row.stats);
  const failedRows = rows.filter((row) => !row.stats);
  const totalUploaded = successfulRows.reduce((sum, row) => sum + (row.stats?.uploaded ?? 0), 0);
  const totalDownloaded = successfulRows.reduce((sum, row) => sum + (row.stats?.downloaded ?? 0), 0);
  const uploaded = formatBytesCompact(totalUploaded);
  const downloaded = formatBytesCompact(totalDownloaded);
  const topRows = successfulRows
    .toSorted((a, b) => (b.stats?.uploaded ?? 0) - (a.stats?.uploaded ?? 0))
    .slice(0, 4);
  const width = 1600;
  const tableRowHeight = 58;
  const height = Math.max(1050, 760 + rows.length * tableRowHeight);
  const scale = 2;
  const canvas = document.createElement("canvas");
  canvas.width = width * scale;
  canvas.height = height * scale;
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return Promise.reject(new Error("当前浏览器不支持图片导出"));
  }

  ctx.scale(scale, scale);
  ctx.fillStyle = "#fbf7ff";
  ctx.fillRect(0, 0, width, height);

  ctx.strokeStyle = "rgba(126, 96, 194, 0.08)";
  ctx.lineWidth = 1;
  for (let x = 0; x < width; x += 56) {
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, height);
    ctx.stroke();
  }
  for (let y = 0; y < height; y += 56) {
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(width, y);
    ctx.stroke();
  }

  const heroGradient = ctx.createLinearGradient(48, 48, 1552, 280);
  heroGradient.addColorStop(0, "#fffaff");
  heroGradient.addColorStop(0.58, "#f1e8ff");
  heroGradient.addColorStop(1, "#dfd2ff");
  drawRoundRect(ctx, 48, 48, 1504, 238, 36, heroGradient, "rgba(126, 96, 194, 0.18)");

  drawRoundRect(ctx, 88, 88, 92, 92, 26, "#7d5cff");
  drawText(ctx, "云", 111, 148, { font: "900 42px Inter, system-ui, sans-serif", color: "#ffffff" });
  drawText(ctx, "YUNMU PT PROOF", 208, 108, { font: "900 22px Inter, system-ui, sans-serif", color: "#7d5cff" });
  drawText(ctx, "PT 账号数据", 208, 166, { font: "900 56px Inter, system-ui, sans-serif", color: "#20173d" });

  const generatedText = `生成时间 ${generatedAt.toLocaleString()}`;
  drawRoundRect(ctx, 1120, 88, 360, 46, 23, "rgba(255,255,255,0.62)", "rgba(126, 96, 194, 0.16)");
  drawText(ctx, generatedText, 1142, 119, { font: "800 18px Inter, system-ui, sans-serif", color: "#4a347f", maxWidth: 316 });
  drawRoundRect(ctx, 1120, 150, 170, 46, 23, "rgba(255,255,255,0.62)", "rgba(126, 96, 194, 0.16)");
  drawText(ctx, `${successfulRows.length} 个站点已验证`, 1142, 181, { font: "800 18px Inter, system-ui, sans-serif", color: "#4a347f" });
  drawRoundRect(ctx, 1310, 150, 170, 46, 23, "rgba(255,255,255,0.62)", "rgba(126, 96, 194, 0.16)");
  drawText(ctx, `${failedRows.length} 个失败`, 1332, 181, { font: "800 18px Inter, system-ui, sans-serif", color: failedRows.length ? "#d83a57" : "#4a347f" });

  const metrics = [
    ["总上传量", uploaded.value, uploaded.unit],
    ["总下载量", downloaded.value, downloaded.unit],
    ["综合分享率", formatRatio(totalUploaded, totalDownloaded), "ratio"],
    ["可展示站点", `${successfulRows.length}`, `/ ${rows.length}`],
  ];
  metrics.forEach(([label, value, unit], index) => {
    const x = 48 + index * 376;
    drawRoundRect(ctx, x, 320, 352, 150, 26, "rgba(255,255,255,0.82)", "rgba(126, 96, 194, 0.16)");
    drawText(ctx, label, x + 28, 362, { font: "800 20px Inter, system-ui, sans-serif", color: "#6d6289" });
    drawText(ctx, value, x + 28, 430, { font: "900 48px Inter, system-ui, sans-serif", color: "#20173d", maxWidth: 210 });
    drawText(ctx, unit, x + 245, 430, { font: "900 22px Inter, system-ui, sans-serif", color: "#7d5cff", maxWidth: 80 });
  });

  drawRoundRect(ctx, 48, 510, 1504, 154, 28, "rgba(255,255,255,0.72)", "rgba(126, 96, 194, 0.16)");
  drawText(ctx, "上传量排行", 82, 554, { font: "900 26px Inter, system-ui, sans-serif", color: "#20173d" });
  topRows.forEach((row, index) => {
    const x = 82 + index * 360;
    const stats = row.stats;
    drawRoundRect(ctx, x, 584, 328, 54, 18, "rgba(245,238,255,0.78)", "rgba(126, 96, 194, 0.12)");
    drawRoundRect(ctx, x + 14, 597, 30, 30, 12, "#7d5cff");
    drawText(ctx, String(index + 1), x + 24, 619, { font: "900 16px Inter, system-ui, sans-serif", color: "#ffffff" });
    drawText(ctx, row.site.name, x + 56, 609, { font: "900 18px Inter, system-ui, sans-serif", color: "#20173d", maxWidth: 130 });
    drawText(ctx, stats ? formatBytes(stats.uploaded) : "-", x + 56, 630, { font: "800 15px Inter, system-ui, sans-serif", color: "#6d6289", maxWidth: 130 });
    drawText(ctx, stats?.username ?? "-", x + 204, 622, { font: "800 15px Inter, system-ui, sans-serif", color: "#4a347f", maxWidth: 90 });
  });

  const tableY = 708;
  drawText(ctx, "站点明细", 48, tableY - 24, { font: "900 26px Inter, system-ui, sans-serif", color: "#20173d" });
  const cols = [
    ["站点", 74, 190],
    ["UID", 282, 140],
    ["用户名", 440, 210],
    ["上传量", 668, 210],
    ["下载量", 896, 210],
    ["分享率", 1124, 120],
    ["状态", 1268, 210],
  ] as const;
  drawRoundRect(ctx, 48, tableY, 1504, 48, 18, "rgba(238,229,255,0.92)", "rgba(126, 96, 194, 0.16)");
  cols.forEach(([label, x]) =>
    drawText(ctx, label, x, tableY + 31, { font: "900 16px Inter, system-ui, sans-serif", color: "#6d6289" }),
  );

  rows.forEach((row, index) => {
    const y = tableY + 60 + index * tableRowHeight;
    const stats = row.stats;
    drawRoundRect(ctx, 48, y, 1504, 48, 18, index % 2 === 0 ? "rgba(255,255,255,0.78)" : "rgba(246,240,255,0.72)", "rgba(126, 96, 194, 0.12)");
    const values = [
      row.site.name,
      stats?.uid ?? "-",
      stats?.username ?? "-",
      stats ? formatBytes(stats.uploaded) : "-",
      stats ? formatBytes(stats.downloaded) : "-",
      stats ? formatRatio(stats.uploaded, stats.downloaded) : "-",
      row.error ?? "正常",
    ];
    cols.forEach(([, x, maxWidth], colIndex) =>
      drawText(ctx, values[colIndex], x, y + 31, {
        font: colIndex === 0 || colIndex === 3 ? "900 17px Inter, system-ui, sans-serif" : "700 16px Inter, system-ui, sans-serif",
        color: row.error && colIndex === 6 ? "#d83a57" : colIndex === 3 ? "#20173d" : "#5f5478",
        maxWidth,
      }),
    );
  });

  drawText(ctx, "Generated by 云母", 48, height - 38, { font: "800 18px Inter, system-ui, sans-serif", color: "#7d5cff" });

  return new Promise((resolve, reject) => {
    canvas.toBlob((blob) => {
      if (blob) {
        resolve(blob);
      } else {
        reject(new Error("图片生成失败"));
      }
    }, "image/png", 0.95);
  });
}

type SiteOverviewRow = {
  site: SiteRecord;
  stats: SiteStatsRecord | null;
  error: string | null;
};

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

  // overview
  const [overviewRows, setOverviewRows] = useState<SiteOverviewRow[]>([]);
  const [overviewOpen, setOverviewOpen] = useState(false);
  const [overviewLoading, setOverviewLoading] = useState(false);
  const [overviewGeneratedAt, setOverviewGeneratedAt] = useState<Date | null>(null);
  const [overviewExporting, setOverviewExporting] = useState<"copy" | "download" | null>(null);

  const successfulOverviewRows = overviewRows.filter((row) => row.stats);
  const failedOverviewRows = overviewRows.filter((row) => !row.stats);
  const totalUploaded = successfulOverviewRows.reduce((sum, row) => sum + (row.stats?.uploaded ?? 0), 0);
  const totalDownloaded = successfulOverviewRows.reduce((sum, row) => sum + (row.stats?.downloaded ?? 0), 0);
  const totalUploadedCompact = formatBytesCompact(totalUploaded);
  const totalDownloadedCompact = formatBytesCompact(totalDownloaded);
  const topOverviewRows = successfulOverviewRows
    .toSorted((a, b) => (b.stats?.uploaded ?? 0) - (a.stats?.uploaded ?? 0))
    .slice(0, 4);

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

  function handleOverview() {
    setOverviewOpen(true);
    setOverviewLoading(true);
    setOverviewRows([]);
    setOverviewGeneratedAt(null);
    api<SiteRecord[]>("/api/sites/stats-overview")
      .then((data) => {
        setSites(data);
        return data.map((site) => ({
          site,
          stats: site.stats?.uploaded != null && site.stats?.downloaded != null ? site.stats : null,
          error: site.stats?.last_error ?? null,
        }));
      })
      .then((rows) => {
        setOverviewRows(
          rows.toSorted((a, b) => {
            if (a.error && !b.error) return 1;
            if (!a.error && b.error) return -1;
            return a.site.id - b.site.id;
          }),
        );
      })
      .catch((error: Error) => setMessage(error.message || "刷新站点总览失败"))
      .then(() => setOverviewGeneratedAt(new Date()))
      .finally(() => setOverviewLoading(false));
  }

  async function createOverviewImageBlob() {
    const generatedAt = overviewGeneratedAt ?? new Date();
    return renderOverviewProofImage({ rows: overviewRows, generatedAt });
  }

  function downloadOverviewBlob(blob: Blob) {
    const generatedAt = overviewGeneratedAt ?? new Date();
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = `yunmu-pt-proof-${dateStamp(generatedAt)}.png`;
    document.body.appendChild(link);
    link.click();
    link.remove();
    URL.revokeObjectURL(url);
  }

  async function handleCopyOverviewImage() {
    setOverviewExporting("copy");
    try {
      const blob = await createOverviewImageBlob();
      const ClipboardItemCtor = window.ClipboardItem;
      if (!navigator.clipboard || !ClipboardItemCtor) {
        downloadOverviewBlob(blob);
        setMessage("当前浏览器不支持图片剪切板，已改为下载 PNG");
        return;
      }
      await navigator.clipboard.write([new ClipboardItemCtor({ "image/png": blob })]);
      setMessage("PT 数据证明图已复制到剪切板");
    } catch (error) {
      setMessage((error as Error).message || "复制图片失败");
    } finally {
      setOverviewExporting(null);
    }
  }

  async function handleDownloadOverviewImage() {
    setOverviewExporting("download");
    try {
      const blob = await createOverviewImageBlob();
      downloadOverviewBlob(blob);
      setMessage("PT 数据证明图已下载");
    } catch (error) {
      setMessage((error as Error).message || "下载图片失败");
    } finally {
      setOverviewExporting(null);
    }
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
            <div className="flex flex-wrap justify-end gap-2">
              <Button
                variant="outline"
                onClick={handleOverview}
                disabled={loading || sites.length === 0}
              >
                <ListChecks className="mr-2 h-4 w-4" />
                总览
              </Button>
              <Button onClick={openAdd}>
                <Plus className="mr-2 h-4 w-4" />
                添加站点
              </Button>
            </div>
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
                      <TableHead>上传量</TableHead>
                      <TableHead>下载量</TableHead>
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
                        <TableCell className="font-semibold">
                          {site.stats?.uploaded != null ? formatBytes(site.stats.uploaded) : "待刷新"}
                        </TableCell>
                        <TableCell className="text-muted">
                          {site.stats?.downloaded != null ? formatBytes(site.stats.downloaded) : "待刷新"}
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
                    className="rounded-[20px] border border-border bg-surface-container/70 p-3.5"
                  >
                    <div className="flex items-center justify-between">
                      <span className="font-semibold text-sm">{site.name}</span>
                      <span className="rounded-full bg-violet-100 px-2 py-0.5 text-[10px] text-violet-700">
                        {site.site_type}
                      </span>
                    </div>
                    <p className="mt-1 truncate text-[11px] text-muted">
                      {site.base_url}
                    </p>
                    <div className="mt-2.5 grid grid-cols-2 gap-2 text-sm">
                      <div className="rounded-xl bg-card/70 p-2">
                        <div className="text-[10px] font-bold text-muted">上: <span className="font-black text-foreground">{site.stats?.uploaded != null ? formatBytes(site.stats.uploaded) : "-"}</span></div>
                      </div>
                      <div className="rounded-xl bg-card/70 p-2">
                        <div className="text-[10px] font-bold text-muted">下: <span className="font-black text-foreground">{site.stats?.downloaded != null ? formatBytes(site.stats.downloaded) : "-"}</span></div>
                      </div>
                    </div>
                    <div className="mt-2.5 grid grid-cols-3 gap-2">
                      <Button
                        variant="outline"
                        className="h-7 px-0 text-[11px]"
                        onClick={() => handleTest(site)}
                      >
                        <Activity className="mr-1 h-3 w-3" />
                        测试
                      </Button>
                      <Button
                        variant="secondary"
                        className="h-7 px-0 text-[11px]"
                        onClick={() => openEdit(site)}
                      >
                        <Pencil className="mr-1 h-3 w-3" />
                        编辑
                      </Button>
                      <Button
                        variant="destructive"
                        className="h-7 px-0 text-[11px]"
                        onClick={() => setDeleteTarget(site)}
                      >
                        <Trash2 className="mr-1 h-3 w-3" />
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
        escMode="double"
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
            <Select
              value={form.site_type}
              onChange={(val) => {
                const v = val as SiteForm["site_type"];
                patch({
                  site_type: v,
                  auth_type: v === "mteam" ? "api_key" : "cookie",
                });
              }}
              options={[
                { value: "nexusphp", label: "NexusPHP" },
                { value: "mteam", label: "M-Team" },
              ]}
            />
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

      {/* ---- overview dialog ---- */}
      <Dialog
        open={overviewOpen}
        onClose={() => setOverviewOpen(false)}
        title="PT 数据总览"
        panelClassName="max-w-7xl"
      >
        {overviewLoading ? (
          <div className="flex min-h-[420px] items-center justify-center py-8 text-muted">
            <Loader2 className="mr-2 h-5 w-5 animate-spin" />
            正在并发拉取所有站点数据…
          </div>
        ) : overviewRows.length === 0 ? (
          <p className="py-8 text-center text-muted">暂无站点统计数据</p>
        ) : (
          <div className="flex flex-col gap-5 p-4 sm:p-6">
            <section className="relative overflow-hidden rounded-[32px] border border-primary/15 bg-gradient-to-br from-[#fff9ff] via-[#f2eaff] to-[#dfd2ff] p-5 shadow-card sm:p-7">
              <div className="pointer-events-none absolute -right-10 -top-16 h-56 w-56 rounded-full bg-blossom/20 blur-3xl" />
              <div className="pointer-events-none absolute bottom-0 right-10 h-px w-56 bg-gradient-to-r from-transparent via-primary/40 to-transparent" />

              <div className="relative flex flex-col gap-5 lg:flex-row lg:items-end lg:justify-between">
                <div>
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="rounded-full border border-primary/20 bg-white/70 px-3 py-1 text-xs font-black tracking-[0.18em] text-primary">
                      YUNMU PT PROOF
                    </span>
                    <span className="rounded-full border border-blossom/20 bg-white/60 px-3 py-1 text-xs font-bold text-[#b43767]">
                      {successfulOverviewRows.length} 个站点已验证
                    </span>
                  </div>
                  <h3 className="mt-4 text-3xl font-black tracking-tight text-foreground sm:text-5xl">
                    PT 账号数据
                  </h3>
                </div>

                <div className="flex flex-col gap-3 lg:w-[380px]">
                  <div className="self-start rounded-full border border-primary/15 bg-white/65 p-1 shadow-[inset_0_1px_0_rgba(255,255,255,0.9)] backdrop-blur lg:self-end">
                    <div className="flex items-center gap-1">
                      <span className="px-3 text-xs font-black tracking-[0.14em] text-muted">导出 PNG</span>
                      <button
                        type="button"
                        className="inline-flex h-9 w-9 items-center justify-center rounded-full text-primary transition hover:bg-secondary disabled:opacity-50"
                        onClick={() => void handleCopyOverviewImage()}
                        disabled={overviewExporting !== null}
                        title="复制图片到剪切板"
                        aria-label="复制图片到剪切板"
                      >
                        {overviewExporting === "copy" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Copy className="h-4 w-4" />}
                      </button>
                      <button
                        type="button"
                        className="inline-flex h-9 w-9 items-center justify-center rounded-full text-primary transition hover:bg-secondary disabled:opacity-50"
                        onClick={() => void handleDownloadOverviewImage()}
                        disabled={overviewExporting !== null}
                        title="下载图片"
                        aria-label="下载图片"
                      >
                        {overviewExporting === "download" ? <Loader2 className="h-4 w-4 animate-spin" /> : <DownloadIcon className="h-4 w-4" />}
                      </button>
                    </div>
                  </div>
                  <div className="grid gap-2 text-sm sm:grid-cols-2">
                    <ProofInfo label="生成时间" value={overviewGeneratedAt?.toLocaleString() ?? "-"} />
                    <ProofInfo label="配置站点" value={`${overviewRows.length} 个`} />
                    <ProofInfo label="成功统计" value={`${successfulOverviewRows.length} 个`} />
                    <ProofInfo label="失败站点" value={`${failedOverviewRows.length} 个`} />
                  </div>
                </div>
              </div>
            </section>

            <section className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
              <OverviewMetricCard icon={UploadCloud} label="总上传量" value={totalUploadedCompact.value} unit={totalUploadedCompact.unit} />
              <OverviewMetricCard icon={DownloadCloud} label="总下载量" value={totalDownloadedCompact.value} unit={totalDownloadedCompact.unit} />
              <OverviewMetricCard icon={Gauge} label="综合分享率" value={formatRatio(totalUploaded, totalDownloaded)} unit="ratio" />
              <OverviewMetricCard icon={ShieldCheck} label="可展示站点" value={`${successfulOverviewRows.length}`} unit={`/ ${overviewRows.length}`} />
            </section>

            {topOverviewRows.length > 0 ? (
              <section className="rounded-[28px] border border-border bg-card/85 p-4 shadow-card backdrop-blur sm:p-5">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div>
                    <h4 className="text-base font-black">上传量排行</h4>
                    <p className="mt-1 text-xs text-muted">按上传量取前 {topOverviewRows.length} 个站点</p>
                  </div>
                  <span className="rounded-full border border-primary/15 bg-secondary px-3 py-1 text-xs font-bold text-secondary-foreground">
                    TOP {topOverviewRows.length}
                  </span>
                </div>

                <div className="mt-4 grid gap-2 lg:grid-cols-2 2xl:grid-cols-4">
                  {topOverviewRows.map((row, index) => (
                    <OverviewRankCard key={row.site.id} row={row} rank={index + 1} />
                  ))}
                </div>
              </section>
            ) : null}

            <section className="hidden lg:block">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>站点</TableHead>
                    <TableHead>UID</TableHead>
                    <TableHead>用户名</TableHead>
                    <TableHead>上传量</TableHead>
                    <TableHead>下载量</TableHead>
                    <TableHead>分享率</TableHead>
                    <TableHead>状态</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {overviewRows.map((row) => (
                    <TableRow key={row.site.id}>
                      <TableCell className="font-bold">{row.site.name}</TableCell>
                      <TableCell className="font-mono text-xs">{row.stats?.uid ?? "-"}</TableCell>
                      <TableCell>{row.stats?.username ?? "-"}</TableCell>
                      <TableCell className="font-semibold">
                        {row.stats ? formatBytes(row.stats.uploaded) : "-"}
                      </TableCell>
                      <TableCell className="text-muted">
                        {row.stats ? formatBytes(row.stats.downloaded) : "-"}
                      </TableCell>
                      <TableCell className="font-semibold">
                        {row.stats ? formatRatio(row.stats.uploaded, row.stats.downloaded) : "-"}
                      </TableCell>
                      <TableCell className={row.error ? "text-red-600" : "text-muted"}>
                        {row.error ?? "正常"}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </section>

            <section className="grid gap-3 lg:hidden">
              {overviewRows.map((row) => (
                <OverviewMobileCard key={row.site.id} row={row} />
              ))}
            </section>

            <div className="flex justify-end print:hidden">
              <Button variant="secondary" onClick={() => setOverviewOpen(false)}>
                关闭
              </Button>
            </div>
          </div>
        )}
      </Dialog>
    </div>
  );
}

function ProofInfo({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-2xl border border-white/60 bg-white/55 px-4 py-3 backdrop-blur">
      <div className="text-[11px] font-bold uppercase tracking-[0.16em] text-muted">{label}</div>
      <div className="mt-1 truncate text-sm font-black text-foreground">{value}</div>
    </div>
  );
}

function OverviewMetricCard({
  icon: Icon,
  label,
  value,
  unit,
}: {
  icon: typeof UploadCloud;
  label: string;
  value: string;
  unit: string;
}) {
  return (
    <div className="rounded-[26px] border border-border bg-card/85 p-5 shadow-card backdrop-blur">
      <div className="flex items-center justify-between gap-3">
        <span className="text-sm font-bold text-muted">{label}</span>
        <span className="flex h-11 w-11 items-center justify-center rounded-2xl bg-secondary text-primary">
          <Icon className="h-5 w-5" />
        </span>
      </div>
      <div className="mt-4 flex items-end gap-2">
        <span className="text-4xl font-black tracking-tight text-foreground">{value}</span>
        <span className="pb-1 text-sm font-bold text-muted">{unit}</span>
      </div>
    </div>
  );
}

function OverviewRankCard({ row, rank }: { row: SiteOverviewRow; rank: number }) {
  const stats = row.stats;
  return (
    <div className="flex items-center gap-3 rounded-2xl border border-border bg-surface-container/70 p-3">
      <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-2xl bg-primary text-sm font-black text-primary-foreground">
        {rank}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <div className="truncate font-black">{row.site.name}</div>
          <Trophy className="h-4 w-4 shrink-0 text-blossom" />
        </div>
        <div className="mt-0.5 truncate text-xs text-muted">UID {stats?.uid ?? "-"} · {stats?.username ?? "-"}</div>
      </div>
      <div className="shrink-0 text-right">
        <div className="text-[11px] font-bold text-muted">上传</div>
        <div className="text-sm font-black">{stats ? formatBytes(stats.uploaded) : "-"}</div>
      </div>
    </div>
  );
}

function OverviewMobileCard({ row }: { row: SiteOverviewRow }) {
  const stats = row.stats;
  return (
    <div className="rounded-[20px] border border-border bg-card/85 p-3.5 shadow-card">
      <div className="flex items-start justify-between gap-3">
        <div>
          <div className="text-base font-black">{row.site.name}</div>
          <div className="mt-0.5 text-[11px] text-muted">UID {stats?.uid ?? "-"} · {stats?.username ?? "-"}</div>
        </div>
        <span className={`rounded-full px-2.5 py-0.5 text-[10px] font-bold ${row.error ? "bg-red-100 text-red-700" : "bg-emerald-100 text-emerald-700"}`}>
          {row.error ? "失败" : "正常"}
        </span>
      </div>
      <div className="mt-3 grid grid-cols-3 gap-2 text-sm">
        <div className="rounded-xl bg-surface-container/70 p-2.5">
          <div className="text-[10px] font-bold text-muted">上:</div>
          <div className="mt-0.5 text-sm font-black truncate">{stats ? formatBytes(stats.uploaded) : "-"}</div>
        </div>
        <div className="rounded-xl bg-surface-container/70 p-2.5">
          <div className="text-[10px] font-bold text-muted">下:</div>
          <div className="mt-0.5 text-sm font-black truncate">{stats ? formatBytes(stats.downloaded) : "-"}</div>
        </div>
        <div className="rounded-xl bg-surface-container/70 p-2.5">
          <div className="text-[10px] font-bold text-muted">率:</div>
          <div className="mt-0.5 text-sm font-black truncate">{stats ? formatRatio(stats.uploaded, stats.downloaded) : "-"}</div>
        </div>
      </div>
      {row.error ? <p className="mt-3 text-xs text-red-600">{row.error}</p> : null}
    </div>
  );
}
