//! Keep Apple's native WiFi service and session alive for the shared transport.
use crate::apple::AMDServiceConnectionRef;
use crate::device::ActiveDeviceSession;
use anyhow::{Result, ensure};
use std::io::{self, Read, Write};
use std::mem::ManuallyDrop;
use std::net::TcpStream;
use std::os::windows::io::FromRawSocket;
use std::sync::Arc;
use std::time::Duration;

#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSAGetLastError() -> i32;
}

pub struct NativeStream {
    service: AMDServiceConnectionRef,
    session: Arc<ActiveDeviceSession>,
}
impl NativeStream {
    pub fn open(session: Arc<ActiveDeviceSession>, name: &str) -> Result<Self> {
        let service = session.start_service(name)?;
        ensure!(
            !service.is_null(),
            "Apple returned an empty service connection"
        );
        Ok(Self { service, session })
    }
    pub fn set_timeout(&self, timeout: Duration) -> Result<()> {
        let socket = unsafe { (self.session.libs.amd_service_connection_get_socket)(self.service) };
        ensure!(socket > 0, "Invalid Apple service socket");
        // Borrow the socket only for configuration; Apple owns closing it.
        let borrowed = ManuallyDrop::new(unsafe {
            TcpStream::from_raw_socket(socket as std::os::windows::io::RawSocket)
        });
        borrowed.set_read_timeout(Some(timeout))?;
        borrowed.set_write_timeout(Some(timeout))?;
        Ok(())
    }
}
impl Drop for NativeStream {
    fn drop(&mut self) {
        unsafe {
            (self.session.libs.amd_service_connection_invalidate)(self.service);
        }
    }
}
fn io_result(n: i32, capacity: usize) -> io::Result<usize> {
    if n < 0 {
        let code = unsafe { WSAGetLastError() };
        return Err(if code != 0 {
            io::Error::from_raw_os_error(code)
        } else {
            io::Error::other("Apple service I/O failed")
        });
    }
    if n as usize > capacity {
        return Err(io::Error::other("Invalid Apple service byte count"));
    }
    Ok(n as usize)
}
impl Read for NativeStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let n = unsafe {
            (self.session.libs.amd_service_connection_receive)(
                self.service,
                bytes.as_mut_ptr(),
                bytes.len(),
            )
        };
        io_result(n, bytes.len())
    }
}
impl Write for NativeStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let n = unsafe {
            (self.session.libs.amd_service_connection_send)(
                self.service,
                bytes.as_ptr(),
                bytes.len(),
            )
        };
        io_result(n, bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
