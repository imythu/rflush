export type TimeUnit = "second" | "minute" | "hour";

export type GlobalConfig = {
  download_rate_limit: {
    requests: number;
    interval: number;
    unit: TimeUnit;
  };
  retry_interval_secs: number;
  throttle_interval_secs: number;
  max_concurrent_downloads: number;
  max_concurrent_rss_fetches: number;
  log_level: string | null;
};

export type RssSubscription = {
  id: number;
  name: string;
  url: string;
  enabled: boolean;
  created_at: string;
  updated_at: string;
};

export type DownloadRecord = {
  id: number;
  run_id: number;
  task_id: number | null;
  finished_at: string;
  rss_name: string;
  guid: string;
  title: string;
  retry_count: number;
  refresh_count: number;
  bytes: number | null;
  file_name: string | null;
  saved_path: string | null;
  final_status: string;
  final_message: string | null;
  file_deleted: boolean;
};

export type JobInfo = {
  id: number;
  scope: string;
  task_id: number | null;
  status: string;
  started_at: string;
  finished_at: string | null;
  run_id: number | null;
  summary: {
    total: number;
    succeeded: number;
    skipped_existing: number;
    failed: number;
  } | null;
  error: string | null;
};

export type DownloadRun = {
  id: number;
  started_at: string;
  finished_at: string;
  retry_delay_secs: number;
  total: number;
  succeeded: number;
  skipped_existing: number;
  failed: number;
};

export type PaginatedRunRecords = {
  run: DownloadRun;
  page: number;
  page_size: number;
  total_records: number;
  records: DownloadRecord[];
};

export type TaskRecordsResponse = {
  task: RssSubscription;
  page: number;
  page_size: number;
  total_records: number;
  records: DownloadRecord[];
};

// ========== PT 刷流模块类型 ==========

export type SiteRecord = {
  id: number;
  name: string;
  site_type: string;
  base_url: string;
  auth_config: string;
  created_at: string;
  updated_at: string;
};

export type SiteTestResult = {
  success: boolean;
  message: string;
  user_stats: UserStats | null;
};

export type UserStats = {
  username: string;
  uploaded: number;
  downloaded: number;
  ratio: number | null;
  bonus: number | null;
  seeding_count: number | null;
  leeching_count: number | null;
};

export type DownloaderRecord = {
  id: number;
  name: string;
  downloader_type: string;
  url: string;
  username: string;
  password: string;
  created_at: string;
  updated_at: string;
};

export type DownloaderTestResult = {
  success: boolean;
  message: string;
  version: string | null;
  free_space: number | null;
};

export type DownloaderSpaceStats = {
  free_space: number;
  pending_download_bytes: number;
  effective_free_space: number;
  torrent_count: number;
  incomplete_count: number;
};

export type BrushTaskRecord = {
  id: number;
  name: string;
  cron_expression: string;
  site_id: number | null;
  downloader_id: number;
  tag: string;
  rss_url: string;
  seed_volume_gb: number | null;
  save_dir: string | null;
  active_time_windows: string | null;
  promotion: string;
  skip_hit_and_run: boolean;
  max_concurrent: number;
  download_speed_limit: number | null;
  upload_speed_limit: number | null;
  size_ranges: string | null;
  seeder_ranges: string | null;
  delete_mode: string;
  min_seed_time_hours: number | null;
  hr_min_seed_time_hours: number | null;
  target_ratio: number | null;
  max_upload_gb: number | null;
  download_timeout_hours: number | null;
  min_avg_upload_speed_kbs: number | null;
  max_inactive_hours: number | null;
  min_disk_space_gb: number | null;
  enabled: boolean;
  created_at: string;
  updated_at: string;
};

export type BrushTaskRequest = {
  name: string;
  cron_expression: string;
  site_id?: number | null;
  downloader_id: number;
  tag: string;
  rss_url: string;
  seed_volume_gb?: number | null;
  save_dir?: string | null;
  active_time_windows?: string | null;
  promotion?: string | null;
  skip_hit_and_run?: boolean | null;
  max_concurrent?: number | null;
  download_speed_limit?: number | null;
  upload_speed_limit?: number | null;
  size_ranges?: string | null;
  seeder_ranges?: string | null;
  delete_mode?: string | null;
  min_seed_time_hours?: number | null;
  hr_min_seed_time_hours?: number | null;
  target_ratio?: number | null;
  max_upload_gb?: number | null;
  download_timeout_hours?: number | null;
  min_avg_upload_speed_kbs?: number | null;
  max_inactive_hours?: number | null;
  min_disk_space_gb?: number | null;
};

export type BrushTorrentRecord = {
  id: number;
  task_id: number;
  torrent_id: string | null;
  torrent_link: string | null;
  torrent_hash: string;
  torrent_name: string;
  added_at: string;
  size_bytes: number | null;
  is_hr: boolean;
  status: string;
  removed_at: string | null;
  remove_reason: string | null;
};

export type BrushCacheStats = {
  ttl_secs: number;
  max_concurrency: number;
  site_bucket_count: number;
  cached_entry_count: number;
  total_cache_hits: number;
  total_fetch_successes: number;
};

export type TaskStatsSnapshot = {
  id: number;
  task_id: number;
  total_uploaded: number;
  total_downloaded: number;
  torrent_count: number;
  recorded_at: string;
};

export type DownloaderSpeedSnapshot = {
  id: number;
  downloader_id: number;
  upload_speed: number;
  download_speed: number;
  recorded_at: string;
};

export type StatsOverview = {
  tasks: TaskOverview[];
};

export type TaskOverview = {
  task_id: number;
  task_name: string;
  total_uploaded: number;
  total_downloaded: number;
  torrent_count: number;
  enabled: boolean;
};
