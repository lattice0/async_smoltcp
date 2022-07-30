use super::virtual_tun::VirtualTunInterface as VirtualTunDevice;
#[cfg(feature = "vpn")]
use super::virtual_tun::{VirtualTunReadError, VirtualTunWriteError};
use core::task::{Context, Poll, Waker};
use crossbeam_channel::{bounded, unbounded};
use crossbeam_channel::{Receiver, SendError, Sender, TryRecvError};
use futures::executor::block_on;
use futures::lock::Mutex as FutMutex;
#[cfg(feature = "log")]
#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
#[cfg(feature = "simple_socket")]
use simple_socket::{
    Packet, SocketConnectionError, SocketSendError, SocketType, TcpPacket, UdpPacket,
};
#[cfg(feature = "vpn")]
use simple_vpn::{
    PhyReceiveError,
    PhySendError,
    VpnClient,
    //VpnConnectionError,VpnDisconnectionError,
};
use smoltcp::iface::SocketStorage;
use smoltcp::iface::{Interface, InterfaceBuilder, Routes, SocketHandle};
use smoltcp::phy::{Device, Medium, TunTapInterface};
use smoltcp::socket::Socket;
use smoltcp::time::Instant;
pub use smoltcp::wire::IpAddress;
pub use smoltcp::{Error as SmoltcpError, Result as SmoltcpResult};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
//use smoltcp::phy::TunInterface as TunDevice;
use smoltcp::socket::{TcpSocket, TcpSocketBuffer, UdpSocket, UdpSocketBuffer};
pub use smoltcp::wire::{IpCidr, IpEndpoint, Ipv4Address, Ipv6Address};
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
#[cfg(feature = "async")]
use tokio::io::{AsyncRead, AsyncWrite};

#[allow(unused)]
use crate::DEFAULT_MTU;

pub enum PollWaitError {
    NoPollWait,
}

pub type OnPollWait = Arc<dyn Fn(Duration) -> std::result::Result<(), PollWaitError> + Send + Sync>;

#[derive(Debug)]
pub enum SpinError {
    NoCallback,
    NoSocket,
    ClosedSocket,
    Unknown(String),
}

const MAX_PACKETS_CHANNEL: usize = 10;

#[doc(hidden)]
pub fn __feed__<T, R>(arg: T, f: impl FnOnce(T) -> R) -> R {
    f(arg)
}

macro_rules! choose_device {
    (
    $scrutinee:expr, $closure:expr $(,)?
) => {{
        use $crate::async_smoltcp::SmolStackWithDevice;
        use $crate::async_smoltcp::__feed__;

        match $scrutinee {
            SmolStackWithDevice::VirtualTun(inner) => __feed__(inner, $closure),
            SmolStackWithDevice::Tun(inner) => __feed__(inner, $closure),
            SmolStackWithDevice::Tap(inner) => __feed__(inner, $closure),
        }
    }};
}

pub struct SmolStack<DeviceT>
where
    DeviceT: for<'d> Device<'d>,
{
    //pub sockets: Vec<SocketHandle>,
    pub interface: Interface<'static, DeviceT>,
    //`Sender<Arc<Packet>>` sends data to socket, `Receiver<Arc<Packet>>` received data from socket
    socket_handles: HashMap<
        SocketHandle,
        (
            SocketType,
            Sender<Arc<Packet>>,
            Receiver<Arc<Packet>>,
            Arc<FutMutex<Option<Waker>>>,
        ),
    >,
    should_stack_thread_stop: Arc<AtomicBool>,
    #[allow(unused)]
    read_wake_deque: Option<Arc<FutMutex<VecDeque<Waker>>>>,
    write_wake_deque: Option<Arc<FutMutex<VecDeque<Waker>>>>,
}

pub enum SmolStackWithDevice {
    VirtualTun(SmolStack<VirtualTunDevice>),
    Tun(SmolStack<TunTapInterface>),
    Tap(SmolStack<TunTapInterface>),
}

