import { Play, Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { formatDate } from "@/lib/format";
import type { RssSubscription } from "@/types";

export function RssPage({
  rss,
  newRss,
  setNewRss,
  onAddRss,
  onRunOne,
  onRemoveRss,
}: {
  rss: RssSubscription[];
  newRss: { name: string; url: string };
  setNewRss: React.Dispatch<React.SetStateAction<{ name: string; url: string }>>;
  onAddRss: () => Promise<void>;
  onRunOne: (id: number) => Promise<void>;
  onRemoveRss: (id: number) => Promise<void>;
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>RSS 订阅管理</CardTitle>
        <CardDescription>在这里新增、删除订阅，并按订阅粒度手动启动下载。</CardDescription>
      </CardHeader>
      <CardContent className="space-y-5">
        <div className="grid gap-3 lg:grid-cols-[220px_minmax(0,1fr)_auto]">
          <Input
            placeholder="订阅名称"
            value={newRss.name}
            onChange={(event) => setNewRss((prev) => ({ ...prev, name: event.target.value }))}
          />
          <Input
            placeholder="RSS 地址"
            value={newRss.url}
            onChange={(event) => setNewRss((prev) => ({ ...prev, url: event.target.value }))}
          />
          <Button className="w-full lg:w-auto" onClick={() => void onAddRss()}>
            <Plus className="mr-2 h-4 w-4" />
            添加订阅
          </Button>
        </div>

        <div className="hidden lg:block">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>ID</TableHead>
                <TableHead>名称</TableHead>
                <TableHead>地址</TableHead>
                <TableHead>更新时间</TableHead>
                <TableHead>操作</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rss.map((item) => (
                <TableRow key={item.id}>
                  <TableCell>{item.id}</TableCell>
                  <TableCell className="font-medium">{item.name}</TableCell>
                  <TableCell className="max-w-[520px] truncate">{item.url}</TableCell>
                  <TableCell>{formatDate(item.updated_at)}</TableCell>
                  <TableCell>
                    <div className="flex flex-wrap gap-2">
                      <Button variant="secondary" onClick={() => void onRunOne(item.id)}>
                        <Play className="mr-2 h-4 w-4" />
                        启动
                      </Button>
                      <Button variant="destructive" onClick={() => void onRemoveRss(item.id)}>
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

        <div className="grid gap-3 lg:hidden">
          {rss.map((item) => (
            <div key={item.id} className="rounded-2xl border border-border bg-surface-container/70 p-4">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="text-sm text-muted">ID {item.id}</div>
                  <div className="mt-1 text-base font-semibold">{item.name}</div>
                </div>
              </div>
              <div className="mt-3 break-all text-sm text-muted">{item.url}</div>
              <div className="mt-3 text-xs text-muted">更新时间：{formatDate(item.updated_at)}</div>
              <div className="mt-4 flex flex-col gap-2 sm:flex-row">
                <Button variant="secondary" className="w-full sm:w-auto" onClick={() => void onRunOne(item.id)}>
                  <Play className="mr-2 h-4 w-4" />
                  启动
                </Button>
                <Button variant="destructive" className="w-full sm:w-auto" onClick={() => void onRemoveRss(item.id)}>
                  <Trash2 className="mr-2 h-4 w-4" />
                  删除
                </Button>
              </div>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  );
}
