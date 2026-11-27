//! Authenticate release metadata using a key pinned independently of downloads.
use anyhow::{ensure, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ring::signature;
use serde::Deserialize;
use std::collections::BTreeMap;

pub(crate) const PLATFORMS: [&str; 3] = ["darwin-arm64", "linux-x86_64", "linux-arm64"];
const MAX_LIFETIME: u64 = 14 * 86_400;
const SPKI_PREFIX: &[u8] = &[
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Trust {
    pub public_key_pem: String,
    pub apple_team_id: String,
}

impl Trust {
    pub(crate) fn pinned() -> Result<Self> {
        serde_json::from_str(include_str!("../../release/cli-trust.json"))
            .context("invalid embedded CLI release trust configuration")
    }

    fn public_point(&self) -> Result<Vec<u8>> {
        ensure!(
            self.apple_team_id.len() == 10
                && self
                    .apple_team_id
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
            "trusted CLI publisher is not configured; use the authenticated installer"
        );
        let encoded = self
            .public_key_pem
            .trim()
            .strip_prefix("-----BEGIN PUBLIC KEY-----")
            .and_then(|s| s.strip_suffix("-----END PUBLIC KEY-----"))
            .context("trusted CLI release key is not configured")?;
        let encoded: String = encoded
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        let der = STANDARD
            .decode(encoded)
            .context("invalid trusted CLI release key")?;
        ensure!(
            der.len() == 91 && der.starts_with(SPKI_PREFIX) && der[26] == 4,
            "trusted CLI release key must be ECDSA P-256"
        );
        Ok(der[26..].to_vec())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    payload: String,
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    schema: u32,
    pub release: String,
    channel: String,
    source_commit: String,
    issued_at: u64,
    expires_at: u64,
    pub platforms: BTreeMap<String, String>,
    apple_team_id: String,
}

impl Manifest {
    pub(crate) fn is_stable(&self) -> bool {
        self.channel == "stable"
    }
}

pub(crate) fn stable_version(value: &str) -> Result<[u64; 3]> {
    let parts: Vec<_> = value.split('.').collect();
    ensure!(
        parts.len() == 3,
        "release must use a stable semantic version"
    );
    let mut version = [0; 3];
    for (index, part) in parts.into_iter().enumerate() {
        ensure!(
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part == "0" || !part.starts_with('0')),
            "release must use a stable semantic version"
        );
        version[index] = part.parse().context("release version is too large")?;
    }
    Ok(version)
}

pub(crate) fn verify(bytes: &[u8], trust: &Trust, now: u64) -> Result<Manifest> {
    let envelope: Envelope = serde_json::from_slice(bytes)
        .context("a signed CLI release manifest is required; use https://magicmtn.ai/install.sh to upgrade older installations")?;
    let payload = STANDARD
        .decode(envelope.payload)
        .context("invalid signed release payload")?;
    let signature_bytes = STANDARD
        .decode(envelope.signature)
        .context("invalid release signature encoding")?;
    signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_ASN1, trust.public_point()?)
        .verify(&payload, &signature_bytes)
        .map_err(|_| anyhow::anyhow!("CLI release signature verification failed"))?;
    let manifest: Manifest =
        serde_json::from_slice(&payload).context("invalid signed release metadata")?;
    ensure!(
        manifest.schema == 1
            && matches!(
                manifest.channel.as_str(),
                "alpha" | "beta" | "prerelease" | "stable"
            ),
        "unsupported signed CLI release schema or channel"
    );
    ensure!(
        !manifest.release.is_empty()
            && manifest.release.len() <= 100
            && manifest
                .release
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-.".contains(&c)),
        "invalid signed release identifier"
    );
    ensure!(
        manifest.source_commit.len() == 40
            && manifest
                .source_commit
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "signed release has an invalid source commit"
    );
    ensure!(
        manifest.issued_at <= now.saturating_add(300)
            && manifest.expires_at > now
            && manifest.expires_at > manifest.issued_at
            && manifest.expires_at - manifest.issued_at <= MAX_LIFETIME,
        "signed CLI release metadata is expired or has invalid dates"
    );
    ensure!(
        manifest.apple_team_id == trust.apple_team_id,
        "signed release uses an unexpected Apple publisher"
    );
    ensure!(
        manifest.platforms.len() == PLATFORMS.len()
            && PLATFORMS
                .iter()
                .all(|name| manifest.platforms.contains_key(*name))
            && manifest.platforms.values().all(|hash| hash.len() == 64
                && hash
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())),
        "signed release has invalid platform checksums"
    );
    Ok(manifest)
}