impl SmolStackWithDevice {
    pub fn spawn_stack_thread(stack: Arc<FutMutex<Self>>) {
        std::thread::Builder::new()
            .name("stack_thread".to_string())
            .spawn(move || {
                let stack = stack.clone();
                //let write_wake_deque_ = block_on(stack.clone().lock()).get_write_wake_deque();
                loop {
                    block_on(stack.clone().lock()).poll().unwrap();
                    if let Err(_err) = block_on(stack.clone().lock()).spin_all() {
                        #[cfg(feature = "log")]
                        error!("{:?}", _err);
                    }
                    //TODO: use wake deque future FutMutex or normal FutMutex?
                    /*
                    let waker = block_on(write_wake_deque_.clone().lock()).pop_front();
                    match waker {
                        Some(waker) => {
                            //info!("waking from write wake deque");
                            waker.wake()
                        }
                        None => {}
                    }
                    */
                    std::thread::sleep(std::time::Duration::from_millis(35));
                    if block_on(stack.clone().lock()).should_stop() {
                        #[cfg(feature = "log")]
                        info!("end of stack_thread");
                        break;
                    }
                }
            })
            .unwrap();
    }
}

impl SmolStackWithDevice {
    #[cfg(feature = "vpn")]
    pub fn new_from_vpn(
        interface_name: &str,
        //address: Option<IpCidr>,
        default_v4_gateway: Option<Ipv4Address>,
        default_v6_gateway: Option<Ipv6Address>,
        mtu: Option<usize>,
        ip_addrs: Option<Vec<IpCidr>>,
        vpn_client: Arc<FutMutex<dyn VpnClient + Send>>,
    ) -> SmolStackWithDevice {
        let mtu = mtu.unwrap_or(DEFAULT_MTU);
        let vpn_client_ = vpn_client.clone();
        let on_virtual_tun_read = Arc::new(
            move |buffer: &mut [u8]| -> std::result::Result<usize, VirtualTunReadError> {
                let mut size = 0;
                //Since there should be very few or no more than one IP stacks
                //connected to the same VPN client, it's no big deal to block here
                let mut vpn_client_ = block_on(vpn_client_.lock());
                match vpn_client_.phy_receive(None, &mut |openvpn_buffer: &[u8]| {
                    size = openvpn_buffer.len();
                    for i in 0..size {
                        buffer[i] = openvpn_buffer[i]
                    }
                    #[cfg(feature = "packet_log")]
                    crate::packet_log::log(&buffer[..size], true);
                }) {
                    Ok(s) => Ok(s),
                    Err(PhyReceiveError::NoDataAvailable) => Err(VirtualTunReadError::WouldBlock),
                    //TODO: do not panic below and treat the error?
                    Err(PhyReceiveError::Unknown(error_string)) => {
                        panic!("openvpn_client.receive() unknown error: {}", error_string);
                    }
                }
            },
        );

        let vpn_client_ = vpn_client.clone();
        let on_virtual_tun_write = Arc::new(
            move |f: &mut dyn FnMut(&mut [u8]),
                  size: usize|
                  -> std::result::Result<usize, VirtualTunWriteError> {
                let mut buffer = vec![0; size];
                f(buffer.as_mut_slice());
                #[cfg(feature = "packet_log")]
                crate::packet_log::log(buffer.as_slice(), false);
                //Since there should be very few or no more than one IP stacks
                //connected to the same VPN client, it's no big deal to block here
                match block_on(vpn_client_.lock()).phy_send(buffer.as_slice()) {
                    Ok(s) => Ok(s),
                    Err(PhySendError::Unknown(error_string)) => {
                        //TODO: not panic here, just treat the error
                        panic!("{}", error_string);
                    }
                }
            },
        );

        let _on_poll_wait = Arc::new(
            |_duration: std::time::Duration| -> std::result::Result<(), PollWaitError> { Ok(()) },
        );

        let device = VirtualTunDevice::new(
            interface_name,
            on_virtual_tun_read,
            on_virtual_tun_write,
            mtu,
        )
        .unwrap();

        let default_ip_address = IpCidr::new(IpAddress::v4(192, 168, 69, 2), 24);
        let mut routes = Routes::new(BTreeMap::new());

        let default_v4_gateway = default_v4_gateway.unwrap_or(Ipv4Address::new(192, 168, 69, 100));

        routes.add_default_ipv4_route(default_v4_gateway).unwrap();

        if default_v6_gateway.is_some() {
            //TODO: find a good ipv6 to use
            let default_v6_gateway =
                default_v6_gateway.unwrap_or(Ipv6Address::new(1, 1, 1, 1, 1, 1, 1, 1));
            routes.add_default_ipv6_route(default_v6_gateway).unwrap();
        }
        let ip_addrs = ip_addrs.unwrap_or(vec![default_ip_address]);
        let interface = InterfaceBuilder::new(device)
            .ip_addrs(ip_addrs)
            .routes(routes)
            .finalize();

        let socket_set = SocketSet::new(vec![]);

        SmolStackWithDevice::VirtualTun(SmolStack {
            sockets: socket_set,
            interface: interface,
            socket_handles: HashMap::new(),
            should_stack_thread_stop: Arc::new(AtomicBool::new(false)),
            read_wake_deque: None,
            write_wake_deque: None,
        })
    }

