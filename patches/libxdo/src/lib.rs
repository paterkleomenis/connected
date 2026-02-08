//! No-op stub for `libxdo` that provides the same public API without
//! linking to the system `libxdo.so`. This eliminates the runtime
//! dependency on `libxdo.so` which has different sonames across distros
//! (e.g. `libxdo.so.3` on Ubuntu vs `libxdo.so.4` on Arch).
//!
//! Only menu keyboard-accelerator simulation is lost; menus still work.

use std::fmt;

pub struct XDo {
    _private: (),
}

#[derive(Debug)]
pub enum CreationError {
    Unavailable,
}

impl fmt::Display for CreationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "libxdo stub: not available")
    }
}

impl std::error::Error for CreationError {}

#[derive(Debug)]
pub enum OpError {
    Failed,
}

impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "libxdo stub: operation not supported")
    }
}

impl std::error::Error for OpError {}

pub type OpResult = Result<(), OpError>;

impl XDo {
    pub fn new(_display: Option<&str>) -> Result<XDo, CreationError> {
        Err(CreationError::Unavailable)
    }

    pub fn move_mouse(&self, _x: i32, _y: i32, _screen: i32) -> OpResult {
        Err(OpError::Failed)
    }

    pub fn move_mouse_relative(&self, _x: i32, _y: i32) -> OpResult {
        Err(OpError::Failed)
    }

    pub fn click(&self, _button: i32) -> OpResult {
        Err(OpError::Failed)
    }

    pub fn mouse_down(&self, _button: i32) -> OpResult {
        Err(OpError::Failed)
    }

    pub fn mouse_up(&self, _button: i32) -> OpResult {
        Err(OpError::Failed)
    }

    pub fn enter_text(&self, _text: &str, _delay_microsecs: u32) -> OpResult {
        Err(OpError::Failed)
    }

    pub fn send_keysequence(&self, _sequence: &str, _delay_microsecs: u32) -> OpResult {
        Err(OpError::Failed)
    }

    pub fn send_keysequence_up(&self, _sequence: &str, _delay_microsecs: u32) -> OpResult {
        Err(OpError::Failed)
    }

    pub fn send_keysequence_down(&self, _sequence: &str, _delay_microsecs: u32) -> OpResult {
        Err(OpError::Failed)
    }
}
