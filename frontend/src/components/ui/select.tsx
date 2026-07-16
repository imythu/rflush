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
}: {
  value: string;
  onChange: (val: string) => void;
  options: readonly SelectOption[];
  className?: string;
  id?: string;
  disabled?: boolean;
  "aria-describedby"?: string;
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

  function openDropdown(index = selectedIndex) {
    if (disabled || options.length === 0) return;
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
    if (open && containerRef.current) {
      const rect = containerRef.current.getBoundingClientRect();
      setDropdownStyle({
        top: rect.bottom + window.scrollY + 6,
        left: rect.left + window.scrollX,
        width: rect.width,
      });
    }
  }, [open]);

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

  const selectedOption = options.find((o) => o.value === value) || options[0];

  return (
    <div ref={containerRef} className={cn("relative", className)}>
      <button
        ref={triggerRef}
        id={triggerId}
        type="button"
        disabled={disabled}
        aria-describedby={ariaDescribedBy}
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
            closeDropdown({ restoreFocus: true });
          }
        }}
        className="flex h-11 w-full items-center justify-between rounded-2xl border border-border bg-input px-4 py-2 text-sm shadow-sm transition-colors focus:outline-none focus:ring-2 focus:ring-ring/30 hover:bg-accent/50 disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:bg-input"
      >
        <span className="truncate">{selectedOption?.label ?? ""}</span>
        <ChevronDown className="ml-2 h-4 w-4 shrink-0 opacity-50" aria-hidden="true" />
      </button>

      {open && !disabled && typeof document !== "undefined" && createPortal(
        <div
          id={listboxId}
          ref={dropdownRef}
          role="listbox"
          aria-labelledby={triggerId}
          style={dropdownStyle}
          className="absolute z-[100] max-h-60 overflow-auto rounded-2xl border border-border bg-card p-1 shadow-card backdrop-blur-xl animate-in fade-in-0 zoom-in-95"
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
                    closeDropdown({ restoreFocus: true });
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
