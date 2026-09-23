//! An independent fake USB peer exercises the real framing, pairing, TLS,
//! encrypted persistence and syslog connection without touching a phone.
use super::*;
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPublicKey, pkcs8::EncodePrivateKey};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::net::TcpListener;
use std::sync::Arc;
const UDID: &str = "000081300000000000000001";

#[test]
#[ignore = "requires iPhone; round-trip only in unique app test directory"]
fn connected_iphone_afc_and_sync_roundtrip() {
    let mut session =
        DeviceSession::open(None, crate::device::ConnectionMode::Usb, &AtomicBool::new(false), &mut |s| eprintln!("{s}")).unwrap();
    let afc = crate::afc::AfcClient::new(&mut session).unwrap();
    let dir = format!("aircard-test-{}", uuid::Uuid::new_v4());
    afc.make_directory(&dir).unwrap();
    let seed = format!("{dir}/seed");
    afc.write_file(&seed, b"AFC round trip").unwrap();
    assert_eq!(afc.read_file(&seed).unwrap(), b"AFC round trip");
    let device = crate::device::list_connected_devices().unwrap().remove(0);
    let result = crate::flasher::write_system_files_batch(
        &device.udid,
        crate::device::ConnectionMode::Usb,
        &format!("/var/mobile/Media/{dir}"),
        &[("first", b"first payload"), ("second", b"second payload")],
        |s| eprintln!("{s}"),
    );
    let check = (|| -> Result<()> {
        result?;
        ensure!(
            afc.read_file(&format!("{dir}/first"))? == b"first payload",
            "First payload mismatch"
        );
        ensure!(
            afc.read_file(&format!("{dir}/second"))? == b"second payload",
            "Second payload mismatch"
        );
        Ok(())
    })();
    for name in ["seed", "first", "second"] {
        let _ = afc.remove_path(&format!("{dir}/{name}"));
    }
    let _ = afc.remove_path(&dir);
    check.unwrap();
    eprintln!("Real AFC + StreamingZip + AirTraffic read-back passed; Wallet cards untouched.");
}

