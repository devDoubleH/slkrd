use std::env;
use std::fs::File;
use std::io::{self, Read, Write, Seek, ErrorKind};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process;
use std::time::Duration;
use rand::Rng;
use socket2::Socket;

const BUFFER_SIZE: usize = 64 * 1024; // 64KB
const DISCOVERY_PORT: u16 = 45678;
const PASSCODE_LENGTH: usize = 6;
const MAX_RETRIES: u32 = 3;
const TCP_KEEPALIVE_DURATION: Duration = Duration::from_secs(60);

#[derive(Debug)]
enum FileTransferError {
    FileNotFound,
    InvalidPasscode,
    ConnectionFailed,
    TransferError,
    FileExists,
    Timeout,
    IncompleteTransfer(u64, u64),
}

impl From<io::Error> for FileTransferError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            ErrorKind::NotFound => FileTransferError::FileNotFound,
            ErrorKind::InvalidInput => FileTransferError::InvalidPasscode,
            ErrorKind::TimedOut | ErrorKind::WouldBlock => FileTransferError::Timeout,
            ErrorKind::AlreadyExists => FileTransferError::FileExists,
            _ => FileTransferError::TransferError,
        }
    }
}

fn main() -> Result<(), FileTransferError> {
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
            FileTransferError::FileNotFound => eprintln!("Error: File not found"),
            FileTransferError::InvalidPasscode => eprintln!("Error: Invalid passcode format"),
            FileTransferError::ConnectionFailed => eprintln!("Error: Connection failed"),
            FileTransferError::TransferError => eprintln!("Error: Transfer failed"),
            FileTransferError::FileExists => eprintln!("Error: File already exists at destination"),
            FileTransferError::Timeout => eprintln!("Error: Connection timed out"),
            FileTransferError::IncompleteTransfer(received, expected) => eprintln!(
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

fn validate_passcode(passcode: &str) -> Result<(), FileTransferError> {
    if passcode.len() != PASSCODE_LENGTH || !passcode.chars().all(|c| c.is_ascii_alphanumeric()) {
        Err(FileTransferError::InvalidPasscode)
    } else {
        Ok(())
    }
}

fn send_file(file_path: &str) -> Result<(), FileTransferError> {
    let path = Path::new(file_path);
    if !path.exists() {
        return Err(FileTransferError::FileNotFound);
    }

    let passcode = generate_passcode();
    println!("Generated passcode: {}", passcode);

    let listener = TcpListener::bind(("0.0.0.0", DISCOVERY_PORT))?;
    println!("Waiting for receiver... (Port: {})", DISCOVERY_PORT);

    let mut file = File::open(file_path)?;
    let file_size = file.metadata()?.len();
    let filename = path.file_name().unwrap().to_string_lossy().to_string();

    let mut retries = 0;
    while retries < MAX_RETRIES {
        match listener.accept() {
            Ok((mut stream, _)) => {
                println!("Receiver connected. Validating passcode...");
                
                // Receive passcode from client
                let mut received_passcode = [0u8; PASSCODE_LENGTH];
                stream.read_exact(&mut received_passcode)?;
                
                if passcode.as_bytes() != &received_passcode {
                    println!("Invalid passcode received. Waiting for new connection...");
                    continue;
                }

                // Send filename
                stream.write_all(filename.len().to_le_bytes().as_ref())?;
                stream.write_all(filename.as_bytes())?;

                println!("Starting file transfer...");
                match transfer_file(&mut stream, &mut file, file_size) {
                    Ok(()) => return Ok(()),
                    Err(FileTransferError::Timeout) => {
                        eprintln!("Transfer timed out, retrying... ({}/{})", retries + 1, MAX_RETRIES);
                        retries += 1;
                        file.seek(std::io::SeekFrom::Start(0))?;
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(e) => {
                eprintln!("Connection error: {}", e);
                retries += 1;
                if retries >= MAX_RETRIES {
                    return Err(FileTransferError::ConnectionFailed);
                }
            }
        }
    }

    Err(FileTransferError::Timeout)
}

fn receive_file(passcode: &str) -> Result<(), FileTransferError> {
    validate_passcode(passcode)?;

    println!("Connecting to sender...");
    let mut stream = TcpStream::connect(("localhost", DISCOVERY_PORT))
        .or_else(|_| TcpStream::connect(("127.0.0.1", DISCOVERY_PORT)))?;

    // Send passcode
    stream.write_all(passcode.as_bytes())?;

    // Receive filename
    let mut filename_len = [0u8; 8];
    stream.read_exact(&mut filename_len)?;
    let filename_len = usize::from_le_bytes(filename_len);
    
    let mut filename_bytes = vec![0u8; filename_len];
    stream.read_exact(&mut filename_bytes)?;
    let filename = String::from_utf8_lossy(&filename_bytes).to_string();

    receive_and_save_file(&mut stream, &filename)
}

fn transfer_file(stream: &mut TcpStream, file: &mut File, file_size: u64) -> Result<(), FileTransferError> {
    configure_tcp_stream(stream)?;

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
            print_progress(transferred, file_size, start_time);
            last_update = now;
        }
    }

    if transferred != file_size {
        return Err(FileTransferError::IncompleteTransfer(transferred, file_size));
    }

    println!("\nTransfer complete! Total time: {:.1}s", start_time.elapsed().as_secs_f64());
    Ok(())
}

fn receive_and_save_file(stream: &mut TcpStream, filename: &str) -> Result<(), FileTransferError> {
    if Path::new(filename).exists() {
        return Err(FileTransferError::FileExists);
    }

    configure_tcp_stream(stream)?;

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
            print_progress(received, file_size, start_time);
            last_update = now;
        }
    }

    if received != file_size {
        return Err(FileTransferError::IncompleteTransfer(received, file_size));
    }

    println!("\nTransfer complete! Total time: {:.1}s", start_time.elapsed().as_secs_f64());
    Ok(())
}

fn configure_tcp_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(600)))?;
    stream.set_write_timeout(Some(Duration::from_secs(600)))?;

    let socket = Socket::from(stream.try_clone()?);
    socket.set_keepalive(true)?;
    socket.set_tcp_keepalive(&socket2::TcpKeepalive::new().with_time(TCP_KEEPALIVE_DURATION))?;

    Ok(())
}

fn print_progress(current: u64, total: u64, start_time: std::time::Instant) {
    let elapsed = start_time.elapsed().as_secs_f64();
    let speed = current as f64 / elapsed;
    let remaining = (total - current) as f64 / speed;

    print!("\rProgress: {:.1}% ({} / {}) - {}/s - ETA: {:.0}s",
        (current as f64 / total as f64) * 100.0,
        format_size(current),
        format_size(total),
        format_size(speed as u64),
        remaining.ceil()
    );
    let _ = io::stdout().flush();
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