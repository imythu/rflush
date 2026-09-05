use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use regex::{Regex, escape};
use reqwest::header::{COOKIE, HeaderMap, HeaderValue};
use reqwest::{Client, Url};
use scraper::{Html, Selector};
use serde_json::Value;
use tracing::{debug, warn};

use super::{
    SiteAdapter, SiteAuth, SiteTestResult, TorrentAttributes, UserStats, UserStatsDetails,
};
use std::future::Future;
use std::pin::Pin;

pub struct NexusPhpAdapter {
    base_url: String,
    auth: SiteAuth,
    request_headers: HeaderMap,
    client: Client,
}

impl NexusPhpAdapter {
    pub fn new(
        base_url: String,
        auth: SiteAuth,
        request_headers: HeaderMap,
        client: Client,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            auth,
            request_headers,
            client,
        }
    }

    fn cookie_value(&self) -> Option<&str> {
        match &self.auth {
            SiteAuth::Cookie { cookie } => Some(cookie.as_str()),
            SiteAuth::CookiePasskey { cookie, .. } => Some(cookie.as_str()),
            _ => None,
        }
    }

    fn build_headers(&self) -> HeaderMap {
        let mut headers = self.request_headers.clone();
        if let Some(cookie) = self.cookie_value()
            && let Ok(val) = HeaderValue::from_str(cookie)
        {
            headers.insert(COOKIE, val);
        }
        headers
    }

    async fn fetch_user_info_api(&self) -> Result<UserStats, String> {
        let url = format!("{}/api/user", self.base_url);
        debug!("NexusPHP API request: {}", url);

        let resp = self
            .client
            .get(&url)
            .headers(self.build_headers())
            .send()
            .await
            .map_err(|e| format!("请求失败: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))?;
        let json: Value =
            serde_json::from_str(&text).map_err(|_| "响应不是有效JSON".to_string())?;

        if json.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(json_error_message(&json).unwrap_or_else(|| "API 返回失败".to_string()));
        }

        let response_data = json.get("data").unwrap_or(&json);
        let data = response_data.get("user").unwrap_or(response_data);
        let counters = data
            .get("memberCount")
            .or_else(|| data.get("stats"))
            .or_else(|| response_data.get("memberCount"))
            .or_else(|| response_data.get("stats"))
            .unwrap_or(data);

        let username = data
            .get("username")
            .or_else(|| data.get("name"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("unknown"))
            .ok_or_else(|| "API 响应缺少用户名".to_string())?
            .to_string();

        let uid = data
            .get("uid")
            .or_else(|| data.get("id"))
            .or_else(|| data.get("user_id"))
            .and_then(json_value_to_string);

        let uploaded = counters
            .get("uploaded")
            .or_else(|| counters.get("upload"))
            .or_else(|| data.get("upload"))
            .and_then(json_value_to_bytes);

        let downloaded = counters
            .get("downloaded")
            .or_else(|| counters.get("download"))
            .or_else(|| data.get("download"))
            .and_then(json_value_to_bytes);

        let uploaded = uploaded.ok_or_else(|| "API 响应缺少上传量".to_string())?;
        let downloaded = downloaded.ok_or_else(|| "API 响应缺少下载量".to_string())?;

        let ratio = counters
            .get("ratio")
            .or_else(|| data.get("ratio"))
            .and_then(json_value_to_f64);
        let bonus = data
            .get("bonus")
            .or_else(|| data.get("seedbonus"))
            .or_else(|| counters.get("bonus"))
            .and_then(json_value_to_f64);

        let roots = [data, counters, response_data, &json];
        let status_root = data.get("memberStatus").unwrap_or(data);
        let details = UserStatsDetails {
            is_donor: first_json_value(&roots, &["isDonor", "is_donor", "donor"])
                .and_then(json_value_to_bool),
            level_id: first_json_value(&roots, &["levelId", "level_id", "class_id"])
                .and_then(json_value_to_i64),
            level_name: first_json_value(
                &roots,
                &[
                    "levelName",
                    "level_name",
                    "className",
                    "class_name",
                    "class",
                ],
            )
            .and_then(json_value_to_nonempty_string),
            join_time: first_json_value(
                &roots,
                &[
                    "joinTime",
                    "join_time",
                    "joinedAt",
                    "joined_at",
                    "createdAt",
                    "created_at",
                ],
            )
            .and_then(json_value_to_timestamp_millis),
            last_access_at: first_json_value(
                &[status_root, data, response_data, &json],
                &[
                    "lastAccessAt",
                    "last_access_at",
                    "lastBrowse",
                    "last_access",
                    "lastSeen",
                ],
            )
            .and_then(json_value_to_timestamp_millis),
            message_count: first_json_value(
                &roots,
                &[
                    "messageCount",
                    "message_count",
                    "unreadMessages",
                    "unread_messages",
                ],
            )
            .and_then(json_value_to_u64),
            invites: first_json_value(&roots, &["invites", "invite_count"])
                .and_then(json_value_to_u64),
            avatar: first_json_value(&roots, &["avatar", "avatarUrl", "avatar_url"])
                .and_then(json_value_to_nonempty_string),
            total_traffic: first_json_value(&roots, &["totalTraffic", "total_traffic"])
                .and_then(json_value_to_bytes),
            true_downloaded: first_json_value(
                &roots,
                &["trueDownloaded", "true_downloaded", "actualDownloaded"],
            )
            .and_then(json_value_to_bytes),
            true_uploaded: first_json_value(
                &roots,
                &["trueUploaded", "true_uploaded", "actualUploaded"],
            )
            .and_then(json_value_to_bytes),
            true_ratio: first_json_value(&roots, &["trueRatio", "true_ratio"])
                .and_then(json_value_to_f64),
            seeding_size: first_json_value(&roots, &["seedingSize", "seeding_size"])
                .and_then(json_value_to_bytes),
            seeding_time: first_json_value(&roots, &["seedingTime", "seeding_time"])
                .and_then(json_value_to_u64),
            average_seeding_time: first_json_value(
                &roots,
                &[
                    "averageSeedingTime",
                    "average_seeding_time",
                    "averageSeedtime",
                ],
            )
            .and_then(json_value_to_u64),
            seeding_bonus: first_json_value(
                &roots,
                &["seedingBonus", "seeding_bonus", "seedingPoints"],
            )
            .and_then(json_value_to_f64),
            bonus_per_hour: first_json_value(&roots, &["bonusPerHour", "bonus_per_hour"])
                .and_then(json_value_to_f64),
            seeding_bonus_per_hour: first_json_value(
                &roots,
                &["seedingBonusPerHour", "seeding_bonus_per_hour"],
            )
            .and_then(json_value_to_f64),
            uploads: first_json_value(&roots, &["uploads", "upload_count"])
                .and_then(json_value_to_u64),
            snatches: first_json_value(&roots, &["snatches", "snatched"])
                .and_then(json_value_to_u64),
            posts: first_json_value(&roots, &["posts", "post_count"]).and_then(json_value_to_u64),
            adoptions: first_json_value(&roots, &["adoptions", "adoption_count"])
                .and_then(json_value_to_u64),
            hnr_unsatisfied: first_json_value(
                &roots,
                &["hnrUnsatisfied", "hnr_unsatisfied", "unsatisfieds"],
            )
            .and_then(json_value_to_u64),
            hnr_pre_warning: first_json_value(
                &roots,
                &["hnrPreWarning", "hnr_pre_warning", "prewarn"],
            )
            .and_then(json_value_to_u64),
            ..Default::default()
        };

        let mut stats = UserStats {
            uid,
            username,
            uploaded,
            downloaded,
            ratio: ratio.or_else(|| ratio_from_totals(Some(uploaded), Some(downloaded))),
            bonus,
            seeding_count: counters
                .get("seeding")
                .or_else(|| counters.get("seeding_count"))
                .or_else(|| counters.get("seederCount"))
                .and_then(json_value_to_u32),
            leeching_count: counters
                .get("leeching")
                .or_else(|| counters.get("leeching_count"))
                .or_else(|| counters.get("leecherCount"))
                .and_then(json_value_to_u32),
            details,
        };
        stats.fill_derived();
        Ok(stats)
    }

    async fn fetch_user_info_html(&self) -> Result<UserStats, String> {
        let url = format!("{}/index.php", self.base_url);
        debug!("NexusPHP HTML request: {}", url);

        let index_html = self.fetch_html_page(&url, "首页").await?;
        // Some NexusPHP themes do not render the current-user anchor in the raw homepage HTML.
        // The old implementation could still read the transfer totals from that page, so making
        // the anchor mandatory caused otherwise valid sessions to regress. Prefer the page link,
        // then recover the stable user id from the NexusPHP login cookie; if neither is available,
        // keep parsing the homepage instead of treating the missing optional link as auth failure.
        let identity = extract_current_user(&index_html).or_else(|| {
            self.cookie_value()
                .and_then(extract_current_user_from_cookie)
        });
        let (detail_html, detail_page_loaded) = if let Some(identity) = identity.as_ref() {
            let detail_url = self.resolve_same_origin_url(&identity.href)?;
            match self.fetch_html_page(&detail_url, "用户详情页").await {
                Ok(html) => (html, true),
                Err(error) => {
                    debug!(%error, "NexusPHP 用户详情页获取失败，回退首页统计");
                    (index_html.clone(), false)
                }
            }
        } else {
            (index_html.clone(), false)
        };

        let detail_identity = extract_current_user(&detail_html);
        let uid = detail_identity
            .as_ref()
            .map(|value| value.uid.clone())
            .or_else(|| identity.as_ref().map(|value| value.uid.clone()));
        let username = detail_identity
            .as_ref()
            .and_then(|value| value.username.clone())
            .or_else(|| identity.as_ref().and_then(|value| value.username.clone()))
            .or_else(|| extract_username(&detail_html))
            .or_else(|| extract_username(&index_html))
            .unwrap_or_else(|| uid.clone().unwrap_or_else(|| "unknown".to_string()));

        let detail_text = extract_visible_text(&detail_html);
        let index_text = extract_visible_text(&index_html);
        let uploaded = parse_labeled_size(&detail_text, &["上传量", "上傳量", "Uploaded"])
            .or_else(|| parse_labeled_size(&index_text, &["上传量", "上傳量", "Uploaded"]));
        let downloaded = parse_labeled_size(&detail_text, &["下载量", "下載量", "Downloaded"])
            .or_else(|| parse_labeled_size(&index_text, &["下载量", "下載量", "Downloaded"]));

        let page_label = if detail_page_loaded {
            "用户详情页和首页"
        } else {
            "首页"
        };
        let uploaded = uploaded
            .ok_or_else(|| format!("{page_label}没有找到上传量，站点页面结构可能需要单独适配"))?;
        let downloaded = downloaded
            .ok_or_else(|| format!("{page_label}没有找到下载量，站点页面结构可能需要单独适配"))?;

        let ratio = parse_labeled_number(&detail_text, &["分享率", "Ratio"])
            .or_else(|| ratio_from_totals(Some(uploaded), Some(downloaded)));
        let bonus = parse_labeled_number(
            &detail_text,
            &[
                "魔力值",
                "Karma Points",
                "魅力值",
                "星焱",
                "沙粒",
                "魔力",
                "Bonus",
            ],
        );
        let mut seeding_count = parse_labeled_integer(
            &detail_text,
            &["当前做种", "當前做種", "做种数", "做種數", "Seeding"],
        );
        let leeching_count = parse_labeled_integer(
            &detail_text,
            &["当前下载", "當前下載", "下载数", "下載數", "Leeching"],
        );

        let (hnr_pre_warning, hnr_unsatisfied) = parse_hnr_counts(&index_text);
        let mut details = UserStatsDetails {
            is_donor: Some(detect_donor(&detail_html)),
            level_name: parse_table_labeled_value(&detail_html, &["等级", "等級", "Class"]),
            join_time: parse_table_labeled_value(
                &detail_html,
                &["加入日期", "加入時間", "Join date", "Joined"],
            )
            .as_deref()
            .and_then(parse_user_datetime_millis),
            last_access_at: parse_table_labeled_value(
                &detail_html,
                &["最近动向", "最近動向", "Last Action", "Last access"],
            )
            .as_deref()
            .and_then(parse_user_datetime_millis),
            message_count: parse_message_count(&index_html),
            invites: parse_labeled_u64(&detail_text, &["邀请", "邀請", "Invites", "Invitations"]),
            avatar: extract_avatar(&detail_html)
                .and_then(|avatar| self.resolve_same_origin_url(&avatar).ok().or(Some(avatar))),
            true_downloaded: parse_labeled_size(
                &detail_text,
                &[
                    "实际下载量",
                    "真实下载量",
                    "實際下載量",
                    "真實下載量",
                    "Real Downloaded",
                    "Actual Downloaded",
                ],
            ),
            true_uploaded: parse_labeled_size(
                &detail_text,
                &[
                    "实际上傳量",
                    "实际上传量",
                    "真实上传量",
                    "實際上傳量",
                    "真實上傳量",
                    "Real Uploaded",
                    "Actual Uploaded",
                ],
            ),
            seeding_time: parse_labeled_duration_seconds(
                &detail_text,
                &["做种时间", "做種時間", "Seeding Time", "Seed Time"],
            ),
            average_seeding_time: parse_labeled_duration_seconds(
                &detail_text,
                &[
                    "平均做种时间",
                    "平均做種時間",
                    "Average Seeding Time",
                    "Average Seed Time",
                ],
            ),
            seeding_bonus: parse_labeled_number(
                &detail_text,
                &["做种积分", "做種積分", "Seeding Points", "保种积分"],
            ),
            uploads: parse_labeled_u64(
                &detail_text,
                &["发布数", "發佈數", "上传种子", "上傳種子", "Uploads"],
            ),
            snatches: parse_labeled_u64(
                &detail_text,
                &["完成数", "完成數", "下载完成", "下載完成", "Snatches"],
            ),
            posts: parse_labeled_u64(&detail_text, &["论坛发帖", "論壇發帖", "Posts"]),
            adoptions: parse_labeled_u64(&detail_text, &["认领种子", "認領種子", "Adoptions"]),
            hnr_unsatisfied,
            hnr_pre_warning,
            ..Default::default()
        };

        // PT-Depiler 的 NexusPHP 通用 schema 会用 AJAX 补充做种量和发布数。
        if let Some(user_id) = uid.as_deref() {
            if let Some((count, size)) = self.fetch_user_torrent_summary(user_id, "seeding").await {
                seeding_count = Some(count);
                details.seeding_size = size;
            }
            if let Some((count, _)) = self.fetch_user_torrent_summary(user_id, "uploaded").await {
                details.uploads = Some(count as u64);
            }
        }

        let bonus_url = format!("{}/mybonus.php", self.base_url);
        match self.fetch_html_page(&bonus_url, "魔力值页面").await {
            Ok(bonus_html) => {
                let bonus_text = extract_visible_text(&bonus_html);
                details.bonus_per_hour = parse_labeled_number(
                    &bonus_text,
                    &[
                        "你当前每小时能获取",
                        "你當前每小時能獲取",
                        "每小时魔力值",
                        "每小時魔力值",
                        "Bonus per hour",
                        "You are currently getting",
                    ],
                );
                details.seeding_bonus_per_hour = parse_labeled_number(
                    &bonus_text,
                    &[
                        "每小时做种积分",
                        "每小時做種積分",
                        "每小时保种积分",
                        "Seeding points per hour",
                    ],
                );
            }
            Err(error) => debug!(%error, "NexusPHP 魔力值页面获取失败"),
        }

        let mut stats = UserStats {
            uid,
            username,
            uploaded,
            downloaded,
            ratio,
            bonus,
            seeding_count,
            leeching_count,
            details,
        };
        stats.fill_derived();
        Ok(stats)
    }

    async fn fetch_html_page(&self, url: &str, label: &str) -> Result<String, String> {
        let response = self
            .client
            .get(url)
            .headers(self.build_headers())
            .send()
            .await
            .map_err(|error| format!("{label}请求失败: {error}"))?;
        let status = response.status();
        let final_url = response.url().clone();
        if !status.is_success() {
            return Err(format!("{label}返回 HTTP {status}"));
        }
        let html = response
            .text()
            .await
            .map_err(|error| format!("读取{label}响应失败: {error}"))?;

        if looks_like_cloudflare_challenge(&html) {
            return Err(format!("{label}被 Cloudflare 验证页拦截"));
        }
        if looks_like_login_page(&html, &final_url) {
            return Err("Cookie 无效或已过期，站点返回了登录页".to_string());
        }
        Ok(html)
    }

    fn resolve_same_origin_url(&self, href: &str) -> Result<String, String> {
        let base = Url::parse(&format!("{}/", self.base_url.trim_end_matches('/')))
            .map_err(|error| format!("站点地址无效: {error}"))?;
        let url = base
            .join(href)
            .map_err(|error| format!("用户详情链接无效: {error}"))?;
        if base.scheme() != url.scheme()
            || base.host_str() != url.host_str()
            || base.port_or_known_default() != url.port_or_known_default()
        {
            return Err("用户详情链接跳转到了其他站点，已拒绝请求".to_string());
        }
        Ok(url.to_string())
    }

    async fn fetch_user_torrent_summary(
        &self,
        user_id: &str,
        torrent_type: &str,
    ) -> Option<(u32, Option<u64>)> {
        let mut url = Url::parse(&format!("{}/getusertorrentlistajax.php", self.base_url)).ok()?;
        url.query_pairs_mut()
            .append_pair("userid", user_id)
            .append_pair("type", torrent_type);
        let response = self
            .client
            .get(url)
            .headers(self.build_headers())
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let html = response.text().await.ok()?;
        let summary = parse_user_torrent_ajax_summary(&html);
        if summary.is_none() {
            warn!(
                torrent_type,
                "NexusPHP 用户种子列表已返回，但无法解析记录数"
            );
        }
        summary
    }

    async fn fetch_torrent_detail_html(&self, detail_url: &str) -> Result<String, String> {
        debug!("NexusPHP detail request: {}", detail_url);

        let resp = self
            .client
            .get(detail_url)
            .headers(self.build_headers())
            .send()
            .await
            .map_err(|e| format!("请求失败: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))?;
        if html.contains("login.php") && !html.contains("details") {
            return Err("Cookie 无效或已过期".to_string());
        }

        Ok(html)
    }

    fn detect_torrent_attributes(html: &str) -> TorrentAttributes {
        let document = Html::parse_document(html);
        let selectors = [
            "body",
            ".torrentname",
            ".embedded",
            ".sticky",
            ".pro_free",
            ".pro_free2up",
            ".free",
            ".twoupfree",
            ".twoup",
            ".hitandrun",
            ".hr",
            ".promotion-tag",
            ".torrent-promote",
            ".torrent-detail",
            ".torrent_info",
            "span",
            "a",
            "b",
            "strong",
            "font",
        ];
        let mut fragments = Vec::new();
        for selector_str in selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                for element in document.select(&selector) {
                    let text = element.text().collect::<Vec<_>>().join(" ");
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        fragments.push(trimmed.to_string());
                    }
                    if let Some(class) = element.value().attr("class") {
                        fragments.push(class.to_string());
                    }
                    if let Some(title) = element.value().attr("title") {
                        fragments.push(title.to_string());
                    }
                }
            }
        }
        let upper = fragments.join(" ").to_ascii_uppercase();

        let has_two_x_free = contains_any(
            &upper,
            &[
                "2XFREE",
                "2X FREE",
                "FREE 2XUP",
                "FREE,2XUP",
                "TWOUPFREE",
                "PRO_FREE2UP",
            ],
        );
        let has_free = has_two_x_free
            || contains_any(
                &upper,
                &[
                    "FREELEECH",
                    "FREE LEECH",
                    " FREE ",
                    "PRO_FREE",
                    " 免费 ",
                    " FREE<",
                    ">FREE ",
                ],
            );
        let hit_and_run = contains_any(
            &upper,
            &[
                "H&R",
                "HIT AND RUN",
                "HIT&RUN",
                "HR:",
                "HNR",
                "HITRUN",
                "HITANDRUN",
                " HR ",
            ],
        );

        let (download_volume_factor, upload_volume_factor) = if has_two_x_free {
            (Some(0.0), Some(2.0))
        } else if has_free {
            (Some(0.0), Some(1.0))
        } else {
            (
                detect_download_factor(&upper).or(Some(1.0)),
                detect_upload_factor(&upper).or(Some(1.0)),
            )
        };

        let free_end_timestamp = if has_free {
            detect_free_end_timestamp(html)
        } else {
            None
        };

        TorrentAttributes {
            free: has_free || download_volume_factor == Some(0.0),
            two_x_free: has_two_x_free,
            hit_and_run,
            seeder_count: None,
            leecher_count: None,
            free_end_timestamp,
            download_volume_factor,
            upload_volume_factor,
        }
    }
}

