//! Temporary: why does a BlobRef payload fail to decode?
use mwydyn_data::{encode, decode, DecodeOptions, Value};

fn main() {
    let digest = Value::blob_digest(b"hello", "text/plain").unwrap();
    println!("digest = {digest} (len {})", digest.len());
    let rf = Value::BlobRef { digest: digest.clone(), media_type: "text/plain".into(), size: 5 };
    let bytes = encode(&rf);
    println!("hex = {}", bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>());
    match decode(&bytes, &DecodeOptions::default()) {
        Ok(v) => println!("decoded ok: {v:?}"),
        Err(e) => println!("decode ERR: {e:?}"),
    }
}
