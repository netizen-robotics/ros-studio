//! Offline signing utility. No network or publication; private key bytes never print.
use ed25519_dalek::{Signer, SigningKey};
use std::{fs, path::PathBuf};
use studio_launcher::catalog::{Catalog, SignedCatalog};
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("Usage: studio-release-catalog UNSIGNED-CATALOG PRIVATE-KEY-HEX-FILE OUTPUT (output must not exist)".into());
    }
    if args[0] == "--verify" {
        let bytes = fs::read(PathBuf::from(&args[1])).map_err(|e| e.to_string())?;
        let key: [u8; 32] = hex::decode(args[2].to_str().ok_or("Public key must be UTF-8 hex")?)
            .map_err(|e| e.to_string())?
            .try_into()
            .map_err(|_| "Public key must contain 32 bytes")?;
        studio_launcher::catalog::verify(&bytes, &key)?;
        println!("Catalog signature and release metadata verified");
        return Ok(());
    }
    let input = PathBuf::from(&args[0]);
    let key_path = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);
    let bytes = fs::read(input).map_err(|e| e.to_string())?;
    if bytes.len() > 128 * 1024 {
        return Err("Unsigned catalog exceeds 128 KiB".into());
    }
    let payload = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    let catalog: Catalog = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
    if !matches!(catalog.schema, 1 | 2)
        || catalog.releases.is_empty()
        || catalog.releases.len() > 16
    {
        return Err("Invalid catalog schema/release count".into());
    }
    for release in &catalog.releases {
        release.validate()?;
    }
    let key = fs::read_to_string(key_path).map_err(|_| "Could not read release signing key")?;
    let seed: [u8; 32] = hex::decode(key.trim())
        .map_err(|_| "Release signing key is invalid hex")?
        .try_into()
        .map_err(|_| "Release signing seed must contain 32 bytes")?;
    let key = SigningKey::from_bytes(&seed);
    let signature = hex::encode(key.sign(payload.as_bytes()).to_bytes());
    let bytes = serde_json::to_vec_pretty(&SignedCatalog { payload, signature })
        .map_err(|e| e.to_string())?;
    studio_launcher::catalog::verify(&bytes, &key.verifying_key().to_bytes())?;
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    println!(
        "Signed catalog written. Public verification key: {}",
        hex::encode(key.verifying_key().to_bytes())
    );
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
