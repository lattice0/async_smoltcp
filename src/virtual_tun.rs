use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc};
use std::vec::Vec;

pub enum VirtualTunReadError {
    WouldBlock
}

#[derive(Debug)]
pub enum VirtualTunWriteError {
    NoData
}

pub type OnVirtualTunRead = Arc<dyn Fn(&mut [u8]) -> Result<usize,VirtualTunReadError> + Send + Sync>;
pub type OnVirtualTunWrite = Arc<
    dyn Fn(&mut dyn FnMut(&mut [u8]), usize) -> Result<usize, VirtualTunWriteError> + Send + Sync,
>;

#[derive(Clone)]
pub struct VirtualTunInterface {
    mtu: usize,
    //has_data: Arc<(Mutex<()>, Condvar)>,
    on_virtual_tun_read: OnVirtualTunRead,
    on_virtual_tun_write: OnVirtualTunWrite,
}

impl<'a> VirtualTunInterface {
    pub fn new(
        _name: &str,
        on_virtual_tun_read: OnVirtualTunRead,
        on_virtual_tun_write: OnVirtualTunWrite,
        mtu: usize
    ) -> smoltcp::Result<VirtualTunInterface> {
        Ok(VirtualTunInterface {
            mtu: mtu,
            on_virtual_tun_read: on_virtual_tun_read,
            on_virtual_tun_write: on_virtual_tun_write,
        })
    }
}

impl<'d> Device<'d> for VirtualTunInterface {
    type RxToken = RxToken;
    type TxToken = TxToken;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut d = DeviceCapabilities::default();
        d.max_transmission_unit = self.mtu;
        d.medium = Medium::Ip;
        d
    }

    fn receive(&'d mut self) -> Option<(Self::RxToken, Self::TxToken)> {
        let mut buffer = vec![0; self.mtu];
        let r = (self.on_virtual_tun_read)(buffer.as_mut_slice());
        match r {
            Ok(size) => {
                buffer.resize(size, 0);
                let rx = RxToken {
                    //lower: Rc::new(RefCell::new(self.clone())),
                    buffer,
                    //size
                };
                let tx = TxToken {
                    lower: Rc::new(RefCell::new(self.clone())),
                };
                Some((rx, tx))
            },
            //Simulates a tun/tap device that returns EWOULDBLOCK
            Err(VirtualTunReadError::WouldBlock) => None,
        }
    }

    fn transmit(&'d mut self) -> Option<Self::TxToken> {
        Some(TxToken {
            lower: Rc::new(RefCell::new(self.clone())),
        })
    }
}

#[doc(hidden)]
pub struct RxToken {
    //lower: Rc<RefCell<VirtualTunInterface>>,
    buffer: Vec<u8>,
    //size: usize
}

impl phy::RxToken for RxToken {
    fn consume<R, F>(mut self, _timestamp: Instant, f: F) -> smoltcp::Result<R>
    where
        F: FnOnce(&mut [u8]) -> smoltcp::Result<R>,
    {
        //let lower = self.lower.as_ref().borrow_mut();
        f(self.buffer.as_mut_slice())
        //r
    }
}

//https://stackoverflow.com/a/66579120/5884503
//https://users.rust-lang.org/t/storing-the-return-value-from-an-fn-closure/57386/3?u=guerlando
trait CallOnceSafe<R> {
    fn call_once_safe(&mut self, x: &mut [u8]) -> smoltcp::Result<R>;
}

impl<R, F: FnOnce(&mut [u8]) -> smoltcp::Result<R>> CallOnceSafe<R> for Option<F> {
    fn call_once_safe(&mut self, x: &mut [u8]) -> smoltcp::Result<R> {
        // panics if called more than once - but A::consume() calls it only once
        let func = self.take().unwrap();
        func(x)
    }
}

#[doc(hidden)]
pub struct TxToken {
    lower: Rc<RefCell<VirtualTunInterface>>,
}

impl<'a> phy::TxToken for TxToken {
    fn consume<R, F>(self, _timestamp: Instant, len: usize, f: F) -> smoltcp::Result<R>
    where
        F: FnOnce(&mut [u8]) -> smoltcp::Result<R>,
    {
        let lower = self.lower.as_ref().borrow_mut();
        let mut r: Option<smoltcp::Result<R>> = None;
        let mut f = Some(f);

        match (lower.on_virtual_tun_write)(&mut |x| {
            r = Some(f.call_once_safe(x))
        }, len) {
            Ok(_size) => {
                //println!("WROTE {} BYTES TO VIRTUAL TUN", size);
            },
            Err(_) => {
                panic!("virtual tun receive unknown error");
            }
        }
        r.unwrap()
    }
}
