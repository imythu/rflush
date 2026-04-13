import type { GlobalConfig } from "@/types";

const API_BASE = import.meta.env.DEV ? "http://127.0.0.1:3000" : "";

export const defaultSettings: GlobalConfig = {
  download_rate_limit: {
    requests: 2,
    interval: 1,
    unit: "second",
  },
  retry_interval_secs: 5,
  throttle_interval_secs: 30,
  max_concurrent_downloads: 32,
  max_concurrent_rss_fetches: 8,
  log_level: "info",
};

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    headers: {
      "Content-Type": "application/json",
      ...(init?.headers ?? {}),
    },
    ...init,
  });

  if (!response.ok) {
    const body = await response.json().catch(() => ({ error: response.statusText }));
    throw new Error(body.error ?? response.statusText);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return response.json() as Promise<T>;
}
