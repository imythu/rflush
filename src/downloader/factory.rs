use super::{DownloaderClient, DownloaderRecord, DownloaderType, qbittorrent};

pub fn create_client(record: &DownloaderRecord) -> Result<Box<dyn DownloaderClient>, String> {
    let downloader_type = DownloaderType::from_str(&record.downloader_type)
        .ok_or_else(|| format!("不支持的下载器类型: {}", record.downloader_type))?;

    Ok(match downloader_type {
        DownloaderType::QBittorrent => Box::new(qbittorrent::QBittorrentClient::new(
            record.url.clone(),
            record.username.clone(),
            record.password.clone(),
        )),
    })
}