    #[cfg(feature = "tap")]
    pub fn new_tap(
        interface_name: &str,
        ip_addrs: Option<Vec<IpCidr>>,
        default_v4_gateway: Option<Ipv4Address>,
        default_v6_gateway: Option<Ipv6Address>,
        //mtu: Option<usize>,
    ) -> SmolStackWithDevice {
        //let mtu = mtu.unwrap_or(DEFAULT_MTU);

        let device = TunTapInterface::new(interface_name, Medium::Ethernet).unwrap();
        let ip_addrs = ip_addrs.unwrap_or(vec![IpCidr::new(IpAddress::v4(192, 168, 69, 2), 24)]);
        let mut routes = Routes::new(BTreeMap::new());
        //let ip_addrs = ip_addrs.unwrap_or(vec![default_ip_address]);

        let default_v4_gateway = default_v4_gateway.unwrap_or(Ipv4Address::new(192, 168, 69, 100));

        routes.add_default_ipv4_route(default_v4_gateway).unwrap();

        if default_v6_gateway.is_some() {
            //TODO: find a good ipv6 to use
            let default_v6_gateway =
                default_v6_gateway.unwrap_or(Ipv6Address::new(1, 1, 1, 1, 1, 1, 1, 1));
            routes.add_default_ipv6_route(default_v6_gateway).unwrap();
        }
        let interface = InterfaceBuilder::new(device, vec![])
            .ip_addrs(ip_addrs)
            .routes(routes)
            .finalize();

        //let socket_set = SocketSet::new(vec![]);

        SmolStackWithDevice::Tap(SmolStack {
            //sockets: socket_set,
            interface: interface,
            socket_handles: HashMap::new(),
            should_stack_thread_stop: Arc::new(AtomicBool::new(false)),
            read_wake_deque: None,
            write_wake_deque: None,
        })
    }

