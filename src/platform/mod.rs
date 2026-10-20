pub mod board;
pub mod network;
pub mod packages;

use crate::config::PlatformConfig;
use board::BoardInfo;
use network::{detect_backend, NetworkBackend};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PlatformInfo {
    pub board: BoardInfo,
    pub network_backend: NetworkBackend,
}

impl PlatformInfo {
    pub fn detect(config: &PlatformConfig) -> Self {
        Self {
            board: BoardInfo::detect(&config.profile),
            network_backend: detect_backend(&config.network_backend),
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "{}\nRed: {}",
            self.board.summary(),
            self.network_backend.label()
        )
    }
}