impl SiteAdapter for NexusPhpAdapter {
    fn test_connection(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<SiteTestResult, String>> + Send + '_>> {
        Box::pin(async move {
            match self.get_user_stats().await {
                Ok(stats) => Ok(SiteTestResult {
                    success: true,
                    message: format!("连接成功，用户: {}", stats.username),
                    user_stats: Some(stats),
                }),
                Err(e) => Ok(SiteTestResult {
                    success: false,
                    message: e,
                    user_stats: None,
                }),
            }
        })
    }

    fn get_user_stats(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<UserStats, String>> + Send + '_>> {
        Box::pin(async move {
            // Cookie-authenticated NexusPHP sites are HTML-first. This matches PT-Depiler's
            // NexusPHP flow and avoids probing a non-existent /api/user endpoint on every refresh.
            match self.fetch_user_info_html().await {
                Ok(stats) => Ok(stats),
                Err(html_error) => match self.fetch_user_info_api().await {
                    Ok(stats) => Ok(stats),
                    Err(api_error) => Err(format!(
                        "HTML 获取失败（{html_error}）；API /api/user 回退失败（{api_error}）"
                    )),
                },
            }
        })
    }

    fn get_torrent_attributes(
        &self,
        detail_url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TorrentAttributes, String>> + Send + '_>> {
        let detail_url = detail_url.to_string();
        Box::pin(async move {
            let html = self.fetch_torrent_detail_html(&detail_url).await?;
            Ok(Self::detect_torrent_attributes(&html))
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CurrentUser {
    uid: String,
    username: Option<String>,
    href: String,
}

fn json_value_to_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
        .or_else(|| value.as_i64().map(|n| n.to_string()))
}

fn json_value_to_nonempty_string(value: &Value) -> Option<String> {
    json_value_to_string(value)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn first_json_value<'a>(roots: &[&'a Value], keys: &[&str]) -> Option<&'a Value> {
    roots
        .iter()
        .find_map(|root| keys.iter().find_map(|key| root.get(key)))
}

fn json_value_to_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|number| number.is_finite() && *number >= 0.0)
                .map(|number| number as u64)
        })
        .or_else(|| {
            let value = value.as_str()?.trim().replace(',', "");
            value.parse::<u64>().ok().or_else(|| {
                value
                    .parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite() && *number >= 0.0)
                    .map(|number| number.min(u64::MAX as f64) as u64)
            })
        })
}

