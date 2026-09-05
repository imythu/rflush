import { useCallback, useEffect, useState } from "react";

/** Keep sub-pages in hash history without changing the application's top-level route. */
export function updateHashParams(patch: Record<string, string>) {
  const [path, query = ""] = window.location.hash.split("?");
  const params = new URLSearchParams(query);
  Object.entries(patch).forEach(([key, value]) => params.set(key, value));
  window.location.hash = `${path}?${params}`;
}

export function useHashChoice<T extends string>(key: string, choices: readonly T[], fallback: T) {
  const read = useCallback(() => {
    const value = new URLSearchParams(window.location.hash.split("?")[1]).get(key);
    return choices.includes(value as T) ? value as T : fallback;
  }, [key, choices, fallback]);
  const [value, setValue] = useState(read);
  useEffect(() => {
    const sync = () => setValue(read());
    sync();
    window.addEventListener("hashchange", sync);
    return () => window.removeEventListener("hashchange", sync);
  }, [read]);
  const select = useCallback((next: T) => {
    setValue(next);
    updateHashParams({ [key]: next });
  }, [key]);
  return [value, select] as const;
}

// Query drafts stay in this browser tab; never store credentials or submitted payloads here.
export function readQueryDraft(key: string) {
  try { return sessionStorage.getItem(`yunmu-query:${key}`) ?? ""; } catch { return ""; }
}
export function writeQueryDraft(key: string, value: string) {
  try { sessionStorage.setItem(`yunmu-query:${key}`, value); } catch { /* Storage can be disabled. */ }
}
