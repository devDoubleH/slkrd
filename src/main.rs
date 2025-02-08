use std::env;
use std::fs::File;
use std::io::{self, Read, Write, ErrorKind};
use std::net::{UdpSocket, TcpListener, TcpStream};
use std::path::Path;
use std::process;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use rand::Rng;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::RTCPeerConnection;
use tokio::runtime::Runtime;
use bytes::Bytes;

const MAX_RETRIES: u32 = 3;
const BUFFER_SIZE: usize = 1024 * 1024;
const DISCOVERY_PORT: u16 = 45678;
const TRANSFER_PORT: u16 = 45679;
const PASSCODE_LENGTH: usize = 6;

#[derive(Debug)]
enum SlkrdError {
    FileNotFound,
    InvalidPasscode,
    ConnectionFailed,
    TransferError,
    FileExists,
    Timeout,
    IncompleteTransfer(u64, u64),
}

impl From<io::Error> for SlkrdError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            ErrorKind::NotFound => SlkrdError::FileNotFound,
            ErrorKind::InvalidInput => SlkrdError::InvalidPasscode,
            ErrorKind::TimedOut | ErrorKind::WouldBlock => SlkrdError::Timeout,
            ErrorKind::AlreadyExists => SlkrdError::FileExists,
            _ => SlkrdError::TransferError,
        }
    }
}

fn main() -> Result<(), SlkrdError> {
    let args: Vec<String> = env::args().collect();

    let result = match args.len() {
        3 => match args[1].as_str() {
            "-s" => send_file(&args[2]),
            "-r" => receive_file(&args[2]),
            _ => {
                print_usage();
                Ok(())
            }
        },
        _ => {
            print_usage();
            Ok(())
        }
    };

    if let Err(e) = &result {
        match e {
            SlkrdError::FileNotFound => eprintln!("Error: File not found"),
            SlkrdError::InvalidPasscode => eprintln!("Error: Invalid passcode format"),
            SlkrdError::ConnectionFailed => eprintln!("Error: Connection failed"),
            SlkrdError::TransferError => eprintln!("Error: Transfer failed"),
            SlkrdError::FileExists => eprintln!("Error: File already exists at destination"),
            SlkrdError::Timeout => eprintln!("Error: Connection timed out"),
            SlkrdError::IncompleteTransfer(received, expected) => eprintln!(
                "Error: Incomplete transfer (received {} of {})",
                format_size(*received),
                format_size(*expected)
            ),
        }
    }

    result
}

fn print_usage() {
    println!("Usage:");
    println!("  Send file:    slkrd -s <file_path>");
    println!("  Receive file: slkrd -r <passcode>");
    process::exit(1);
}

fn generate_passcode() -> String {
    const CHARSET: &[u8] = b"0123456789";
    let mut rng = rand::thread_rng();
    (0..PASSCODE_LENGTH)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

fn validate_passcode(passcode: &str) -> Result<(), SlkrdError> {
    if passcode.len() != PASSCODE_LENGTH || !passcode.chars().all(|c| c.is_ascii_digit()) {
        Err(SlkrdError::InvalidPasscode)
    } else {
        Ok(())
    }
}

struct WebRTCChannel {
    peer_connection: Arc<RTCPeerConnection>,
    data_channel: Arc<RTCDataChannel>,
    received_data: Arc<Mutex<Vec<u8>>>,
    runtime: Runtime,
}

impl WebRTCChannel {
    fn new() -> Result<Self, SlkrdError> {
        let runtime = Runtime::new().map_err(|_| SlkrdError::ConnectionFailed)?;
        let config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                ..Default::default()
            }],
            ..Default::default()
        };

        let api = APIBuilder::new().build();
        let peer_connection = Arc::new(
            runtime.block_on(api.new_peer_connection(config))
                .map_err(|_| SlkrdError::ConnectionFailed)?
        );

        // Create offer
        let offer = runtime.block_on(peer_connection.create_offer(None))
            .map_err(|_| SlkrdError::ConnectionFailed)?;

        // Set local description
        runtime.block_on(peer_connection.set_local_description(offer.clone()))
            .map_err(|_| SlkrdError::ConnectionFailed)?;

        let data_channel = runtime.block_on(peer_connection.create_data_channel(
            "data",
            None,
        ))
        .map_err(|_| SlkrdError::ConnectionFailed)?;

        let received_data = Arc::new(Mutex::new(Vec::new()));
        let received_data_clone = received_data.clone();

        let dc = Arc::new(data_channel.clone());
        runtime.block_on(async {
            dc.on_message(Box::new(move |msg: DataChannelMessage| {
                let mut data = received_data_clone.lock().unwrap();
                data.extend_from_slice(&msg.data);
                Box::pin(async {})
            }));
        });

        // Handle ICE candidate gathering
        let pc = peer_connection.clone();
        runtime.block_on(async {
            pc.on_ice_candidate(Box::new(move |candidate_opt| {
                if let Some(candidate) = candidate_opt {
                    println!("New ICE candidate: {}", candidate.to_string());
                }
                Box::pin(async {})
            }));
        });

        Ok(WebRTCChannel {
            peer_connection,
            data_channel,
            received_data,
            runtime,
        })
    }

    fn send(&self, data: &[u8]) -> Result<(), SlkrdError> {
        self.runtime
            .block_on(self.data_channel.send(&Bytes::from(data.to_vec())))
            .map(|_| ())
            .map_err(|_| SlkrdError::TransferError)
    }

    fn receive(&self) -> Result<Vec<u8>, SlkrdError> {
        let mut data = self.received_data.lock().unwrap();
        let result = data.clone();
        data.clear();
        Ok(result)
    }
}

use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

