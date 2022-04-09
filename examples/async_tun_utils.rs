use core::task::{Context, Poll};
use core::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::io::{AsyncRead, AsyncWrite};
use hyper::service::Service;
use hyper::client::connect::{Connection, Connected};
use async_smoltcp::AsyncRW;

#[derive(Clone)]
pub struct AsyncTransporter {
    stream: Arc<Mutex<dyn AsyncRW + Send + Sync + Unpin>>,
}

impl Service<hyper::Uri> for AsyncTransporter {
    type Response = CustomResponse;
    type Error = hyper::http::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: hyper::Uri) -> Self::Future {
        let resp = CustomResponse{
            stream: self.stream.clone()
        };
         
        let fut = async move{
            Ok(resp)
        };

        Box::pin(fut)
    }
}

impl AsyncTransporter {
    pub fn new(stream: Arc<Mutex<dyn AsyncRW + Send + Sync + Unpin>>) -> AsyncTransporter {
        AsyncTransporter{
            stream: stream.clone()
        }
    }
}

impl Connection for AsyncTransporter {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

pub struct CustomResponse {
    stream: Arc<Mutex<dyn AsyncRW + Send + Sync + Unpin>>,
}

impl Connection for CustomResponse {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

const E: &str = "tried to unlock locked stream on custom response";

impl AsyncRead for CustomResponse {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.stream.try_lock().expect(E)).poll_read(cx, buf)
    }
}

impl AsyncWrite for CustomResponse {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8]
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut *self.stream.try_lock().expect(E)).poll_write(cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut *self.stream.try_lock().expect(E)).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>
    ) -> Poll<Result<(), std::io::Error>>
    {
        Pin::new(&mut *self.stream.try_lock().expect(E)).poll_shutdown(cx)
    }
}

fn main() {
    
}
