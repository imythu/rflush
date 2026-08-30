use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use cron::Schedule;
use tokio::sync::RwLock;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, warn};

use crate::db::Database;
use crate::sign_in::{SignInTaskRecord, execute_task};

const AUTOMATIC_DELAY_MAX_SECS: u64 = 360;

#[derive(Clone, Copy)]
enum TriggerSource {
    Automatic,
    Manual,
}

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

            if should_trigger(&task) {
                self.spawn_task(task, TriggerSource::Automatic).await;
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
        if !self.spawn_task(task, TriggerSource::Manual).await {
            return Err("签到任务正在运行中".to_string());
        }
        Ok(())
    }

    pub async fn stop_task(&self, task_id: i64) {
        let mut running = self.running_tasks.write().await;
        if let Some(handle) = running.remove(&task_id) {
            handle.abort();
        }
    }

    async fn spawn_task(&self, task: SignInTaskRecord, trigger: TriggerSource) -> bool {
        let mut running = self.running_tasks.write().await;
        if running.contains_key(&task.id) {
            return false;
        }

        let db = self.db.clone();
        let base_dir = self.base_dir.clone();
        let running_tasks = self.running_tasks.clone();
        let task_id = task.id;
        let task_name = task.name.clone();
        let delay_secs = trigger_delay_secs(trigger, task_id);

        match trigger {
            TriggerSource::Automatic => info!(
                "[签到][{}] cron 触发，随机延迟 {} 秒后执行 (id={})",
                task_name, delay_secs, task_id
            ),
            TriggerSource::Manual => {
                info!("[签到][{}] 手动触发 (id={})", task_name, task_id)
            }
        }
        let handle = tokio::spawn(async move {
            if delay_secs > 0 {
                sleep(Duration::from_secs(delay_secs)).await;
            }
            info!("[签到][{}] 开始执行 (id={})", task_name, task_id);
            if let Err(error) = run_and_record(&db, base_dir, task).await {
                error!("[签到][{}] 执行失败: {}", task_name, error);
            }
            let mut running = running_tasks.write().await;
            running.remove(&task_id);
        });

        running.insert(task_id, handle);
        true
    }
}

fn trigger_delay_secs(trigger: TriggerSource, task_id: i64) -> u64 {
    match trigger {
        TriggerSource::Automatic => random_automatic_delay_secs(task_id),
        TriggerSource::Manual => 0,
    }
}

fn random_automatic_delay_secs(task_id: i64) -> u64 {
    let mut random = [0_u8; 8];
    if let Err(error) = getrandom::fill(&mut random) {
        let fallback = task_id.unsigned_abs() % (AUTOMATIC_DELAY_MAX_SECS + 1);
        warn!(
            "[签到][{}] 生成随机延迟失败，使用 {} 秒确定性延迟: {}",
            task_id, fallback, error
        );
        return fallback;
    }
    automatic_delay_secs_from_random(u64::from_le_bytes(random))
}

fn automatic_delay_secs_from_random(random: u64) -> u64 {
    random % (AUTOMATIC_DELAY_MAX_SECS + 1)
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
    let settings = db.get_settings().await.map_err(|e| e.to_string())?;
    let site_name = site.name.clone();
    let result = execute_task(base_dir, task.clone(), site, settings).await;
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
    should_trigger_at(task, Utc::now())
}

fn should_trigger_at(task: &SignInTaskRecord, now: DateTime<Utc>) -> bool {
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

    let last_run_at = task
        .last_run_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc));

    let window_start = now - chrono::Duration::seconds(45);
    let Some(due_at) = schedule.after(&window_start).next() else {
        return false;
    };
    if due_at > now {
        debug!(
            "[签到][{}] 未到执行时间: due_at={} now={}",
            task.name,
            due_at.to_rfc3339(),
            now.to_rfc3339()
        );
        return false;
    }
    if let Some(last_run_at) = last_run_at {
        if last_run_at < due_at {
            debug!(
                "[签到][{}] cron 到点: due_at={} now={}",
                task.name,
                due_at.to_rfc3339(),
                now.to_rfc3339()
            );
            return true;
        }
        debug!(
            "[签到][{}] 跳过已执行的计划点: due_at={} last_run_at={}",
            task.name,
            due_at.to_rfc3339(),
            last_run_at.to_rfc3339()
        );
        return false;
    }

    debug!(
        "[签到][{}] cron 到点: due_at={} now={}",
        task.name,
        due_at.to_rfc3339(),
        now.to_rfc3339()
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn task(last_run_at: Option<String>) -> SignInTaskRecord {
        SignInTaskRecord {
            id: 1,
            name: "test".to_string(),
            site_id: 1,
            cron_expression: "0 0 0/8 * * *".to_string(),
            browser: "lightpanda".to_string(),
            sign_in_method: crate::sign_in::SIGN_IN_METHOD_OPEN_PAGE.to_string(),
            enabled: true,
            last_status: None,
            last_message: None,
            last_run_at,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn sign_in_cron_does_not_trigger_before_due_time() {
        let now = Utc.with_ymd_and_hms(2026, 6, 27, 7, 59, 30).unwrap();

        assert!(!should_trigger_at(&task(None), now));
    }

    #[test]
    fn sign_in_cron_triggers_once_for_due_window() {
        let now = Utc.with_ymd_and_hms(2026, 6, 27, 8, 0, 5).unwrap();
        let mut task = task(None);

        assert!(should_trigger_at(&task, now));

        task.last_run_at = Some(
            Utc.with_ymd_and_hms(2026, 6, 27, 8, 0, 20)
                .unwrap()
                .to_rfc3339(),
        );

        assert!(!should_trigger_at(
            &task,
            now + chrono::Duration::seconds(25)
        ));
    }

    #[test]
    fn automatic_delay_includes_zero_and_maximum() {
        assert_eq!(automatic_delay_secs_from_random(0), 0);
        assert_eq!(
            automatic_delay_secs_from_random(AUTOMATIC_DELAY_MAX_SECS),
            AUTOMATIC_DELAY_MAX_SECS
        );
        assert_eq!(
            automatic_delay_secs_from_random(AUTOMATIC_DELAY_MAX_SECS + 1),
            0
        );
        assert!(automatic_delay_secs_from_random(u64::MAX) <= AUTOMATIC_DELAY_MAX_SECS);
    }

    #[test]
    fn manual_trigger_has_no_random_delay() {
        assert_eq!(trigger_delay_secs(TriggerSource::Manual, 1), 0);
    }
}
