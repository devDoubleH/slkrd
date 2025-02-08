use clap::{Arg, Command};
use tokio::io::AsyncWriteExt;
use tokio::io::AsyncReadExt;
use indicatif::{ProgressBar, ProgressStyle};
use rand::Rng;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tokio::net::TcpListener;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use serde::{Serialize, Deserialize};

const CHUNK_SIZE: usize = 65536; // 64KB chunks for efficient transfer
const PASSCODE_LENGTH: usize = 6;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = Command::new("slkrd")
        .version("1.0")
        .author("Your Name")
        .about("P2P file sharing tool")
        .arg(
            Arg::new("send")
                .short('s')
                .value_name("FILE")
                .help("Send a file"),
        )
        .arg(
            Arg::new("receive")
                .short('r')
                .value_name("PASSCODE")
                .help("Receive a file with passcode"),
        )
        .get_matches();

    if let Some(file_path) = matches.get_one::<String>("send") {
        sender_mode(file_path).await?;
    } else if let Some(passcode) = matches.get_one::<String>("receive") {
        receiver_mode(passcode).await?;
    } else {
        println!("Please use -s to send or -r to receive");
    }

    Ok(())
}

async fn sender_mode(file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Validate file exists
    let path = Path::new(file_path);
    if !path.exists() {
        return Err("File not found".into());
    }

    // Generate random 6-digit passcode
    let passcode = generate_passcode();
    println!("Your passcode is: {}", passcode);

    // Create WebRTC peer connection
    let api = APIBuilder::new().build();
    let config = RTCConfiguration::default();
    let peer_connection = api.new_peer_connection(config).await?;

    // Create data channel
    let dc = peer_connection
        .create_data_channel(
            "fileTransfer",
            Some(RTCDataChannelInit {
                ordered: Some(true),
                ..Default::default()
            }),
        )
        .await?;

    // Set up TCP listener for signaling
    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    println!("Waiting for receiver to connect...");

    let (mut socket, _) = listener.accept().await?;
    
    // Handle connection state changes
    let pc_clone = peer_connection.close();
    peer_connection.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        println!("Connection State: {:?}", s);
        Box::pin(async {})
    }));

    // Create offer
    let offer = peer_connection.create_offer(None).await?;
    peer_connection.set_local_description(offer.clone()).await?;

    // Send offer to receiver
    let offer_str = serde_json::to_string(&offer)?;
    socket.write_all(offer_str.as_bytes()).await?;

    // Get answer from receiver
    let mut buffer = Vec::new();
    socket.read_to_end(&mut buffer).await?;
    let answer: RTCSessionDescription = serde_json::from_slice(&buffer)?;
    peer_connection.set_remote_description(answer).await?;

    // Send file when data channel opens
    let file_path = file_path.to_string();
    let dc_clone = dc.clone();
    dc.on_open(Box::new(move || {
        let dc = dc_clone;
        Box::pin(async move {
            send_file(&dc, &file_path).await.unwrap();
        })
    }));

    // Keep connection alive
    tokio::signal::ctrl_c().await?;
    peer_connection.close().await?;

    Ok(())
}

async fn receiver_mode(passcode: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Validate passcode format
    if passcode.len() != PASSCODE_LENGTH || !passcode.chars().all(|c| c.is_digit(10)) {
        return Err("Invalid passcode format".into());
    }

    // Create WebRTC peer connection
    let api = APIBuilder::new().build();
    let config = RTCConfiguration::default();
    let peer_connection = api.new_peer_connection(config).await?;

    // Connect to sender
    let mut socket = tokio::net::TcpStream::connect("127.0.0.1:3000").await?;

    // Handle data channel
    peer_connection.on_data_channel(Box::new(|dc| {
        println!("New Data Channel: {}", dc.label());
        
        // Set up progress bar
        let pb = ProgressBar::new(0);
        pb.set_style(ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"));

        dc.on_message(Box::new(move |msg| {
            let pb = pb.clone();
            Box::pin(async move {
                handle_received_data(&msg.data, &pb).await.unwrap();
            })
        }));

        Box::pin(async {})
    }));

    // Get offer from sender
    let mut buffer = Vec::new();
    socket.read_to_end(&mut buffer).await?;
    let offer: RTCSessionDescription = serde_json::from_slice(&buffer)?;
    peer_connection.set_remote_description(offer).await?;

    // Create answer
    let answer = peer_connection.create_answer(None).await?;
    peer_connection.set_local_description(answer.clone()).await?;

    // Send answer to sender
    let answer_str = serde_json::to_string(&answer)?;
    socket.write_all(answer_str.as_bytes()).await?;

    // Keep connection alive
    tokio::signal::ctrl_c().await?;
    peer_connection.close().await?;

    Ok(())
}

// Add this to your imports at the top
use bytes::Bytes;

async fn send_file(
    dc: &webrtc::data_channel::RTCDataChannel,
    file_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = File::open(file_path)?;
    let file_size = file.metadata()?.len();
    
    // Set up progress bar
    let pb = ProgressBar::new(file_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
        .unwrap()
        .progress_chars("#>-"));

    // Send file metadata
    let metadata = FileMetadata {
        name: Path::new(file_path).file_name().unwrap().to_str().unwrap().to_string(),
        size: file_size,
    };
    dc.send(&serde_json::to_vec(&metadata)?.into()).await?;

    // Send file in chunks
    let mut buffer = vec![0; CHUNK_SIZE];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        // Convert the buffer slice to Bytes before sending
        dc.send(&Bytes::copy_from_slice(&buffer[..n])).await?;
        pb.inc(n as u64);
    }

    pb.finish_with_message("File sent successfully");
    Ok(())
}

async fn handle_received_data(
    data: &Bytes,  // Changed from DataChannelMessage to Bytes
    pb: &ProgressBar,
) -> Result<(), Box<dyn std::error::Error>> {
    static mut FILE: Option<File> = None;
    static mut METADATA: Option<FileMetadata> = None;

    unsafe {
        if METADATA.is_none() {
            // First message contains metadata
            let metadata: FileMetadata = serde_json::from_slice(data.as_ref())?;  // Changed from data.data.as_slice()
            pb.set_length(metadata.size);
            FILE = Some(File::create(&metadata.name)?);
            METADATA = Some(metadata);
        } else {
            // Write file chunk
            if let Some(file) = &mut FILE {
                file.write_all(data.as_ref())?;  // Changed from data.data.as_slice()
                pb.inc(data.len() as u64);  // Changed from data.data.len()
                
                if pb.position() >= pb.length().unwrap() {
                    pb.finish_with_message("File received successfully");
                    FILE = None;
                    METADATA = None;
                }
            }
        }
    }

    Ok(())
}

fn generate_passcode() -> String {
    let mut rng = rand::thread_rng();
    (0..PASSCODE_LENGTH)
        .map(|_| rng.gen_range(0..10).to_string())
        .collect()
}

#[derive(Serialize, Deserialize)]
struct FileMetadata {
    name: String,
    size: u64,
}