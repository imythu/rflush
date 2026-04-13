import { useEffect } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import type { GlobalConfig } from "@/types";

const COMMON_LOG_LEVELS = ["trace", "debug", "info", "warn", "error"];

export function SystemSettingsPage({
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
  useEffect(() => {
    if (!COMMON_LOG_LEVELS.includes(settings.log_level ?? "")) {
      setSettings((prev) => ({ ...prev, log_level: "info" }));
    }
  }, [settings.log_level, setSettings]);

  return (
    <Card>
      <CardHeader>
        <CardTitle>系统设置</CardTitle>
        <CardDescription>全局后端程序配置。日志级别保存后会立即作用到整个后端进程。</CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        <div className="space-y-2">
          <Label>全局日志级别</Label>
          <select
            className="flex h-11 w-full rounded-2xl border border-border bg-input px-4 py-2 text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring/30"
            value={COMMON_LOG_LEVELS.includes(settings.log_level ?? "") ? settings.log_level ?? "info" : "info"}
            onChange={(event) =>
              setSettings((prev) => ({
                ...prev,
                log_level: event.target.value,
              }))
            }
          >
            {COMMON_LOG_LEVELS.map((level) => (
              <option key={level} value={level}>
                {level}
              </option>
            ))}
          </select>
        </div>

        <div className="sm:col-span-2 xl:col-span-3">
          <Button onClick={() => void onSave()} disabled={saving}>
            {saving ? "保存中..." : "保存系统设置"}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
