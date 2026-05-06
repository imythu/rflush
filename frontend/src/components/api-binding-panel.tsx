import { Link2 } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

export type ApiBindingItem = {
  operation: string;
  method: "GET" | "POST" | "PUT" | "DELETE";
  path: string;
};

export function ApiBindingPanel({
  pageLabel,
  items,
}: {
  pageLabel: string;
  items: ApiBindingItem[];
}) {
  return (
    <Card className="changli-card">
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center gap-2 text-base">
          <Link2 className="h-4 w-4 text-primary" />
          页面 API 绑定 · {pageLabel}
        </CardTitle>
      </CardHeader>
      <CardContent className="pt-0">
        <div className="grid gap-2">
          {items.map((item) => (
            <div
              key={`${item.method}-${item.path}-${item.operation}`}
              className="rounded-2xl border border-border/30 bg-surface-container/80 px-3 py-2 text-xs sm:text-sm"
            >
              <span className="font-semibold text-foreground">{item.operation}</span>
              <span className="mx-2 rounded-full border border-primary/20 bg-primary/10 px-2 py-0.5 text-[11px] font-semibold text-primary">
                {item.method}
              </span>
              <span className="font-mono text-muted">{item.path}</span>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  );
}

