//! Run Apple native AirTraffic authentication in a bounded worker process.
use crate::device::DeviceTransport;
use anyhow::{Context, Result, ensure};
use std::time::{Duration, Instant};
#[derive(serde::Serialize, serde::Deserialize)]
struct SyncJob {
    udid: String,
    transport: DeviceTransport,
    assets: Vec<(String, String)>,
}

pub fn run_native_worker() -> i32 {
    let result = (|| -> Result<()> {
        use std::io::{Read, Write};
        let mut input = Vec::new();
        std::io::stdin()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut input)?;
        ensure!(input.len() <= 1024 * 1024, "Sync job too large");
        let job: SyncJob = serde_json::from_slice(&input)?;
        let refs: Vec<_> = job
            .assets
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        crate::native_sync::run(&job.udid, job.transport, &refs, |message| {
            println!("{message}");
            let _ = std::io::stdout().flush();
        })
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            println!("Native sync failed: {e:#}");
            1
        }
    }
}

fn native_worker_sync<L: FnMut(&str)>(
    udid: &str,
    transport: DeviceTransport,
    assets: &[(&str, &str)],
    log: &mut L,
) -> Result<()> {
    use std::io::{BufRead, Write};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let job = SyncJob {
        udid: udid.into(),
        transport,
        assets: assets
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
    };
    let payload = serde_json::to_vec(&job)?;
    #[cfg(not(test))]
    let executable = std::env::current_exe()?;
    #[cfg(test)]
    let executable =
        std::path::PathBuf::from(std::env::var_os("AIRCARD_NATIVE_WORKER").context(
            "Hardware tests require AIRCARD_NATIVE_WORKER pointing to a built aircard.exe",
        )?);
    let mut child = Command::new(executable)
        .arg("--native-sync-worker")
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Cannot start Apple sync worker")?;
    let result = (|| -> Result<()> {
        child
            .stdin
            .take()
            .context("Sync worker input unavailable")?
            .write_all(&payload)?;
        let output = child
            .stdout
            .take()
            .context("Sync worker output unavailable")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(output).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let deadline = Instant::now()
            + Duration::from_secs(
                (if transport == DeviceTransport::Wifi {
                    120
                } else {
                    60
                })
                .max(assets.len() as u64 * 2),
            );
        loop {
            while let Ok(line) = rx.try_recv() {
                log(&line);
            }
            if let Some(status) = child.try_wait()? {
                for line in rx {
                    log(&line);
                }
                ensure!(
                    status.success(),
                    "Apple native sync worker failed; see preceding diagnostic"
                );
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Apple native sync timed out; worker stopped before restoring Books state"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    // Do not restore device state until the worker can no longer send writes.
    child.wait().context("Failed to reap Apple sync worker")?;
    result
}
pub fn sync_assets_via_airtraffic<L: FnMut(&str)>(
    udid: &str,
    transport: DeviceTransport,
    assets: &[(&str, &str)],
    mut log: L,
) -> Result<()> {
    native_worker_sync(udid, transport, assets, &mut log)
}
