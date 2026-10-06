//! Release signing helper (dev/CI only, not part of the app).
//!
//!   cargo run --release --example sign -- keygen <secret-key-out> <public-key-out>
//!   cargo run --release --example sign -- sign <file> <secret-key-file> <trusted-comment>
//!   cargo run --release --example sign -- verify <file> <public-key-file> <trusted-comment>
//!
//! `sign` writes `<file>.minisig`. The app only accepts a signature whose trusted comment is
//! exactly "cursor-stream-switcher <version>".

use minisign::{KeyPair, PublicKey, PublicKeyBox, SecretKey, SecretKeyBox, SignatureBox};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["keygen", sk_out, pk_out] => {
            if fs::metadata(sk_out).is_ok() {
                return Err(format!("{sk_out} already exists; refusing to overwrite a key").into());
            }
            let kp = KeyPair::generate_unencrypted_keypair()?;
            fs::write(sk_out, kp.sk.to_box(Some("cursor-stream-switcher release key"))?.into_string())?;
            fs::write(pk_out, kp.pk.to_box()?.into_string())?;
            println!("wrote {sk_out} and {pk_out}");
        }
        ["sign", file, sk_file, comment] => {
            let sk_text = fs::read_to_string(sk_file)?;
            let sk = SecretKey::from_unencrypted_box(SecretKeyBox::from_string(sk_text.trim())?)?;
            let data = fs::File::open(file)?;
            let sig = minisign::sign(None, &sk, data, Some(comment), Some("cursor-stream-switcher release"))?;
            fs::write(format!("{file}.minisig"), sig.into_string())?;
            println!("signed {file}");
        }
        ["verify", file, pk_file, comment] => {
            let pk = PublicKey::from_box(PublicKeyBox::from_string(&fs::read_to_string(pk_file)?)?)?;
            let sig = SignatureBox::from_string(&fs::read_to_string(format!("{file}.minisig"))?)?;
            minisign::verify(&pk, &sig, fs::File::open(file)?, true, false, false)?;
            if sig.trusted_comment()? != *comment {
                return Err(format!("trusted comment is {:?}, expected {comment:?}", sig.trusted_comment()?).into());
            }
            println!("verified {file} against {pk_file}");
        }
        _ => return Err("usage: sign keygen <sk> <pk> | sign sign <file> <sk> <comment> | sign verify <file> <pk> <comment>".into()),
    }
    Ok(())
}
