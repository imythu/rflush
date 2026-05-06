import { Radio } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

export function SystemLogsPage({
  connected,
  logs,
  maxLines,
  logLevelFilter,
  setLogLevelFilter,
  selectableLevels,
  keyword,
  setKeyword,
  onClear,
}: {
  connected: boolean;
  logs: string[];
  maxLines: number;
  logLevelFilter: string;
  setLogLevelFilter: (value: "trace" | "debug" | "info" | "warn" | "error") => void;
  selectableLevels: Array<"trace" | "debug" | "info" | "warn" | "error">;
  keyword: string;
  setKeyword: (value: string) => void;
  onClear: () => void;
}) {
  return (
    <div className="grid gap-4">
      <div className="changli-card rounded-[30px] p-5 sm:p-6">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <span
            className={`inline-flex items-center gap-2 rounded-full px-3 py-1 text-xs font-semibold ${
              connected ? "status-chip status-chip-success" : "status-chip status-chip-warning"
            }`}
          >
            <Radio className="h-3.5 w-3.5" />
            {connected ? "SSE 已连接" : "SSE 重连中"}
          </span>
          <span className="text-xs text-muted">最多保留 {maxLines} 行</span>
        </div>
        <div className="mt-4 grid gap-3 md:grid-cols-[220px_minmax(0,1fr)_auto]">
          <select
            className="ui-select"
            value={logLevelFilter}
            onChange={(event) => setLogLevelFilter(event.target.value as "trace" | "debug" | "info" | "warn" | "error")}
          >
            {selectableLevels.map((level) => (
              <option key={level} value={level}>
                {level.toUpperCase()}
              </option>
            ))}
          </select>
          <Input value={keyword} onChange={(event) => setKeyword(event.target.value)} placeholder="关键词过滤日志" />
          <Button variant="outline" onClick={onClear}>
            清空视图
          </Button>
        </div>
      </div>

      <div className="changli-card overflow-hidden rounded-[30px]">
        <div className="max-h-[68vh] overflow-auto border border-border/20 bg-[rgba(8,13,22,0.94)] p-4 font-mono text-xs leading-6 text-[rgb(var(--text-primary))]">
          {logs.length === 0 ? (
            <div className="text-muted">暂无匹配日志。</div>
          ) : (
            logs.map((line, index) => (
              <div key={`${index}-${line.slice(0, 24)}`} className="whitespace-pre-wrap break-all">
                {line}
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

