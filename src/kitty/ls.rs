//! The parts of `kitty @ ls` that kitmux reads. Every field defaults, so a kitty
//! that adds, renames, or drops one does not break parsing; the full output is
//! kept alongside in each snapshot regardless.

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct OsWindow {
    pub id: u64,
    pub is_active: bool,
    pub is_focused: bool,
    pub wm_class: String,
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Tab {
    pub id: u64,
    pub title: String,
    pub is_active: bool,
    pub layout: String,
    pub layout_state: Value,
    pub windows: Vec<Window>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Window {
    pub id: u64,
    pub title: String,
    pub cwd: String,
    pub pid: u32,
    pub cmdline: Vec<String>,
    pub is_focused: bool,
    pub is_active: bool,
    pub foreground_processes: Vec<Process>,
    pub user_vars: Map<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Process {
    pub pid: u32,
    pub cmdline: Vec<String>,
    pub cwd: String,
}

pub fn parse(ls: &Value) -> serde_json::Result<Vec<OsWindow>> {
    Vec::<OsWindow>::deserialize(ls)
}
