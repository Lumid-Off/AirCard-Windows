use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn invalid_environment_requests_exit_without_opening_a_window() {
    for input in ["not json", "{}"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_aircard"))
            .arg("--environment-worker")
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
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Invalid diagnostic request did not terminate");
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
        let check: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(check["status"], "Fail");
        assert_eq!(check["title"], "Environment worker");
    }
}
