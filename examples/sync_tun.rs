/*
Attention: this has nothing to do with async_smoltcp, it's just 
a simple smoltcp for testing purposes. It's here to facilitate testing


To run this example, install below:

apt-get install -y iproute2 iptables dnsutils

then do this with sudo if outside of docker:

ip tuntap add dev tun1 mode tun user `id -un`
ip link set dev tun1 up
ip addr add dev tun1 local 192.168.1.0 remote 192.168.1.1
iptables -t filter -I FORWARD -i tun1 -o eth0 -j ACCEPT
iptables -t filter -I FORWARD -m state --state ESTABLISHED,RELATED -j ACCEPT
iptables -t nat -I POSTROUTING -o eth0 -j MASQUERADE
sysctl net.ipv4.ip_forward=1

then

cargo run --example async_tun --features="default log" -- http://neversll.com 80

More information about the ip commands: https://serverfault.com/a/1067366/554980
*/

mod utils;

use std::str::{self, FromStr};
use std::collections::BTreeMap;
use std::os::unix::io::AsRawFd;
use log::debug;

use smoltcp::phy::{Device, Medium, wait as phy_wait};
use smoltcp::wire::{EthernetAddress, Ipv4Address, IpAddress, IpCidr};
use smoltcp::iface::{NeighborCache, InterfaceBuilder, Routes};
use smoltcp::socket::{SocketSet, TcpSocket, TcpSocketBuffer};
use smoltcp::time::Instant;

fn main() {
    #[cfg(feature = "log")]
    utils::setup_logging("");

    let (mut opts, mut free) = utils::create_options();
    utils::add_tuntap_options(&mut opts, &mut free);
    utils::add_middleware_options(&mut opts, &mut free);
    free.push("ADDRESS");
    free.push("PORT");

    let mut matches = utils::parse_options(&opts, free);
    let device = utils::parse_tuntap_options(&mut matches);

    let fd = device.as_raw_fd();
    let device = utils::parse_middleware_options(&mut matches, device, /*loopback=*/false);
    let address = IpAddress::from_str(&matches.free[0]).expect("invalid address format");
    let port = u16::from_str(&matches.free[1]).expect("invalid port format");

    let neighbor_cache = NeighborCache::new(BTreeMap::new());

    let tcp_rx_buffer = TcpSocketBuffer::new(vec![0; 64]);
    let tcp_tx_buffer = TcpSocketBuffer::new(vec![0; 128]);
    let tcp_socket = TcpSocket::new(tcp_rx_buffer, tcp_tx_buffer);

    let ethernet_addr = EthernetAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x02]);
    let ip_addrs = [IpCidr::new(IpAddress::v4(192, 168, 69, 2), 24)];
    let default_v4_gw = Ipv4Address::new(192, 168, 69, 100);
    let mut routes_storage = [None; 1];
    let mut routes = Routes::new(&mut routes_storage[..]);
    routes.add_default_ipv4_route(default_v4_gw).unwrap();
    
    let medium = device.capabilities().medium;
    let mut builder = InterfaceBuilder::new(device)
            .ip_addrs(ip_addrs)
            .routes(routes);
    if medium == Medium::Ethernet {
        builder = builder
            .ethernet_addr(ethernet_addr)
            .neighbor_cache(neighbor_cache);
    }
    let mut iface = builder.finalize();

    let mut sockets = SocketSet::new(vec![]);
    let tcp_handle = sockets.add(tcp_socket);

    {
        let mut socket = sockets.get::<TcpSocket>(tcp_handle);
        socket.connect((address, port), 49500).unwrap();
    }

    let mut tcp_active = false;
    loop {
        let timestamp = Instant::now();
        match iface.poll(&mut sockets, timestamp) {
            Ok(_) => {},
            Err(e) => {
                debug!("poll error: {}", e);
            }
        }

        {
            let mut socket = sockets.get::<TcpSocket>(tcp_handle);
            if socket.is_active() && !tcp_active {
                debug!("connected");
            } else if !socket.is_active() && tcp_active {
                debug!("disconnected");
                break
            }
            tcp_active = socket.is_active();

            if socket.may_recv() {
                let data = socket.recv(|data| {
                    let mut data = data.to_owned();
                    if !data.is_empty() {
                        debug!("recv data: {:?}",
                               str::from_utf8(data.as_ref()).unwrap_or("(invalid utf8)"));
                        data = data.split(|&b| b == b'\n').collect::<Vec<_>>().concat();
                        data.reverse();
                        data.extend(b"\n");
                    }
                    (data.len(), data)
                }).unwrap();
                if socket.can_send() && !data.is_empty() {
                    debug!("send data: {:?}",
                           str::from_utf8(data.as_ref()).unwrap_or("(invalid utf8)"));
                    socket.send_slice(&data[..]).unwrap();
                }
            } else if socket.may_send() {
                debug!("close");
                socket.close();
            }
        }

        phy_wait(fd, iface.poll_delay(&sockets, timestamp)).expect("wait error");
    }
}
