use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use cron::Schedule;
use tokio::sync::RwLock;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, warn};

use crate::db::Database;
use crate::sign_in::{SignInTaskRecord, execute_task};

pub struct SignInScheduler {
    db: Database,
    base_dir: PathBuf,
    running_tasks: Arc<RwLock<HashMap<i64, tokio::task::JoinHandle<()>>>>,
}

impl SignInScheduler {
    pub fn new(db: Database, base_dir: PathBuf) -> Self {
        Self {
            db,
            base_dir,
            running_tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn start(self: Arc<Self>) {
        info!("sign-in scheduler started");
        loop {
            if let Err(error) = self.check_and_schedule().await {
                error!("sign-in scheduler error: {}", error);
            }
            sleep(Duration::from_secs(30)).await;
        }
    }

    async fn check_and_schedule(&self) -> Result<(), String> {
        let tasks = self
            .db
            .list_sign_in_tasks()
            .await
            .map_err(|e| e.to_string())?;
        for task in tasks {
            if !task.enabled {
                self.stop_task(task.id).await;
                continue;
            }

            let running = self.running_tasks.read().await;
            if running.contains_key(&task.id) {
                continue;
            }
            drop(running);

            if should_trigger(&task) {
                self.spawn_task(task).await;
            }
        }
        Ok(())
    }

    pub async fn trigger_task(&self, task_id: i64) -> Result<(), String> {
        let task = self
            .db
            .get_sign_in_task(task_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "签到任务不存在".to_string())?;
        let running = self.running_tasks.read().await;
        if running.contains_key(&task_id) {
            return Err("签到任务正在运行中".to_string());
        }
        drop(running);
        self.spawn_task(task).await;
        Ok(())
    }

    pub async fn stop_task(&self, task_id: i64) {
        let mut running = self.running_tasks.write().await;
        if let Some(handle) = running.remove(&task_id) {
            handle.abort();
        }
    }

    async fn spawn_task(&self, task: SignInTaskRecord) {
        let db = self.db.clone();
        let base_dir = self.base_dir.clone();
        let running_tasks = self.running_tasks.clone();
        let task_id = task.id;
        let task_name = task.name.clone();

        info!("[签到][{}] 开始执行 (id={})", task_name, task_id);
        let handle = tokio::spawn(async move {
            if let Err(error) = run_and_record(&db, base_dir, task).await {
                error!("[签到][{}] 执行失败: {}", task_name, error);
            }
            let mut running = running_tasks.write().await;
            running.remove(&task_id);
        });

        let mut running = self.running_tasks.write().await;
        running.insert(task_id, handle);
    }
}

async fn run_and_record(
    db: &Database,
    base_dir: PathBuf,
    task: SignInTaskRecord,
) -> Result<(), String> {
    let site = db
        .get_site(task.site_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "站点不存在".to_string())?;
    let site_name = site.name.clone();
    let result = execute_task(base_dir, task.clone(), site).await;
    match result {
        Ok(result) => {
            info!(
                "[签到][{}] 完成: status={} message={}",
                task.name, result.status, result.message
            );
            db.insert_sign_in_record(&task, task.site_id, &site_name, &result)
                .await
                .map_err(|e| e.to_string())?;
            db.update_sign_in_task_result(
                task.id,
                &result.status,
                &result.message,
                &result.finished_at,
            )
            .await
            .map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(message) => {
            let now = Utc::now().to_rfc3339();
            let result = crate::sign_in::SignInResult {
                status: "failed".to_string(),
                message,
                started_at: now.clone(),
                finished_at: now,
            };
            db.insert_sign_in_record(&task, task.site_id, &site_name, &result)
                .await
                .map_err(|e| e.to_string())?;
            db.update_sign_in_task_result(
                task.id,
                &result.status,
                &result.message,
                &result.finished_at,
            )
            .await
            .map_err(|e| e.to_string())?;
            Err(result.message)
        }
    }
}

fn should_trigger(task: &SignInTaskRecord) -> bool {
    let cron_expr = {
        let fields: Vec<&str> = task.cron_expression.split_whitespace().collect();
        if fields.len() == 5 {
            format!("0 {}", task.cron_expression.trim())
        } else {
            task.cron_expression.trim().to_string()
        }
    };
    let schedule: Schedule = match cron_expr.parse() {
        Ok(schedule) => schedule,
        Err(error) => {
            warn!("invalid sign-in cron '{}': {}", task.cron_expression, error);
            return false;
        }
    };

    let now = Utc::now();
    if let Some(next) = schedule.upcoming(Utc).next() {
        let diff = (next - now).num_seconds().abs();
        debug!(
            "[签到][{}] next={} diff={}s",
            task.name,
            next.to_rfc3339(),
            diff
        );
        diff <= 30
    } else {
        false
    }
}
