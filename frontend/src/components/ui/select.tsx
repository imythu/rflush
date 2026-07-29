import * as React from "react";
import { createPortal } from "react-dom";
import { ChevronDown, Check } from "lucide-react";
import { cn } from "@/lib/utils";

export interface SelectOption {
  value: string;
  label: string;
}

export function Select({
  value,
  onChange,
  options,
  className,
  id,
  disabled = false,
  "aria-describedby": ariaDescribedBy,
  "aria-invalid": ariaInvalid,
}: {
  value: string;
  onChange: (val: string) => void;
  options: readonly SelectOption[];
  className?: string;
  id?: string;
  disabled?: boolean;
  "aria-describedby"?: string;
  "aria-invalid"?: React.AriaAttributes["aria-invalid"];
}) {
  const [open, setOpen] = React.useState(false);
  const [activeIndex, setActiveIndex] = React.useState(0);
  const containerRef = React.useRef<HTMLDivElement>(null);
  const triggerRef = React.useRef<HTMLButtonElement>(null);
  const dropdownRef = React.useRef<HTMLDivElement>(null);
  const optionRefs = React.useRef<Array<HTMLButtonElement | null>>([]);
  const [dropdownStyle, setDropdownStyle] = React.useState<React.CSSProperties>({});
  const generatedId = React.useId();
  const triggerId = id ?? generatedId;
  const listboxId = `${triggerId}-listbox`;
  const matchedSelectedIndex = options.findIndex((option) => option.value === value);
  const selectedIndex = Math.max(0, matchedSelectedIndex);

  function updateDropdownPosition() {
    const container = containerRef.current;
    if (!container) return;
    const rect = container.getBoundingClientRect();
    const viewportMargin = 8;
    const dropdownGap = 6;
    const maximumHeight = 240;
    const availableBelow = window.innerHeight - rect.bottom - dropdownGap - viewportMargin;
    const availableAbove = rect.top - dropdownGap - viewportMargin;
    const openAbove = availableBelow < Math.min(160, maximumHeight) && availableAbove > availableBelow;
    const width = Math.min(rect.width, window.innerWidth - viewportMargin * 2);
    const left = Math.min(
      Math.max(rect.left, viewportMargin),
      Math.max(viewportMargin, window.innerWidth - viewportMargin - width),
    );
    setDropdownStyle({
      position: "fixed",
      top: openAbove ? undefined : rect.bottom + dropdownGap,
      bottom: openAbove ? window.innerHeight - rect.top + dropdownGap : undefined,
      left,
      width,
      maxHeight: Math.max(72, Math.min(maximumHeight, openAbove ? availableAbove : availableBelow)),
    });
  }

  function openDropdown(index = selectedIndex) {
    if (disabled || options.length === 0) return;
    updateDropdownPosition();
    setActiveIndex(Math.min(Math.max(index, 0), options.length - 1));
    setOpen(true);
  }

  function closeDropdown({ restoreFocus = false } = {}) {
    setOpen(false);
    if (restoreFocus) {
      requestAnimationFrame(() => triggerRef.current?.focus());
    }
  }

  function focusOption(index: number) {
    if (options.length === 0) return;
    const nextIndex = (index + options.length) % options.length;
    setActiveIndex(nextIndex);
    optionRefs.current[nextIndex]?.focus();
  }

  function focusAdjacentToTrigger(backward: boolean) {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const scope = trigger.closest<HTMLElement>('[role="dialog"]') ?? document.body;
    const focusable = Array.from(scope.querySelectorAll<HTMLElement>(
      'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    )).filter((element) => element.offsetParent !== null && element.tabIndex >= 0);
    const triggerIndex = focusable.indexOf(trigger);
    if (triggerIndex < 0 || focusable.length === 0) {
      trigger.focus();
      return;
    }
    const offset = backward ? -1 : 1;
    focusable[(triggerIndex + offset + focusable.length) % focusable.length]?.focus();
  }

  React.useEffect(() => {
    function handleClickOutside(e: MouseEvent) {
      const target = e.target as Node;
      if (
        containerRef.current &&
        !containerRef.current.contains(target) &&
        dropdownRef.current &&
        !dropdownRef.current.contains(target)
      ) {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  React.useEffect(() => {
    if (!open) return;
    function handleScroll(e: Event) {
      if (dropdownRef.current && dropdownRef.current.contains(e.target as Node)) {
        return;
      }
      setOpen(false);
    }
    window.addEventListener("scroll", handleScroll, true);
    window.addEventListener("resize", handleScroll);
    return () => {
      window.removeEventListener("scroll", handleScroll, true);
      window.removeEventListener("resize", handleScroll);
    };
  }, [open]);

  React.useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  React.useEffect(() => {
    if (!open) return;
    const frame = requestAnimationFrame(() => optionRefs.current[activeIndex]?.focus());
    return () => cancelAnimationFrame(frame);
  }, [activeIndex, open]);

  const selectedOption = options.find((option) => option.value === value);
  const selectedLabel = selectedOption?.label ?? (value ? "当前选项不可用" : "请选择");

  return (
    <div ref={containerRef} className={cn("relative", className)}>
      <button
        ref={triggerRef}
        id={triggerId}
        type="button"
        disabled={disabled}
        aria-describedby={ariaDescribedBy}
        aria-invalid={ariaInvalid}
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-controls={listboxId}
        aria-owns={open ? listboxId : undefined}
        onClick={() => (open ? closeDropdown() : openDropdown())}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            openDropdown(selectedIndex);
          } else if (event.key === "ArrowUp") {
            event.preventDefault();
            openDropdown(matchedSelectedIndex >= 0 ? selectedIndex : options.length - 1);
          } else if (event.key === "Escape" && open) {
            event.preventDefault();
            event.stopPropagation();
            closeDropdown({ restoreFocus: true });
          }
        }}
        className="flex h-11 w-full items-center justify-between rounded-2xl border border-border bg-input px-4 py-2 text-sm shadow-sm transition-colors hover:bg-accent/50 focus:outline-none focus:ring-2 focus:ring-ring/30 aria-[invalid=true]:border-destructive disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:bg-input"
      >
        <span className="truncate">{selectedLabel}</span>
        <ChevronDown className="ml-2 h-4 w-4 shrink-0 opacity-50" aria-hidden="true" />
      </button>

      {open && !disabled && typeof document !== "undefined" && createPortal(
        <div
          id={listboxId}
          ref={dropdownRef}
          role="listbox"
          aria-labelledby={triggerId}
          data-dialog-focus-portal="true"
          style={dropdownStyle}
          className="absolute z-[100] max-h-60 overflow-auto rounded-2xl border border-border bg-card p-1 shadow-card backdrop-blur-xl animate-in fade-in-0 zoom-in-95"
          onClick={(event) => event.stopPropagation()}
        >
          {options.map((opt, index) => {
            const isSelected = value === opt.value;
            return (
              <button
                key={opt.value}
                ref={(element) => {
                  optionRefs.current[index] = element;
                }}
                type="button"
                role="option"
                aria-selected={isSelected}
                tabIndex={activeIndex === index ? 0 : -1}
                className={cn(
                  "flex w-full items-center justify-between rounded-xl px-3 py-2.5 text-sm transition-colors",
                  isSelected
                    ? "bg-primary font-semibold text-primary-foreground shadow-glow"
                    : "text-foreground hover:bg-accent",
                )}
                onClick={() => {
                  onChange(opt.value);
                  closeDropdown({ restoreFocus: true });
                }}
                onKeyDown={(event) => {
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    focusOption(index + 1);
                  } else if (event.key === "ArrowUp") {
                    event.preventDefault();
                    focusOption(index - 1);
                  } else if (event.key === "Home") {
                    event.preventDefault();
                    focusOption(0);
                  } else if (event.key === "End") {
                    event.preventDefault();
                    focusOption(options.length - 1);
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    event.stopPropagation();
                    closeDropdown({ restoreFocus: true });
                  } else if (event.key === "Tab") {
                    event.preventDefault();
                    event.stopPropagation();
                    closeDropdown();
                    requestAnimationFrame(() => focusAdjacentToTrigger(event.shiftKey));
                  }
                }}
              >
                <span className="truncate">{opt.label}</span>
                {isSelected && <Check className="ml-2 h-4 w-4 shrink-0" />}
              </button>
            );
          })}
        </div>,
        document.body
      )}
    </div>
  );
}