    #[cfg(feature = "tun")]
    pub fn new_tun(
        interface_name: &str,
        default_v4_gateway: Option<Ipv4Address>,
        default_v6_gateway: Option<Ipv6Address>,
        ip_addrs: Option<Vec<IpCidr>>,
    ) -> SmolStackWithDevice {
        let device = TunTapInterface::new(interface_name, Medium::Ip).unwrap();

        let default_ip_address = IpCidr::new(IpAddress::v4(192, 168, 69, 2), 24);
        let mut routes = Routes::new(BTreeMap::new());
        let ip_addrs = ip_addrs.unwrap_or(vec![default_ip_address]);
        let default_v4_gateway = default_v4_gateway.unwrap_or(Ipv4Address::new(192, 168, 69, 100));

        routes.add_default_ipv4_route(default_v4_gateway).unwrap();

        if default_v6_gateway.is_some() {
            //TODO: find a good ipv6 to use
            let default_v6_gateway =
                default_v6_gateway.unwrap_or(Ipv6Address::new(1, 1, 1, 1, 1, 1, 1, 1));
            routes.add_default_ipv6_route(default_v6_gateway).unwrap();
        }

        let interface = InterfaceBuilder::new(device, vec![])
            .ip_addrs(ip_addrs)
            .routes(routes)
            .finalize();

        //let socket_set = SocketSet::new(vec![]);

        SmolStackWithDevice::Tun(SmolStack {
            //sockets: socket_set,
            interface: interface,
            socket_handles: HashMap::new(),
            should_stack_thread_stop: Arc::new(AtomicBool::new(false)),
            read_wake_deque: None,
            write_wake_deque: None,
        })
    }

    #[allow(unused)]
    fn get_write_wake_deque(&self) -> Arc<FutMutex<VecDeque<Waker>>> {
        choose_device!(self, |stack| stack
            .write_wake_deque
            .as_ref()
            .unwrap()
            .clone())
    }

    fn should_stop(&self) -> bool {
        choose_device!(self, |stack| {
            stack.should_stack_thread_stop.load(Ordering::Relaxed)
        })
    }

    pub fn close_socket(&mut self, socket: SocketHandle) {
        choose_device!(self, |stack_| {
            stack_.interface.remove_socket(socket);
        });
    }

    pub fn add_tcp_socket(&mut self) -> Result<SmolSocket, ()> {
        choose_device!(self, |stack_| {
            let rx_buffer = TcpSocketBuffer::new(vec![0; 65000]);
            let tx_buffer = TcpSocketBuffer::new(vec![0; 65000]);
            let socket = TcpSocket::new(rx_buffer, tx_buffer);
            let handle = stack_.interface.add_socket(socket);

            let (socket_tx, stack_rx_from_socket) = bounded(MAX_PACKETS_CHANNEL);
            let (stack_tx_to_socket, socket_rx) = bounded(MAX_PACKETS_CHANNEL);

            let waker = Arc::new(FutMutex::new(None));

            stack_.socket_handles.insert(
                handle,
                (
                    SocketType::TCP,
                    stack_tx_to_socket,
                    stack_rx_from_socket,
                    waker.clone(),
                ),
            );
            let smol_socket = SmolSocket {
                handle: handle,
                channel: Some((socket_tx, socket_rx)),
                current: None,
                waker: waker.clone(),
            };
            Ok(smol_socket)
        })
    }

    pub fn add_udp_socket(&mut self) -> Result<SmolSocket, ()> {
        choose_device!(self, |stack_| {
            let rx_buffer = UdpSocketBuffer::new(Vec::new(), vec![0; 1024]);
            let tx_buffer = UdpSocketBuffer::new(Vec::new(), vec![0; 1024]);
            let socket = UdpSocket::new(rx_buffer, tx_buffer);
            let handle = stack_.interface.add_socket(socket);

            let (socket_tx, stack_rx_from_socket) = unbounded();
            let (stack_tx_to_socket, socket_rx) = unbounded();

            let waker = Arc::new(FutMutex::new(None));

            stack_.socket_handles.insert(
                handle,
                (
                    SocketType::UDP,
                    stack_tx_to_socket,
                    stack_rx_from_socket,
                    waker.clone(),
                ),
            );
            let smol_socket = SmolSocket {
                handle: handle,
                channel: Some((socket_tx, socket_rx)),
                current: None,
                waker: waker.clone(),
            };
            Ok(smol_socket)
        })
    }

