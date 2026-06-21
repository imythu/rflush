use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{Duration, sleep};
use tracing::{error, info};

use crate::db::Database;

/// 系统快照保留天数
const SYSTEM_SNAPSHOT_RETENTION_DAYS: i64 = 30;

/// DB 存储结构（与 system_snapshots 表 1:1）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemSnapshotRecord {
    pub id: i64,
    pub process_cpu_usage: f64,
    pub process_memory_bytes: i64,
    pub system_cpu_usage: f64,
    pub system_total_memory_bytes: i64,
    pub system_used_memory_bytes: i64,
    pub system_available_memory_bytes: i64,
    pub recorded_at: String,
}

/// 内存中的完整快照（含计算字段，供实时 API 用）
#[derive(Debug, Clone, Serialize)]
pub struct SystemSnapshot {
    pub recorded_at: String,

    // 本进程
    pub process_cpu_usage: f32,
    pub process_memory_bytes: u64,
    pub process_memory_mb: f64,

    // 机器整体
    pub system_cpu_usage: f32,
    pub system_total_memory_bytes: u64,
    pub system_used_memory_bytes: u64,
    pub system_available_memory_bytes: u64,
    pub system_memory_usage_percent: f64,
}

impl SystemSnapshot {
    fn to_record(&self) -> SystemSnapshotRecord {
        SystemSnapshotRecord {
            id: 0, // INSERT 时由 SQLite 自动生成
            process_cpu_usage: self.process_cpu_usage as f64,
            process_memory_bytes: self.process_memory_bytes as i64,
            system_cpu_usage: self.system_cpu_usage as f64,
            system_total_memory_bytes: self.system_total_memory_bytes as i64,
            system_used_memory_bytes: self.system_used_memory_bytes as i64,
            system_available_memory_bytes: self.system_available_memory_bytes as i64,
            recorded_at: self.recorded_at.clone(),
        }
    }
}

pub struct SystemMonitor {
    sys: Mutex<sysinfo::System>,
    pid: sysinfo::Pid,
    db: Database,
    latest: RwLock<Option<SystemSnapshot>>,
}

impl SystemMonitor {
    pub fn new(db: Database) -> Self {
        let pid = sysinfo::get_current_pid().expect("failed to get current PID");
        let mut sys = sysinfo::System::new();
        // 首次刷新，建立基线（CPU 需要两次采样才能算 delta）
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        sys.refresh_memory();
        sys.refresh_cpu_all();
        Self {
            sys: Mutex::new(sys),
            pid,
            db,
            latest: RwLock::new(None),
        }
    }

    /// 采集一次快照，写入 DB，更新 latest 缓存
    pub async fn snapshot(&self) -> SystemSnapshot {
        let mut sys = self.sys.lock().await;

        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[self.pid]), true);
        sys.refresh_memory();
        sys.refresh_cpu_all();

        let process = sys.process(self.pid);
        let process_cpu = process.map(|p| p.cpu_usage()).unwrap_or(0.0);
        let process_mem = process.map(|p| p.memory()).unwrap_or(0);

        let total_mem = sys.total_memory();
        let used_mem = sys.used_memory();

        let snap = SystemSnapshot {
            recorded_at: Utc::now().to_rfc3339(),
            process_cpu_usage: process_cpu,
            process_memory_bytes: process_mem,
            process_memory_mb: process_mem as f64 / 1024.0 / 1024.0,
            system_cpu_usage: sys.global_cpu_usage(),
            system_total_memory_bytes: total_mem,
            system_used_memory_bytes: used_mem,
            system_available_memory_bytes: sys.available_memory(),
            system_memory_usage_percent: if total_mem > 0 {
                used_mem as f64 / total_mem as f64 * 100.0
            } else {
                0.0
            },
        };

        // 写入 DB
        if let Err(e) = self.db.insert_system_snapshot(&snap.to_record()).await {
            error!("failed to record system snapshot: {}", e);
        }

        // 更新缓存
        *self.latest.write().await = Some(snap.clone());

        snap
    }

    /// 读取最近一次快照（不触发采集）
    pub async fn latest(&self) -> Option<SystemSnapshot> {
        self.latest.read().await.clone()
    }

    /// 后台循环，每 10 秒采集一次
    pub async fn start(self: Arc<Self>) {
        info!("system monitor started (interval: 10s)");
        loop {
            self.snapshot().await;
            let _ = self
                .db
                .cleanup_old_system_snapshots(SYSTEM_SNAPSHOT_RETENTION_DAYS)
                .await;
            sleep(Duration::from_secs(10)).await;
        }
    }
}
