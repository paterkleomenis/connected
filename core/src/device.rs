use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceType {
    Android,
    Linux,
    Windows,
    MacOS,
    Unknown,
}

impl DeviceType {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "android" => DeviceType::Android,
            "linux" => DeviceType::Linux,
            "windows" => DeviceType::Windows,
            "macos" => DeviceType::MacOS,
            _ => DeviceType::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceType::Android => "android",
            DeviceType::Linux => "linux",
            DeviceType::Windows => "windows",
            DeviceType::MacOS => "macos",
            DeviceType::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub device_type: DeviceType,
}

impl Device {
    pub fn new(id: String, name: String, ip: IpAddr, port: u16, device_type: DeviceType) -> Self {
        Self {
            id,
            name,
            ip: ip.to_string(),
            port,
            device_type,
        }
    }

    pub fn ip_addr(&self) -> Option<IpAddr> {
        self.ip.parse().ok()
    }
}
