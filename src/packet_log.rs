#[cfg(feature = "packet_log")]

#[allow(unused_imports)]
use log::{debug, error, info, warn};
#[cfg(feature = "packet_log")]

pub fn log(buffer: &[u8], read: bool) {
    use etherparse::{IpHeader, TransportHeader};
    if read {
        debug!("PHY <<<<");
    } else {
        debug!("PHY >>>>");
    }
    match etherparse::PacketHeaders::from_ip_slice(buffer) {
        Err(value) => error!("ip packet parsing error {:?}", value),
        Ok(value) => {
            let ip_src_dest = |_ip: &IpHeader| {
                match value.ip.as_ref().unwrap() {
                    IpHeader::Version4(ipv4) => (
                        format!("{:?}", &ipv4.source),
                        format!("{:?}", &ipv4.destination),
                    ),
                    IpHeader::Version6(ipv6) => (
                        format!("{:?}", &ipv6.source),
                        format!("{:?}", &ipv6.destination),
                    ),
                }
            };

            let is_tcp = |transport: &TransportHeader| {
                match transport {
                    TransportHeader::Tcp(_) => true,
                    TransportHeader::Udp(_) => false,
                }
            };

            let transport_src_dest = |transport: &TransportHeader| {
                match transport {
                    TransportHeader::Tcp(tcp) => (tcp.source_port, tcp.destination_port),
                    TransportHeader::Udp(udp) => (udp.source_port, udp.destination_port),
                }
            };

            let syn_ack = |transport: &TransportHeader| {
                match transport {
                    TransportHeader::Tcp(tcp) => (tcp.syn, tcp.ack),
                    _ => (false, false),
                }
            };

            let ip_src_dest_val = ip_src_dest(&value.ip.as_ref().unwrap());
            debug!("IP {} -> {}", ip_src_dest_val.0, ip_src_dest_val.1);

            let v = value.transport.as_ref().unwrap();
            let trasnsport_src_dest_val = transport_src_dest(v);

            if is_tcp(v) {
                let syn_ack_val = syn_ack(v);

                let syn_ack_text = if value.payload.len() == 0 {
                    format!("syn: {}, ack: {}", syn_ack_val.0, syn_ack_val.1)
                } else {
                    "".to_string()
                };
                debug!(
                    "TCP: {} -> {} {}",
                    trasnsport_src_dest_val.0, trasnsport_src_dest_val.1, syn_ack_text
                );
            } else {
                debug!(
                    "UDP: {} -> {}",
                    trasnsport_src_dest_val.0, trasnsport_src_dest_val.1,
                );
            }

            if value.payload.len() > 0 {
                debug!("payload: {:?}", String::from_utf8_lossy(value.payload));
            }
        }
    }
}