    /*
    //TODO: dreprecate it since not used?
    pub fn tcp_socket_send(
        &mut self,
        socket_handle: SocketHandle,
        data: &[u8],
    ) -> Result<usize, SocketSendError> {
        choose_device!(self, |stack| {
            let mut socket = stack.sockets.get::<TcpSocket>(socket_handle);
            socket
                .send_slice(data)
                .map_err(|e| SocketSendError::Unknown("".into()))
        })
    }

    //TODO: deprecate it since not used?
    pub fn udp_socket_send(
        &mut self,
        socket_handle: SocketHandle,
        data: &[u8],
        addr: SocketAddr,
    ) -> Result<usize, SocketSendError> {
        choose_device!(self, |stack| {
            let mut socket = stack.sockets.get::<UdpSocket>(socket_handle);
            socket
                .send_slice(data, addr.into())
                .map(|_| data.len())
                .map_err(|e| SocketSendError::Unknown("".into()))
        })
    }
    */

    pub fn tcp_connect(
        &mut self,
        handle: &SocketHandle,
        addr: SocketAddr,
        src_port: u16,
    ) -> Result<(), SocketConnectionError> {
        choose_device!(self, |stack| {
            let (socket, context) = stack.interface.get_socket_and_context::<TcpSocket>(handle.clone());
            socket
                .connect(context, addr, src_port)
                .map_err(|e| smol_e_connection(e))
        })
    }

    pub fn poll(&mut self) -> SmoltcpResult<bool> {
        choose_device!(self, |stack| {
            let timestamp = Instant::now();
            match stack.interface.poll(timestamp) {
                Ok(b) => Ok(b),
                Err(e) => {
                    panic!("{}", e);
                }
            }
        })
    }

    pub fn spin_tcp(
        socket: &mut TcpSocket,
        send_to_socket: &Sender<Arc<Packet>>,
        receive_from_socket: &Receiver<Arc<Packet>>,
        waker: &Arc<FutMutex<Option<Waker>>>, //on_tcp_socket_data: OnTcpSocketData,
    ) -> std::result::Result<(), SpinError> {
        if socket.can_recv() {
            let r = socket.recv(|data| {
                let p = Arc::new(Packet::new_tcp(TcpPacket {
                    data: data.to_vec(),
                }));
                match send_to_socket.send(p) {
                    Ok(()) => {}
                    Err(SendError(_message)) => {
                        //If we arrived here it probably means the channel got disconnected,
                        //return Err(SpinError::ClosedSocket);
                        //return (0, Err(smoltcp::Error::Dropped));
                        #[cfg(feature = "log")]
                        error!("spin_tcp: send_to_socket error");
                    }
                }
                block_on(waker.lock()).as_ref().map(|w| w.clone().wake());
                (data.len(), ())
            });
            if let Err(_r) = r {
                #[cfg(feature = "log")]
                error!("spin_tcp error: {}", _r);
            }
        }
        if socket.can_send() {
            match receive_from_socket.try_recv() {
                Ok(packet) => {
                    let p = packet.tcp.as_ref().unwrap().data.as_slice();
                    #[cfg(feature = "log")]
                    debug!("C -> S: {:?}", std::str::from_utf8(p).unwrap());
                    socket.send_slice(p).map_err(|e| smol_e_send(e)).unwrap();
                }
                Err(TryRecvError::Disconnected) => {
                    #[cfg(feature = "log")]
                    debug!("socket close");
                    socket.close();
                }
                Err(TryRecvError::Empty) => {
                    //#[cfg(feature = "log")]
                    //info!("received empty packet");
                }
            }
        } else {
            #[cfg(feature = "trace")]
            debug!("socket cannot send");
        }
        Ok(())
    }

