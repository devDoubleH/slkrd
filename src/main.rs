use std::env;
use std::fs::File;
use std::io::{self, Read, Write, Seek, ErrorKind};
use std::net::{TcpListener, TcpStream, UdpSocket, SocketAddr};
use std::net::{SocketAddrV4};
use std::path::Path;
use std::process;
use std::time::Duration;
use rand::Rng;
use socket2::{Socket, Domain, Type, Protocol};

const BUFFER_SIZE: usize = 64 * 1024; // 64KB
const DISCOVERY_PORT: u16 = 45678;
const TRANSFER_PORT: u16 = 45679;
const PASSCODE_LENGTH: usize = 6;
const MAX_RETRIES: u32 = 3;
const TCP_KEEPALIVE_DURATION: Duration = Duration::from_secs(60);

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
    const CHARSET: &[u8] = b"1234567890";
    let mut rng = rand::thread_rng();
    (0..PASSCODE_LENGTH)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

fn validate_passcode(passcode: &str) -> Result<(), SlkrdError> {
    if passcode.len() != PASSCODE_LENGTH || !passcode.chars().all(|c| c.is_ascii_alphanumeric()) {
        Err(SlkrdError::InvalidPasscode)
    } else {
        Ok(())
    }
}

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

    broadcast_and_transfer(&discovery_socket, &listener, &passcode, &filename, &mut file, file_size)
}

fn receive_and_save_file(stream: &mut TcpStream, filename: &str) -> Result<(), SlkrdError> {
    if Path::new(filename).exists() {
        return Err(SlkrdError::FileExists);
    }

    // Set TCP keepalive
    let socket = Socket::from(stream.try_clone()?);
    socket.set_keepalive(true)?;
    socket.set_tcp_keepalive(&socket2::TcpKeepalive::new().with_time(TCP_KEEPALIVE_DURATION))?;

    stream.set_read_timeout(Some(Duration::from_secs(600)))?;
    stream.set_write_timeout(Some(Duration::from_secs(600)))?;

    let mut size_bytes = [0u8; 8];
    stream.read_exact(&mut size_bytes)?;
    let file_size = u64::from_le_bytes(size_bytes);

    let mut file = File::create(filename)?;
    let mut buffer = vec![0; BUFFER_SIZE];
    let mut received = 0;
    let start_time = std::time::Instant::now();
    let mut last_update = start_time;

    println!("Receiving file: {} ({})", filename, format_size(file_size));

    while received < file_size {
        let n = stream.read(&mut buffer)?;
        if n == 0 { break; }
        file.write_all(&buffer[..n])?;
        received += n as u64;

        let now = std::time::Instant::now();
        if now.duration_since(last_update).as_millis() >= 100 {
            let elapsed = now.duration_since(start_time).as_secs_f64();
            let speed = received as f64 / elapsed;
            let remaining = (file_size - received) as f64 / speed;

            print!("\rProgress: {:.1}% ({} / {}) - {}/s - ETA: {:.0}s",
                (received as f64 / file_size as f64) * 100.0,
                format_size(received),
                format_size(file_size),
                format_size(speed as u64),
                remaining.ceil()
            );
            io::stdout().flush()?;
            last_update = now;
        }
    }

    if received != file_size {
        return Err(SlkrdError::IncompleteTransfer(received, file_size));
    }

    println!("\nTransfer complete! Total time: {:.1}s", start_time.elapsed().as_secs_f64());
    Ok(())
}

