use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use rand::Rng;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::thread;
use std::time::Duration;

const BUFFER_SIZE: usize = 8192;
const PORT: u16 = 3000;
const MAX_RETRIES: u32 = 30; // 30 seconds max wait time

#[derive(Parser)]
#[command(author, version, about)]
struct Cli {
    /// Send file
    #[arg(short = 's')]
    send: Option<String>,

    /// Receive file with passcode
    #[arg(short = 'r')]
    receive: Option<String>,
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    if let Some(file_path) = cli.send {
        let passcode = generate_passcode();
        println!("Your passcode is: {}", passcode);
        println!("Waiting for receiver to connect...");
        send_file(&file_path, &passcode)
    } else if let Some(passcode) = cli.receive {
        println!("Attempting to connect to sender...");
        match receive_file(&passcode) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                println!("Could not connect to sender. Make sure:");
                println!("1. The sender has started the program first");
                println!("2. Both sender and receiver are on the same network");
                println!("3. The passcode is correct");
                println!("4. No firewall is blocking port {}", PORT);
                Err(e)
            }
            Err(e) => Err(e),
        }
    } else {
        println!("Please use -s to send or -r to receive");
        Ok(())
    }
}

fn generate_passcode() -> String {
    let mut rng = rand::thread_rng();
    format!("{:06}", rng.gen_range(0..999999))
}

fn send_file(file_path: &str, passcode: &str) -> std::io::Result<()> {
    let path = Path::new(file_path);
    if !path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "File not found",
        ));
    }

    let listener = TcpListener::bind(format!("0.0.0.0:{}", PORT))?;
    println!("Listening on port {}...", PORT);
    println!("Make sure the receiver uses the passcode: {}", passcode);

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                println!("Connection received, verifying passcode...");
                
                // First, receive the passcode from client
                let mut received_passcode = [0; 6];
                stream.read_exact(&mut received_passcode)?;
                
                if passcode != String::from_utf8_lossy(&received_passcode) {
                    println!("Wrong passcode received, waiting for correct passcode...");
                    continue;
                }

                println!("Passcode verified, starting file transfer...");
                let mut file = File::open(path)?;
                let file_size = file.metadata()?.len();
                
                // Send file size
                stream.write_all(&file_size.to_le_bytes())?;
                
                // Send filename
                let filename = path.file_name().unwrap().to_str().unwrap();
                let filename_bytes = filename.as_bytes();
                let filename_len = filename_bytes.len() as u8;
                stream.write_all(&[filename_len])?;
                stream.write_all(filename_bytes)?;

                let pb = ProgressBar::new(file_size);
                pb.set_style(ProgressStyle::default_bar()
                    .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                    .unwrap()
                    .progress_chars("#>-"));

                let mut buffer = [0; BUFFER_SIZE];
                let mut sent = 0;
                while sent < file_size {
                    let n = file.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    stream.write_all(&buffer[..n])?;
                    sent += n as u64;
                    pb.set_position(sent);
                }

                pb.finish_with_message("File sent successfully!");
                return Ok(());
            }
            Err(e) => {
                println!("Connection error: {}", e);
            }
        }
    }
    Ok(())
}

fn receive_file(passcode: &str) -> std::io::Result<()> {
    let mut retry_count = 0;
    let mut last_error = None;

    while retry_count < MAX_RETRIES {
        match TcpStream::connect(format!("127.0.0.1:{}", PORT)) {
            Ok(mut stream) => {
                stream.set_read_timeout(Some(Duration::from_secs(30)))?;
                println!("Connected to sender, verifying passcode...");
                
                // Send passcode
                stream.write_all(passcode.as_bytes())?;
                
                // Receive file size
                let mut size_bytes = [0u8; 8];
                match stream.read_exact(&mut size_bytes) {
                    Ok(_) => {
                        let file_size = u64::from_le_bytes(size_bytes);
                        
                        // Receive filename
                        let mut filename_len = [0u8; 1];
                        stream.read_exact(&mut filename_len)?;
                        let mut filename_bytes = vec![0u8; filename_len[0] as usize];
                        stream.read_exact(&mut filename_bytes)?;
                        let filename = String::from_utf8_lossy(&filename_bytes);

                        println!("Starting to receive file: {}", filename);
                        
                        let pb = ProgressBar::new(file_size);
                        pb.set_style(ProgressStyle::default_bar()
                            .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                            .unwrap()
                            .progress_chars("#>-"));

                        let mut file = File::create(&*filename)?;
                        let mut buffer = [0; BUFFER_SIZE];
                        let mut received = 0;
                        
                        while received < file_size {
                            let n = stream.read(&mut buffer)?;
                            if n == 0 {
                                break;
                            }
                            file.write_all(&buffer[..n])?;
                            received += n as u64;
                            pb.set_position(received);
                        }

                        pb.finish_with_message("File received successfully!");
                        return Ok(());
                    }
                    Err(e) => {
                        println!("Wrong passcode or connection interrupted. Please verify the passcode and try again.");
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                last_error = Some(e);
                retry_count += 1;
                thread::sleep(Duration::from_secs(1));
                print!("\rAttempting to connect... {}/{}", retry_count, MAX_RETRIES);
                std::io::stdout().flush()?;
            }
        }
    }
    
    println!("\nCould not connect after {} attempts.", MAX_RETRIES);
    Err(last_error.unwrap())
}