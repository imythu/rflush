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
        <div className="hidden xl:block">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>完成时间</TableHead>
                <TableHead>RSS</TableHead>
                <TableHead>标题</TableHead>
                <TableHead>状态</TableHead>
                <TableHead>重试</TableHead>
                <TableHead>刷新</TableHead>
                <TableHead>种子删除</TableHead>
                <TableHead>保存路径</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {history.map((record) => (
                <TableRow key={record.id}>
                  <TableCell>{formatDate(record.finished_at)}</TableCell>
                  <TableCell>{record.rss_name}</TableCell>
                  <TableCell className="max-w-[360px] truncate">{record.title}</TableCell>
                  <TableCell>
                    <span className={`rounded-full px-3 py-1 text-xs font-medium ${statusBadge(record.final_status)}`}>
                      {record.final_status}
                    </span>
                  </TableCell>
                  <TableCell>{record.retry_count}</TableCell>
                  <TableCell>{record.refresh_count}</TableCell>
                  <TableCell>{record.file_deleted ? "已删除" : "未删除"}</TableCell>
                  <TableCell className="max-w-[320px] truncate">{record.saved_path ?? "-"}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>

        <div className="grid gap-3 xl:hidden">
          {history.map((record) => (
            <div key={record.id} className="rounded-2xl border border-border bg-surface-container/70 p-4">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="font-semibold">{record.rss_name}</div>
                <span className={`rounded-full px-3 py-1 text-xs font-medium ${statusBadge(record.final_status)}`}>
                  {record.final_status}
                </span>
              </div>
              <div className="mt-2 text-sm leading-6 text-foreground">{record.title}</div>
              <div className="mt-3 grid gap-2 text-xs text-muted sm:grid-cols-2">
                <div>完成时间：{formatDate(record.finished_at)}</div>
                <div>重试次数：{record.retry_count}</div>
                <div>刷新次数：{record.refresh_count}</div>
                <div>种子删除：{record.file_deleted ? "已删除" : "未删除"}</div>
                <div className="break-all">保存路径：{record.saved_path ?? "-"}</div>
              </div>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  );
}