fn json_value_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| value.as_str()?.trim().replace(',', "").parse::<i64>().ok())
}

fn json_value_to_bool(value: &Value) -> Option<bool> {
    value
        .as_bool()
        .or_else(|| value.as_i64().map(|number| number != 0))
        .or_else(|| {
            value
                .as_str()
                .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
                    "true" | "yes" | "1" => Some(true),
                    "false" | "no" | "0" => Some(false),
                    _ => None,
                })
        })
}

fn json_value_to_timestamp_millis(value: &Value) -> Option<i64> {
    if let Some(timestamp) = json_value_to_i64(value) {
        return normalize_timestamp_millis(timestamp);
    }
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(parse_user_datetime_millis)
}

fn normalize_timestamp_millis(timestamp: i64) -> Option<i64> {
    if timestamp.unsigned_abs() < 100_000_000_000 {
        timestamp.checked_mul(1000)
    } else {
        Some(timestamp)
    }
}

fn json_value_to_bytes(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|number| *number >= 0.0)
                .map(|number| number as u64)
        })
        .or_else(|| {
            let value = value.as_str()?.trim().replace(',', "");
            value
                .parse::<u64>()
                .ok()
                .or_else(|| extract_size_value(&value))
        })
}

fn json_value_to_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().replace(',', "").parse::<f64>().ok())
}

