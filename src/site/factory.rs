use super::{SiteAdapter, SiteAuth, SiteRecord, SiteType, mteam, nexusphp};

pub fn create_adapter(record: &SiteRecord) -> Result<Box<dyn SiteAdapter>, String> {
    let site_type = SiteType::from_str(&record.site_type)
        .ok_or_else(|| format!("不支持的站点类型: {}", record.site_type))?;
    let auth = serde_json::from_str::<SiteAuth>(&record.auth_config)
        .map_err(|e| format!("认证配置解析失败: {}", e))?;

    Ok(match site_type {
        SiteType::NexusPhp => Box::new(nexusphp::NexusPhpAdapter::new(
            record.base_url.clone(),
            auth,
        )),
        SiteType::MTeam => Box::new(mteam::MTeamAdapter::new(record.base_url.clone(), auth)),
    })
}
