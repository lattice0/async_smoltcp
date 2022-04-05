use futures::lock::Mutex as FutMutex;
use hyper::{Client, Uri};
#[allow(unused_imports)]
use log::{debug, error, info, warn};
use std::env;
use std::fs::File;
use std::io::prelude::*;
use std::net::{ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
mod async_transporter;
use async_smoltcp::{IpAddress, IpCidr, SmolStackWithDevice};
use async_transporter::AsyncTransporter;
use libopenvpn3::openvpn::{
    OVPNClient, OVPNEvent
};

#[tokio::main]
async fn main() -> Result<(), ()> {
    println!("Hyper OpenVPN cli 1.0");
    //let ip_packet_debugging = true;

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
    let openvpn_username: Option<&str>;
    let openvpn_password: Option<&str>;
    let openvpn_profile_dir = env::var("VPN_PROFILES_DIR").expect(
        format!(
            "couldn't read {} environment variable needed to find openvpn profile",
            "VPN_PROFILES_DIR"
        )
        .as_str(),
    );
    let chosen_vpn;
    #[allow(unused_variables)]
    {
        let vpn_japan = "jp-free-03.protonvpn.com.udp.ovpn";
        let vpn_us = "protonvpn_us_free.ovpn";
        chosen_vpn = vpn_us;
        openvpn_username = None;
        openvpn_password = None;
    }
    info!("using vpn file {}", chosen_vpn);
    let openvpn_file_path = openvpn_profile_dir + "/" + chosen_vpn;
    info!(
        "gonna read openvpn profile from {}",
        openvpn_file_path.clone()
    );
    let mut openvpn_profile_file = File::open(openvpn_file_path.clone())
        .expect(format!("couldn't open file {}", openvpn_file_path.clone()).as_str());
    let mut openvpn_profile_string = String::new();
    openvpn_profile_file
        .read_to_string(&mut openvpn_profile_string)
        .unwrap();

    let vpn_connected = Arc::new(AtomicBool::new(false));
    let vpn_connected_ = vpn_connected.clone();
    let on_vpn_event = Arc::new(Mutex::new(move |event: OVPNEvent| {
        if event.name == "CONNECTED" {
            vpn_connected_.store(true, Ordering::Relaxed);
        } else if event.name == "DISCONNECTED" {
            error!("openvpn disconnected!");
        }
        info!("VPN: {}", event);
    }));

    let on_vpn_log = Arc::new(Mutex::new(|_message: String| {
        //println!("vpn message: {}", message);
    }));

    let openvpn_client = Arc::new(FutMutex::new(
        OVPNClient::new(
            openvpn_profile_string,
            openvpn_username,
            openvpn_password,
            None,
            None,
            Some(on_vpn_log),
            Some(on_vpn_event),
            &stack_addr_ipv4,
            &stack_addr_ipv6, //Some(read_wake_deque.clone()),
        )
        .unwrap(),
    ));

    match openvpn_client.lock().await.connect() {
        Ok(()) => {
            info!("openvpn connection launched")
        }
        Err(_) => {
            error!("problem opening openvpn connection")
        }
    }

    loop {
        if vpn_connected.load(Ordering::Relaxed) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let ip_addrs = vec![
        IpCidr::new(IpAddress::from(stack_addr_ipv4), 0),
        IpCidr::new(IpAddress::from(stack_addr_ipv6), 0),
    ];

    let stack = Arc::new(FutMutex::new(SmolStackWithDevice::new_from_vpn(
        "openvpn1",
        None,
        None,
        None,
        Some(ip_addrs),
        openvpn_client.clone(),
    )));

    //let s = stack.clone();
    let socket = Arc::new(Mutex::new(stack.lock().await.add_tcp_socket().unwrap()));

    //let address = IpAddress::v4(172,217,172,206);
    let uri = url.parse::<Uri>().unwrap();
    let domain_port = format!("{}:{}", uri.host().unwrap(), uri.port_u16().unwrap_or(80));
    let ip_address = domain_port.to_socket_addrs().unwrap().next().unwrap();

    let src_port: u16 = async_smoltcp::random_source_port();
    //let dst_port: u16 = 80;
    //let endpoint = std::net::SocketAddr::V4(std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(172,217,172,206), dst_port));
    //let endpoint = IpEndpoint::new(address, dst_port);

    stack.lock().await
        .tcp_connect(&socket.lock().unwrap().handle, ip_address, src_port)
        .unwrap();

    //let stack_ = stack.clone();

    //let write_wake_deque_ = write_wake_deque.clone();

    //stack.lock().run();
    SmolStackWithDevice::spawn_stack_thread(stack.clone());

    info!("sleeping...");
    //std::thread::sleep(std::time::Duration::from_secs(5));
    
    let connector = AsyncTransporter::new(socket.clone());
    let client: Client<AsyncTransporter, hyper::Body> = Client::builder().build(connector.clone());

    info!("getting {} at ip address {}", url, ip_address);
    let res = client.get(url.parse::<Uri>().unwrap()).await.unwrap();
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
