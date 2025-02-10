use clap::{Arg, Command};
use indicatif::{ProgressBar, ProgressStyle};
use rand::Rng;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;

const BUFFER_SIZE: usize = 8192;
const PORT: u16 = 8080;
const PASSCODE_CHARS: &[u8] = b"0123456789";

#[derive(Debug)]
enum SlkrdError {
    IoError(std::io::Error),
    NetworkError(String),
    InvalidPasscode,
}

impl From<std::io::Error> for SlkrdError {
    fn from(error: std::io::Error) -> Self {
        SlkrdError::IoError(error)
    }
}

fn generate_passcode() -> String {
    let mut rng = rand::thread_rng();
    (0..6)
        .map(|_| {
            let idx = rng.gen_range(0..PASSCODE_CHARS.len());
            PASSCODE_CHARS[idx] as char
        })
        .collect()
}

fn send_file(filepath: &str) -> Result<(), SlkrdError> {
    let path = Path::new(filepath);
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let passcode = generate_passcode();
    
    println!("Starting file server with passcode: {}", passcode);
    
    let listener = TcpListener::bind(format!("0.0.0.0:{}", PORT))?;
    
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let mut received_passcode = String::new();
                stream.read_to_string(&mut received_passcode)?;
                
                if received_passcode.trim() != passcode {
                    return Err(SlkrdError::InvalidPasscode);
                }
                
                // Send file size first
                stream.write_all(&file_size.to_le_bytes())?;
                
                // Create progress bar
                let pb = ProgressBar::new(file_size);
                pb.set_style(ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                    .unwrap()
                    .progress_chars("#>-"));
                
                // Send file data
                let mut file = File::open(path)?;
                let mut buffer = [0; BUFFER_SIZE];
                let mut sent = 0;
                
                while sent < file_size {
                    let n = file.read(&mut buffer)?;
                    if n == 0 { break; }
                    stream.write_all(&buffer[..n])?;
                    sent += n as u64;
                    pb.set_position(sent);
                }
                
                pb.finish_with_message("Transfer complete");
                return Ok(());
            }
            Err(e) => return Err(SlkrdError::NetworkError(e.to_string())),
        }
    }
    
    Ok(())
}

fn receive_file(passcode: &str) -> Result<(), SlkrdError> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", PORT))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    
    // Send passcode
    stream.write_all(passcode.as_bytes())?;
    
    // Read file size
    let mut size_bytes = [0u8; 8];
    stream.read_exact(&mut size_bytes)?;
    let file_size = u64::from_le_bytes(size_bytes);
    
    // Create progress bar
    let pb = ProgressBar::new(file_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
        .unwrap()
        .progress_chars("#>-"));
    
    // Receive and write file
    let mut file = File::create("received_file")?;
    let mut buffer = [0; BUFFER_SIZE];
    let mut received = 0;
    
    while received < file_size {
        let n = stream.read(&mut buffer)?;
        if n == 0 { break; }
        file.write_all(&buffer[..n])?;
        received += n as u64;
        pb.set_position(received);
    }
    
    pb.finish_with_message("Transfer complete");
    Ok(())
}

fn main() {
    let matches = Command::new("slkrd")
        .version("1.0")
        .author("Your Name")
        .about("Fast peer-to-peer file sharing tool")
        .arg(Arg::new("send")
            .short('s')
            .value_name("FILE")
            .help("Send a file"))
        .arg(Arg::new("receive")
            .short('r')
            .value_name("PASSCODE")
            .help("Receive a file with passcode"))
        .get_matches();

    if let Some(file) = matches.get_one::<String>("send") {
        match send_file(file) {
            Ok(_) => println!("File sent successfully"),
            Err(e) => eprintln!("Error sending file: {:?}", e),
        }
    } else if let Some(passcode) = matches.get_one::<String>("receive") {
        match receive_file(passcode) {
            Ok(_) => println!("File received successfully"),
            Err(e) => eprintln!("Error receiving file: {:?}", e),
        }
    } else {
        println!("Please use -s to send a file or -r to receive a file");
    }
}