import { useEffect, useMemo, useRef, useState } from "react";
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
  Check,
  Eye,
  EyeOff,
  ExternalLink,
  Download as DownloadIcon,
  RefreshCw,
  RotateCcw,
  Search,
  CloudCog,
  Server,
  KeyRound,
  Clock3,
  CircleCheck,
  CircleX,
  Settings2,
  ChevronLeft,
  ChevronRight,
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
import type {
  PtdBackupConfig,
  PtdBackupRunResult,
  PtdBackupTestResult,
  SiteCredentialsRecord,
  SiteRecord,
  SiteRequestHeader,
  SiteStatsRefreshStartResponse,
  SiteStatsRefreshStatusResponse,
  SiteStatsRecord,
  SiteTestResult,
} from "@/types";

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

function formatDateTime(value: string | null | undefined): string {
  if (!value) return "从未";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

type SiteHealth = "healthy" | "failed" | "pending";
const SITE_PAGE_SIZE = 20;

function getSiteHealth(site: SiteRecord): SiteHealth {
  if (site.stats?.last_error) return "failed";
  if (site.stats?.uploaded != null && site.stats?.downloaded != null) return "healthy";
  return "pending";
}

type AuthType = "cookie" | "passkey" | "cookie_passkey" | "api_key";
type CredentialField = "cookie" | "passkey" | "api_key";

const credentialFieldLabels: Record<CredentialField, string> = {
  cookie: "Cookie",
  passkey: "Passkey",
  api_key: "API Key",
};

function credentialFieldsFor(authType: SiteRecord["auth_type"]): CredentialField[] {
  switch (authType) {
    case "cookie":
      return ["cookie"];
    case "passkey":
      return ["passkey"];
    case "cookie_passkey":
      return ["cookie", "passkey"];
    case "api_key":
      return ["api_key"];
    default:
      return [];
  }
}

function credentialStateKey(siteId: number, field: CredentialField): string {
  return `${siteId}:${field}`;
}

function credentialActionStateKey(
  siteId: number,
  field: CredentialField,
  action: "reveal" | "copy",
): string {
  return `${credentialStateKey(siteId, field)}:${action}`;
}

async function writeClipboardText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(value);
      return;
    } catch {
      // Fall through for browsers that expose Clipboard API but deny it on HTTP.
    }
  }

  const textarea = document.createElement("textarea");
  textarea.value = value;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  const copied = document.execCommand("copy");
  textarea.remove();
  if (!copied) {
    throw new Error("当前浏览器不支持复制到剪切板");
  }
}

interface SiteForm {
  name: string;
  site_type: "nexusphp" | "mteam";
  base_url: string;
  use_proxy: boolean;
  auth_type: AuthType;
  cookie: string;
  passkey: string;
  api_key: string;
  request_headers: SiteRequestHeader[];
}

interface PtdBackupForm {
  enabled: boolean;
  webdav_url: string;
  username: string;
  password: string;
  clear_password: boolean;
  use_proxy: boolean;
  backup_interval_hours: number;
  site_mappings: Record<string, string>;
}

const emptyPtdBackupForm: PtdBackupForm = {
  enabled: false,
  webdav_url: "",
  username: "",
  password: "",
  clear_password: false,
  use_proxy: false,
  backup_interval_hours: 24,
  site_mappings: {},
};

const defaultRequestHeaders: ReadonlyArray<Readonly<SiteRequestHeader>> = [
  {
    name: "Accept",
    value: "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
  },
  { name: "Accept-Encoding", value: "gzip, deflate, br, zstd" },
  {
    name: "Accept-Language",
    value: "zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7,zh-TW;q=0.6",
  },
  { name: "DNT", value: "1" },
  {
    name: "User-Agent",
    value: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36",
  },
  {
    name: "sec-ch-ua",
    value: '"Not=A?Brand";v="99", "Google Chrome";v="151", "Chromium";v="151"',
  },
  { name: "sec-ch-ua-arch", value: '"x86"' },
  { name: "sec-ch-ua-bitness", value: '"64"' },
  { name: "sec-ch-ua-full-version", value: '"151.0.7922.109"' },
  {
    name: "sec-ch-ua-full-version-list",
    value: '"Not=A?Brand";v="99.0.0.0", "Google Chrome";v="151.0.7922.109", "Chromium";v="151.0.7922.109"',
  },
  { name: "sec-ch-ua-mobile", value: "?0" },
  { name: "sec-ch-ua-model", value: '""' },
  { name: "sec-ch-ua-platform", value: '"Windows"' },
  { name: "sec-ch-ua-platform-version", value: '"19.0.0"' },
];

function freshDefaultRequestHeaders(): SiteRequestHeader[] {
  return defaultRequestHeaders.map((header) => ({ ...header }));
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
  use_proxy: true,
  auth_type: "cookie",
  cookie: "",
  passkey: "",
  api_key: "",
  request_headers: freshDefaultRequestHeaders(),
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
    case "api_key":
      return { auth_type: "api_key", api_key: form.api_key };
    default:
      return { auth_type: form.auth_type };
  }
}

function SiteCredentialList({
  site,
  credentials,
  revealedKeys,
  loadingKeys,
  copiedKey,
  compact = false,
  onToggle,
  onCopy,
}: {
  site: SiteRecord;
  credentials?: SiteCredentialsRecord;
  revealedKeys: Set<string>;
  loadingKeys: Set<string>;
  copiedKey: string | null;
  compact?: boolean;
  onToggle: (site: SiteRecord, field: CredentialField) => void;
  onCopy: (site: SiteRecord, field: CredentialField) => void;
}) {
  const fields = credentialFieldsFor(site.auth_type);
  if (!site.auth_configured || fields.length === 0) {
    return <span className="text-xs text-muted">未配置</span>;
  }

  return (
    <div className={compact ? "space-y-1.5" : "min-w-[230px] space-y-1.5"}>
      {fields.map((field) => {
        const stateKey = credentialStateKey(site.id, field);
        const revealed = revealedKeys.has(stateKey);
        const revealLoading = loadingKeys.has(credentialActionStateKey(site.id, field, "reveal"));
        const copyLoading = loadingKeys.has(credentialActionStateKey(site.id, field, "copy"));
        const loading = revealLoading || copyLoading;
        const copied = copiedKey === stateKey;
        const value = credentials?.[field] ?? "";
        const visibleValue = revealed ? value || "未配置" : "••••••••••••";

        return (
          <div key={field} className="flex h-8 min-w-0 items-center gap-1.5">
            <span className="w-[4.25rem] shrink-0 text-[11px] font-semibold text-muted">
              {credentialFieldLabels[field]}
            </span>
            <span
              className={`min-w-0 flex-1 truncate font-mono text-xs ${revealed ? "text-foreground" : "select-none text-muted"}`}
              title={revealed ? visibleValue : "凭据已隐藏"}
            >
              {visibleValue}
            </span>
            <button
              type="button"
              className="flex size-7 shrink-0 items-center justify-center rounded-lg text-muted transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40 disabled:cursor-wait disabled:opacity-60"
              onClick={() => onToggle(site, field)}
              disabled={loading}
              aria-label={`${revealed ? "隐藏" : "显示"}${site.name}的${credentialFieldLabels[field]}`}
              aria-pressed={revealed}
              title={revealed ? "隐藏明文" : "显示明文"}
            >
              {revealLoading ? (
                <Loader2 className="size-4 animate-spin" />
              ) : revealed ? (
                <EyeOff className="size-4" />
              ) : (
                <Eye className="size-4" />
              )}
            </button>
            <button
              type="button"
              className="flex size-7 shrink-0 items-center justify-center rounded-lg text-muted transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40 disabled:cursor-wait disabled:opacity-60"
              onClick={() => onCopy(site, field)}
              disabled={loading}
              aria-label={`${copied ? "已复制" : "复制"}${site.name}的${credentialFieldLabels[field]}`}
              title={copied ? "已复制" : "复制"}
            >
              {copyLoading ? (
                <Loader2 className="size-4 animate-spin" />
              ) : copied ? (
                <Check className="size-4 text-jade" />
              ) : (
                <Copy className="size-4" />
              )}
            </button>
          </div>
        );
      })}
    </div>
  );
}