fn json_value_to_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .or_else(|| value.as_i64().and_then(|number| u32::try_from(number).ok()))
        .or_else(|| value.as_str()?.trim().replace(',', "").parse::<u32>().ok())
}

fn json_error_message(value: &Value) -> Option<String> {
    ["message", "msg", "error"]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::to_string)
}

fn ratio_from_totals(uploaded: Option<u64>, downloaded: Option<u64>) -> Option<f64> {
    match (uploaded, downloaded) {
        (Some(uploaded), Some(downloaded)) if downloaded > 0 => {
            Some(uploaded as f64 / downloaded as f64)
        }
        _ => None,
    }
}

fn extract_current_user(html: &str) -> Option<CurrentUser> {
    let document = Html::parse_document(html);
    let selectors = [
        "#info_block a[href*='userdetails.php'][href*='id=']",
        "#info_block a[href*='user.php'][href*='id=']",
        "a[href*='userdetails.php'][class*='Name']",
        "a[href*='userdetails.php'][href*='id=']",
        "a[href*='user.php'][href*='id=']",
    ];

    for selector in selectors {
        let Ok(selector) = Selector::parse(selector) else {
            continue;
        };
        for element in document.select(&selector) {
            let Some(href) = element.value().attr("href") else {
                continue;
            };
            let Some(uid) = extract_user_id_from_href(href) else {
                continue;
            };
            let username =
                normalize_text(element.text()).filter(|name| !name.eq_ignore_ascii_case("details"));
            return Some(CurrentUser {
                uid,
                username,
                href: href.to_string(),
            });
        }
    }
    None
}

fn extract_current_user_from_cookie(cookie: &str) -> Option<CurrentUser> {
    let uid = extract_user_id_from_cookie(cookie)?;
    Some(CurrentUser {
        href: format!("/userdetails.php?id={uid}"),
        uid,
        username: None,
    })
}

fn extract_user_id_from_cookie(cookie: &str) -> Option<String> {
    let pairs = cookie.split(';').filter_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        let value = value.trim().trim_matches('"');
        let decoded = urlencoding::decode(value).ok()?.into_owned();
        Some((name.trim().to_ascii_lowercase(), decoded))
    });
    let pairs = pairs.collect::<Vec<_>>();

    // Older NexusPHP installations expose a dedicated uid cookie.
    for (name, value) in &pairs {
        if (name == "c_secure_uid" || name.ends_with("_secure_uid"))
            && let Some(uid) = decode_cookie_user_id(value)
        {
            return Some(uid);
        }
    }

    // Current NexusPHP stores {"user_id": ..., "expires": ...}.<signature> in
    // a base64 encoded c_secure_pass cookie.
    for (name, value) in &pairs {
        if (name == "c_secure_pass" || name.ends_with("_secure_pass"))
            && let Some(uid) = decode_cookie_user_id(value)
        {
            return Some(uid);
        }
    }
    None
}

