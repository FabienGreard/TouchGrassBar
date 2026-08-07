use std::{env, fs, process::ExitCode};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use serde_json::Value;

fn decoded_tauri_value(value: &str) -> Result<String, ()> {
    let decoded = STANDARD.decode(value).map_err(|_| ())?;
    String::from_utf8(decoded).map_err(|_| ())
}

fn verify_update_signature(
    archive: &[u8],
    signature_value: &str,
    public_key_value: &str,
) -> Result<(), ()> {
    let public_key = PublicKey::decode(&decoded_tauri_value(public_key_value)?).map_err(|_| ())?;
    let signature = Signature::decode(&decoded_tauri_value(signature_value)?).map_err(|_| ())?;
    public_key.verify(archive, &signature, true).map_err(|_| ())
}

fn run() -> Result<(), ()> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let [archive_path, signature_path, config_path] = arguments.as_slice() else {
        return Err(());
    };
    let archive = fs::read(archive_path).map_err(|_| ())?;
    let signature = fs::read_to_string(signature_path).map_err(|_| ())?;
    let config = serde_json::from_str::<Value>(&fs::read_to_string(config_path).map_err(|_| ())?)
        .map_err(|_| ())?;
    let public_key = config
        .pointer("/plugins/updater/pubkey")
        .and_then(Value::as_str)
        .ok_or(())?;
    verify_update_signature(&archive, signature.trim(), public_key)
}

fn main() -> ExitCode {
    if run().is_err() {
        eprintln!("Updater signature verification failed.");
        return ExitCode::FAILURE;
    }
    println!("Updater signature: verified");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\ntrusted comment: timestamp:1555779966\tfile:test\nQtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==";

    #[test]
    fn verifies_the_same_encoded_values_used_by_the_tauri_updater() {
        // PublicKey::from_base64 does not preserve the Minisign box. Use the
        // canonical box shape that the Tauri signer stores before base64.
        let encoded_public_key = STANDARD.encode(format!(
            "untrusted comment: minisign public key\n{PUBLIC_KEY}\n"
        ));
        let encoded_signature = STANDARD.encode(SIGNATURE);

        verify_update_signature(b"test", &encoded_signature, &encoded_public_key).unwrap();
    }

    #[test]
    fn rejects_changed_archive_bytes() {
        let encoded_public_key = STANDARD.encode(format!(
            "untrusted comment: minisign public key\n{PUBLIC_KEY}\n"
        ));
        let encoded_signature = STANDARD.encode(SIGNATURE);

        assert!(
            verify_update_signature(b"changed", &encoded_signature, &encoded_public_key).is_err()
        );
    }
}