#[test]
#[ignore = "requires a connected unlocked iPhone; opens syslog without writing cards"]
fn connected_iphone_tls_and_syslog() {
    let mut connection = WalletConnection::open(None, crate::device::ConnectionMode::Usb, &AtomicBool::new(false), &mut |line| {
        eprintln!("{line}")
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buffer = [0; 4096];
    loop {
        match connection.stream.read(&mut buffer) {
            Ok(0) => panic!("iPhone closed the log stream"),
            Ok(n) => {
                eprintln!("Real device syslog received: {n} bytes (contents omitted)");
                break;
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                assert!(
                    Instant::now() < deadline,
                    "Connected, but no syslog data in 10 seconds"
                );
            }
            Err(e) => panic!("Real syslog read: {e}"),
        }
    }
}
const LOG: &[u8] = b"passd: /var/mobile/Library/Passes/Cards/OM6NYhwXMZrAw0sRUjR62wmF4ZQ=.pkpass\n";

#[test]
#[ignore = "requires AIRCARD_TEST_PYTHON pointing to Python with OpenSSL"]
fn openssl_tls12_rsa_cbc_client_auth_and_pin() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let python = std::env::var("AIRCARD_TEST_PYTHON").expect("Set AIRCARD_TEST_PYTHON");
    let key = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
    let public = key
        .to_public_key()
        .to_pkcs1_pem(Default::default())
        .unwrap();
    let record = pairing::generate(public.as_bytes(), "openssl-test").unwrap();
    let directory = std::env::temp_dir().join(format!("aircard-tls-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("device.pem"),
        record.data("DeviceCertificate").unwrap(),
    )
    .unwrap();
    std::fs::write(
        directory.join("root.pem"),
        record.data("RootCertificate").unwrap(),
    )
    .unwrap();
    std::fs::write(
        directory.join("key.pem"),
        key.to_pkcs8_pem(Default::default()).unwrap().as_bytes(),
    )
    .unwrap();
    for reject in [false, true] {
        let mut child = Command::new(&python)
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/tls_peer.py"))
            .arg(&directory)
            .arg(if reject { "reject" } else { "accept" })
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let port: u16 = line.trim().parse().expect("OpenSSL peer startup");
        let socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut expected = PairRecord(record.0.clone());
        if reject {
            // A valid, different certificate: TLS can succeed but pinning must fail.
            expected.0.insert(
                "DeviceCertificate".into(),
                Value::Data(record.data("RootCertificate").unwrap().to_vec()),
            );
        }
        let result = secure(socket, &expected, "Independent OpenSSL regression");
        if reject {
            let error = result.err().expect("wrong peer must be rejected");
            assert!(format!("{error:#}").contains("does not match"), "{error:#}");
        } else {
            let mut stream = result.unwrap();
            stream.write_all(b"ping").unwrap();
            let mut reply = [0; 4];
            stream.read_exact(&mut reply).unwrap();
            assert_eq!(&reply, b"pong");
        }
        assert!(child.wait().unwrap().success());
    }
    std::fs::remove_dir_all(directory).unwrap();
}

fn accept(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match listener.accept() {
            Ok((s, _)) => {
                s.set_nonblocking(false).unwrap();
                s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
                s.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
                return s;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "mock accept timed out");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("mock accept: {e}"),
        }
    }
}
fn receive<S: Read>(s: &mut S, mux: bool) -> Dictionary {
    let mut h = vec![0; if mux { 16 } else { 4 }];
    s.read_exact(&mut h).unwrap();
    let n = if mux {
        u32::from_le_bytes(h[..4].try_into().unwrap()) as usize - 16
    } else {
        u32::from_be_bytes(h[..4].try_into().unwrap()) as usize
    };
    assert!(n < 128 * 1024);
    let mut b = vec![0; n];
    s.read_exact(&mut b).unwrap();
    Value::from_reader(std::io::Cursor::new(b))
        .unwrap()
        .into_dictionary()
        .unwrap()
}
fn send<S: Write>(s: &mut S, mux: bool, value: Value) {
    let mut b = Vec::new();
    value.to_writer_xml(&mut b).unwrap();
    let mut packet = Vec::new();
    if mux {
        for n in [(b.len() + 16) as u32, 1, 8, 1] {
            packet.extend(n.to_le_bytes())
        }
    } else {
        packet.extend((b.len() as u32).to_be_bytes());
    }
    packet.extend(b);
    // Force fragmented headers and payloads at the transport boundary.
    for part in packet.chunks(113) {
        s.write_all(part).unwrap();
    }
    s.flush().unwrap();
}
fn accept_channel(listener: &TcpListener, port: u16) -> TcpStream {
    let mut s = accept(listener);
    let m = receive(&mut s, true);
    assert_eq!(m["MessageType"].as_string(), Some("Connect"));
    assert_eq!(
        m["PortNumber"].as_unsigned_integer(),
        Some(u64::from(port.to_be()))
    );
    send(&mut s, true, dict([("Number", Value::Integer(0.into()))]));
    s
}
fn tls_server(
    s: TcpStream,
    pair: &Dictionary,
    private: &[u8],
) -> StreamOwned<ServerConnection, TcpStream> {
    let cert = rustls_pemfile::certs(&mut std::io::Cursor::new(
        pair["DeviceCertificate"].as_data().unwrap(),
    ))
    .next()
    .unwrap()
    .unwrap();
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert],
                rustls::pki_types::PrivatePkcs8KeyDer::from(private.to_vec()).into(),
            )
            .unwrap();
    StreamOwned::new(ServerConnection::new(Arc::new(config)).unwrap(), s)
}

