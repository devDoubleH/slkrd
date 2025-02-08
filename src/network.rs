use std::sync::Arc;  // Add this import
use crate::error::SlkrdError;
use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::RTCPeerConnection;

pub struct NetworkManager {
    pub peer_connection: RTCPeerConnection,
    pub data_channel: Option<Arc<RTCDataChannel>>,
}

impl NetworkManager {
    pub async fn new(config: &crate::config::Config) -> Result<Self, SlkrdError> {
        let mut webrtc_config = RTCConfiguration::default();
        // Add STUN/TURN servers from config
        
        let api = APIBuilder::new().build();
        let peer_connection = api
            .new_peer_connection(webrtc_config)
            .await
            .map_err(SlkrdError::WebRTC)?;

        Ok(Self {
            peer_connection,
            data_channel: None,
        })
    }

    pub async fn create_data_channel(&mut self, label: &str) -> Result<(), SlkrdError> {
        let data_channel = self
            .peer_connection
            .create_data_channel(label, None)
            .await
            .map_err(SlkrdError::WebRTC)?;
        
        self.data_channel = Some(data_channel);
        Ok(())
    }
}