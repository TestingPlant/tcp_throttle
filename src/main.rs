use clap::Parser;
use std::io::Write;
use std::net::SocketAddr;
use std::os::fd::AsRawFd;
use std::ptr::addr_of_mut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};

const BUFFER_SIZE: usize = 16 * 1024;
#[derive(Parser)]
#[command(name = "tcp_throttle")]
#[command(bin_name = "tcp_throttle")]
enum Cli {
    Throttle(ThrottleArgs),
}

#[derive(Debug, clap::Args)]
#[command(author, version, about, long_about = None)]
struct ThrottleArgs {
    #[arg(long)]
    server: SocketAddr,
    #[arg(long)]
    bind: SocketAddr,
    #[arg(long)]
    client_download_speed: u64,
    #[arg(long)]
    client_upload_speed: u64,
    #[arg(long)]
    limit_server_stream_recv_window: bool,
}

fn queued_read(stream: &TcpStream) -> libc::c_int {
    let fd = stream.as_raw_fd();
    let mut value: libc::c_int = 0;
    unsafe {
        assert_ne!(libc::ioctl(fd, libc::FIONREAD, addr_of_mut!(value)), -1);
    }
    value
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    match Cli::parse() {
        Cli::Throttle(args) => {
            let listener = TcpListener::bind(args.bind).await.unwrap();
            let (mut client_stream, _) = listener.accept().await.unwrap();
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            let server_socket = TcpSocket::new_v4().unwrap();
            if args.limit_server_stream_recv_window {
                server_socket
                    .set_recv_buffer_size(args.client_download_speed as u32)
                    .unwrap();
            }
            let mut server_stream = server_socket.connect(args.server).await.unwrap();

            let mut bytes_downloaded_this_second = 0u64;
            let mut bytes_uploaded_this_second = 0u64;
            let mut client_download_buffer = [0; BUFFER_SIZE];
            let mut client_upload_buffer = [0; BUFFER_SIZE];
            loop {
                let max_client_download = std::cmp::min(
                    (args.client_download_speed - bytes_downloaded_this_second) as usize,
                    BUFFER_SIZE,
                );
                let max_client_upload = std::cmp::min(
                    (args.client_upload_speed - bytes_uploaded_this_second) as usize,
                    BUFFER_SIZE,
                );
                tokio::select! {
                    bytes_read = server_stream.read(&mut client_download_buffer[0..max_client_download]), if max_client_download > 0 => {
                        let bytes_read = bytes_read.unwrap();
                        if bytes_read == 0 {
                            return;
                        }
                        client_stream.write_all(&client_download_buffer[0..bytes_read]).await.unwrap();
                        bytes_downloaded_this_second += bytes_read as u64;
                    },
                    bytes_read = client_stream.read(&mut client_upload_buffer[0..max_client_upload]), if max_client_upload > 0 => {
                        let bytes_read = bytes_read.unwrap();
                        if bytes_read == 0 {
                            return;
                        }
                        server_stream.write_all(&client_upload_buffer[0..bytes_read]).await.unwrap();
                        bytes_uploaded_this_second += bytes_read as u64;
                    },
                    _ = interval.tick() => {
                        bytes_downloaded_this_second = 0;
                        bytes_uploaded_this_second = 0;

                        print!("\rserver -> client: {} bytes in queue (excludes bytes from server's send queue). client -> server: {} bytes in queue", queued_read(&server_stream), queued_read(&client_stream));
                        std::io::stdout().flush().unwrap();
                    }
                }
            }
        }
    }
}
