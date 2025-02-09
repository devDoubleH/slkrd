use quinn::{Endpoint, ServerConfig, ClientConfig};
use std::{fs::File, io::{Read, Write}, net::SocketAddr, sync::Arc};
use tokio::io::{AsyncReadExt};
use rand::Rng;
use structopt::StructOpt;
use rcgen::generate_simple_self_signed;


#[derive(StructOpt)]
struct Cli {
    #[structopt(short, long)]
    mode: String,
    #[structopt(short, long)]
    file: Option<String>,
    #[structopt(short, long)]
    passcode: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Cli::from_args();
    
    if args.mode == "s" {
        let file_path = args.file.expect("File required");
        let passcode = generate_passcode();
        println!("Passcode: {}", passcode);
        run_sender(file_path, passcode).await;
    } else if args.mode == "r" {
        let passcode = args.passcode.expect("Passcode required");
        run_receiver(passcode).await;
    } else {
        eprintln!("Invalid mode! Use -s for send, -r for receive.");
    }
}

fn generate_passcode() -> String {
    rand::thread_rng().gen_range(100000..999999).to_string()
}

fn configure_server() -> ServerConfig {
    let cert = generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_der = cert.serialize_der().unwrap();
    let key_der = cert.serialize_private_key_der();

    let rustls_config = RustlsServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(vec![Certificate(cert_der)], PrivateKey(key_der))
        .unwrap();
    
    ServerConfig::with_crypto(Arc::new(rustls_config))
}

fn configure_client() -> ClientConfig {
    let mut root_store = RootCertStore::empty();
    root_store.add_parsable_certificates(&[generate_simple_self_signed(vec!["localhost".into()]).unwrap().serialize_der().unwrap()]);

    let rustls_config = RustlsClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    
    ClientConfig::new(Arc::new(rustls_config))
}

async fn run_sender(file_path: String, passcode: String) {
    let addr: SocketAddr = "0.0.0.0:5000".parse().unwrap();
    let server_config = configure_server();
    let endpoint = Endpoint::server(server_config, addr).unwrap();
    println!("Waiting for receiver...");
    
    while let Some(conn) = endpoint.accept().await {
        if let Ok(new_conn) = conn.await {
            let mut file = File::open(&file_path).expect("File not found");
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).unwrap();
            
            let (mut send_stream, _) = new_conn.open_bi().await.unwrap();
            send_stream.write_all(passcode.as_bytes()).await.unwrap();
            send_stream.write_all(&buffer).await.unwrap();
            println!("File '{}' sent successfully!", file_path);
        }
    }
}

async fn run_receiver(passcode: String) {
    let addr: SocketAddr = "192.168.1.100:5000".parse().unwrap();
    let endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
    let client_config = configure_client();
    let new_conn = endpoint.connect_with(client_config, addr, "localhost").unwrap().await.unwrap();
    println!("Connected to sender!");
    
    let (mut recv_stream, _) = new_conn.accept_bi().await.unwrap();
    let mut received_passcode = [0; 6];
    recv_stream.read_exact(&mut received_passcode).await.unwrap();
    
    if passcode.as_bytes() != &received_passcode {
        eprintln!("Incorrect passcode!");
        return;
    }
    
    let mut buffer = Vec::new();
    recv_stream.read_to_end(&mut buffer).await.unwrap();
    let received_filename = "received_".to_owned() + &passcode;
    let mut file = File::create(&received_filename).unwrap();
    file.write_all(&buffer).unwrap();
    println!("File received successfully as '{}'!", received_filename);
}
