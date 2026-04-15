use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

use clap::Parser;

use crate::error::AppError;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "rflush",
    version,
    about = "PT 刷流与 RSS 下载控制台",
    long_about = "启动 rflush Web 服务。\n\n默认行为:\n- 监听地址: 0.0.0.0:3000\n- 数据库: ./data/rflush.db\n- RSS 下载输出: 当前目录\n\n指定 --data-dir 后:\n- 数据库与下载输出都写入该目录\n\n示例:\n- rflush\n- rflush -H 127.0.0.1 -p 8080\n- rflush -d ./runtime-data\n- RFLUSH_DATA_DIR=/data rflush"
)]
pub struct Cli {
    #[arg(
        short = 'H',
        long,
        env = "RFLUSH_HOST",
        default_value = "0.0.0.0",
        help = "Web 服务监听地址 (env: RFLUSH_HOST)"
    )]
    pub host: String,

    #[arg(
        short = 'p',
        long,
        env = "RFLUSH_PORT",
        default_value_t = 3000,
        help = "Web 服务监听端口 (env: RFLUSH_PORT)"
    )]
    pub port: u16,

    #[arg(
        short = 'd',
        long = "data-dir",
        env = "RFLUSH_DATA_DIR",
        value_name = "DIR",
        help = "应用数据目录。指定后数据库和本地下载输出都写入该目录 (env: RFLUSH_DATA_DIR)"
    )]
    pub data_dir: Option<PathBuf>,
}

impl Cli {
    pub fn resolve_paths(&self, current_dir: &Path) -> (PathBuf, PathBuf) {
        if let Some(data_dir) = &self.data_dir {
            (data_dir.clone(), data_dir.clone())
        } else {
            (current_dir.to_path_buf(), current_dir.join("data"))
        }
    }

    pub fn resolve_listen_addr(&self) -> Result<SocketAddr, AppError> {
        let mut addrs = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|error| AppError::InvalidConfig {
                message: format!(
                    "invalid listen address {}:{}: {}",
                    self.host, self.port, error
                ),
            })?;

        addrs.next().ok_or_else(|| AppError::InvalidConfig {
            message: format!("no socket address resolved for {}:{}", self.host, self.port),
        })
    }
}