pub(crate) fn verify_macos_binary(path: &std::path::Path, trust: &Trust) -> Result<()> {
    let requirement = format!(
        "anchor apple generic and certificate leaf[subject.OU] = \"{}\"",
        trust.apple_team_id
    );
    let result = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "-R", &requirement])
        .arg(path)
        .output()
        .context("checking the downloaded CLI's Developer ID signature")?;
    ensure!(
        result.status.success(),
        "downloaded CLI does not have the expected Developer ID signature"
    );
    Ok(())
}

#[cfg(test)]
pub(crate) fn sign_test_manifest(payload: serde_json::Value) -> (Trust, Vec<u8>) {
    use ring::signature::KeyPair;
    let random = ring::rand::SystemRandom::new();
    let document = signature::EcdsaKeyPair::generate_pkcs8(
        &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
        &random,
    )
    .unwrap();
    let key = signature::EcdsaKeyPair::from_pkcs8(
        &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
        document.as_ref(),
        &random,
    )
    .unwrap();
    let mut der = SPKI_PREFIX.to_vec();
    der.extend_from_slice(key.public_key().as_ref());
    let trust = Trust {
        public_key_pem: format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            STANDARD.encode(der)
        ),
        apple_team_id: "ABCDE12345".into(),
    };
    let bytes = serde_json::to_vec(&payload).unwrap();
    let signature = key.sign(&random, &bytes).unwrap();
    let envelope = serde_json::to_vec(&serde_json::json!({"payload": STANDARD.encode(bytes), "signature": STANDARD.encode(signature.as_ref())})).unwrap();
    (trust, envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Trust, Vec<u8>, u64) {
        (
            serde_json::from_str(include_str!("../../tests/fixtures/cli-release/trust.json"))
                .unwrap(),
            include_bytes!("../../tests/fixtures/cli-release/signed.json").to_vec(),
            include_str!("../../tests/fixtures/cli-release/verified-at.txt")
                .trim()
                .parse()
                .unwrap(),
        )
    }

    #[test]
    fn accepts_openssl_signed_manifest_and_rejects_tampering_and_replay() {
        let (trust, bytes, now) = fixture();
        assert_eq!(verify(&bytes, &trust, now).unwrap().release, "test-release");
        let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut payload = STANDARD
            .decode(envelope["payload"].as_str().unwrap())
            .unwrap();
        payload[10] ^= 1;
        envelope["payload"] = STANDARD.encode(payload).into();
        assert!(verify(&serde_json::to_vec(&envelope).unwrap(), &trust, now).is_err());
        assert!(verify(&bytes, &trust, now + MAX_LIFETIME).is_err());
        assert!(verify(&bytes, &trust, now - 1000).is_err());
        assert!(verify(br#"{"release":"unsigned","platforms":{}}"#, &trust, now).is_err());
    }

    #[test]
    fn requires_the_pinned_publisher_for_every_channel() {
        let (trust, bytes, now) = fixture();
        let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let payload: serde_json::Value = serde_json::from_slice(
            &STANDARD
                .decode(envelope["payload"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        for channel in ["alpha", "beta", "prerelease", "stable"] {
            let mut payload = payload.clone();
            payload["channel"] = channel.into();
            let (other_trust, signed) = sign_test_manifest(payload);
            assert!(verify(&signed, &other_trust, now).is_ok());
            assert!(
                verify(&signed, &trust, now).is_err(),
                "{channel} must not bypass publisher verification"
            );
        }
    }
}
