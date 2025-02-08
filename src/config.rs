use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub chunk_size: usize,
    pub stun_servers: Vec<String>,
    pub turn_servers: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            chunk_size: 65536,
            stun_servers: vec!["stun:stun.l.google.com:19302".to_string()],
            turn_servers: vec![],
        }
    }
}
