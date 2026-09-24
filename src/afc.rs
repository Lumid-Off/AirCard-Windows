use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::apple::{
    AFCConnectionRef, AFCDirectoryRef, AFCFileRef, AFCKeyValueRef, AMDServiceConnectionRef,
    AppleLibraries, get_apple_libraries,
};
use crate::device::ActiveDeviceSession;

pub struct AfcClient {
    libs: Arc<AppleLibraries>,
    conn: AFCConnectionRef,
    service_conn: AMDServiceConnectionRef,
}

impl Drop for AfcClient {
    fn drop(&mut self) {
        unsafe {
            if !self.conn.is_null() {
                (self.libs.afc_connection_close)(self.conn);
            }
            if !self.service_conn.is_null() {
                (self.libs.amd_service_connection_invalidate)(self.service_conn);
            }
        }
    }
}

impl AfcClient {
    pub fn new(session: &ActiveDeviceSession) -> Result<Self> {
        let libs = get_apple_libraries()?;
        let service_conn = session.start_service("com.apple.afc")?;

        unsafe {
            let socket = (libs.amd_service_connection_get_socket)(service_conn);
            let mut conn: AFCConnectionRef = ptr::null_mut();
            let status = (libs.afc_connection_open)(socket, 0, &mut conn);
            if status != 0 || conn.is_null() {
                (libs.amd_service_connection_invalidate)(service_conn);
                bail!("AFCConnectionOpen failed with code {}", status);
            }

            let secure_context = (libs.amd_service_connection_get_secure_io_context)(service_conn);
            if !secure_context.is_null() {
                (libs.afc_connection_set_secure_context)(conn, secure_context);
                (libs.afc_connection_set_dispose_secure_context)(conn, 0);
                (libs.afc_connection_set_io_timeout)(conn, 30);
            }

            Ok(Self {
                libs,
                conn,
                service_conn,
            })
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        let Ok(c_path) = CString::new(path) else {
            return false;
        };
        unsafe {
            let mut info: AFCKeyValueRef = ptr::null_mut();
            let status = (self.libs.afc_file_info_open)(self.conn, c_path.as_ptr(), &mut info);
            if !info.is_null() {
                (self.libs.afc_key_value_close)(info);
            }
            status == 0
        }
    }

    pub fn file_size(&self, path: &str) -> Option<usize> {
        let c_path = CString::new(path).ok()?;
        unsafe {
            let mut info: AFCKeyValueRef = ptr::null_mut();
            if (self.libs.afc_file_info_open)(self.conn, c_path.as_ptr(), &mut info) != 0
                || info.is_null()
            {
                return None;
            }

            let mut size = None;
            let mut key: *const std::ffi::c_char = ptr::null();
            let mut val: *const std::ffi::c_char = ptr::null();

            while (self.libs.afc_key_value_read)(info, &mut key, &mut val) == 0
                && !key.is_null()
                && !val.is_null()
            {
                if let (Ok(k), Ok(v)) = (CStr::from_ptr(key).to_str(), CStr::from_ptr(val).to_str())
                {
                    if k == "st_size" {
                        if let Ok(num) = v.parse::<usize>() {
                            size = Some(num);
                            break;
                        }
                    }
                }
                key = ptr::null();
                val = ptr::null();
            }

            (self.libs.afc_key_value_close)(info);
            size
        }
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let c_path = CString::new(path).context("Path contains null byte")?;
        let size = self
            .file_size(path)
            .context("Could not get file size for reading")?;

        unsafe {
            let mut file: AFCFileRef = 0;
            let open_status = (self.libs.afc_file_ref_open)(
                self.conn,
                c_path.as_ptr(),
                1, /* read */
                &mut file,
            );
            if open_status != 0 || file == 0 {
                bail!(
                    "AFCFileRefOpen failed for {} with code {}",
                    path,
                    open_status
                );
            }

            let mut data = vec![0u8; size];
            let mut total_read = 0;

            while total_read < size {
                let mut chunk_len = (size - total_read) as isize;
                let read_status = (self.libs.afc_file_ref_read)(
                    self.conn,
                    file,
                    data.as_mut_ptr().add(total_read),
                    &mut chunk_len,
                );
                if read_status != 0 || chunk_len <= 0 {
                    let _ = (self.libs.afc_file_ref_close)(self.conn, file);
                    bail!(
                        "AFCFileRefRead failed after {} bytes with code {}",
                        total_read,
                        read_status
                    );
                }
                total_read += chunk_len as usize;
            }

            let close_status = (self.libs.afc_file_ref_close)(self.conn, file);
            if close_status != 0 {
                bail!("AFCFileRefClose failed with code {}", close_status);
            }

            Ok(data)
        }
    }

    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<()> {
        let c_path = CString::new(path).context("Path contains null byte")?;
        unsafe {
            let mut file: AFCFileRef = 0;
            let open_status = (self.libs.afc_file_ref_open)(
                self.conn,
                c_path.as_ptr(),
                3, /* write */
                &mut file,
            );
            if open_status != 0 || file == 0 {
                bail!(
                    "AFCFileRefOpen failed for {} with code {}",
                    path,
                    open_status
                );
            }

            let write_status = if data.is_empty() {
                0
            } else {
                (self.libs.afc_file_ref_write)(self.conn, file, data.as_ptr(), data.len() as isize)
            };

            let close_status = (self.libs.afc_file_ref_close)(self.conn, file);
            if write_status != 0 || close_status != 0 {
                bail!(
                    "AFC write failed: write_status={}, close_status={}",
                    write_status,
                    close_status
                );
            }

            Ok(())
        }
    }

    pub fn make_directory(&self, path: &str) -> Result<()> {
        if self.exists(path) {
            return Ok(());
        }
        let c_path = CString::new(path).context("Path contains null byte")?;
        let status = unsafe { (self.libs.afc_directory_create)(self.conn, c_path.as_ptr()) };
        if status != 0 && !self.exists(path) {
            bail!(
                "AFCDirectoryCreate failed for {} with code {}",
                path,
                status
            );
        }
        Ok(())
    }

    pub fn make_directory_recursive(&self, path: &str) -> Result<()> {
        let mut current = String::new();
        for part in path.split('/') {
            if part.is_empty() {
                continue;
            }
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(part);
            self.make_directory(&current)?;
        }
        Ok(())
    }

    pub fn remove_path(&self, path: &str) -> Result<()> {
        if !self.exists(path) {
            return Ok(());
        }
        let c_path = CString::new(path).context("Path contains null byte")?;
        let status = unsafe { (self.libs.afc_remove_path)(self.conn, c_path.as_ptr()) };
        if status != 0 && self.exists(path) {
            bail!("AFCRemovePath failed for {} with code {}", path, status);
        }
        Ok(())
    }

    pub fn list_directory(&self, path: &str) -> Result<Vec<String>> {
        let c_path = CString::new(path).context("Path contains null byte")?;
        unsafe {
            let mut dir: AFCDirectoryRef = ptr::null_mut();
            let open_status = (self.libs.afc_directory_open)(self.conn, c_path.as_ptr(), &mut dir);
            if open_status != 0 || dir.is_null() {
                bail!(
                    "AFCDirectoryOpen failed for {} with code {}",
                    path,
                    open_status
                );
            }

            let mut entries = Vec::new();
            let mut entry_ptr: *const std::ffi::c_char = ptr::null();

            for _ in 0..8192 {
                let read_status = (self.libs.afc_directory_read)(self.conn, dir, &mut entry_ptr);
                if read_status != 0 || entry_ptr.is_null() {
                    break;
                }
                if let Ok(name) = CStr::from_ptr(entry_ptr).to_str() {
                    if name != "." && name != ".." {
                        entries.push(name.to_owned());
                    }
                }
                entry_ptr = ptr::null();
            }

            let _ = (self.libs.afc_directory_close)(self.conn, dir);
            Ok(entries)
        }
    }

    pub fn remove_tree(&self, path: &str) -> Result<()> {
        self.remove_tree_internal(path, 0)
    }

    fn remove_tree_internal(&self, path: &str, depth: usize) -> Result<()> {
        if depth > 32 || !self.exists(path) {
            return Ok(());
        }

        if let Ok(children) = self.list_directory(path) {
            for child in children {
                let child_path = format!("{}/{}", path.trim_end_matches('/'), child);
                self.remove_tree_internal(&child_path, depth + 1)?;
            }
        }

        self.remove_path(path)
    }
}
