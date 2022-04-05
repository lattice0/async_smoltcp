/*
apt-get install -y iproute2 iptables

https://serverfault.com/a/1067366/554980

ip tuntap add dev tun1 mode tun user `id -un`
ip link set dev tun1 up
ip addr add dev tun1 local 192.168.69.0 remote 192.168.69.1
iptables -t filter -I FORWARD -i tun1 -o eth0 -j ACCEPT
iptables -t filter -I FORWARD -m state --state ESTABLISHED,RELATED -j ACCEPT
iptables -t nat -I POSTROUTING -o eth0 -j MASQUERADE
sysctl net.ipv4.ip_forward=1
*/
use core::task::{Context, Poll, Waker};
use futures::executor::block_on;
use futures::lock::Mutex as FutMutex;
use hyper::body::HttpBody;
use hyper::{Client, Uri};
#[allow(dead_code)]
use log::{debug, error, info, warn};
use std::collections::VecDeque;
use std::env;
use std::fs::File;
use std::io::prelude::*;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{self, AsyncWriteExt as _};

mod async_tun_utils;
use async_smoltcp::{IpAddress, IpCidr, IpEndpoint, SmolSocket, SmolStackWithDevice};
use async_tun_utils::AsyncTransporter;

#[tokio::main]
async fn main() -> Result<(), ()> {
    println!("Hyper async tun cli 1.0");
    let ip_packet_debugging = true;

    let stack_addr_ipv4 = std::net::Ipv4Addr::new(192, 168, 1, 2);
    let stack_addr_ipv6 = std::net::Ipv6Addr::new(1, 1, 1, 1, 1, 1, 1, 1);

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("trace")).init();
    // Some simple CLI args requirements...
    let url = match env::args().nth(1) {
        Some(url) => url,
        None => {
            info!("Usage: client <url>");
            return Ok(());
        }
    };
    
    let ip_addrs = vec![
        IpCidr::new(IpAddress::from(stack_addr_ipv4), 0),
        IpCidr::new(IpAddress::from(stack_addr_ipv6), 0),
    ];

    let stack = Arc::new(FutMutex::new(SmolStackWithDevice::new_tun(
        "tun1",
        None,
        None,
        None,
        Some(ip_addrs),
        openvpn_client.clone(),
    )));

    let socket = Arc::new(Mutex::new(block_on(stack.lock()).add_tcp_socket().unwrap()));

    let uri = url.parse::<Uri>().unwrap();
    let domain_port = format!("{}:{}", uri.host().unwrap(), uri.port_u16().unwrap_or(80));
    let ip_address = domain_port.to_socket_addrs().unwrap().next().unwrap();

    let src_port: u16 = async_smoltcp::random_source_port();

    block_on(stack.lock())
        .tcp_connect(&socket.lock().unwrap().handle, ip_address, src_port)
        .unwrap();

    SmolStackWithDevice::spawn_stack_thread(stack.clone());

    info!("sleeping...");
    //std::thread::sleep(std::time::Duration::from_secs(5));
    
    let connector = AsyncTransporter::new(socket.clone());
    let client: Client<AsyncTransporter, hyper::Body> = Client::builder().build(connector.clone());

    info!("getting {} at ip address {}", url, ip_address);
    let mut res = client.get(url.parse::<Uri>().unwrap()).await.unwrap();
    info!("Response: {}", res.status());
    //info!("Headers: {:#?}\n", res.headers());
    /*
    while let Some(next) = res.data().await {
        let chunk = next.unwrap();
        std::io::stdout().write_all(&chunk);
    }
    */
    info!("done!");
    Ok(())
}
