use anyhow::{Result, ensure};
use plist::Value;
use std::io::{Read, Write};
pub fn send_plist<S: Write>(s: &mut S, value: &Value, little: bool) -> Result<()> {
    let mut b = Vec::new();
    value.to_writer_binary(&mut b)?;
    ensure!(b.len() <= 32 * 1024 * 1024, "Service message too large");
    let n = b.len() as u32;
    s.write_all(&if little {
        n.to_le_bytes()
    } else {
        n.to_be_bytes()
    })?;
    s.write_all(&b)?;
    s.flush()?;
    Ok(())
}
pub fn receive_plist<S: Read>(s: &mut S, little: bool) -> Result<Value> {
    let mut h = [0; 4];
    s.read_exact(&mut h)?;
    let n = if little {
        u32::from_le_bytes(h)
    } else {
        u32::from_be_bytes(h)
    } as usize;
    ensure!(
        n > 0 && n <= 32 * 1024 * 1024,
        "Invalid service message length"
    );
    let mut b = vec![0; n];
    s.read_exact(&mut b)?;
    Ok(Value::from_reader(std::io::Cursor::new(b))?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_byte_orders_and_oversize() {
        for little in [false, true] {
            let v = Value::String("transport".into());
            let mut b = vec![];
            send_plist(&mut b, &v, little).unwrap();
            assert_eq!(
                receive_plist(&mut std::io::Cursor::new(b), little).unwrap(),
                v
            );
            assert!(receive_plist(&mut std::io::Cursor::new([255; 4]), little).is_err());
        }
    }
}
