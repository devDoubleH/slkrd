use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use socket2::{Socket, Domain, Type, Protocol};

const BUFFER_SIZE: usize = 64 * 1024; // 64 KB
const DISCOVERY_PORT: u16 = 45678;
const TRANSFER_PORT: u16 = 45679;
const MAX_RETRIES: u32 = 5;
const TCP_KEEPALIVE_TIME: Duration = Duration::from_secs(10);

#[derive(Debug)]
enum SlkrdError {
    FileNotFound,
    ConnectionFailed,
    TransferError,
    Timeout,
    IncompleteTransfer(u64, u64),
}

impl From<io::Error> for SlkrdError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => SlkrdError::FileNotFound,
            io::ErrorKind::TimedOut => SlkrdError::Timeout,
            _ => SlkrdError::TransferError,
        }
    }
}

fn main() -> Result<(), SlkrdError> {
    let args: Vec<String> = env::args().collect();

    if args.len() != 3 {
        println!("Usage:\n  Send: slkrd -s <file_path>\n  Receive: slkrd -r");
        return Ok(());
    }

    match args[1].as_str() {
        "-s" => send_file(&args[2]),
        "-r" => receive_file(),
        _ => {
            println!("Invalid option. Use -s to send or -r to receive.");
            Ok(())
        }
    }
}

/// **Fix 1: Correctly Bind TCP Listener on Windows/macOS**
fn create_tcp_listener(port: u16) -> Result<TcpListener, SlkrdError> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    socket.set_reuse_port(true)?;
    socket.bind(&std::net::SocketAddr::from(([0, 0, 0, 0], port)).into())?;
    socket.listen(1)?;
    Ok(socket.into())
}

fn send_file(file_path: &str) -> Result<(), SlkrdError> {
    let path = Path::new(file_path);
    if !path.exists() {
        return Err(SlkrdError::FileNotFound);
    }

    let listener = create_tcp_listener(TRANSFER_PORT)?;
    println!("Waiting for receiver to connect...");

    let mut file = File::open(file_path)?;
    let file_size = file.metadata()?.len();
    let start_time = Instant::now();

    for _ in 0..MAX_RETRIES {
        if let Ok((mut stream, addr)) = listener.accept() {
            println!("Receiver connected from {}. Sending file...", addr);
            return transfer_file(&mut stream, &mut file, file_size, start_time);
        }
        thread::sleep(Duration::from_secs(2));
    }

    Err(SlkrdError::Timeout)
}

fn receive_file() -> Result<(), SlkrdError> {
    let mut stream = TcpStream::connect(("0.0.0.0", TRANSFER_PORT))?;
    configure_socket(&stream)?;

    println!("Connected to sender. Receiving file...");

    let mut size_buf = [0; 8];
    stream.read_exact(&mut size_buf)?;
    let file_size = u64::from_le_bytes(size_buf);

    let filename = "received_file.bin";
    let mut file = File::create(filename)?;

    let mut received = 0;
    let mut buffer = vec![0; BUFFER_SIZE];
    let start_time = Instant::now();

    while received < file_size {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                file.write_all(&buffer[..n])?;
                received += n as u64;
                print_progress(received, file_size, start_time);
            }
            Err(e) => return Err(SlkrdError::TransferError),
        }
    }

    println!("\nTransfer complete!");
    stream.shutdown(std::net::Shutdown::Both)?; // **Fix 2: Proper Shutdown**
    Ok(())
}

/// **Fix 2: Ensure File Transfer Completes Correctly**
fn transfer_file(
    stream: &mut TcpStream,
    file: &mut File,
    file_size: u64,
    start_time: Instant,
) -> Result<(), SlkrdError> {
    configure_socket(stream)?;

    stream.write_all(&file_size.to_le_bytes())?;

    let mut transferred = 0;
    let mut buffer = vec![0; BUFFER_SIZE];

    while transferred < file_size {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        stream.write_all(&buffer[..n])?;
        stream.flush()?; // **Fix 3: Ensure Data Is Flushed**

        transferred += n as u64;
        print_progress(transferred, file_size, start_time);
    }

    stream.shutdown(std::net::Shutdown::Both)?; // **Fix 2: Close TCP Properly**
    println!("\nFile sent successfully.");
    Ok(())
}

/// **Fix 3: Configure TCP for Cross-Platform Stability**
fn configure_socket(stream: &TcpStream) -> Result<(), io::Error> {
    let socket = Socket::from(stream.try_clone()?);
    socket.set_keepalive(true)?;
    socket.set_tcp_keepalive(&socket2::TcpKeepalive::new().with_time(TCP_KEEPALIVE_TIME))?;
    Ok(())
}

fn print_progress(transferred: u64, total: u64, start_time: Instant) {
    let percent = (transferred as f64 / total as f64) * 100.0;
    let elapsed = start_time.elapsed().as_secs_f64();
    let speed = if elapsed > 0.0 { transferred as f64 / elapsed } else { 0.0 };

    print!(
        "\rProgress: {:.1}% ({} / {}) - {:.2} MB/s",
        percent,
        transferred,
        total,
        speed / 1_000_000.0
    );
    io::stdout().flush().unwrap();
}
