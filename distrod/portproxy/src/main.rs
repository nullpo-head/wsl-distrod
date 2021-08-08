use anyhow::{Context, Result};
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames};
use tokio::io::AsyncWriteExt;
use tokio::io::{self, BufReader};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, StructOpt)]
#[structopt(name = "portproxy", rename_all = "kebab")]
pub struct Opts {
    #[structopt(subcommand)]
    pub command: Subcommand,
}

#[derive(Debug, StructOpt)]
pub enum Subcommand {
    Proxy(ProxyOpts),
    Show(ShowOpts),
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct ProxyOpts {
    pub dest_addr: String,
    #[structopt(short, long)]
    pub tcp4: Vec<u16>,
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct ShowOpts {
    pub show: ShowItem,
}

#[derive(Clone, Debug, EnumString, EnumVariantNames)]
#[strum(serialize_all = "kebab-case")]
pub enum ShowItem {
    Ipv4(String),
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::from_args();

    if let Err(e) = run(opts).await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }

    Ok(())
}

async fn run(opts: Opts) -> Result<()> {
    match opts.command {
        Subcommand::Proxy(proxy_opts) => run_proxy(proxy_opts).await,
        Subcommand::Show(show_opts) => run_show(show_opts)?,
    };
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_show(_opts: ShowOpts) -> Result<()> {
    use anyhow::anyhow;
    use nix::sys::socket::{InetAddr, SockAddr};

    let mut addrs = nix::ifaddrs::getifaddrs()?;
    let eth0_addr = addrs
        .find_map(|iaddr| {
            if iaddr.interface_name != "eth0" {
                return None;
            }
            match iaddr.address {
                Some(SockAddr::Inet(addr @ InetAddr::V4(_))) => Some(addr.ip()),
                _ => None,
            }
        })
        .ok_or_else(|| anyhow!("'eth0' is not found."))?;
    println!("{}", eth0_addr);
    Ok(())
}

#[cfg(target_os = "windows")]
fn run_show(_opts: ShowOpts) -> Result<()> {
    use anyhow::bail;

    bail!("Show command is not implemented on Windows.");
}

async fn run_proxy(opts: ProxyOpts) {
    let mut handles = vec![];
    for tcp_port in opts.tcp4 {
        if tcp_port == 0 {
            continue;
        }
        let dest_addr = format!("{}:{}", &opts.dest_addr, tcp_port);
        handles.push(tokio::spawn(async move {
            if let Err(e) = proxy_tcp_port(tcp_port, dest_addr).await {
                eprintln!("Error: {}", e);
            }
        }));
    }
    for handle in handles {
        let _ = handle.await;
    }
}

async fn proxy_tcp_port(port: u16, dest_addr: String) -> Result<()> {
    let listen_addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&listen_addr)
        .await
        .with_context(|| format!("Failed to bind {}.", &listen_addr))?;
    println!("Forwarding {} to {}", &listen_addr, &dest_addr);
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .with_context(|| format!("Failed to accept on the port {}.", port))?;
        let dest = dest_addr.clone();
        tokio::spawn(async move {
            if let Err(e) = proxy_tcp_stream(stream, dest).await {
                eprintln!("Error: {}", e);
            }
        });
    }
}

async fn proxy_tcp_stream(mut client: TcpStream, upstream_addr: String) -> Result<()> {
    let buf_size = 1 << 16;

    let mut upstream = TcpStream::connect(upstream_addr)
        .await
        .with_context(|| "Failed to connect to the upstream.")?;

    let (client_read, mut client_write) = client.split();
    let (upstream_read, mut upstream_write) = upstream.split();

    let client_to_upstream = async {
        let mut buf_read = BufReader::with_capacity(buf_size, client_read);
        io::copy_buf(&mut buf_read, &mut upstream_write)
            .await
            .with_context(|| "Copy to the upstream failed.")?;
        upstream_write
            .shutdown()
            .await
            .with_context(|| "Shutting down the client_to_upsteam failed.")?;
        Ok::<(), anyhow::Error>(())
    };

    let upstream_to_client = async {
        let mut buf_read = BufReader::with_capacity(buf_size, upstream_read);
        io::copy(&mut buf_read, &mut client_write)
            .await
            .with_context(|| "Copy to the client failed.")?;
        client_write
            .shutdown()
            .await
            .with_context(|| "Shutting down the upstream_to_client failed.")?;
        Ok(())
    };

    tokio::try_join!(client_to_upstream, upstream_to_client)?;

    Ok(())
}
