use crate::error::SlkrdError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct SignalingMessage {
    pub session_id: Uuid,
    pub passcode: String,
    pub message_type: SignalingMessageType,
    pub payload: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SignalingMessageType {
    Offer,
    Answer,
    IceCandidate,
}

pub struct SignalingServer {
    socket: UdpSocket,
    sessions: HashMap<String, SessionInfo>,
}

#[derive(Debug)]
struct SessionInfo {
    id: Uuid,
    sender_addr: SocketAddr,
    receiver_addr: Option<SocketAddr>,
}

impl SignalingServer {
    pub async fn new(bind_addr: &str) -> Result<Self, SlkrdError> {
        let socket = UdpSocket::bind(bind_addr)
            .await
            .map_err(|e| SlkrdError::Network(e.to_string()))?;
        
        Ok(Self {
            socket,
            sessions: HashMap::new(),
        })
    }

    pub async fn run(&mut self) -> Result<(), SlkrdError> {
        let mut buf = vec![0u8; 65536];
        
        loop {
            let (len, addr) = self
                .socket
                .recv_from(&mut buf)
                .await
                .map_err(|e| SlkrdError::Network(e.to_string()))?;

            let message: SignalingMessage = serde_json::from_slice(&buf[..len])
                .map_err(|e| SlkrdError::Network(e.to_string()))?;

            self.handle_message(message, addr).await?;
        }
    }

    async fn handle_message(
        &mut self,
        message: SignalingMessage,
        addr: SocketAddr,
    ) -> Result<(), SlkrdError> {
        match message.message_type {
            SignalingMessageType::Offer => {
                let session = SessionInfo {
                    id: message.session_id,
                    sender_addr: addr,
                    receiver_addr: None,
                };
                self.sessions.insert(message.passcode.clone(), session);
            }
            SignalingMessageType::Answer => {
                if let Some(session) = self.sessions.get_mut(&message.passcode) {
                    session.receiver_addr = Some(addr);
                    // Forward answer to sender
                    if let Err(e) = self
                        .socket
                        .send_to(&serde_json::to_vec(&message).unwrap(), session.sender_addr)
                        .await
                    {
                        return Err(SlkrdError::Network(e.to_string()));
                    }
                }
            }
            SignalingMessageType::IceCandidate => {
                if let Some(session) = self.sessions.get(&message.passcode) {
                    // Forward ICE candidate to the other peer
                    let target_addr = if addr == session.sender_addr {
                        session.receiver_addr
                    } else {
                        Some(session.sender_addr)
                    };

                    if let Some(target) = target_addr {
                        if let Err(e) = self
                            .socket
                            .send_to(&serde_json::to_vec(&message).unwrap(), target)
                            .await
                        {
                            return Err(SlkrdError::Network(e.to_string()));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}