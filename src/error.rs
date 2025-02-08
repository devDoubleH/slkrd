use std::fmt;

#[derive(Debug)]
pub enum SlkrdError {
    Io(std::io::Error),
    WebRTC(webrtc::Error),
    Network(String),
    InvalidPasscode,
    TransferFailed(String),
}

impl std::error::Error for SlkrdError {}

impl fmt::Display for SlkrdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlkrdError::Io(e) => write!(f, "IO error: {}", e),
            SlkrdError::WebRTC(e) => write!(f, "WebRTC error: {}", e),
            SlkrdError::Network(msg) => write!(f, "Network error: {}", msg),
            SlkrdError::InvalidPasscode => write!(f, "Invalid passcode format"),
            SlkrdError::TransferFailed(msg) => write!(f, "Transfer failed: {}", msg),
        }
    }
}