#[test]
fn full_usb_pair_tls_scan_then_renew_stale_identity() {
    let key = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
    let public = key
        .to_public_key()
        .to_pkcs1_pem(Default::default())
        .unwrap()
        .into_bytes();
    let private = key.to_pkcs8_der().unwrap().as_bytes().to_vec();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let mut pair: Option<Dictionary> = None;
        let mut pair_count = 0;
        for scan in 0..2 {
            let mut list = accept(&listener);
            let message = receive(&mut list, true);
            assert_eq!(message["MessageType"].as_string(), Some("ListDevices"));
            let properties = dict([
                ("ConnectionType", string("USB")),
                ("DeviceID", Value::Integer(9.into())),
                ("SerialNumber", string(UDID)),
            ]);
            let entries = Value::Array(vec![dict([("Properties", properties)])]);
            send(&mut list, true, dict([("DeviceList", entries)]));
            drop(list);
            let mut socket = accept_channel(&listener, 62078);
            let mut renewed = false;
            loop {
                let message = receive(&mut socket, false);
                match message["Request"].as_string().unwrap() {
                    "GetValue" => send(
                        &mut socket,
                        false,
                        dict([("Value", Value::Data(public.clone()))]),
                    ),
                    "Pair" => {
                        let incoming = message["PairRecord"].as_dictionary().unwrap();
                        assert!(!incoming.contains_key("HostPrivateKey"));
                        assert!(!incoming.contains_key("RootPrivateKey"));
                        pair = Some(incoming.clone());
                        pair_count += 1;
                        send(
                            &mut socket,
                            false,
                            dict([
                                ("Request", string("Pair")),
                                ("EscrowBag", Value::Data(vec![1, 2, 3])),
                            ]),
                        );
                    }
                    "StartSession" => {
                        if scan == 1 && !renewed {
                            send(
                                &mut socket,
                                false,
                                dict([("Error", string("InvalidHostID"))]),
                            );
                            drop(socket);
                            socket = accept_channel(&listener, 62078);
                            renewed = true;
                            continue;
                        }
                        assert_eq!(message["HostID"], pair.as_ref().unwrap()["HostID"]);
                        send(
                            &mut socket,
                            false,
                            dict([
                                ("SessionID", string("mock-session")),
                                ("EnableSessionSSL", Value::Boolean(true)),
                            ]),
                        );
                        break;
                    }
                    request => panic!("Unexpected request: {request}"),
                }
            }
            let mut control = tls_server(socket, pair.as_ref().unwrap(), &private);
            let request = receive(&mut control, false);
            assert_eq!(
                request["Service"].as_string(),
                Some("com.apple.syslog_relay")
            );
            send(
                &mut control,
                false,
                dict([
                    ("Port", Value::Integer(12345.into())),
                    ("EnableServiceSSL", Value::Boolean(true)),
                ]),
            );
            let stream = accept_channel(&listener, 12345);
            let mut log = tls_server(stream, pair.as_ref().unwrap(), &private);
            log.write_all(LOG).unwrap();
            log.flush().unwrap();
            // Keep TLS sockets alive until the client consumes the log.
            std::thread::sleep(Duration::from_millis(150));
        }
        assert_eq!(
            pair_count, 2,
            "one initial pairing and one stale-identity repair"
        );
    });
    let directory =
        std::env::temp_dir().join(format!("aircard-protocol-test-{}", uuid::Uuid::new_v4()));
    let mut logs = Vec::new();
    for _ in 0..2 {
        let mut connection = WalletConnection::open_with(
            UsbMux::at(address),
            PairStore::at(directory.clone()),
            Some("00008130-0000000000000001"),
            &AtomicBool::new(false),
            &mut |s| logs.push(s),
        )
        .unwrap();
        let mut received = vec![0; LOG.len()];
        connection.stream.read_exact(&mut received).unwrap();
        assert_eq!(
            crate::scanner::extract_card_hashes(std::str::from_utf8(&received).unwrap()),
            vec!["OM6NYhwXMZrAw0sRUjR62wmF4ZQ="]
        );
    }
    peer.join().unwrap();
    assert!(logs.iter().any(|s| s.contains("Renewing app pairing once")));
    std::fs::remove_dir_all(directory).unwrap();
}
