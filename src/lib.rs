mod async_smoltcp;
mod virtual_tun;
pub(crate) mod packet_log;
pub use async_smoltcp::*;
use rand::Rng; // 0.8.0
//TODO: synchronize this MTU with the VPN's MTU
pub const DEFAULT_MTU: usize = 1500;

pub fn random_source_port() -> u16 
{   
    //TODO: better numbers
    rand::thread_rng().gen_range(42000..65000)
}