fn transfer_file(stream: &mut TcpStream, file: &mut File, file_size: u64) -> Result<(), SlkrdError> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(600)))?;
    stream.set_write_timeout(Some(Duration::from_secs(600)))?;

    // Set TCP keepalive
    let socket = Socket::from(stream.try_clone()?);
    socket.set_keepalive(true)?;
    socket.set_tcp_keepalive(&socket2::TcpKeepalive::new().with_time(TCP_KEEPALIVE_DURATION))?;

    stream.write_all(&file_size.to_le_bytes())?;

    let mut buffer = vec![0; BUFFER_SIZE];
    let mut transferred = 0;
    let start_time = std::time::Instant::now();
    let mut last_update = start_time;

    println!("Starting transfer of {}", format_size(file_size));

    while transferred < file_size {
        let n = file.read(&mut buffer)?;
        if n == 0 { break; }
        stream.write_all(&buffer[..n])?;
        transferred += n as u64;

        let now = std::time::Instant::now();
        if now.duration_since(last_update).as_millis() >= 100 {
            let elapsed = now.duration_since(start_time).as_secs_f64();
            let speed = transferred as f64 / elapsed;
            let remaining = (file_size - transferred) as f64 / speed;

            print!("\rProgress: {:.1}% ({} / {}) - {}/s - ETA: {:.0}s",
                (transferred as f64 / file_size as f64) * 100.0,
                format_size(transferred),
                format_size(file_size),
                format_size(speed as u64),
                remaining.ceil()
            );
            io::stdout().flush()?;
            last_update = now;
        }
    }

    if transferred != file_size {
        return Err(SlkrdError::IncompleteTransfer(transferred, file_size));
    }

    println!("\nTransfer complete! Total time: {:.1}s", start_time.elapsed().as_secs_f64());
    Ok(())
}

fn broadcast_and_transfer(
    discovery_socket: &UdpSocket,
    listener: &TcpListener,
    passcode: &str,
    filename: &str,
    file: &mut File,
    file_size: u64,
) -> Result<(), SlkrdError> {
    let message = format!("SLKRD:{}:{}", passcode, filename);
    let mut retries = 0;

    while retries < MAX_RETRIES {
        // Get all network interfaces and their broadcast addresses
        let interfaces = match get_if_addrs::get_if_addrs() {
            Ok(interfaces) => interfaces,
            Err(e) => {
                eprintln!("Failed to get network interfaces: {}", e);
                // Fallback to global broadcast
                let _ = discovery_socket.send_to(message.as_bytes(), ("255.255.255.255", DISCOVERY_PORT));
                vec![]
            }
        };

        // Send broadcast to each valid interface
        for interface in interfaces {
            if interface.is_loopback() {
                continue;
            }
            if let get_if_addrs::IfAddr::V4(addr) = interface.addr {
                if let Some(broadcast) = addr.broadcast {
                    let target = SocketAddrV4::new(broadcast, DISCOVERY_PORT);
                    if let Err(e) = discovery_socket.send_to(message.as_bytes(), target) {
                        eprintln!("Failed to send to {}: {}", broadcast, e);
                    }
                }
            }
        }

        match listener.accept() {
            Ok((mut stream, _)) => {
                println!("Receiver connected. Starting transfer...");
                match transfer_file(&mut stream, file, file_size) {
                    Ok(()) => return Ok(()),
                    Err(SlkrdError::Timeout) => {
                        eprintln!("Transfer timed out, retrying... ({}/{})", retries + 1, MAX_RETRIES);
                        retries += 1;
                        file.seek(std::io::SeekFrom::Start(0))?;
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                eprintln!("Connection error: {}", e);
                return Err(SlkrdError::ConnectionFailed);
            }
        }
    }

    Err(SlkrdError::Timeout)
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_index])
}

fn receive_file(passcode: &str) -> Result<(), SlkrdError> {
    validate_passcode(passcode)?;

    let socket = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))?;
    println!("Searching for sender...");

    let (sender_addr, filename) = find_sender(&socket, passcode)?;

    let mut stream = TcpStream::connect((sender_addr.ip(), TRANSFER_PORT))?;
    receive_and_save_file(&mut stream, &filename)
}

fn find_sender(socket: &UdpSocket, target_passcode: &str) -> Result<(SocketAddr, String), SlkrdError> {
    let mut buf = [0; 1024];

    loop {
        match socket.recv_from(&mut buf) {
            Ok((size, addr)) => {
                let message = String::from_utf8_lossy(&buf[..size]);
                if let Some(data) = message.strip_prefix("SLKRD:") {
                    let parts: Vec<&str> = data.split(':').collect();
                    if parts.len() == 2 && parts[0] == target_passcode {
                        println!("Found sender. Connecting...");
                        return Ok((addr, parts[1].to_string()));
                    }
                }
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(_) => return Err(SlkrdError::ConnectionFailed),
        }
    }
}