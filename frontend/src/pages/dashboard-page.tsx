import { ArrowRight, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
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
      <div className="grid gap-4 sm:grid-cols-2 2xl:grid-cols-4">
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
        <Card>
          <CardHeader>
            <CardTitle>快捷操作</CardTitle>
            <CardDescription>为大屏和小屏都保留低认知成本的操作入口。</CardDescription>
          </CardHeader>
          <CardContent className="grid gap-3 sm:grid-cols-2">
            <ActionCard
              title="一键全量下载"
              description="按当前全局配置拉取全部订阅，并共享域名级并发限制。"
              actionLabel="立即启动"
              onClick={() => void onRunAll()}
            />
            <ActionCard
              title="管理任务"
              description="新增 RSS 任务、批量暂停/启动，并按任务查看历史。"
              actionLabel="前往任务页"
              onClick={onGoRss}
            />
            <ActionCard
              title="查看历史"
              description="手机上看卡片，平板/桌面上看更完整的列表与表格。"
              actionLabel="前往历史页"
              onClick={onGoHistory}
            />
            <ActionCard
              title="快速启动首个订阅"
              description={rss[0] ? `当前首个订阅：${rss[0].name}` : "暂无订阅，先去 RSS 页添加。"}
              actionLabel={rss[0] ? "启动首个订阅" : "去添加订阅"}
              onClick={() => (rss[0] ? void onRunOne(rss[0].id) : onGoRss())}
            />
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>最近历史</CardTitle>
          <CardDescription>保留更适合手机/平板的卡片式摘要。</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-3">
          {latestRecords.length === 0 ? (
            <div className="rounded-2xl border border-dashed border-border bg-surface-container/60 p-5 text-sm text-muted">
              还没有历史记录。
            </div>
          ) : (
            latestRecords.map((record) => (
              <div key={record.id} className="rounded-2xl border border-border bg-surface-container/70 p-4">
                <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                  <div className="min-w-0">
                    <div className="text-sm font-semibold">{record.title}</div>
                    <div className="mt-1 text-xs text-muted">
                      {record.rss_name} · {formatDate(record.finished_at)}
                    </div>
                  </div>
                  <span className={`w-fit rounded-full px-3 py-1 text-xs font-medium ${statusBadge(record.final_status)}`}>
                    {record.final_status}
                  </span>
                </div>
              </div>
            ))
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