    pub fn spin_udp(
        socket: &mut UdpSocket,
        send_to_socket: &Sender<Arc<Packet>>,
        receive_from_socket: &Receiver<Arc<Packet>>,
        waker: &Arc<FutMutex<Option<Waker>>>, //on_udp_socket_data: OnUdpSocketData,
    ) -> std::result::Result<(), SpinError> {
        if socket.can_recv() {
            if let Ok((buffer, endpoint)) = socket.recv() {
                let addr: IpAddr = match endpoint.addr {
                    IpAddress::Ipv4(ipv4) => IpAddr::V4(ipv4.into()),
                    IpAddress::Ipv6(ipv6) => IpAddr::V6(ipv6.into()),
                    _ => return Err(SpinError::Unknown("spin address conversion error".into())),
                };
                let port = endpoint.port;
                let p = Arc::new(Packet::new_udp(UdpPacket {
                    data: buffer.to_vec(),
                    from_ip: addr,
                    from_port: port,
                }));
                match send_to_socket.send(p) {
                    Ok(()) => {}
                    Err(SendError(_message)) => {
                        //If we arrived here it probably means the channel got disconnected,
                        //so let's close this socket
                        #[cfg(feature = "log")]
                        debug!("socket close");
                        socket.close();
                    }
                }
                block_on(waker.lock()).as_ref().map(|w| w.clone().wake());
            }
        }
        if socket.can_send() {
            match receive_from_socket.try_recv() {
                Ok(packet) => {
                    let s = packet.udp.as_ref().unwrap().data.as_slice();
                    let addr = packet.udp.as_ref().unwrap().from_ip;
                    let port = packet.udp.as_ref().unwrap().from_port;
                    let endpoint = std::net::SocketAddr::new(addr, port);
                    socket
                        .send_slice(s, endpoint.into())
                        .map(|_| s.len())
                        .map_err(|e| smol_e_send(e))
                        //TODO: no uwnrap
                        .unwrap();
                }
                Err(TryRecvError::Disconnected) => {
                    #[cfg(feature = "log")]
                    debug!("socket close");
                    socket.close();
                }
                Err(TryRecvError::Empty) => {}
            }
        } else {
            #[cfg(feature = "log")]
            trace!("socket cannot send");
        }

        Ok(())
    }

    pub fn spin_all(&mut self) -> std::result::Result<(), SpinError> {
        choose_device!(self, |stack| {
            let mut smol_socket_handles = Vec::<SocketHandle>::new();

            //TODO: this is not fast
            for (smol_socket_handle, _) in stack.socket_handles.iter() {
                smol_socket_handles.push(smol_socket_handle.clone());
            }

            for smol_socket_handle in smol_socket_handles.iter_mut() {
                let (socket_type, send_to_socket, receive_from_socket, waker) = stack
                    .socket_handles
                    .get(&smol_socket_handle)
                    .ok_or(SpinError::NoSocket)
                    .unwrap();
                match socket_type {
                    SocketType::TCP => {
                        let (mut socket, context) = stack.interface.get_socket_and_context::<TcpSocket>(smol_socket_handle.clone());
                        let s = SmolStackWithDevice::spin_tcp(
                            &mut socket,
                            send_to_socket,
                            receive_from_socket,
                            &waker.clone(), //on_tcp_socket_data,
                        );

                        if let Err(_s) = s {
                            #[cfg(feature = "log")]
                            debug!("spin_tcp error: {:?}", _s);
                            socket.close();
                        }
                    }
                    SocketType::UDP => {
                        let (mut socket, context) = stack.interface.get_socket_and_context::<UdpSocket>(smol_socket_handle.clone());
                        let s = SmolStackWithDevice::spin_udp(
                            &mut socket,
                            send_to_socket,
                            receive_from_socket,
                            &waker.clone(), //on_udp_socket_data,
                        );
                        if let Err(_s) = s {
                            #[cfg(feature = "log")]
                            debug!("spin_udp error: {:?}", _s);
                            socket.close();
                        }
                    }
                    _ => unimplemented!("socket type not implemented yet"),
                }
            }
        });
        Ok(())
    }
}