fn decode_cookie_user_id(value: &str) -> Option<String> {
    let value = value.trim();
    if !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(value.to_string());
    }

    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        let Ok(decoded) = engine.decode(value) else {
            continue;
        };
        let Ok(decoded) = String::from_utf8(decoded) else {
            continue;
        };
        let payload = decoded
            .split_once('.')
            .map_or(decoded.as_str(), |(json, _)| json);
        if !payload.is_empty() && payload.chars().all(|ch| ch.is_ascii_digit()) {
            return Some(payload.to_string());
        }
        let Ok(json) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(uid) = ["user_id", "uid", "id"]
            .iter()
            .find_map(|key| json.get(key).and_then(json_value_to_string))
            .filter(|uid| !uid.is_empty() && uid.chars().all(|ch| ch.is_ascii_digit()))
        {
            return Some(uid);
        }
    }
    None
}

fn extract_username(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    for selector in [
        "#info_block .User_Name",
        "#info_block .username",
        ".User_Name",
        ".username",
    ] {
        let Ok(selector) = Selector::parse(selector) else {
            continue;
        };
        if let Some(username) = document
            .select(&selector)
            .find_map(|element| normalize_text(element.text()))
        {
            return Some(username);
        }
    }
    None
}

fn extract_user_id_from_href(href: &str) -> Option<String> {
    let base = Url::parse("https://tracker.invalid/").ok()?;
    let url = base.join(href).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == "id" || key == "userid" || key == "user_id")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_text<'a>(parts: impl Iterator<Item = &'a str>) -> Option<String> {
    let text = parts
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

fn extract_visible_text(html: &str) -> String {
    let document = Html::parse_document(html);
    normalize_text(document.root_element().text()).unwrap_or_default()
}

fn parse_labeled_size(text: &str, labels: &[&str]) -> Option<u64> {
    labels.iter().find_map(|label| {
        let expression = format!(
            r"(?i){}\s*[^0-9]{{0,48}}([0-9][0-9,.]*)\s*(bytes?|[kmgtpez]i?b)",
            escape(label)
        );
        let captures = Regex::new(&expression).ok()?.captures(text)?;
        size_from_parts(captures.get(1)?.as_str(), captures.get(2)?.as_str())
    })
}

fn parse_labeled_number(text: &str, labels: &[&str]) -> Option<f64> {
    labels.iter().find_map(|label| {
        let expression = format!(r"(?i){}\s*[^0-9]{{0,48}}([0-9][0-9,.]*)", escape(label));
        let value = Regex::new(&expression)
            .ok()?
            .captures(text)?
            .get(1)?
            .as_str()
            .replace(',', "");
        value.parse::<f64>().ok()
    })
}

fn parse_labeled_integer(text: &str, labels: &[&str]) -> Option<u32> {
    parse_labeled_number(text, labels).and_then(|value| {
        if value.is_finite() && value >= 0.0 && value <= u32::MAX as f64 {
            Some(value as u32)
        } else {
            None
        }
    })
}

fn parse_labeled_u64(text: &str, labels: &[&str]) -> Option<u64> {
    parse_labeled_number(text, labels).and_then(|value| {
        if value.is_finite() && value >= 0.0 && value <= u64::MAX as f64 {
            Some(value as u64)
        } else {
            None
        }
    })
}

fn parse_labeled_duration_seconds(text: &str, labels: &[&str]) -> Option<u64> {
    labels.iter().find_map(|label| {
        let expression = format!(
            r"(?i){}\s*[^0-9]{{0,48}}([0-9][0-9,.]*)\s*(years?|months?|weeks?|days?|hours?|hrs?|minutes?|mins?|seconds?|secs?|年|月|周|週|天|日|小时|小時|时|時|分钟|分鐘|分|秒)",
            escape(label)
        );
        let captures = Regex::new(&expression).ok()?.captures(text)?;
        let value = captures.get(1)?.as_str().replace(',', "").parse::<f64>().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        let unit = captures.get(2)?.as_str().to_ascii_lowercase();
        let multiplier = if matches!(unit.as_str(), "year" | "years" | "年") {
            365 * 24 * 60 * 60
        } else if matches!(unit.as_str(), "month" | "months" | "月") {
            30 * 24 * 60 * 60
        } else if matches!(unit.as_str(), "week" | "weeks" | "周" | "週") {
            7 * 24 * 60 * 60
        } else if matches!(unit.as_str(), "day" | "days" | "天" | "日") {
            24 * 60 * 60
        } else if matches!(unit.as_str(), "hour" | "hours" | "hr" | "hrs" | "小时" | "小時" | "时" | "時") {
            60 * 60
        } else if matches!(unit.as_str(), "minute" | "minutes" | "min" | "mins" | "分钟" | "分鐘" | "分") {
            60
        } else {
            1
        };
        Some((value * multiplier as f64).min(u64::MAX as f64) as u64)
    })
}

fn parse_table_labeled_value(html: &str, labels: &[&str]) -> Option<String> {
    let document = Html::parse_document(html);
    let row_selector = Selector::parse("tr").ok()?;
    let cell_selector = Selector::parse("th, td").ok()?;
    let image_selector = Selector::parse("img[title], img[alt]").ok()?;

    let mut fallback = None;
    for row in document.select(&row_selector) {
        let cells = row.select(&cell_selector).collect::<Vec<_>>();
        if cells.len() < 2 {
            continue;
        }
        let Some(label_text) = normalize_text(cells[0].text()) else {
            continue;
        };
        for label in labels {
            let exact = label_text.trim().eq_ignore_ascii_case(label);
            if !exact
                && !label_text
                    .to_ascii_lowercase()
                    .contains(&label.to_ascii_lowercase())
            {
                continue;
            }
            let value = cells[1]
                .select(&image_selector)
                .find_map(|image| {
                    image
                        .value()
                        .attr("title")
                        .or_else(|| image.value().attr("alt"))
                })
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| normalize_text(cells[1].text()));
            if exact {
                return value;
            }
            fallback = fallback.or(value);
        }
    }
    fallback
}

fn parse_user_datetime_millis(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Ok(timestamp) = value.parse::<i64>() {
        return normalize_timestamp_millis(timestamp);
    }
    if let Ok(datetime) = DateTime::parse_from_rfc3339(value) {
        return Some(datetime.timestamp_millis());
    }
    if let Some(timestamp) = extract_datetime_to_utc8(value) {
        return timestamp.checked_mul(1000);
    }
    let date = NaiveDate::parse_from_str(value.split(" (").next()?, "%Y-%m-%d").ok()?;
    let timezone = FixedOffset::east_opt(8 * 3600)?;
    timezone
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .single()
        .map(|datetime| datetime.timestamp_millis())
}

