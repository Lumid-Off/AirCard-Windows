#[allow(dead_code)]
#[path = "../src/platform.rs"]
mod platform;

#[test]
fn test_hex_token() {
    let token = platform::generate_token().unwrap();
    assert_eq!(token.len(), 20);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(token, platform::generate_token().unwrap());
}
