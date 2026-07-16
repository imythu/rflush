import { useEffect, useId, useRef } from "react";
import { X } from "lucide-react";
import { cn } from "@/lib/utils";

export function Dialog({
  open,
  onClose,
  title,
  description,
  children,
  escMode = "single",
  panelClassName,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  description?: string;
  children: React.ReactNode;
  escMode?: "single" | "double";
  panelClassName?: string;
}) {
  const lastEscAtRef = useRef(0);
  const titleId = useId();
  const descriptionId = useId();

  useEffect(() => {
    if (!open) {
      lastEscAtRef.current = 0;
      return;
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") {
        return;
      }

      if (escMode === "single") {
        event.preventDefault();
        onClose();
        return;
      }

      const now = Date.now();
      if (now - lastEscAtRef.current <= 700) {
        event.preventDefault();
        lastEscAtRef.current = 0;
        onClose();
        return;
      }

      lastEscAtRef.current = now;
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [escMode, onClose, open]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center bg-night/45 p-0 backdrop-blur-sm sm:items-center sm:p-4" onClick={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
        className={cn(
          "w-full max-w-5xl rounded-t-[30px] border border-border bg-card shadow-card backdrop-blur-xl sm:rounded-[30px]",
          "max-h-[90dvh] overflow-hidden",
          panelClassName,
        )}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="flex items-start justify-between gap-4 border-b border-border bg-surface-container/45 px-4 py-4 sm:px-6">
          <div>
            <h3 id={titleId} className="text-lg font-bold">{title}</h3>
            {description ? <p id={descriptionId} className="mt-1 text-sm text-muted">{description}</p> : null}
          </div>
          <button
            type="button"
            aria-label={`关闭${title}`}
            className="rounded-full p-2 text-muted transition hover:bg-accent hover:text-foreground"
            onClick={onClose}
          >
            <X className="h-4 w-4" />
          </button>
        </div>
        <div className="max-h-[calc(90dvh-88px)] overflow-auto">{children}</div>
      </div>
    </div>
  );
}
