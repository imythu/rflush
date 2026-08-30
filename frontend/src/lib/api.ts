import type { GlobalConfig } from "@/types";

export const API_BASE = "";

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
  proxy: null,
  use_proxy_for_lightpanda: true,
  lightpanda: {
    endpoint: null,
    token: null,
    region: "euwest",
    browser: "lightpanda",
    proxy: "fast_dc",
    country: null,
  },
  cloakbrowser: {
    license_key: null,
    headless: false,
    humanize: true,
    human_preset: "careful",
    proxy: null,
    geoip: true,
  },
  tag_rule_scan_interval_mins: 7,
  ocr_api_key: null,
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
    throw new ApiError(body.error ?? response.statusText, response.status);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return response.json() as Promise<T>;
}

export const APP_VERSION = import.meta.env.VITE_APP_VERSION as string | undefined;

export class ApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message);
    this.name = "ApiError";
  }
}
