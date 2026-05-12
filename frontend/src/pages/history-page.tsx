import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { formatDate, statusBadge } from "@/lib/format";
import type { DownloadRecord } from "@/types";

export function HistoryPage({ history }: { history: DownloadRecord[] }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>历史下载记录</CardTitle>
        <CardDescription>移动端展示为卡片列表，桌面端展示为表格。</CardDescription>
      </CardHeader>
      <CardContent>
        <div className="grid gap-3 grid-cols-1 sm:grid-cols-2 lg:grid-cols-3">
          {history.map((record) => (
            <div key={record.id} className="rounded-[20px] border border-border bg-surface-container/30 p-3.5 shadow-sm transition-all hover:bg-surface-container/50">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="text-[13px] font-semibold truncate text-foreground">{record.rss_name}</div>
                  <div className="text-[10px] text-muted-foreground mt-0.5">{formatDate(record.finished_at)}</div>
                </div>
                <span className={`shrink-0 rounded-full px-2 py-0.5 text-[10px] font-medium ${statusBadge(record.final_status)}`}>
                  {record.final_status}
                </span>
              </div>
              <div className="mt-2.5 text-[11px] leading-relaxed text-foreground line-clamp-2">{record.title}</div>
              <div className="mt-3.5 grid grid-cols-2 gap-2 border-t border-border/30 pt-3">
                <div className="space-y-0.5">
                  <div className="text-[9px] uppercase tracking-wider text-muted-foreground">重试 / 刷新</div>
                  <div className="text-[11px] font-medium text-foreground">{record.retry_count} / {record.refresh_count}</div>
                </div>
                <div className="space-y-0.5">
                  <div className="text-[9px] uppercase tracking-wider text-muted-foreground">种子状态</div>
                  <div className="text-[11px] font-medium text-foreground">{record.file_deleted ? "已删除" : "未删除"}</div>
                </div>
                <div className="col-span-2 space-y-0.5">
                  <div className="text-[9px] uppercase tracking-wider text-muted-foreground">保存路径</div>
                  <div className="text-[11px] font-medium text-foreground truncate">{record.saved_path ?? "-"}</div>
                </div>
              </div>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  );
}
