import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { GlobalConfig, TimeUnit } from "@/types";

export function SettingsPage({
  settings,
  setSettings,
  saving,
  onSave,
}: {
  settings: GlobalConfig;
  setSettings: React.Dispatch<React.SetStateAction<GlobalConfig>>;
  saving: boolean;
  onSave: () => Promise<void>;
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>任务设置</CardTitle>
        <CardDescription>下载策略、限流规则和日志输出都在这里统一配置。</CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        <FormNumber
          label="限流请求数"
          value={settings.download_rate_limit.requests}
          onChange={(value) =>
            setSettings((prev) => ({
              ...prev,
              download_rate_limit: { ...prev.download_rate_limit, requests: value },
            }))
          }
        />
        <FormNumber
          label="限流窗口"
          value={settings.download_rate_limit.interval}
          onChange={(value) =>
            setSettings((prev) => ({
              ...prev,
              download_rate_limit: { ...prev.download_rate_limit, interval: value },
            }))
          }
        />

        <div className="space-y-2">
          <Label>限流单位</Label>
          <select
            className="flex h-11 w-full rounded-2xl border border-border bg-input px-4 py-2 text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring/30"
            value={settings.download_rate_limit.unit}
            onChange={(event) =>
              setSettings((prev) => ({
                ...prev,
                download_rate_limit: {
                  ...prev.download_rate_limit,
                  unit: event.target.value as TimeUnit,
                },
              }))
            }
          >
            <option value="second">second</option>
            <option value="minute">minute</option>
            <option value="hour">hour</option>
          </select>
        </div>

        <FormNumber
          label="重试间隔（秒）"
          value={settings.retry_interval_secs}
          onChange={(value) => setSettings((prev) => ({ ...prev, retry_interval_secs: value }))}
        />
        <FormNumber
          label="限流暂停（秒）"
          value={settings.throttle_interval_secs}
          onChange={(value) => setSettings((prev) => ({ ...prev, throttle_interval_secs: value }))}
        />
        <FormNumber
          label="最大并发下载"
          value={settings.max_concurrent_downloads}
          onChange={(value) =>
            setSettings((prev) => ({ ...prev, max_concurrent_downloads: value }))
          }
        />
        <FormNumber
          label="最大并发 RSS 抓取"
          value={settings.max_concurrent_rss_fetches}
          onChange={(value) =>
            setSettings((prev) => ({ ...prev, max_concurrent_rss_fetches: value }))
          }
        />

        <div className="space-y-2">
          <Label>日志级别</Label>
          <Input
            value={settings.log_level ?? ""}
            onChange={(event) =>
              setSettings((prev) => ({ ...prev, log_level: event.target.value || null }))
            }
            placeholder="info"
          />
        </div>

        <div className="sm:col-span-2 xl:col-span-3">
          <Button onClick={() => void onSave()} disabled={saving}>
            {saving ? "保存中..." : "保存任务设置"}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function FormNumber({
  label,
  value,
  onChange,
}: {
  label: string;
  value: number;
  onChange: (value: number) => void;
}) {
  return (
    <div className="space-y-2">
      <Label>{label}</Label>
      <Input type="number" min={1} value={value} onChange={(event) => onChange(Number(event.target.value))} />
    </div>
  );
}
