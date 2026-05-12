import { ArrowRight, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { formatDate, statusBadge } from "@/lib/format";
import type { DownloadRecord, RssSubscription } from "@/types";

export function DashboardPage({
  rss,
  history,
  onRunAll,
  onGoRss,
  onGoHistory,
  onRunOne,
}: {
  rss: RssSubscription[];
  history: DownloadRecord[];
  onRunAll: () => Promise<void>;
  onGoRss: () => void;
  onGoHistory: () => void;
  onRunOne: (id: number) => Promise<void>;
}) {
  const latestRecords = history.slice(0, 5);

  return (
    <div className="grid gap-4 xl:gap-6">
      <div className="grid gap-3 grid-cols-1 sm:grid-cols-2 2xl:grid-cols-4">
        <MetricCard label="RSS 订阅数" value={rss.length} detail="当前已配置订阅" />
        <MetricCard label="历史记录数" value={history.length} detail="来自 SQLite 持久化" />
        <MetricCard label="已启用订阅" value={rss.filter((item) => item.enabled).length} detail="当前启用中的 RSS 任务" />
        <MetricCard
          label="最近成功数"
          value={history.filter((item) => item.final_status === "success").slice(0, 20).length}
          detail="最近记录窗口"
        />
      </div>

      <div className="grid gap-4 xl:gap-6">
        <Card className="rounded-[20px] border-border bg-surface-container/30 shadow-sm overflow-hidden">
          <CardHeader className="pb-2">
            <CardTitle className="text-lg">快捷操作</CardTitle>
            <CardDescription className="text-[11px]">快速触达核心功能。</CardDescription>
          </CardHeader>
          <CardContent className="grid gap-3 sm:grid-cols-2">
            <ActionCard
              title="一键全量下载"
              description="按当前全局配置拉取全部订阅。"
              actionLabel="立即启动"
              onClick={() => void onRunAll()}
            />
            <ActionCard
              title="管理任务"
              description="新增 RSS 任务、批量暂停/启动。"
              actionLabel="前往任务页"
              onClick={onGoRss}
            />
          </CardContent>
        </Card>
      </div>

      <Card className="rounded-[20px] border-border bg-surface-container/30 shadow-sm overflow-hidden">
        <CardHeader className="pb-2">
          <CardTitle className="text-lg">最近历史</CardTitle>
          <CardDescription className="text-[11px]">最新抓取的 RSS 种子记录。</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-3">
          {latestRecords.length === 0 ? (
            <div className="rounded-2xl border border-dashed border-border bg-surface-container/60 p-5 text-[11px] text-muted">
              还没有历史记录。
            </div>
          ) : (
            <div className="grid gap-3 grid-cols-1 sm:grid-cols-2 lg:grid-cols-3">
              {latestRecords.map((record) => (
                <div key={record.id} className="rounded-[20px] border border-border bg-surface-container/30 p-3.5 shadow-sm">
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
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function MetricCard({ label, value, detail }: { label: string; value: number; detail: string }) {
  return (
    <Card>
      <CardContent className="p-5">
        <div className="text-sm text-muted">{label}</div>
        <div className="mt-3 text-3xl font-semibold tracking-tight">{value}</div>
        <div className="mt-2 text-xs leading-5 text-muted">{detail}</div>
      </CardContent>
    </Card>
  );
}

function ActionCard({
  title,
  description,
  actionLabel,
  onClick,
}: {
  title: string;
  description: string;
  actionLabel: string;
  onClick: () => void;
}) {
  return (
    <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
      <div className="text-base font-semibold">{title}</div>
      <div className="mt-2 text-sm leading-6 text-muted">{description}</div>
      <Button className="mt-4 w-full justify-center sm:w-auto" variant="secondary" onClick={onClick}>
        {actionLabel.includes("启动") ? <Play className="mr-2 h-4 w-4" /> : <ArrowRight className="mr-2 h-4 w-4" />}
        {actionLabel}
      </Button>
    </div>
  );
}