fn send_file(file_path: &str) -> Result<(), SlkrdError> {
    let path = Path::new(file_path);
    if !path.exists() {
        return Err(SlkrdError::FileNotFound);
    }

    let passcode = generate_passcode();
    println!("Generated passcode: {}", passcode);

    let discovery_socket = UdpSocket::bind("0.0.0.0:0")?;
    discovery_socket.set_broadcast(true)?;
    discovery_socket.set_read_timeout(Some(Duration::from_secs(30)))?;

    let listener = TcpListener::bind(("0.0.0.0", TRANSFER_PORT))?;
    listener.set_nonblocking(true)?;

    println!("Waiting for receiver...");

    let mut file = File::open(file_path)?;
    let file_size = file.metadata()?.len();
    let filename = path.file_name().unwrap().to_string_lossy().to_string();

    let mut retries = 0;
    while retries < MAX_RETRIES {
        let message = format!("SLKRD:{}:{}", passcode, filename);
        discovery_socket.send_to(message.as_bytes(), ("255.255.255.255", DISCOVERY_PORT))?;

        match listener.accept() {
            Ok((mut stream, _)) => {
                println!("Receiver connected. Starting transfer...");
                let channel = WebRTCChannel::new()?;
                
                // Send file size first
                stream.write_all(&file_size.to_le_bytes())?;
                
                // Exchange WebRTC signaling
                let offer = channel.runtime.block_on(channel.peer_connection.create_offer(None))
                    .map_err(|_| SlkrdError::ConnectionFailed)?;
                let offer_sdp = serde_json::to_string(&offer)
                    .map_err(|_| SlkrdError::ConnectionFailed)?;
                stream.write_all(offer_sdp.as_bytes())?;

                // Receive answer
                let mut answer_buf = Vec::new();
                stream.read_to_end(&mut answer_buf)?;
                let answer: RTCSessionDescription = serde_json::from_slice(&answer_buf)
                    .map_err(|_| SlkrdError::ConnectionFailed)?;
                channel.runtime.block_on(channel.peer_connection.set_remote_description(answer))
                    .map_err(|_| SlkrdError::ConnectionFailed)?;

                let mut buffer = vec![0; BUFFER_SIZE];
                let mut transferred = 0;
                
                while transferred < file_size {
                    let n = file.read(&mut buffer)?;
                    if n == 0 { break; }
                    channel.send(&buffer[..n])?;
                    transferred += n as u64;
                    print_progress(transferred, file_size);
                }
                
                if transferred == file_size {
                    println!("\nTransfer complete!");
                    return Ok(());
                }
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(_) => {
                retries += 1;
                println!("Connection attempt failed. Retrying... ({}/{})", retries, MAX_RETRIES);
                continue;
            }
        }
    }
    
    Err(SlkrdError::ConnectionFailed)
}

fn receive_file(passcode: &str) -> Result<(), SlkrdError> {
    validate_passcode(passcode)?;

    let socket = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))?;
    println!("Searching for sender...");

    let (sender_addr, filename) = {
        let mut buf = [0u8; 1024];
        let mut sender_found = None;
        
        while sender_found.is_none() {
            match socket.recv_from(&mut buf) {
                Ok((size, addr)) => {
                    let message = String::from_utf8_lossy(&buf[..size]);
                    if let Some(content) = message.strip_prefix("SLKRD:") {
                        let parts: Vec<&str> = content.split(':').collect();
                        if parts.len() == 2 && parts[0] == passcode {
                            sender_found = Some((addr, parts[1].to_string()));
                            break;
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(_) => return Err(SlkrdError::ConnectionFailed),
            }
        }
        
        sender_found.ok_or(SlkrdError::ConnectionFailed)?
    };
    
    if Path::new(&filename).exists() {
        return Err(SlkrdError::FileExists);
    }

    let mut stream = TcpStream::connect((sender_addr.ip(), TRANSFER_PORT))?;
    let channel = WebRTCChannel::new()?;
    
    // Read file size first
    let mut size_bytes = [0u8; 8];
    stream.read_exact(&mut size_bytes)?;
    let file_size = u64::from_le_bytes(size_bytes);

    // Receive offer
    let mut offer_buf = Vec::new();
    stream.read_to_end(&mut offer_buf)?;
    let offer: RTCSessionDescription = serde_json::from_slice(&offer_buf)
        .map_err(|_| SlkrdError::ConnectionFailed)?;
    channel.runtime.block_on(channel.peer_connection.set_remote_description(offer))
        .map_err(|_| SlkrdError::ConnectionFailed)?;

    // Create and send answer
    let answer = channel.runtime.block_on(channel.peer_connection.create_answer(None))
        .map_err(|_| SlkrdError::ConnectionFailed)?;
    channel.runtime.block_on(channel.peer_connection.set_local_description(answer.clone()))
        .map_err(|_| SlkrdError::ConnectionFailed)?;
    let answer_sdp = serde_json::to_string(&answer)
        .map_err(|_| SlkrdError::ConnectionFailed)?;
    stream.write_all(answer_sdp.as_bytes())?;

    let mut file = File::create(&filename)?;
    let mut received = 0;

    while received < file_size {
        let data = channel.receive()?;
        if data.is_empty() { break; }
        
        file.write_all(&data)?;
        received += data.len() as u64;
        print_progress(received, file_size);
    }

    if received != file_size {
        return Err(SlkrdError::IncompleteTransfer(received, file_size));
    }

    println!("\nTransfer complete!");
    Ok(())
}

fn print_progress(current: u64, total: u64) {
    print!("\rProgress: {:.1}% ({} / {})", 
        (current as f64 / total as f64) * 100.0,
        format_size(current),
        format_size(total)
    );
    io::stdout().flush().unwrap();
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", size, UNITS[unit_index])
    } else {
        format!("{:.2} {}", size, UNITS[unit_index])
    }
}