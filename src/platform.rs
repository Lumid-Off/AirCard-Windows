use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Result};

pub const APP_TITLE: &str = concat!("AirCard v", env!("CARGO_PKG_VERSION"));

pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes)
        .map_err(|err| anyhow::anyhow!("System random source failed: {err}"))?;
    Ok(bytes)
}

pub fn generate_token() -> Result<String> {
    Ok(random_bytes::<10>()?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

pub fn generate_uuid_v4() -> Result<String> {
    let mut bytes = random_bytes::<16>()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

pub fn data_dir() -> PathBuf {
    data_dir_for(std::env::consts::OS, |key| {
        std::env::var_os(key).map(PathBuf::from)
    })
}

fn data_dir_for(os: &str, env: impl Fn(&str) -> Option<PathBuf>) -> PathBuf {
    let home = env("HOME");
    let base = match os {
        "windows" => {
            env("LOCALAPPDATA").or_else(|| env("USERPROFILE").map(|p| p.join("AppData/Local")))
        }
        "macos" => home.map(|p| p.join("Library/Application Support")),
        _ => env("XDG_DATA_HOME")
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|p| p.join(".local/share"))),
    };
    base.unwrap_or_else(std::env::temp_dir).join("AirCard")
}

pub fn device_setup_help() -> &'static str {
    if cfg!(windows) {
        "Install 64-bit iTunes / Apple Mobile Device Support, then connect and trust your iPhone."
    } else if cfg!(target_os = "macos") {
        "Connect your iPhone by USB and trust it in Finder and on the phone. Uses macOS Apple frameworks."
    } else {
        "Linux supports image and theme previews. Device scanning and applying themes require Windows or macOS; Apple's AirTraffic runtime is unavailable on Linux."
    }
}

/// Set a timeout on a borrowed socket; ownership remains with MobileDevice.
pub fn set_receive_timeout(socket: i32, duration: Duration) -> Result<()> {
    if socket < 0 {
        bail!("Invalid service socket: {socket}");
    }
    #[cfg(windows)]
    let status = unsafe {
        #[link(name = "ws2_32")]
        unsafe extern "system" {
            fn setsockopt(s: usize, level: i32, name: i32, value: *const u8, len: i32) -> i32;
            fn WSAGetLastError() -> i32;
        }
        let millis = u32::try_from(duration.as_millis())
            .unwrap_or(u32::MAX)
            .max(1);
        let result = setsockopt(
            socket as usize,
            0xffff,
            0x1006,
            (&millis as *const u32).cast(),
            size_of::<u32>() as i32,
        );
        if result != 0 {
            bail!(
                "Could not set service receive timeout: Winsock error {}",
                WSAGetLastError()
            );
        }
        result
    };
    #[cfg(unix)]
    let status = unsafe {
        let timeout = libc::timeval {
            tv_sec: duration.as_secs().try_into()?,
            tv_usec: duration.subsec_micros().try_into()?,
        };
        libc::setsockopt(
            socket,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&timeout as *const libc::timeval).cast(),
            size_of::<libc::timeval>() as libc::socklen_t,
        )
    };
    if status != 0 {
        bail!(
            "Could not set service receive timeout: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_data_directories() {
        let env = |key: &str| match key {
            "HOME" => Some(PathBuf::from("/home/test")),
            "LOCALAPPDATA" => Some(PathBuf::from("local-data")),
            _ => None,
        };
        assert_eq!(
            data_dir_for("windows", env),
            PathBuf::from("local-data/AirCard")
        );
        assert_eq!(
            data_dir_for("macos", env),
            PathBuf::from("/home/test/Library/Application Support/AirCard")
        );
        assert_eq!(
            data_dir_for("linux", env),
            PathBuf::from("/home/test/.local/share/AirCard")
        );
        let xdg = |key: &str| {
            if key == "XDG_DATA_HOME" {
                Some(std::env::temp_dir())
            } else {
                env(key)
            }
        };
        assert_eq!(
            data_dir_for("linux", xdg),
            std::env::temp_dir().join("AirCard")
        );
        let relative_xdg = |key: &str| {
            if key == "XDG_DATA_HOME" {
                Some(PathBuf::from("relative"))
            } else {
                env(key)
            }
        };
        assert_eq!(
            data_dir_for("linux", relative_xdg),
            data_dir_for("linux", env)
        );
    }

    #[test]
    fn uuid_has_version_and_variant() {
        let uuid = generate_uuid_v4().unwrap();
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
        assert!(matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_ne!(uuid, generate_uuid_v4().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn receive_timeout_does_not_close_borrowed_socket() {
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixStream;
        let (socket, _peer) = UnixStream::pair().unwrap();
        set_receive_timeout(socket.as_raw_fd(), Duration::from_millis(500)).unwrap();
        assert_eq!(
            socket.read_timeout().unwrap(),
            Some(Duration::from_millis(500))
        );
        assert!(set_receive_timeout(-1, Duration::from_millis(500)).is_err());
    }
}
