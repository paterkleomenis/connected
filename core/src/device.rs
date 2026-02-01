use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Represents the type/platform of a connected device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceType {
    Android,
    Linux,
    Windows,
    /// Reserved for future macOS support. Not currently implemented but kept
    /// for forward compatibility with the protocol and stored device data.
    MacOS,
    Unknown,
}

impl DeviceType {
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

impl std::str::FromStr for DeviceType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "android" => Ok(DeviceType::Android),
            "linux" => Ok(DeviceType::Linux),
            "windows" => Ok(DeviceType::Windows),
            "macos" => Ok(DeviceType::MacOS),
            _ => Ok(DeviceType::Unknown),
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
