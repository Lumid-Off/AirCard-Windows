//! AFC over the same app-owned paired session as Wallet scanning.
use crate::wallet_connection::{DeviceSession, DeviceStream};
use anyhow::{Context, Result, ensure};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::{Read, Write},
    time::Duration,
};
const LIMIT: usize = 32 * 1024 * 1024;
struct Wire<S> {
    stream: S,
    sequence: u64,
}
#[derive(Debug)]
struct Status(u64);
impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AFC status {}", self.0)
    }
}
impl std::error::Error for Status {}
impl<S: Read + Write> Wire<S> {
    fn call(&mut self, op: u64, args: &[u8], payload: &[u8]) -> Result<(u64, Vec<u8>)> {
        self.sequence += 1;
        let mut packet = b"CFA6LPAA".to_vec();
        for n in [
            (40 + args.len() + payload.len()) as u64,
            (40 + args.len()) as u64,
            self.sequence,
            op,
        ] {
            packet.extend(n.to_le_bytes());
        }
        packet.extend(args);
        packet.extend(payload);
        self.stream.write_all(&packet)?;
        self.stream.flush()?;
        let mut h = [0; 40];
        self.stream.read_exact(&mut h)?;
        ensure!(&h[..8] == b"CFA6LPAA", "Invalid AFC magic");
        let n = u64::from_le_bytes(h[8..16].try_into()?);
        let head = u64::from_le_bytes(h[16..24].try_into()?);
        let seq = u64::from_le_bytes(h[24..32].try_into()?);
        let code = u64::from_le_bytes(h[32..40].try_into()?);
        ensure!(
            n >= 40 && n <= LIMIT as u64 && head >= 40 && head <= n,
            "Invalid AFC frame length"
        );
        ensure!(seq == self.sequence, "AFC sequence mismatch");
        let mut data = vec![0; (n - 40) as usize];
        self.stream.read_exact(&mut data)?;
        if code == 1 {
            ensure!(data.len() == 8, "Invalid AFC status size");
            let status = u64::from_le_bytes(data[..8].try_into()?);
            if status != 0 {
                return Err(Status(status).into());
            }
        }
        Ok((code, data))
    }
}
fn path_arg(path: &str) -> Result<Vec<u8>> {
    ensure!(!path.as_bytes().contains(&0), "NUL in AFC path");
    let mut b = path.as_bytes().to_vec();
    b.push(0);
    Ok(b)
}
pub struct AfcClient {
    wire: RefCell<Wire<DeviceStream>>,
}
impl AfcClient {
    pub fn new(session: &mut DeviceSession) -> Result<Self> {
        let stream = session.start_service("com.apple.afc")?;
        stream.set_timeout(Duration::from_secs(20))?;
        Ok(Self {
            wire: RefCell::new(Wire {
                stream,
                sequence: 0,
            }),
        })
    }
    fn call(&self, op: u64, args: &[u8], payload: &[u8], expected: u64) -> Result<Vec<u8>> {
        let (code, data) = self.wire.borrow_mut().call(op, args, payload)?;
        ensure!(
            code == expected,
            "Unexpected AFC response {code}, expected {expected}"
        );
        Ok(data)
    }
    fn info(&self, path: &str) -> Result<HashMap<String, String>> {
        let data = self.call(10, &path_arg(path)?, &[], 2)?;
        let fields: Vec<_> = data.split(|b| *b == 0).filter(|s| !s.is_empty()).collect();
        ensure!(fields.len() % 2 == 0, "Malformed AFC file information");
        fields
            .chunks(2)
            .map(|p| {
                Ok((
                    std::str::from_utf8(p[0])?.to_owned(),
                    std::str::from_utf8(p[1])?.to_owned(),
                ))
            })
            .collect()
    }
    pub fn try_exists(&self, path: &str) -> Result<bool> {
        match self.info(path) {
            Ok(_) => Ok(true),
            Err(e) if e.downcast_ref::<Status>().is_some_and(|s| s.0 == 8) => Ok(false),
            Err(e) => Err(e),
        }
    }
    pub fn exists(&self, path: &str) -> bool {
        self.try_exists(path).unwrap_or(false)
    }
    fn open_file(&self, path: &str, mode: u64) -> Result<u64> {
        let mut args = mode.to_le_bytes().to_vec();
        args.extend(path_arg(path)?);
        let d = self
            .call(13, &args, &[], 14)
            .with_context(|| format!("Open AFC file {path}"))?;
        ensure!(d.len() == 8, "Invalid AFC file handle");
        Ok(u64::from_le_bytes(d[..8].try_into()?))
    }
    fn close_file(&self, handle: u64) -> Result<()> {
        self.call(20, &handle.to_le_bytes(), &[], 1)?;
        Ok(())
    }
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let size: usize = self
            .info(path)?
            .get("st_size")
            .context("AFC size missing")?
            .parse()?;
        ensure!(
            size <= 128 * 1024 * 1024,
            "AFC file exceeds backup size limit"
        );
        let handle = self.open_file(path, 1)?;
        let result = (|| {
            let mut out = Vec::with_capacity(size);
            while out.len() < size {
                let mut args = handle.to_le_bytes().to_vec();
                args.extend(((size - out.len()).min(65536) as u64).to_le_bytes());
                let d = self.call(15, &args, &[], 2)?;
                ensure!(
                    !d.is_empty() && out.len() + d.len() <= size,
                    "Unexpected AFC read size"
                );
                out.extend(d);
            }
            Ok(out)
        })();
        let closed = self.close_file(handle);
        let out = result?;
        closed?;
        Ok(out)
    }
    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<()> {
        let handle = self.open_file(path, 3)?;
        let result = (|| {
            for chunk in data.chunks(65536) {
                self.call(16, &handle.to_le_bytes(), chunk, 1)?;
            }
            Ok::<_, anyhow::Error>(())
        })();
        let closed = self.close_file(handle);
        result?;
        closed?;
        Ok(())
    }
    pub fn make_directory(&self, path: &str) -> Result<()> {
        if self.try_exists(path)? {
            return Ok(());
        }
        self.call(9, &path_arg(path)?, &[], 1)?;
        Ok(())
    }
    pub fn make_directory_recursive(&self, path: &str) -> Result<()> {
        let mut current = String::new();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            ensure!(
                part != "." && part != "..",
                "Invalid AFC directory component"
            );
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(part);
            self.make_directory(&current)?;
        }
        Ok(())
    }
    pub fn remove_path(&self, path: &str) -> Result<()> {
        if self.try_exists(path)? {
            self.call(8, &path_arg(path)?, &[], 1)?;
        }
        Ok(())
    }
    pub fn list_directory(&self, path: &str) -> Result<Vec<String>> {
        self.call(3, &path_arg(path)?, &[], 2)?
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty() && *p != b"." && *p != b"..")
            .map(|p| Ok(std::str::from_utf8(p)?.to_owned()))
            .collect()
    }
    pub fn remove_tree(&self, path: &str) -> Result<()> {
        ensure!(
            path.starts_with("airlift-src-") && !path.contains('/') && !path.contains('\\'),
            "Cleanup must target an app staging directory"
        );
        self.remove_tree_inner(path, 0)
    }
    fn remove_tree_inner(&self, path: &str, depth: usize) -> Result<()> {
        ensure!(depth <= 32, "AFC cleanup nesting limit");
        if !self.try_exists(path)? {
            return Ok(());
        }
        if self.info(path)?.get("st_ifmt").map(String::as_str) == Some("S_IFDIR") {
            for child in self.list_directory(path)? {
                ensure!(
                    !child.contains('/') && !child.contains('\\'),
                    "Invalid AFC child name"
                );
                self.remove_tree_inner(&format!("{path}/{child}"), depth + 1)?;
            }
        }
        self.remove_path(path)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        input: std::io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Read for Fake {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            let n = b.len().min(3);
            self.input.read(&mut b[..n])
        }
    }
    impl Write for Fake {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.output.extend(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn peer(length: u64, status: u64) -> Wire<Fake> {
        let mut d = b"CFA6LPAA".to_vec();
        for n in [length, 48, 1, 1, status] {
            d.extend(n.to_le_bytes());
        }
        Wire {
            stream: Fake {
                input: std::io::Cursor::new(d),
                output: vec![],
            },
            sequence: 0,
        }
    }
    #[test]
    fn fragmented_status_and_write_header() {
        let mut p = peer(48, 0);
        p.call(16, &7u64.to_le_bytes(), b"abc").unwrap();
        let d = &p.stream.output;
        assert_eq!(u64::from_le_bytes(d[8..16].try_into().unwrap()), 51);
        assert_eq!(u64::from_le_bytes(d[16..24].try_into().unwrap()), 48);
        assert_eq!(&d[48..], b"abc");
    }
    #[test]
    fn oversized_and_error_status_are_rejected() {
        assert!(peer(u64::MAX, 0).call(3, b"", b"").is_err());
        let e = peer(48, 8).call(3, b"", b"").unwrap_err();
        assert_eq!(e.downcast_ref::<Status>().unwrap().0, 8);
    }
}
