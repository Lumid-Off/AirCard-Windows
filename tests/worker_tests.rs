//! Exercise worker entry/exit without Apple components or a connected phone.
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn invalid_sync_jobs_exit_without_opening_the_ui_or_contacting_a_device() {
    for input in ["not json", "{}"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_aircard"))
            .arg("--native-sync-worker")
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Invalid worker job failed to terminate");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(!status.success());
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(output.contains("Native sync failed:"), "{output}");
    }
}
