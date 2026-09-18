#[test]
fn test_hex_token() {
    let mut bytes = [0u8; 10];
    unsafe {
        #[link(name = "bcrypt")]
        unsafe extern "system" {
            fn BCryptGenRandom(
                hAlgorithm: *mut std::ffi::c_void,
                pbBuffer: *mut u8,
                cbBuffer: u32,
                dwFlags: u32,
            ) -> i32;
        }
        let status = BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 10, 2);
        assert_eq!(status, 0);
    }
    let token: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    assert_eq!(token.len(), 20);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
}