pub struct SmolSocket {
    pub handle: SocketHandle,
    channel: Option<(Sender<Arc<Packet>>, Receiver<Arc<Packet>>)>,
    waker: Arc<FutMutex<Option<Waker>>>,
    //If we could not deliver an entire `Packet` in `poll_read`, we store it here for the next iteration
    current: Option<(Arc<Packet>, usize)>,
}

#[cfg(feature = "async")]
pub trait AsyncRW: tokio::io::AsyncRead + tokio::io::AsyncWrite {}

#[cfg(feature = "async")]
impl<T> AsyncRW for T where T: AsyncRead + AsyncWrite {}

#[cfg(feature = "async")]
impl AsyncRead for SmolSocket {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let channel = match &self.channel {
            Some((a, b)) => (a, b),
            None => {
                //If I'm right this should never arrive
                panic!("this should't have happen, I guess");
                //return Poll::Ready(Ok(()));
            }
        };
        let packet = match channel.1.try_recv() {
            Ok(packet) => {
                #[cfg(feature = "log")]
                debug!(
                    "C <- S: {}",
                    std::str::from_utf8(&packet.tcp.as_ref().unwrap().data.as_slice()).unwrap()
                );
                packet
            }
            Err(TryRecvError::Empty) => {
                //blocking here is no big deal since it's just to replace the variable. Very fast
                block_on(self.waker.lock()).replace(cx.waker().clone());
                return Poll::Pending;
            }
            Err(TryRecvError::Disconnected) => {
                //TODO: return what in this case?
                block_on(self.waker.lock()).replace(cx.waker().clone());
                return Poll::Pending;
            }
        };

        let mut consume = |packet: &Arc<Packet>, consumed: &mut usize| {
            if let Some(tcp_packet) = &packet.tcp {
                if tcp_packet.data.len() <= buf.remaining() {
                    buf.put_slice(tcp_packet.data.as_slice());
                    Poll::Ready(Ok(()))
                } else {
                    let remaining = buf.remaining();
                    buf.put_slice(&tcp_packet.data.as_slice()[*consumed..remaining]);
                    //packet.index = consumed;
                    //*current = Some((packet.clone(), remaining));
                    *consumed = remaining;
                    //buf.advance(remaining)
                    Poll::Ready(Ok(()))
                }
            } else {
                Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "invalid socket access",
                )))
            }
        };

        //If there's already a leftover buffer from the previous iteration
        if let Some((packet, mut consumed)) = &self.current {
            return consume(&packet, &mut consumed);
        } else {
            return consume(&packet, &mut 0);
        }
    }
}

#[cfg(feature = "async")]
impl AsyncWrite for SmolSocket {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let channel = match &self.channel {
            Some((a, b)) => (a, b),
            None => {
                return Poll::Ready(Ok(0));
            }
        };
        channel
            .0
            .send(Arc::new(Packet::new_tcp(TcpPacket { data: buf.to_vec() })))
            .unwrap();
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let mut p = Pin::new(self);
        p.channel.take();
        Poll::Ready(Ok(()))
    }
}

fn smol_e_send(e: smoltcp::Error) -> SocketSendError {
    match e {
        smoltcp::Error::Exhausted => SocketSendError::Exhausted,
        _ => SocketSendError::Unknown(format!("{}", e)),
    }
}

fn smol_e_connection(e: smoltcp::Error) -> SocketConnectionError {
    match e {
        smoltcp::Error::Exhausted => SocketConnectionError::Exhausted,
        _ => SocketConnectionError::Unknown(format!("{}", e)),
    }
}

/*
fn smol_e_receive(e: smoltcp::Error) -> SocketReceiveError {
    match e {
        smoltcp::Error::Exhausted => SocketReceiveError::SocketNotReady,
        _ => SocketReceiveError::Unknown(format!("{}", e))
    }
}
*/

pub type SafeSmolStackWithDevice = SmolStackWithDevice;

//TODO: instead, implement Send for the C types?
unsafe impl Send for SafeSmolStackWithDevice {}