fn parse_message_count(html: &str) -> Option<u64> {
    let document = Html::parse_document(html);
    let selectors = [
        "td[style*='background: red'] a[href*='messages.php']",
        "td[style*='background:red'] a[href*='messages.php']",
        "a[href*='messages.php']",
    ];
    for selector in selectors {
        let Ok(selector) = Selector::parse(selector) else {
            continue;
        };
        for element in document.select(&selector) {
            let Some(text) = normalize_text(element.text()) else {
                continue;
            };
            if let Some(value) = first_integer(&text) {
                return Some(value);
            }
        }
    }
    None
}

fn first_integer(value: &str) -> Option<u64> {
    Regex::new(r"[0-9][0-9,]*")
        .ok()?
        .find(value)?
        .as_str()
        .replace(',', "")
        .parse()
        .ok()
}

fn extract_avatar(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let selectors = [
        "img.avatar",
        "img[class*='avatar']",
        "img[src*='/avatars/']",
        "img[src*='avatar']",
    ];
    selectors.iter().find_map(|selector| {
        let selector = Selector::parse(selector).ok()?;
        document
            .select(&selector)
            .find_map(|image| image.value().attr("src"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn detect_donor(html: &str) -> bool {
    let document = Html::parse_document(html);
    let Ok(selector) = Selector::parse("img[alt], img[title]") else {
        return false;
    };
    document.select(&selector).any(|image| {
        [image.value().attr("alt"), image.value().attr("title")]
            .into_iter()
            .flatten()
            .any(|value| {
                let value = value.to_ascii_lowercase();
                value.contains("donor") || value.contains("捐赠") || value.contains("捐贈")
            })
    })
}

fn parse_hnr_counts(text: &str) -> (Option<u64>, Option<u64>) {
    let marker = Regex::new(r"(?i)(?:H\s*&\s*R|HNR|Hit\s*(?:and|&)\s*Run)")
        .ok()
        .and_then(|expression| expression.find(text));
    if let Some(marker) = marker {
        let relevant = text[marker.end()..].chars().take(256).collect::<String>();
        if let Ok(expression) = Regex::new(r"([0-9,]+)\s*/\s*([0-9,]+)(?:\s*/\s*[0-9,]+)?") {
            let mut found = false;
            let mut pre_warning = 0u64;
            let mut unsatisfied = 0u64;
            for captures in expression.captures_iter(&relevant) {
                let Some(pre) = captures
                    .get(1)
                    .and_then(|value| value.as_str().replace(',', "").parse::<u64>().ok())
                else {
                    continue;
                };
                let Some(unsatisfied_count) = captures
                    .get(2)
                    .and_then(|value| value.as_str().replace(',', "").parse::<u64>().ok())
                else {
                    continue;
                };
                pre_warning = pre_warning.saturating_add(pre);
                unsatisfied = unsatisfied.saturating_add(unsatisfied_count);
                found = true;
            }
            if found {
                return (Some(pre_warning), Some(unsatisfied));
            }
        }
    }

    (
        parse_labeled_u64(text, &["H&R 预警", "H&R 預警", "HNR pre-warning"]),
        parse_labeled_u64(text, &["H&R 未达标", "H&R 未達標", "HNR unsatisfied"]),
    )
}

fn parse_user_torrent_ajax_summary(html: &str) -> Option<(u32, Option<u64>)> {
    let text = extract_visible_text(html);
    let summary_expression =
        Regex::new(r"(?i)([0-9][0-9,]*)\s*\|\s*([0-9][0-9,.]*)\s*([kmgtpez]i?b|bytes?)").ok()?;
    if let Some(captures) = summary_expression.captures(&text) {
        let count = captures.get(1)?.as_str().replace(',', "").parse().ok()?;
        let size = size_from_parts(captures.get(2)?.as_str(), captures.get(3)?.as_str());
        return Some((count, size));
    }

    if Regex::new(r"(?i)no records?|没有记录|沒有記錄")
        .ok()?
        .is_match(&text)
    {
        return Some((0, Some(0)));
    }

    let record_expression = Regex::new(r"(?i)([0-9][0-9,]*)\s*(?:条记录|條記錄|records?)").ok()?;
    if let Some(captures) = record_expression.captures(&text) {
        let count = captures.get(1)?.as_str().replace(',', "").parse().ok()?;
        return Some((count, None));
    }

    let document = Html::parse_document(html);
    let table_selector = Selector::parse("table").ok()?;
    let row_selector = Selector::parse("tr").ok()?;
    let table = document.select(&table_selector).next_back()?;
    let rows = table.select(&row_selector).skip(1).collect::<Vec<_>>();
    if rows.is_empty() {
        return None;
    }
    let size_expression = Regex::new(r"(?i)([0-9][0-9,.]*)\s*([kmgtpez]i?b|bytes?)").ok()?;
    let mut total_size = 0u64;
    let mut has_size = false;
    for row in &rows {
        let row_text = normalize_text(row.text()).unwrap_or_default();
        if let Some(captures) = size_expression.captures(&row_text)
            && let Some(size) =
                size_from_parts(captures.get(1)?.as_str(), captures.get(2)?.as_str())
        {
            total_size = total_size.saturating_add(size);
            has_size = true;
        }
    }
    Some((
        u32::try_from(rows.len()).unwrap_or(u32::MAX),
        has_size.then_some(total_size),
    ))
}

#[cfg(test)]
fn parse_seeding_ajax_count(html: &str) -> Option<u32> {
    parse_user_torrent_ajax_summary(html).map(|(count, _)| count)
}

fn looks_like_login_page(html: &str, final_url: &Url) -> bool {
    if final_url.path().to_ascii_lowercase().contains("login.php") {
        return true;
    }
    let lower = html.to_ascii_lowercase();
    (lower.contains("type=\"password\"") || lower.contains("type='password'"))
        && (lower.contains("action=\"login.php")
            || lower.contains("action='login.php")
            || lower.contains("action=\"takelogin.php")
            || lower.contains("action='takelogin.php")
            || lower.contains("id=\"form-login\"")
            || lower.contains("id='form-login'")
            || lower.contains("name=\"login\"")
            || lower.contains("name='login'"))
        && extract_current_user(html).is_none()
}

fn looks_like_cloudflare_challenge(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("cf-chl-")
        || lower.contains("challenge-platform")
        || lower.contains("just a moment...")
        || lower.contains("cloudflare ray id")
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn detect_download_factor(upper: &str) -> Option<f64> {
    if contains_any(
        upper,
        &[
            "50%DL",
            "50% DL",
            "0.5X",
            "DOWNLOAD 50%",
            "50%DOWN",
            "半价",
            "五折",
        ],
    ) {
        Some(0.5)
    } else if contains_any(
        upper,
        &["30%DL", "30% DL", "0.3X", "DOWNLOAD 30%", "30%DOWN", "七折"],
    ) {
        Some(0.3)
    } else {
        None
    }
}

fn detect_upload_factor(upper: &str) -> Option<f64> {
    if contains_any(
        upper,
        &[
            "2XUP",
            "2X UP",
            "2XUPLOAD",
            "UPLOAD 200%",
            "UP 200%",
            "双倍上传",
        ],
    ) {
        Some(2.0)
    } else if contains_any(upper, &["0XUP", "UPLOAD 0%", "UP 0%", "零上传", "不计上传"]) {
        Some(0.0)
    } else {
        None
    }
}

fn detect_free_end_timestamp(html: &str) -> Option<i64> {
    const KEYWORDS: &[&str] = &[
        "free结束",
        "free 到期",
        "free截止",
        "free until",
        "promotion until",
        "限时免费",
        "優惠到期",
        "促销结束",
    ];

    let lower = html.to_lowercase();
    for keyword in KEYWORDS {
        let keyword_lower = keyword.to_lowercase();
        if let Some(pos) = lower.find(&keyword_lower) {
            let end = (pos + 160).min(html.len());
            if let Some(timestamp) = extract_datetime_to_utc8(&html[pos..end]) {
                return Some(timestamp);
            }
        }
    }

    extract_datetime_to_utc8(html)
}

fn extract_datetime_to_utc8(text: &str) -> Option<i64> {
    let tz = FixedOffset::east_opt(8 * 3600)?;
    for window in text.as_bytes().windows(19) {
        // 窗口可能切在多字节 UTF-8 字符中间，此时跳过而不是中止整个扫描。
        let Ok(candidate) = std::str::from_utf8(window) else {
            continue;
        };
        if looks_like_datetime(candidate)
            && let Ok(naive) = NaiveDateTime::parse_from_str(candidate, "%Y-%m-%d %H:%M:%S")
            && let Some(datetime) = tz.from_local_datetime(&naive).single()
        {
            return Some(datetime.timestamp());
        }
    }
    None
}

fn looks_like_datetime(value: &str) -> bool {
    value.len() == 19
        && value.chars().enumerate().all(|(index, ch)| match index {
            4 | 7 => ch == '-',
            10 => ch == ' ',
            13 | 16 => ch == ':',
            _ => ch.is_ascii_digit(),
        })
}

fn extract_size_value(text: &str) -> Option<u64> {
    let mut num_start = None;
    let mut num_end = None;

    for (i, ch) in text.char_indices() {
        if num_start.is_none() {
            if ch.is_ascii_digit() {
                num_start = Some(i);
            }
        } else if !ch.is_ascii_digit() && ch != '.' {
            num_end = Some(i);
            break;
        }
    }

    let start = num_start?;
    let end = num_end.unwrap_or(text.len());
    let number = text[start..end].trim();
    let unit_text = text[end..].trim();

    size_from_parts(number, unit_text)
}

fn size_from_parts(number: &str, unit: &str) -> Option<u64> {
    let number: f64 = number.replace(',', "").parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let normalized_unit = unit.trim().to_ascii_lowercase();
    let power = match normalized_unit.as_str() {
        value if value.starts_with("zb") || value.starts_with("zib") => 7,
        value if value.starts_with("eb") || value.starts_with("eib") => 6,
        value if value.starts_with("pb") || value.starts_with("pib") => 5,
        value if value.starts_with("tb") || value.starts_with("tib") => 4,
        value if value.starts_with("gb") || value.starts_with("gib") => 3,
        value if value.starts_with("mb") || value.starts_with("mib") => 2,
        value if value.starts_with("kb") || value.starts_with("kib") => 1,
        _ => 0,
    };
    let bytes = number * 1024_f64.powi(power);
    Some(bytes.min(u64::MAX as f64) as u64)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::http::StatusCode;
    use axum::response::Html;
    use axum::routing::get;
    use base64::Engine as _;
    use reqwest::Client;
    use reqwest::header::{COOKIE, HeaderMap, HeaderValue};

    use super::{
        NexusPhpAdapter, SiteAuth, detect_donor, extract_avatar, extract_current_user,
        extract_user_id_from_cookie, extract_username, extract_visible_text, looks_like_login_page,
        parse_hnr_counts, parse_labeled_duration_seconds, parse_labeled_integer,
        parse_labeled_number, parse_labeled_size, parse_message_count, parse_seeding_ajax_count,
        parse_table_labeled_value, parse_user_datetime_millis, parse_user_torrent_ajax_summary,
    };
    use crate::site::SiteAdapter;

    #[test]
    fn custom_headers_are_applied_without_overriding_authentication() {
        let mut custom = HeaderMap::new();
        custom.insert("x-browser-profile", HeaderValue::from_static("desktop"));
        custom.insert(COOKIE, HeaderValue::from_static("stale=1"));
        let adapter = NexusPhpAdapter::new(
            "https://tracker.example".to_string(),
            SiteAuth::Cookie {
                cookie: "uid=1".to_string(),
            },
            custom,
            Client::new(),
        );

        let headers = adapter.build_headers();
        assert_eq!(headers["x-browser-profile"], "desktop");
        assert_eq!(headers[COOKIE], "uid=1");
    }

    #[test]
    fn extracts_user_id_from_legacy_and_current_nexusphp_cookies() {
        let legacy_uid = base64::engine::general_purpose::STANDARD.encode("42");
        assert_eq!(
            extract_user_id_from_cookie(&format!("c_secure_uid={legacy_uid}; session=ok"))
                .as_deref(),
            Some("42")
        );

        let token = base64::engine::general_purpose::STANDARD
            .encode(r#"{"user_id":314,"expires":1999999999}.signature"#);
        let token = urlencoding::encode(&token);
        assert_eq!(
            extract_user_id_from_cookie(&format!("c_secure_pass={token}; c_lang_folder=chs"))
                .as_deref(),
            Some("314")
        );
    }

    #[test]
    fn recognizes_qingwa_style_login_form() {
        let url = reqwest::Url::parse("https://www.qingwapt.com/index.php").unwrap();
        let html = r#"<form id="form-login" method="post" action="takelogin.php">
            <input name="password" type="password">
        </form>"#;
        assert!(looks_like_login_page(html, &url));
    }

    #[test]
    fn extracts_username_without_requiring_a_profile_anchor() {
        assert_eq!(
            extract_username(
                r#"<div id="info_block"><span class="User_Name"><b>Alice</b></span></div>"#
            )
            .as_deref(),
            Some("Alice")
        );
    }

    #[tokio::test]
    async fn html_stats_do_not_require_a_current_user_anchor_or_api_probe() {
        let api_hits = Arc::new(AtomicUsize::new(0));
        let api_hits_for_route = Arc::clone(&api_hits);
        let app = Router::new()
            .route(
                "/index.php",
                get(|| async {
                    Html(
                        r#"<html><body>
                            <div id="info_block">
                              <span class="User_Name">Alice</span>
                              上传量 1 GiB 下载量 512 MiB 分享率 2.0
                            </div>
                        </body></html>"#,
                    )
                }),
            )
            .route("/mybonus.php", get(|| async { Html("<html></html>") }))
            .route(
                "/api/user",
                get(move || async move {
                    api_hits_for_route.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NOT_FOUND
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let adapter = NexusPhpAdapter::new(
            format!("http://{address}"),
            SiteAuth::Cookie {
                cookie: "session=valid".to_string(),
            },
            HeaderMap::new(),
            Client::new(),
        );
        let stats = adapter.get_user_stats().await.unwrap();

        assert_eq!(stats.username, "Alice");
        assert_eq!(stats.uploaded, 1_073_741_824);
        assert_eq!(stats.downloaded, 536_870_912);
        assert_eq!(api_hits.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[test]
    fn detects_free_and_hr_from_detail_html() {
        let attrs = NexusPhpAdapter::detect_torrent_attributes(
            r#"<html><body><span>FREE</span><span>2XUP</span><span>H&amp;R</span></body></html>"#,
        );

        assert!(attrs.free);
        assert!(attrs.hit_and_run);
        assert_eq!(attrs.download_volume_factor, Some(0.0));
        assert_eq!(attrs.upload_volume_factor, Some(2.0));
    }

    #[test]
    fn detects_free_end_time_from_detail_html() {
        let attrs = NexusPhpAdapter::detect_torrent_attributes(
            r#"<html><body><span>FREE</span><span>Free到期：2026-04-16 12:30:00</span></body></html>"#,
        );

        assert!(attrs.free_end_timestamp.is_some());
    }

    #[test]
    fn parses_pt_depiler_style_nexusphp_user_pages() {
        let html = r#"
            <html><body>
              <div id="info_block">
                <a class="User_Name" href="/userdetails.php?id=42"><b>Alice</b></a>
              </div>
              <table>
                <tr><td class="rowhead">传输</td><td>上传量 1.5 TiB 下载量 256 GiB 分享率 6.0</td></tr>
                <tr><td class="rowhead">魔力值</td><td>12,345.67</td></tr>
                <tr><td class="rowhead">当前做种</td><td>88</td></tr>
              </table>
            </body></html>
        "#;
        let identity = extract_current_user(html).unwrap();
        assert_eq!(identity.uid, "42");
        assert_eq!(identity.username.as_deref(), Some("Alice"));
        assert_eq!(identity.href, "/userdetails.php?id=42");

        let text = extract_visible_text(html);
        assert_eq!(
            parse_labeled_size(&text, &["上传量"]),
            Some(1_649_267_441_664)
        );
        assert_eq!(
            parse_labeled_size(&text, &["下载量"]),
            Some(274_877_906_944)
        );
        assert_eq!(parse_labeled_number(&text, &["分享率"]), Some(6.0));
        assert_eq!(parse_labeled_number(&text, &["魔力值"]), Some(12_345.67));
        assert_eq!(parse_labeled_integer(&text, &["当前做种"]), Some(88));
    }

    #[test]
    fn parses_english_and_traditional_transfer_labels() {
        let text = "Transfers Uploaded: 2.25 TB Downloaded: 512.5 GB 分享率: 4.49";
        assert_eq!(
            parse_labeled_size(text, &["Uploaded"]),
            Some(2_473_901_162_496)
        );
        assert_eq!(
            parse_labeled_size(text, &["Downloaded"]),
            Some(550_292_684_800)
        );

        let traditional = "傳送 上傳量 10 GiB 下載量 2 GiB";
        assert_eq!(
            parse_labeled_size(traditional, &["上傳量"]),
            Some(10_737_418_240)
        );
        assert_eq!(
            parse_labeled_size(traditional, &["下載量"]),
            Some(2_147_483_648)
        );
    }

    #[test]
    fn parses_nexusphp_seeding_ajax_summaries_and_tables() {
        assert_eq!(
            parse_seeding_ajax_count("<div><b>1,234</b> 条记录</div>"),
            Some(1234)
        );
        assert_eq!(
            parse_seeding_ajax_count(
                "<div>56 | 1.2 TiB</div><table><tr><th>标题</th></tr></table>"
            ),
            Some(56)
        );
        assert_eq!(parse_seeding_ajax_count("<div>No records.</div>"), Some(0));
        assert_eq!(
            parse_seeding_ajax_count(
                "<table><tr><th>标题</th></tr><tr><td>A</td></tr><tr><td>B</td></tr></table>"
            ),
            Some(2)
        );
        assert_eq!(
            parse_user_torrent_ajax_summary(
                "<div>56 | 1.2 TiB</div><table><tr><th>标题</th></tr></table>"
            ),
            Some((56, Some(1_319_413_953_331)))
        );
        assert_eq!(
            parse_user_torrent_ajax_summary(
                "<table><tr><th>标题</th><th>大小</th></tr><tr><td>A</td><td>1 GiB</td></tr><tr><td>B</td><td>512 MiB</td></tr></table>"
            ),
            Some((2, Some(1_610_612_736)))
        );
    }

    #[test]
    fn parses_extended_pt_depiler_nexusphp_fields() {
        let html = r#"
            <html><body>
              <div id="info_block">
                <a href="messages.php">消息 (3)</a>
                <a href="myhr.php">H&amp;R: 2/1/5</a>
              </div>
              <h1><img src="pic/flag/donor.gif" alt="Donor"></h1>
              <img class="avatar" src="avatars/42.png">
              <table>
                <tr><td class="rowhead">等级</td><td><img title="Elite User"></td></tr>
                <tr><td class="rowhead">加入日期</td><td>2024-01-02 03:04:05 (100 weeks ago)</td></tr>
                <tr><td class="rowhead">最近动向</td><td>2026-09-05 12:00:00 (now)</td></tr>
              </table>
            </body></html>
        "#;
        assert_eq!(
            parse_table_labeled_value(html, &["等级"]).as_deref(),
            Some("Elite User")
        );
        assert_eq!(parse_message_count(html), Some(3));
        assert_eq!(
            parse_hnr_counts(&extract_visible_text(html)),
            (Some(2), Some(1))
        );
        assert_eq!(
            parse_hnr_counts("H&R: 电影区 2/1/5 剧集区 3/4/10"),
            (Some(5), Some(5))
        );
        assert!(detect_donor(html));
        assert_eq!(extract_avatar(html).as_deref(), Some("avatars/42.png"));
        assert_eq!(
            parse_user_datetime_millis("2026-09-05 12:00:00 (now)"),
            Some(1_788_580_800_000)
        );
        assert_eq!(
            parse_labeled_duration_seconds("平均做种时间：2.5 days", &["平均做种时间"]),
            Some(216_000)
        );
    }
}