function SiteRequestHeadersEditor({
  headers,
  loading,
  onChange,
  onAdd,
  onRemove,
  onRestore,
}: {
  headers: SiteRequestHeader[];
  loading: boolean;
  onChange: (index: number, field: keyof SiteRequestHeader, value: string) => void;
  onAdd: () => void;
  onRemove: (index: number) => void;
  onRestore: () => void;
}) {
  return (
    <section className="min-w-0 space-y-3" aria-labelledby="site-request-headers-label">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-baseline gap-2">
          <Label id="site-request-headers-label">自定义请求头</Label>
          <span className="text-xs text-muted">{headers.length} 项</span>
        </div>
        <div className="flex flex-wrap justify-end gap-2">
          <Button
            type="button"
            variant="outline"
            className="h-8 px-3 text-xs"
            onClick={onRestore}
            disabled={loading}
          >
            <RotateCcw className="mr-1.5 size-3.5" />
            恢复默认
          </Button>
          <Button
            type="button"
            variant="secondary"
            className="h-8 px-3 text-xs"
            onClick={onAdd}
            disabled={loading || headers.length >= 64}
          >
            <Plus className="mr-1.5 size-3.5" />
            添加请求头
          </Button>
        </div>
      </div>

      <div className="overflow-hidden rounded-2xl border border-border bg-surface-container/45">
        <div className="hidden grid-cols-[minmax(8rem,0.7fr)_minmax(12rem,1.3fr)_2.25rem] gap-2 border-b border-border bg-card/65 px-3 py-2 text-xs font-semibold text-muted sm:grid">
          <span>名称</span>
          <span>值</span>
          <span className="sr-only">操作</span>
        </div>
        {loading ? (
          <div className="flex h-40 items-center justify-center text-sm text-muted">
            <Loader2 className="mr-2 size-4 animate-spin" />
            加载请求头…
          </div>
        ) : headers.length === 0 ? (
          <div className="flex h-28 items-center justify-center text-sm text-muted">
            未配置请求头
          </div>
        ) : (
          <div className="max-h-[min(44dvh,28rem)] space-y-2 overflow-y-auto p-2.5">
            {headers.map((header, index) => (
              <div
                key={index}
                className="grid grid-cols-[minmax(0,1fr)_2.25rem] gap-2 rounded-xl border border-border/70 bg-card/75 p-2 sm:grid-cols-[minmax(8rem,0.7fr)_minmax(12rem,1.3fr)_2.25rem]"
              >
                <Input
                  className="h-9 min-w-0 rounded-xl px-3 font-mono text-xs"
                  value={header.name}
                  onChange={(event) => onChange(index, "name", event.target.value)}
                  placeholder="请求头名称"
                  aria-label={`第 ${index + 1} 个请求头名称`}
                  spellCheck={false}
                />
                <Input
                  className="col-start-1 row-start-2 h-9 min-w-0 rounded-xl px-3 font-mono text-xs sm:col-start-2 sm:row-start-1"
                  value={header.value}
                  onChange={(event) => onChange(index, "value", event.target.value)}
                  placeholder="请求头值"
                  aria-label={`第 ${index + 1} 个请求头值`}
                  spellCheck={false}
                />
                <button
                  type="button"
                  className="col-start-2 row-span-2 row-start-1 flex size-9 cursor-pointer items-center justify-center self-center rounded-xl text-muted transition-colors duration-200 hover:bg-destructive/10 hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40 sm:col-start-3 sm:row-span-1"
                  onClick={() => onRemove(index)}
                  aria-label={`删除第 ${index + 1} 个请求头`}
                  title="删除请求头"
                >
                  <Trash2 className="size-4" />
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function SitesPage() {
  const [sites, setSites] = useState<SiteRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");
  const [refreshAllSubmitting, setRefreshAllSubmitting] = useState(false);
  const [refreshingAll, setRefreshingAll] = useState(false);
  const [siteCredentials, setSiteCredentials] = useState<Record<number, SiteCredentialsRecord>>({});
  const [revealedCredentialKeys, setRevealedCredentialKeys] = useState<Set<string>>(() => new Set());
  const [loadingCredentialKeys, setLoadingCredentialKeys] = useState<Set<string>>(() => new Set());
  const [copiedCredentialKey, setCopiedCredentialKey] = useState<string | null>(null);
  const [credentialsTarget, setCredentialsTarget] = useState<SiteRecord | null>(null);
  const [siteQuery, setSiteQuery] = useState("");
  const [siteStatusFilter, setSiteStatusFilter] = useState<"all" | SiteHealth>("all");
  const [siteTypeFilter, setSiteTypeFilter] = useState("all");
  const [sitePage, setSitePage] = useState(1);

  // Hive PTD compatible WebDAV backup
  const [ptdConfig, setPtdConfig] = useState<PtdBackupConfig | null>(null);
  const [ptdConfigLoading, setPtdConfigLoading] = useState(true);
  const [ptdDialogOpen, setPtdDialogOpen] = useState(false);
  const [ptdForm, setPtdForm] = useState<PtdBackupForm>(emptyPtdBackupForm);
  const [ptdFormError, setPtdFormError] = useState("");
  const [ptdSaving, setPtdSaving] = useState(false);
  const [ptdTesting, setPtdTesting] = useState(false);
  const [ptdTestResult, setPtdTestResult] = useState<PtdBackupTestResult | null>(null);
  const [ptdBackingUp, setPtdBackingUp] = useState(false);

  // form dialog
  const [formOpen, setFormOpen] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [form, setForm] = useState<SiteForm>(emptySiteForm);
  const [existingAuth, setExistingAuth] = useState<{
    siteType: SiteForm["site_type"];
    authType: AuthType;
    configured: boolean;
  } | null>(null);
  const [clearAuthConfig, setClearAuthConfig] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState("");
  const [requestHeadersLoading, setRequestHeadersLoading] = useState(false);
  const requestHeadersLoadRef = useRef(0);

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
  const siteCounts = useMemo(
    () => ({
      healthy: sites.filter((site) => getSiteHealth(site) === "healthy").length,
      failed: sites.filter((site) => getSiteHealth(site) === "failed").length,
      pending: sites.filter((site) => getSiteHealth(site) === "pending").length,
    }),
    [sites],
  );
  const filteredSites = useMemo(() => {
    const query = siteQuery.trim().toLocaleLowerCase();
    return sites.filter((site) => {
      if (siteStatusFilter !== "all" && getSiteHealth(site) !== siteStatusFilter) return false;
      if (siteTypeFilter !== "all" && site.site_type !== siteTypeFilter) return false;
      if (!query) return true;
      return [
        site.name,
        site.base_url,
        site.site_type,
        site.stats?.username,
        site.stats?.uid,
        ptdConfig?.site_mappings[String(site.id)],
      ].some((value) => value?.toLocaleLowerCase().includes(query));
    });
  }, [ptdConfig?.site_mappings, siteQuery, siteStatusFilter, siteTypeFilter, sites]);
  const sitePageCount = Math.max(1, Math.ceil(filteredSites.length / SITE_PAGE_SIZE));
  const pagedSites = filteredSites.slice(
    (sitePage - 1) * SITE_PAGE_SIZE,
    sitePage * SITE_PAGE_SIZE,
  );

  useEffect(() => {
    setSitePage(1);
  }, [siteQuery, siteStatusFilter, siteTypeFilter]);

  useEffect(() => {
    setSitePage((current) => Math.min(current, sitePageCount));
  }, [sitePageCount]);

  /* ---- data loading ---- */

  function loadSites() {
    setLoading(true);
    setSiteCredentials({});
    setRevealedCredentialKeys(new Set());
    setLoadingCredentialKeys(new Set());
    setCopiedCredentialKey(null);
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

  function loadPtdConfig() {
    setPtdConfigLoading(true);
    api<PtdBackupConfig>("/api/sites/ptd-backup")
      .then(setPtdConfig)
      .catch((error: Error) => setMessage(error.message || "加载蜂巢 PTD 配置失败"))
      .finally(() => setPtdConfigLoading(false));
  }

  useEffect(() => {
    loadSites();
    loadPtdConfig();
    api<SiteStatsRefreshStatusResponse>("/api/sites/refresh-all")
      .then((status) => setRefreshingAll(status.refreshing))
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    if (!refreshingAll) return;

    let cancelled = false;
    let timer: number | null = null;
    let consecutiveFailures = 0;

    const scheduleStatusCheck = () => {
      timer = window.setTimeout(checkStatus, 1200);
    };

    const checkStatus = async () => {
      try {
        const status = await api<SiteStatsRefreshStatusResponse>("/api/sites/refresh-all");
        if (cancelled) return;
        consecutiveFailures = 0;
        if (status.refreshing) {
          scheduleStatusCheck();
          return;
        }

        const refreshedSites = await api<SiteRecord[]>("/api/sites");
        if (cancelled) return;
        setSites(refreshedSites);
        setRefreshingAll(false);
        const failedCount = refreshedSites.filter((site) => site.stats?.last_error).length;
        setMessage(
          failedCount > 0
            ? `刷新完成，${failedCount} 个站点失败，可在总览中查看详情`
            : "全部站点刷新完成",
        );
      } catch (error) {
        if (cancelled) return;
        consecutiveFailures += 1;
        if (consecutiveFailures < 3) {
          scheduleStatusCheck();
          return;
        }
        setRefreshingAll(false);
        setMessage(`刷新任务已提交，但无法确认执行结果：${(error as Error).message}`);
      }
    };

    scheduleStatusCheck();
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [refreshingAll]);

  async function handleRefreshAll() {
    setRefreshAllSubmitting(true);
    try {
      const response = await api<SiteStatsRefreshStartResponse>("/api/sites/refresh-all", {
        method: "POST",
      });
      setRefreshingAll(true);
      setMessage(
        response.started
          ? "已开始在后台刷新全部站点"
          : "全部站点统计正在后台刷新",
      );
    } catch (error) {
      setMessage((error as Error).message || "启动全站刷新失败");
    } finally {
      setRefreshAllSubmitting(false);
    }
  }

  async function loadSiteCredentials(site: SiteRecord): Promise<SiteCredentialsRecord> {
    const cached = siteCredentials[site.id];
    if (cached) return cached;

    const credentials = await api<SiteCredentialsRecord>(`/api/sites/${site.id}/credentials`);
    setSiteCredentials((current) => ({ ...current, [site.id]: credentials }));
    return credentials;
  }

  function setCredentialLoading(stateKey: string, loadingState: boolean) {
    setLoadingCredentialKeys((current) => {
      const next = new Set(current);
      if (loadingState) {
        next.add(stateKey);
      } else {
        next.delete(stateKey);
      }
      return next;
    });
  }

  async function handleToggleCredential(site: SiteRecord, field: CredentialField) {
    const stateKey = credentialStateKey(site.id, field);
    if (revealedCredentialKeys.has(stateKey)) {
      setRevealedCredentialKeys((current) => {
        const next = new Set(current);
        next.delete(stateKey);
        return next;
      });
      return;
    }

    const loadingKey = credentialActionStateKey(site.id, field, "reveal");
    setCredentialLoading(loadingKey, true);
    try {
      const credentials = await loadSiteCredentials(site);
      if (!credentials[field]) {
        throw new Error(`${credentialFieldLabels[field]} 未配置`);
      }
      setRevealedCredentialKeys((current) => new Set(current).add(stateKey));
    } catch (error) {
      setMessage((error as Error).message || "读取站点凭据失败");
    } finally {
      setCredentialLoading(loadingKey, false);
    }
  }

  async function handleCopyCredential(site: SiteRecord, field: CredentialField) {
    const stateKey = credentialStateKey(site.id, field);
    const loadingKey = credentialActionStateKey(site.id, field, "copy");
    setCredentialLoading(loadingKey, true);
    try {
      const credentials = await loadSiteCredentials(site);
      const value = credentials[field];
      if (!value) {
        throw new Error(`${credentialFieldLabels[field]} 未配置`);
      }
      await writeClipboardText(value);
      setCopiedCredentialKey(stateKey);
      setMessage(`${site.name} 的 ${credentialFieldLabels[field]} 已复制`);
      window.setTimeout(() => {
        setCopiedCredentialKey((current) => (current === stateKey ? null : current));
      }, 2000);
    } catch (error) {
      setMessage((error as Error).message || "复制站点凭据失败");
    } finally {
      setCredentialLoading(loadingKey, false);
    }
  }

  function closeCredentialsDialog() {
    const siteId = credentialsTarget?.id;
    if (siteId != null) {
      setSiteCredentials((current) => {
        const next = { ...current };
        delete next[siteId];
        return next;
      });
      setRevealedCredentialKeys((current) => new Set(
        [...current].filter((key) => !key.startsWith(`${siteId}:`)),
      ));
    }
    setCredentialsTarget(null);
  }

  function handleOpenSite(site: SiteRecord) {
    try {
      const url = new URL(site.base_url.trim());
      if (url.protocol !== "http:" && url.protocol !== "https:") {
        throw new Error("unsupported protocol");
      }
      if (
        (site.site_type === "nexusphp" || site.site_type === "nexus_php") &&
        !url.pathname.endsWith("/index.php")
      ) {
        url.pathname = `${url.pathname.replace(/\/+$/, "")}/index.php`;
      }
      window.open(url.href, "_blank", "noopener,noreferrer");
    } catch {
      setMessage(`${site.name} 的站点地址无效，仅支持 HTTP 或 HTTPS`);
    }
  }

  /* ---- form helpers ---- */

  function patch(partial: Partial<SiteForm>) {
    setForm((prev) => ({ ...prev, ...partial }));
  }

  function closeForm() {
    requestHeadersLoadRef.current += 1;
    setRequestHeadersLoading(false);
    setFormError("");
    setFormOpen(false);
  }

  function updateRequestHeader(
    index: number,
    field: keyof SiteRequestHeader,
    value: string,
  ) {
    setForm((current) => ({
      ...current,
      request_headers: current.request_headers.map((header, headerIndex) =>
        headerIndex === index ? { ...header, [field]: value } : header,
      ),
    }));
  }

  function addRequestHeader() {
    setForm((current) => ({
      ...current,
      request_headers: [...current.request_headers, { name: "", value: "" }],
    }));
  }

  function removeRequestHeader(index: number) {
    setForm((current) => ({
      ...current,
      request_headers: current.request_headers.filter((_, headerIndex) => headerIndex !== index),
    }));
  }

  function restoreDefaultRequestHeaders() {
    patch({ request_headers: freshDefaultRequestHeaders() });
  }

  function openAdd() {
    requestHeadersLoadRef.current += 1;
    setEditingId(null);
    setForm({ ...emptySiteForm, request_headers: freshDefaultRequestHeaders() });
    setExistingAuth(null);
    setClearAuthConfig(false);
    setFormError("");
    setRequestHeadersLoading(false);
    setFormOpen(true);
  }

  function openEdit(site: SiteRecord) {
    const loadId = requestHeadersLoadRef.current + 1;
    requestHeadersLoadRef.current = loadId;
    setEditingId(site.id);
    const siteType = (site.site_type as SiteForm["site_type"]) || "nexusphp";
    const authType = site.auth_type ?? (siteType === "mteam" ? "api_key" : "cookie");
    setForm({
      ...emptySiteForm,
      name: site.name,
      site_type: siteType,
      base_url: site.base_url,
      use_proxy: site.use_proxy,
      auth_type: authType,
      request_headers: [],
    });
    setExistingAuth({ siteType, authType, configured: site.auth_configured });
    setClearAuthConfig(false);
    setFormError("");
    setRequestHeadersLoading(true);
    setFormOpen(true);
    api<SiteRequestHeader[]>(`/api/sites/${site.id}/request-headers`)
      .then((requestHeaders) => {
        if (requestHeadersLoadRef.current !== loadId) return;
        patch({ request_headers: requestHeaders });
      })
      .catch((error: Error) => {
        if (requestHeadersLoadRef.current !== loadId) return;
        setMessage(error.message || "加载站点请求头失败");
        closeForm();
      })
      .finally(() => {
        if (requestHeadersLoadRef.current === loadId) {
          setRequestHeadersLoading(false);
        }
      });
  }

  function handleSubmit() {
    const requestHeaders = form.request_headers.filter(
      (header) => header.name.trim() || header.value.trim(),
    );
    const missingNameIndex = requestHeaders.findIndex((header) => !header.name.trim());
    if (missingNameIndex >= 0) {
      setFormError(`第 ${missingNameIndex + 1} 个请求头名称不能为空`);
      return;
    }
    const seenHeaderNames = new Set<string>();
    for (const header of requestHeaders) {
      const normalizedName = header.name.trim().toLowerCase();
      if (seenHeaderNames.has(normalizedName)) {
        setFormError(`请求头名称不能重复：${header.name.trim()}`);
        return;
      }
      seenHeaderNames.add(normalizedName);
    }
    setFormError("");
    setSubmitting(true);
    const body = {
      name: form.name,
      site_type: form.site_type,
      base_url: form.base_url,
      use_proxy: form.use_proxy,
      auth_config: buildAuthConfig(form),
      request_headers: requestHeaders,
      clear_auth_config: editingId != null && clearAuthConfig,
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
        closeForm();
        setMessage(editingId != null ? "站点已更新" : "站点已创建");
        loadSites();
        loadPtdConfig();
      })
      .catch((error: Error) => setFormError(error.message || "保存站点失败"))
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
        loadPtdConfig();
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
      .catch((error: Error) => setMessage(error.message || "加载站点总览失败"))
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

  function openPtdConfig() {
    const config = ptdConfig;
    setPtdForm(
      config
        ? {
            enabled: config.enabled,
            webdav_url: config.webdav_url,
            username: config.username,
            password: "",
            clear_password: false,
            use_proxy: config.use_proxy,
            backup_interval_hours: config.backup_interval_hours,
            site_mappings: { ...config.site_mappings },
          }
        : { ...emptyPtdBackupForm, site_mappings: {} },
    );
    setPtdFormError("");
    setPtdTestResult(null);
    setPtdDialogOpen(true);
  }

  function ptdRequestBody() {
    return {
      ...ptdForm,
      backup_interval_hours: Number(ptdForm.backup_interval_hours),
      site_mappings: Object.fromEntries(
        Object.entries(ptdForm.site_mappings).map(([siteId, value]) => [
          siteId,
          value.trim().toLowerCase(),
        ]),
      ),
    };
  }

  async function handleSavePtdConfig() {
    setPtdSaving(true);
    setPtdFormError("");
    try {
      const saved = await api<PtdBackupConfig>("/api/sites/ptd-backup", {
        method: "PUT",
        body: JSON.stringify(ptdRequestBody()),
      });
      setPtdConfig(saved);
      setPtdDialogOpen(false);
      setMessage(saved.enabled ? "蜂巢 PTD 自动备份已启用" : "蜂巢 PTD 配置已保存");
    } catch (error) {
      setPtdFormError((error as Error).message || "保存蜂巢 PTD 配置失败");
    } finally {
      setPtdSaving(false);
    }
  }

  async function handleTestPtdConfig() {
    setPtdTesting(true);
    setPtdFormError("");
    setPtdTestResult(null);
    try {
      const result = await api<PtdBackupTestResult>("/api/sites/ptd-backup/test", {
        method: "POST",
        body: JSON.stringify(ptdRequestBody()),
      });
      setPtdTestResult(result);
    } catch (error) {
      setPtdTestResult({
        success: false,
        message: (error as Error).message || "WebDAV 连接测试失败",
      });
    } finally {
      setPtdTesting(false);
    }
  }

  async function handlePtdBackupNow() {
    setPtdBackingUp(true);
    try {
      const result = await api<PtdBackupRunResult>("/api/sites/ptd-backup/run", {
        method: "POST",
      });
      setMessage(
        `已上传 ${result.filename}，包含 ${result.site_count} 个站点（${formatBytes(result.size)}）`,
      );
      loadPtdConfig();
    } catch (error) {
      setMessage((error as Error).message || "蜂巢 PTD 备份失败");
      loadPtdConfig();
    } finally {
      setPtdBackingUp(false);
    }
  }

  /* ---- auth fields ---- */

  const canPreserveAuth = Boolean(
    existingAuth?.configured &&
      existingAuth.siteType === form.site_type &&
      existingAuth.authType === form.auth_type,
  );
  const credentialPlaceholder = canPreserveAuth
    ? "留空以保留已保存的凭据"
    : "输入新的认证凭据";

  function renderAuthFields() {
    if (form.site_type === "mteam") {
      return (
        <div className="space-y-2">
          <Label>API Key</Label>
          <Input
            value={form.api_key}
            onChange={(e) => {
              patch({ api_key: e.target.value });
              if (e.target.value) setClearAuthConfig(false);
            }}
            placeholder={credentialPlaceholder}
            disabled={clearAuthConfig}
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
            onChange={(e) => {
              patch({ auth_type: e.target.value as AuthType });
              setClearAuthConfig(false);
            }}
          >
            <option value="cookie">Cookie</option>
            <option value="passkey">Passkey</option>
            <option value="cookie_passkey">Cookie + Passkey</option>
            <option value="api_key">API Key</option>
          </select>
        </div>

        {form.auth_type === "api_key" && (
          <div className="space-y-2">
            <Label>API Key</Label>
            <Input
              value={form.api_key}
              onChange={(e) => {
                patch({ api_key: e.target.value });
                if (e.target.value) setClearAuthConfig(false);
              }}
              placeholder={credentialPlaceholder}
              disabled={clearAuthConfig}
            />
          </div>
        )}

        {(form.auth_type === "cookie" ||
          form.auth_type === "cookie_passkey") && (
          <div className="space-y-2">
            <Label>Cookie</Label>
            <Input
              value={form.cookie}
              onChange={(e) => {
                patch({ cookie: e.target.value });
                if (e.target.value) setClearAuthConfig(false);
              }}
              placeholder={credentialPlaceholder}
              disabled={clearAuthConfig}
            />
          </div>
        )}

        {(form.auth_type === "passkey" ||
          form.auth_type === "cookie_passkey") && (
          <div className="space-y-2">
            <Label>Passkey</Label>
            <Input
              value={form.passkey}
              onChange={(e) => {
                patch({ passkey: e.target.value });
                if (e.target.value) setClearAuthConfig(false);
              }}
              placeholder={credentialPlaceholder}
              disabled={clearAuthConfig}
            />
          </div>
        )}
      </>
    );
  }

  /* ---- render ---- */

  return (
    <div className="space-y-6">
      <Card className="overflow-hidden rounded-[28px]">
        <CardHeader className="border-b border-border bg-surface-container/35">
          <div className="flex flex-col gap-4 lg:flex-row lg:items-center lg:justify-between">
            <div className="min-w-0">
              <CardTitle className="flex items-center gap-2.5 text-xl">
                <span className="flex size-10 items-center justify-center rounded-2xl bg-primary text-primary-foreground">
                  <Globe className="size-5" />
                </span>
                站点管理
              </CardTitle>
              <CardDescription className="mt-2">集中管理连接、账户数据与蜂巢 PTD 同步</CardDescription>
            </div>
            <div className="flex flex-wrap gap-2 lg:justify-end">
              <Button
                variant="outline"
                onClick={() => void handleRefreshAll()}
                disabled={loading || sites.length === 0 || refreshAllSubmitting || refreshingAll}
                aria-busy={refreshAllSubmitting || refreshingAll}
                title="在后台刷新全部站点统计"
              >
                <RefreshCw className={`mr-2 size-4 ${refreshAllSubmitting || refreshingAll ? "motion-safe:animate-spin" : ""}`} />
                {refreshAllSubmitting ? "提交中" : refreshingAll ? "刷新中" : "刷新所有"}
              </Button>
              <Button variant="outline" onClick={handleOverview} disabled={loading || sites.length === 0}>
                <ListChecks className="mr-2 size-4" />
                数据总览
              </Button>
              <Button onClick={openAdd}>
                <Plus className="mr-2 size-4" />
                添加站点
              </Button>
            </div>
          </div>
        </CardHeader>

        <CardContent className="space-y-5 p-4 sm:p-6">
          {message ? (
            <div
              className="rounded-2xl border border-primary/20 bg-primary/5 px-4 py-3 text-sm"
              role="status"
              aria-live="polite"
            >
              <div className="flex items-start justify-between gap-3">
                <span>{message}</span>
                <button type="button" className="text-muted hover:text-foreground" onClick={() => setMessage("")}>
                  关闭
                </button>
              </div>
            </div>
          ) : null}

          <section className="grid grid-cols-2 gap-3 lg:grid-cols-4" aria-label="站点状态概览">
            {([
              { key: "all", label: "全部站点", value: sites.length, icon: Server, tone: "text-primary bg-primary/10" },
              { key: "healthy", label: "数据正常", value: siteCounts.healthy, icon: CircleCheck, tone: "text-emerald-700 bg-emerald-100" },
              { key: "failed", label: "拉取失败", value: siteCounts.failed, icon: CircleX, tone: "text-red-700 bg-red-100" },
              { key: "pending", label: "等待刷新", value: siteCounts.pending, icon: Clock3, tone: "text-amber-700 bg-amber-100" },
            ] as const).map((metric) => {
              const MetricIcon = metric.icon;
              const selected = siteStatusFilter === metric.key;
              return (
                <button
                  key={metric.key}
                  type="button"
                  className={`flex min-w-0 items-center gap-3 rounded-2xl border p-3 text-left transition-colors duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40 sm:p-4 ${selected ? "border-primary bg-primary/5" : "border-border bg-card hover:bg-accent/45"}`}
                  aria-pressed={selected}
                  onClick={() => setSiteStatusFilter(metric.key)}
                >
                  <span className={`flex size-9 shrink-0 items-center justify-center rounded-xl ${metric.tone}`}>
                    <MetricIcon className="size-4" />
                  </span>
                  <span className="min-w-0">
                    <span className="block truncate text-[11px] font-semibold text-muted">{metric.label}</span>
                    <span className="mt-0.5 block text-xl font-black tracking-tight">{metric.value}</span>
                  </span>
                </button>
              );
            })}
          </section>

          <section className="relative overflow-hidden rounded-[24px] border border-primary/15 bg-gradient-to-br from-primary/10 via-card to-blossom/10 p-4 sm:p-5" aria-labelledby="ptd-backup-heading">
            <div className="pointer-events-none absolute -right-12 -top-16 size-48 rounded-full bg-primary/10 blur-3xl" />
            <div className="relative flex flex-col gap-4 lg:flex-row lg:items-center lg:justify-between">
              <div className="flex min-w-0 items-start gap-3">
                <span className="flex size-11 shrink-0 items-center justify-center rounded-2xl bg-primary text-primary-foreground shadow-sm">
                  <CloudCog className="size-5" />
                </span>
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <h3 id="ptd-backup-heading" className="font-black">蜂巢 PTD</h3>
                    {ptdConfigLoading ? (
                      <span className="text-xs text-muted">读取配置中…</span>
                    ) : (
                      <span className={`rounded-full px-2.5 py-0.5 text-[10px] font-bold ${ptdConfig?.enabled ? "bg-emerald-100 text-emerald-700" : "bg-surface-container text-muted"}`}>
                        {ptdConfig?.enabled ? "自动备份已启用" : ptdConfig?.configured ? "已配置" : "未配置"}
                      </span>
                    )}
                  </div>
                  <p className="mt-1 text-xs leading-5 text-muted">
                    按 PT-Depiler 格式将用户信息打包为 ZIP，并通过 WebDAV 发往蜂巢。
                  </p>
                  <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-muted">
                    <span>上次备份：{formatDateTime(ptdConfig?.last_backup_at)}</span>
                    {ptdConfig?.last_backup_filename ? <span className="max-w-[20rem] truncate font-mono">{ptdConfig.last_backup_filename}</span> : null}
                    {ptdConfig?.last_error ? <span className="max-w-full truncate text-red-600" title={ptdConfig.last_error}>错误：{ptdConfig.last_error}</span> : null}
                  </div>
                </div>
              </div>
              <div className="flex shrink-0 flex-wrap gap-2">
                <Button variant="outline" onClick={openPtdConfig} disabled={ptdConfigLoading}>
                  <Settings2 className="mr-2 size-4" />
                  配置
                </Button>
                <Button
                  onClick={() => void handlePtdBackupNow()}
                  disabled={!ptdConfig?.configured || ptdBackingUp || sites.length === 0}
                >
                  {ptdBackingUp ? <Loader2 className="mr-2 size-4 motion-safe:animate-spin" /> : <UploadCloud className="mr-2 size-4" />}
                  {ptdBackingUp ? "上传中" : "立即备份"}
                </Button>
              </div>
            </div>
          </section>

          <section className="flex flex-col gap-3 rounded-[22px] border border-border bg-surface-container/40 p-3 md:flex-row md:items-center" aria-label="筛选站点">
            <div className="relative min-w-0 flex-1">
              <Label htmlFor="site-search" className="sr-only">搜索站点</Label>
              <Search className="pointer-events-none absolute left-3.5 top-1/2 size-4 -translate-y-1/2 text-muted" />
              <Input
                id="site-search"
                value={siteQuery}
                onChange={(event) => setSiteQuery(event.target.value)}
                className="h-11 rounded-2xl pl-10"
                placeholder="搜索站点、域名、用户名、UID 或 PTD 标识"
              />
            </div>
            <Select
              value={siteTypeFilter}
              onChange={setSiteTypeFilter}
              className="w-full md:w-40"
              options={[
                { value: "all", label: "全部类型" },
                { value: "nexusphp", label: "NexusPHP" },
                { value: "mteam", label: "M-Team" },
              ]}
            />
            <Select
              value={siteStatusFilter}
              onChange={(value) => setSiteStatusFilter(value as "all" | SiteHealth)}
              className="w-full md:w-40"
              options={[
                { value: "all", label: "全部状态" },
                { value: "healthy", label: "数据正常" },
                { value: "failed", label: "拉取失败" },
                { value: "pending", label: "等待刷新" },
              ]}
            />
            <span className="shrink-0 px-1 text-xs font-semibold text-muted">显示 {filteredSites.length} / {sites.length}</span>
          </section>

          {loading ? (
            <div className="flex items-center justify-center py-12 text-muted">
              <Loader2 className="mr-2 h-5 w-5 animate-spin" />
              加载中…
            </div>
          ) : sites.length === 0 ? (
            <div className="rounded-[24px] border border-dashed border-border py-14 text-center">
              <Globe className="mx-auto size-8 text-muted" />
              <p className="mt-3 font-semibold">还没有配置 PT 站点</p>
              <p className="mt-1 text-sm text-muted">添加首个站点后即可刷新用户数据并同步到蜂巢。</p>
              <Button className="mt-4" onClick={openAdd}><Plus className="mr-2 size-4" />添加站点</Button>
            </div>
          ) : filteredSites.length === 0 ? (
            <div className="rounded-[24px] border border-dashed border-border py-12 text-center text-sm text-muted">
              没有符合当前筛选条件的站点
            </div>
          ) : (
            <>
              <div className="hidden md:block">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>站点</TableHead>
                      <TableHead>账户</TableHead>
                      <TableHead>上传 / 下载</TableHead>
                      <TableHead>分享率 / 魔力</TableHead>
                      <TableHead>状态</TableHead>
                      <TableHead className="text-right">操作</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {pagedSites.map((site) => (
                      <TableRow key={site.id}>
                        <TableCell className="px-3 py-3">
                          <div className="min-w-[180px] max-w-[250px]">
                            <div className="flex min-w-0 items-center gap-2">
                              <span className="min-w-0 truncate font-semibold">{site.name}</span>
                              <span className="shrink-0 rounded-full bg-violet-100 px-2 py-0.5 text-[10px] text-violet-700">
                                {site.site_type}
                              </span>
                            </div>
                            <div className="mt-1 truncate text-xs text-muted" title={site.base_url}>
                              {site.base_url}
                            </div>
                            <div className="mt-1 font-mono text-[10px] text-primary">
                              PTD: {ptdConfig?.site_mappings[String(site.id)] ?? "未映射"}
                            </div>
                          </div>
                        </TableCell>
                        <TableCell className="px-3 py-3">
                          <div className="min-w-[110px]">
                            <div className="truncate text-sm font-semibold">{site.stats?.username ?? "待获取"}</div>
                            <div className="mt-1 font-mono text-[11px] text-muted">UID {site.stats?.uid ?? "-"}</div>
                          </div>
                        </TableCell>
                        <TableCell className="px-3 py-3">
                          <div className="min-w-[150px] space-y-1.5 text-xs">
                            <div className="flex items-center gap-2">
                              <UploadCloud className="size-4 shrink-0 text-primary" />
                              <span className="font-semibold tabular-nums">
                                {site.stats?.uploaded != null ? formatBytes(site.stats.uploaded) : "待刷新"}
                              </span>
                            </div>
                            <div className="flex items-center gap-2">
                              <DownloadCloud className="size-4 shrink-0 text-jade" />
                              <span className="font-semibold tabular-nums text-muted">
                                {site.stats?.downloaded != null ? formatBytes(site.stats.downloaded) : "待刷新"}
                              </span>
                            </div>
                          </div>
                        </TableCell>
                        <TableCell className="px-3 py-3">
                          <div className="min-w-[100px] text-xs">
                            <div className="font-black tabular-nums">
                              {site.stats?.uploaded != null && site.stats.downloaded != null ? formatRatio(site.stats.uploaded, site.stats.downloaded) : "-"}
                            </div>
                            <div className="mt-1 text-muted">魔力 {site.stats?.bonus != null ? site.stats.bonus.toFixed(1) : "-"}</div>
                          </div>
                        </TableCell>
                        <TableCell className="px-3 py-3">
                          <div className="max-w-[180px]">
                            <SiteHealthBadge site={site} />
                            <div className="mt-1.5 truncate text-[10px] text-muted" title={site.stats?.last_error ?? undefined}>
                              {site.stats?.last_error ?? formatDateTime(site.stats?.last_checked_at)}
                            </div>
                          </div>
                        </TableCell>
                        <TableCell className="px-3 py-3">
                          <div className="flex justify-end gap-1.5">
                            <Button variant="outline" className="size-8 p-0" onClick={() => setCredentialsTarget(site)} aria-label={`查看${site.name}凭据`} title="查看凭据">
                              <KeyRound className="size-3.5" />
                            </Button>
                            <Button
                              variant="outline"
                              className="size-8 p-0"
                              onClick={() => handleOpenSite(site)}
                              aria-label={`打开${site.name}主页`}
                              title="在新标签页打开站点主页"
                            >
                              <ExternalLink className="size-4" />
                            </Button>
                            <Button
                              variant="outline"
                              className="size-8 p-0"
                              onClick={() => handleTest(site)}
                              aria-label={`测试${site.name}连接`}
                              title="测试连接"
                            >
                              <Activity className="size-4" />
                            </Button>
                            <Button
                              variant="secondary"
                              className="size-8 p-0"
                              onClick={() => openEdit(site)}
                              aria-label={`编辑${site.name}`}
                              title="编辑站点"
                            >
                              <Pencil className="size-4" />
                            </Button>
                            <Button
                              variant="destructive"
                              className="size-8 p-0"
                              onClick={() => setDeleteTarget(site)}
                              aria-label={`删除${site.name}`}
                              title="删除站点"
                            >
                              <Trash2 className="size-4" />
                            </Button>
                          </div>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>

              <div className="grid gap-3 md:hidden">
                {pagedSites.map((site) => (
                  <div
                    key={site.id}
                    className="rounded-[20px] border border-border bg-surface-container/70 p-3.5"
                  >
                    <div className="flex items-start justify-between gap-3">
                      <div className="min-w-0">
                        <span className="block truncate text-sm font-semibold">{site.name}</span>
                        <p className="mt-1 truncate text-[11px] text-muted">{site.base_url}</p>
                      </div>
                      <SiteHealthBadge site={site} />
                    </div>
                    <div className="mt-3 grid grid-cols-3 gap-2 text-sm">
                      <div className="rounded-xl bg-card/70 p-2">
                        <div className="text-[10px] font-bold text-muted">上传</div>
                        <div className="mt-1 truncate text-xs font-black">{site.stats?.uploaded != null ? formatBytes(site.stats.uploaded) : "-"}</div>
                      </div>
                      <div className="rounded-xl bg-card/70 p-2">
                        <div className="text-[10px] font-bold text-muted">下载</div>
                        <div className="mt-1 truncate text-xs font-black">{site.stats?.downloaded != null ? formatBytes(site.stats.downloaded) : "-"}</div>
                      </div>
                      <div className="rounded-xl bg-card/70 p-2">
                        <div className="text-[10px] font-bold text-muted">分享率</div>
                        <div className="mt-1 truncate text-xs font-black">{site.stats?.uploaded != null && site.stats.downloaded != null ? formatRatio(site.stats.uploaded, site.stats.downloaded) : "-"}</div>
                      </div>
                    </div>
                    <div className="mt-2.5 flex justify-end gap-1.5 border-t border-border/70 pt-2.5">
                      <Button variant="outline" className="size-8 p-0" onClick={() => setCredentialsTarget(site)} aria-label="查看凭据"><KeyRound className="size-3.5" /></Button>
                      <Button variant="outline" className="size-8 p-0" onClick={() => handleOpenSite(site)} aria-label="打开站点"><ExternalLink className="size-3.5" /></Button>
                      <Button variant="outline" className="size-8 p-0" onClick={() => handleTest(site)} aria-label="测试连接"><Activity className="size-3.5" /></Button>
                      <Button variant="secondary" className="size-8 p-0" onClick={() => openEdit(site)} aria-label="编辑站点"><Pencil className="size-3.5" /></Button>
                      <Button variant="destructive" className="size-8 p-0" onClick={() => setDeleteTarget(site)} aria-label="删除站点"><Trash2 className="size-3.5" /></Button>
                    </div>
                  </div>
                ))}
              </div>
              {sitePageCount > 1 ? (
                <nav className="mt-4 flex items-center justify-between gap-3 rounded-2xl border border-border bg-surface-container/40 px-3 py-2" aria-label="站点分页">
                  <span className="text-xs text-muted">
                    第 {sitePage} / {sitePageCount} 页 · 每页 {SITE_PAGE_SIZE} 个
                  </span>
                  <div className="flex gap-2">
                    <Button
                      variant="outline"
                      className="h-8 px-3 text-xs"
                      onClick={() => setSitePage((current) => Math.max(1, current - 1))}
                      disabled={sitePage <= 1}
                    >
                      <ChevronLeft className="mr-1 size-3.5" />上一页
                    </Button>
                    <Button
                      variant="outline"
                      className="h-8 px-3 text-xs"
                      onClick={() => setSitePage((current) => Math.min(sitePageCount, current + 1))}
                      disabled={sitePage >= sitePageCount}
                    >
                      下一页<ChevronRight className="ml-1 size-3.5" />
                    </Button>
                  </div>
                </nav>
              ) : null}
            </>
          )}
        </CardContent>
      </Card>

      <Dialog
        open={ptdDialogOpen}
        onClose={() => setPtdDialogOpen(false)}
        title="蜂巢 PTD 备份"
        description="模拟 PT-Depiler 的用户信息备份，通过 WebDAV 上传兼容 ZIP。"
        panelClassName="max-w-6xl"
        escMode="double"
      >
        <div className="grid gap-5 p-4 sm:p-6 lg:grid-cols-[minmax(18rem,0.8fr)_minmax(24rem,1.2fr)]">
          {ptdFormError ? (
            <div role="alert" className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive lg:col-span-2">
              {ptdFormError}
            </div>
          ) : null}

          <section className="space-y-4" aria-labelledby="ptd-webdav-heading">
            <div>
              <h4 id="ptd-webdav-heading" className="flex items-center gap-2 font-black"><Server className="size-4 text-primary" />WebDAV 连接</h4>
              <p className="mt-1 text-xs leading-5 text-muted">地址应指向蜂巢提供的目标目录，备份文件会直接写入该目录。</p>
            </div>
            <div className="space-y-2">
              <Label htmlFor="ptd-webdav-url">WebDAV 地址</Label>
              <Input
                id="ptd-webdav-url"
                value={ptdForm.webdav_url}
                onChange={(event) => setPtdForm((current) => ({ ...current, webdav_url: event.target.value }))}
                placeholder="https://example.com/dav/ptd"
                spellCheck={false}
              />
            </div>
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-1 xl:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="ptd-webdav-username">用户名</Label>
                <Input
                  id="ptd-webdav-username"
                  value={ptdForm.username}
                  onChange={(event) => setPtdForm((current) => ({ ...current, username: event.target.value }))}
                  autoComplete="username"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="ptd-webdav-password">密码</Label>
                <Input
                  id="ptd-webdav-password"
                  type="password"
                  value={ptdForm.password}
                  onChange={(event) => setPtdForm((current) => ({ ...current, password: event.target.value, clear_password: false }))}
                  placeholder={ptdConfig?.password_configured ? "留空以保留已保存密码" : "可留空（匿名 WebDAV）"}
                  disabled={ptdForm.clear_password}
                  autoComplete="new-password"
                />
              </div>
            </div>
            {ptdConfig?.password_configured ? (
              <Label className="flex cursor-pointer items-center gap-2 text-xs text-muted">
                <input
                  type="checkbox"
                  className="size-4 accent-primary"
                  checked={ptdForm.clear_password}
                  onChange={(event) => setPtdForm((current) => ({ ...current, clear_password: event.target.checked, password: "" }))}
                />
                清除已保存的 WebDAV 密码
              </Label>
            ) : null}
            <div className="space-y-2">
              <Label htmlFor="ptd-backup-interval">自动备份周期（小时）</Label>
              <Input
                id="ptd-backup-interval"
                type="number"
                min={1}
                max={720}
                value={ptdForm.backup_interval_hours}
                onChange={(event) => setPtdForm((current) => ({ ...current, backup_interval_hours: Number(event.target.value) }))}
              />
              <p className="text-[11px] leading-5 text-muted">每次站点统计刷新完成后检查周期；只上传最新成功获取的用户信息。</p>
            </div>
            <div className="space-y-2 rounded-2xl border border-border bg-surface-container/55 p-3">
              <Label className="flex cursor-pointer items-center justify-between gap-3">
                <span>
                  <span className="block text-sm font-bold">自动备份</span>
                  <span className="mt-0.5 block text-[11px] font-normal text-muted">到达周期后自动生成并上传</span>
                </span>
                <input
                  type="checkbox"
                  className="size-5 accent-primary"
                  checked={ptdForm.enabled}
                  onChange={(event) => setPtdForm((current) => ({ ...current, enabled: event.target.checked }))}
                />
              </Label>
              <Label className="flex cursor-pointer items-center justify-between gap-3 border-t border-border/70 pt-2">
                <span>
                  <span className="block text-sm font-bold">使用全局代理</span>
                  <span className="mt-0.5 block text-[11px] font-normal text-muted">WebDAV 连接复用系统代理配置</span>
                </span>
                <input
                  type="checkbox"
                  className="size-5 accent-primary"
                  checked={ptdForm.use_proxy}
                  onChange={(event) => setPtdForm((current) => ({ ...current, use_proxy: event.target.checked }))}
                />
              </Label>
            </div>
          </section>

          <section className="min-w-0 space-y-3" aria-labelledby="ptd-mappings-heading">
            <div className="flex items-start justify-between gap-3">
              <div>
                <h4 id="ptd-mappings-heading" className="font-black">PTD 站点标识</h4>
                <p className="mt-1 text-xs leading-5 text-muted">必须与 PT-Depiler 的站点 ID 一致，仅允许小写字母和数字。</p>
              </div>
              <span className="shrink-0 rounded-full bg-secondary px-2.5 py-1 text-[10px] font-bold text-primary">{sites.length} 个站点</span>
            </div>
            <div className="max-h-[min(50dvh,32rem)] space-y-2 overflow-y-auto rounded-2xl border border-border bg-surface-container/40 p-2.5">
              {sites.length === 0 ? (
                <p className="py-10 text-center text-sm text-muted">请先添加站点</p>
              ) : sites.map((site) => (
                <div key={site.id} className="grid gap-2 rounded-xl border border-border/70 bg-card/80 p-3 sm:grid-cols-[minmax(0,1fr)_minmax(9rem,0.8fr)] sm:items-center">
                  <div className="min-w-0">
                    <div className="truncate text-sm font-bold">{site.name}</div>
                    <div className="mt-0.5 truncate text-[10px] text-muted">{site.base_url}</div>
                  </div>
                  <Input
                    value={ptdForm.site_mappings[String(site.id)] ?? ""}
                    onChange={(event) => setPtdForm((current) => ({
                      ...current,
                      site_mappings: { ...current.site_mappings, [String(site.id)]: event.target.value.toLowerCase().replace(/[^a-z0-9]/g, "") },
                    }))}
                    className="h-9 rounded-xl font-mono text-xs"
                    placeholder="例如 mteam"
                    aria-label={`${site.name} 的 PTD 站点标识`}
                    spellCheck={false}
                  />
                </div>
              ))}
            </div>
            {ptdTestResult ? (
              <div className={`flex items-start gap-2 rounded-2xl border px-3 py-2.5 text-sm ${ptdTestResult.success ? "border-emerald-200 bg-emerald-50 text-emerald-800" : "border-red-200 bg-red-50 text-red-700"}`} role="status">
                {ptdTestResult.success ? <CircleCheck className="mt-0.5 size-4 shrink-0" /> : <CircleX className="mt-0.5 size-4 shrink-0" />}
                <span>{ptdTestResult.message}</span>
              </div>
            ) : null}
          </section>

          <div className="flex flex-col-reverse gap-2 border-t border-border pt-4 sm:flex-row sm:justify-end lg:col-span-2">
            <Button variant="secondary" onClick={() => setPtdDialogOpen(false)}>取消</Button>
            <Button variant="outline" onClick={() => void handleTestPtdConfig()} disabled={ptdTesting || ptdSaving}>
              {ptdTesting ? <Loader2 className="mr-2 size-4 motion-safe:animate-spin" /> : <Activity className="mr-2 size-4" />}
              {ptdTesting ? "测试中" : "测试连接"}
            </Button>
            <Button onClick={() => void handleSavePtdConfig()} disabled={ptdSaving || ptdTesting}>
              {ptdSaving ? <Loader2 className="mr-2 size-4 motion-safe:animate-spin" /> : null}
              {ptdSaving ? "保存中" : "保存配置"}
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={credentialsTarget != null}
        onClose={closeCredentialsDialog}
        title="站点凭据"
        description={credentialsTarget ? `${credentialsTarget.name} · 敏感信息仅在本次查看时读取` : undefined}
      >
        {credentialsTarget ? (
          <div className="space-y-4 p-4 sm:p-6">
            <div className="rounded-2xl border border-border bg-surface-container/55 p-4">
              <SiteCredentialList
                site={credentialsTarget}
                credentials={siteCredentials[credentialsTarget.id]}
                revealedKeys={revealedCredentialKeys}
                loadingKeys={loadingCredentialKeys}
                copiedKey={copiedCredentialKey}
                onToggle={handleToggleCredential}
                onCopy={handleCopyCredential}
              />
            </div>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={closeCredentialsDialog}>关闭</Button>
            </div>
          </div>
        ) : null}
      </Dialog>

      {/* ---- add / edit dialog ---- */}
      <Dialog
        open={formOpen}
        onClose={closeForm}
        title={editingId != null ? "编辑站点" : "添加站点"}
        description={
          editingId != null
            ? "修改站点连接配置"
            : "填写站点信息以添加新的 PT 站点"
        }
        escMode="double"
        panelClassName="max-w-6xl"
      >
        <div className="grid gap-5 p-4 sm:p-6 lg:grid-cols-[minmax(17rem,0.72fr)_minmax(28rem,1.28fr)] lg:items-start">
          {formError ? (
            <div
              role="alert"
              className="rounded-xl border border-destructive/30 bg-destructive/10 px-3 py-2.5 text-sm text-destructive lg:col-span-2"
            >
              {formError}
            </div>
          ) : null}
          <div className="min-w-0 space-y-4">
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
                  setClearAuthConfig(false);
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

            {canPreserveAuth ? (
              <Label className="flex cursor-pointer items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  className="size-4 accent-primary"
                  checked={clearAuthConfig}
                  onChange={(event) => {
                    const checked = event.target.checked;
                    setClearAuthConfig(checked);
                    if (checked) {
                      patch({ cookie: "", passkey: "", api_key: "" });
                    }
                  }}
                />
                清除已保存的认证凭据
              </Label>
            ) : null}
          </div>

          <SiteRequestHeadersEditor
            headers={form.request_headers}
            loading={requestHeadersLoading}
            onChange={updateRequestHeader}
            onAdd={addRequestHeader}
            onRemove={removeRequestHeader}
            onRestore={restoreDefaultRequestHeaders}
          />

          <div className="flex justify-end gap-2 border-t border-border pt-4 lg:col-span-2">
            <Button variant="secondary" onClick={closeForm}>
              取消
            </Button>
            <Button onClick={handleSubmit} disabled={submitting || requestHeadersLoading}>
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
            正在读取站点统计数据…
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

function SiteHealthBadge({ site }: { site: SiteRecord }) {
  const health = getSiteHealth(site);
  const styles: Record<SiteHealth, string> = {
    healthy: "bg-emerald-100 text-emerald-700",
    failed: "bg-red-100 text-red-700",
    pending: "bg-amber-100 text-amber-700",
  };
  const labels: Record<SiteHealth, string> = {
    healthy: "正常",
    failed: "失败",
    pending: "待刷新",
  };
  return (
    <span className={`inline-flex shrink-0 items-center rounded-full px-2.5 py-0.5 text-[10px] font-bold ${styles[health]}`}>
      {labels[health]}
    </span>
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
