import * as React from "react";
import { cn } from "@/lib/utils";

export function Table({ className, ...props }: React.TableHTMLAttributes<HTMLTableElement>) {
  return (
    <div className="w-full overflow-auto rounded-[24px] border border-border bg-surface/45 p-3 shadow-[inset_0_1px_0_rgba(255,255,255,0.9)] backdrop-blur">
      <table className={cn("w-full min-w-max border-separate border-spacing-y-2 caption-bottom text-sm", className)} {...props} />
    </div>
  );
}

export function TableHeader({ className, ...props }: React.HTMLAttributes<HTMLTableSectionElement>) {
  return <thead className={cn("[&_tr]:shadow-none", className)} {...props} />;
}

export function TableBody({ className, ...props }: React.HTMLAttributes<HTMLTableSectionElement>) {
  return <tbody className={cn("[&_tr:last-child_td]:border-b", className)} {...props} />;
}

export function TableRow({ className, ...props }: React.HTMLAttributes<HTMLTableRowElement>) {
  return (
    <tr
      className={cn("group transition-colors", className)}
      {...props}
    />
  );
}

export function TableHead({ className, ...props }: React.ThHTMLAttributes<HTMLTableCellElement>) {
  return (
    <th
      className={cn(
        "h-11 border-y border-border bg-surface-container/80 px-4 text-left align-middle text-xs font-black tracking-wide text-muted first:rounded-l-[18px] first:border-l last:rounded-r-[18px] last:border-r",
        className,
      )}
      {...props}
    />
  );
}

export function TableCell({ className, ...props }: React.TdHTMLAttributes<HTMLTableCellElement>) {
  return (
    <td
      className={cn(
        "border-y border-border/70 bg-card/78 p-4 align-middle shadow-[0_1px_0_rgba(255,255,255,0.82)_inset] transition-colors first:rounded-l-[20px] first:border-l last:rounded-r-[20px] last:border-r group-hover:bg-accent/78",
        className,
      )}
      {...props}
    />
  );
}
