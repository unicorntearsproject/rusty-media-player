//! OpenPGP verification of detached signatures against the release key (pure Rust, `pgp`).
//!
//! What is trusted is the key compiled into the program (`packaging/keys/rusty-wave-release.asc`): a download is only used when a
//! detached signature over its exact bytes verifies with it. The signature must carry that key's fingerprint, must not be older
//! than the key, and must not be newer than the key's expiry; the key itself must not be revoked. The release is signed in CI by a
//! signing-only subkey of the release key: a signature by a subkey that belongs to the release key counts for it, as long as the subkey is
//! not revoked and was valid (not yet expired) when it signed.
use pgp::composed::{Deserializable, DetachedSignature, SignedPublicKey};
use pgp::packet::SignatureType;
use pgp::types::KeyDetails;

/// The release key, armored.
pub const RELEASE_KEY_ARMORED: &str = include_str!("../../../packaging/keys/rusty-wave-release.asc");
/// Its fingerprint, upper-case hex.
pub const RELEASE_FINGERPRINT: &str = "E13FF843723D54068E45A3FF54BF2FA407093CEE";

/// Why a signature or a key was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The key could not be read.
    BadKey(String),
    /// The key has been revoked.
    Revoked,
    /// The signature could not be read.
    BadSignature(String),
    /// The signature was made by another key.
    WrongKey,
    /// The signature does not match the data (or is not from the key).
    Mismatch,
    /// The signature is dated before the key existed or after it expired.
    OutOfValidity,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::BadKey(e) => write!(f, "the release key is unreadable: {e}"),
            VerifyError::Revoked => f.write_str("the release key has been revoked"),
            VerifyError::BadSignature(e) => write!(f, "the signature is unreadable: {e}"),
            VerifyError::WrongKey => f.write_str("the signature was not made by the release key"),
            VerifyError::Mismatch => f.write_str("the signature does not match the file"),
            VerifyError::OutOfValidity => f.write_str("the signature is outside the release key's validity"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// A public key that detached signatures are checked against.
pub struct Verifier {
    key: SignedPublicKey,
    fingerprint: Vec<u8>,
}

impl Verifier {
    /// The release key compiled into the program.
    pub fn release() -> Verifier {
        Verifier::from_armored(RELEASE_KEY_ARMORED).expect("the committed release key parses")
    }

    /// A verifier for another key (tests).
    pub fn from_armored(armored: &str) -> Result<Verifier, VerifyError> {
        let (key, _) =
            SignedPublicKey::from_string(armored).map_err(|e| VerifyError::BadKey(e.to_string()))?;
        if !key.details.revocation_signatures.is_empty() {
            return Err(VerifyError::Revoked);
        }
        let fingerprint = key.primary_key.fingerprint().as_bytes().to_vec();
        Ok(Verifier { key, fingerprint })
    }

    /// The key's fingerprint, upper-case hex.
    pub fn fingerprint(&self) -> String {
        hex_upper(&self.fingerprint)
    }

    /// Seconds since the epoch at which the key stops being valid, if it expires.
    fn expires_at(&self) -> Option<u64> {
        let created = u64::from(self.key.primary_key.created_at().as_secs());
        self.key
            .details
            .users
            .iter()
            .flat_map(|u| u.signatures.iter())
            .chain(self.key.details.direct_signatures.iter())
            .filter_map(|s| s.key_expiration_time())
            .map(|d| created + u64::from(d.as_secs()))
            .min()
    }

    /// Check `armored_sig` (an ASCII-armored detached signature) over `data`.
    pub fn verify_detached(&self, data: &[u8], armored_sig: &[u8]) -> Result<(), VerifyError> {
        let text = std::str::from_utf8(armored_sig).map_err(|e| VerifyError::BadSignature(e.to_string()))?;
        let (sig, _) =
            DetachedSignature::from_string(text).map_err(|e| VerifyError::BadSignature(e.to_string()))?;
        let issuers = sig.signature.issuer_fingerprint();
        // The issuer, when the signature names one, is the release key or one of its subkeys.
        let is_ours = |f: &[u8]| {
            f == self.fingerprint.as_slice()
                || self.key.public_subkeys.iter().any(|s| s.key.fingerprint().as_bytes() == f)
        };
        if !issuers.is_empty() && !issuers.iter().any(|f| is_ours(f.as_bytes())) {
            return Err(VerifyError::WrongKey);
        }
        let made = sig.signature.created().map(|t| u64::from(t.as_secs()));
        let made = made.ok_or_else(|| VerifyError::BadSignature("it has no creation time".into()))?;
        if made < u64::from(self.key.primary_key.created_at().as_secs())
            || self.expires_at().is_some_and(|e| made > e)
        {
            return Err(VerifyError::OutOfValidity);
        }
        if sig.verify(&self.key.primary_key, data).is_ok() {
            return Ok(());
        }
        let mut out_of_validity = false;
        for sub in &self.key.public_subkeys {
            if sub.signatures.iter().any(|s| s.typ() == Some(SignatureType::SubkeyRevocation)) {
                continue;
            }
            // A subkey's own validity: not before it was made, not after the expiry its binding signature gives it.
            let created = u64::from(sub.key.created_at().as_secs());
            let expires = sub
                .signatures
                .iter()
                .filter_map(|s| s.key_expiration_time())
                .map(|d| created + u64::from(d.as_secs()))
                .min();
            if made < created || expires.is_some_and(|e| made > e) {
                if sig.verify(&sub.key, data).is_ok() {
                    out_of_validity = true;
                }
                continue;
            }
            if sig.verify(&sub.key, data).is_ok() {
                return Ok(());
            }
        }
        Err(if out_of_validity { VerifyError::OutOfValidity } else { VerifyError::Mismatch })
    }
}

fn hex_upper(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

#[cfg(test)]
pub(crate) mod testkit {
    //! Throwaway keys and signatures for tests (no gpg needed).
    use pgp::composed::{
        ArmorOptions, DetachedSignature, KeyType, SecretKeyParamsBuilder, SignedPublicKey, SignedSecretKey,
        SubkeyParamsBuilder,
    };
    use pgp::crypto::hash::HashAlgorithm;
    use pgp::types::{KeyDetails, Password};

    pub struct TestKey {
        pub secret: SignedSecretKey,
        pub public_armored: String,
    }

    pub fn key(user: &str) -> TestKey {
        let params = SecretKeyParamsBuilder::default()
            .key_type(KeyType::Ed25519Legacy)
            .can_certify(true)
            .can_sign(true)
            .primary_user_id(user.into())
            .build()
            .unwrap();
        let mut rng = rand::thread_rng();
        let secret = params.generate(&mut rng).unwrap();
        let public = SignedPublicKey::from(secret.clone());
        let public_armored = public.to_armored_string(ArmorOptions::default()).unwrap();
        TestKey { secret, public_armored }
    }

    /// A key whose signing is done by a subkey (the primary only certifies), as the release key's CI signing key.
    pub fn key_with_signing_subkey(user: &str) -> TestKey {
        let sub = SubkeyParamsBuilder::default().key_type(KeyType::Ed25519Legacy).can_sign(true).build().unwrap();
        let params = SecretKeyParamsBuilder::default()
            .key_type(KeyType::Ed25519Legacy)
            .can_certify(true)
            .can_sign(false)
            .primary_user_id(user.into())
            .subkey(sub)
            .build()
            .unwrap();
        let mut rng = rand::thread_rng();
        let secret = params.generate(&mut rng).unwrap();
        let public = SignedPublicKey::from(secret.clone());
        let public_armored = public.to_armored_string(ArmorOptions::default()).unwrap();
        TestKey { secret, public_armored }
    }

    /// A detached signature made by the key's first secret subkey.
    pub fn sign_with_subkey(k: &TestKey, data: &[u8]) -> Vec<u8> {
        let mut rng = rand::thread_rng();
        let sig = DetachedSignature::sign_binary_data(
            &mut rng,
            &k.secret.secret_subkeys[0].key,
            &Password::empty(),
            HashAlgorithm::Sha256,
            data,
        )
        .unwrap();
        sig.to_armored_string(ArmorOptions::default()).unwrap().into_bytes()
    }

    pub fn fingerprint(k: &TestKey) -> String {
        super::hex_upper(k.secret.fingerprint().as_bytes())
    }

    pub fn sign(k: &TestKey, data: &[u8]) -> Vec<u8> {
        let mut rng = rand::thread_rng();
        let sig = DetachedSignature::sign_binary_data(
            &mut rng,
            &k.secret.primary_key,
            &Password::empty(),
            HashAlgorithm::Sha256,
            data,
        )
        .unwrap();
        sig.to_armored_string(ArmorOptions::default()).unwrap().into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_release_key_is_the_documented_one() {
        let v = Verifier::release();
        assert_eq!(v.fingerprint(), RELEASE_FINGERPRINT);
    }

    #[test]
    fn a_good_signature_is_accepted_and_a_changed_file_is_not() {
        let k = testkit::key("Test <t@example.com>");
        let v = Verifier::from_armored(&k.public_armored).unwrap();
        let sig = testkit::sign(&k, b"the file");
        assert_eq!(v.verify_detached(b"the file", &sig), Ok(()));
        assert_eq!(v.verify_detached(b"the filE", &sig), Err(VerifyError::Mismatch));
        assert_eq!(v.verify_detached(b"", &sig), Err(VerifyError::Mismatch));
    }

    #[test]
    fn another_keys_signature_is_refused() {
        let (a, b) = (testkit::key("A <a@example.com>"), testkit::key("B <b@example.com>"));
        let v = Verifier::from_armored(&a.public_armored).unwrap();
        assert_eq!(v.verify_detached(b"x", &testkit::sign(&b, b"x")), Err(VerifyError::WrongKey));
    }

    #[test]
    fn a_signature_by_a_subkey_of_the_release_key_is_accepted() {
        let k = testkit::key_with_signing_subkey("Release <r@example.com>");
        let v = Verifier::from_armored(&k.public_armored).unwrap();
        let sig = testkit::sign_with_subkey(&k, b"the file");
        assert_eq!(v.verify_detached(b"the file", &sig), Ok(()));
        assert_eq!(v.verify_detached(b"the filE", &sig), Err(VerifyError::Mismatch));
        // Another key's subkey is still refused.
        let other = testkit::key_with_signing_subkey("Other <o@example.com>");
        assert_eq!(
            v.verify_detached(b"the file", &testkit::sign_with_subkey(&other, b"the file")),
            Err(VerifyError::WrongKey)
        );
    }

    #[test]
    fn the_committed_release_key_carries_the_ci_signing_subkey() {
        let v = Verifier::release();
        assert!(!v.key.public_subkeys.is_empty(), "the committed key lists no signing subkey");
    }

    #[test]
    fn garbage_is_refused_without_a_panic() {
        let k = testkit::key("Test <t@example.com>");
        let v = Verifier::from_armored(&k.public_armored).unwrap();
        for bad in [
            &b""[..],
            b"hello",
            b"-----BEGIN PGP SIGNATURE-----\n\nAAAA\n-----END PGP SIGNATURE-----\n",
            &[0xff, 0xfe][..],
        ] {
            assert!(matches!(v.verify_detached(b"x", bad), Err(VerifyError::BadSignature(_))), "{bad:?}");
        }
        assert!(Verifier::from_armored("not a key").is_err());
    }

    #[test]
    fn a_signature_flipped_in_one_bit_is_refused() {
        let k = testkit::key("Test <t@example.com>");
        let v = Verifier::from_armored(&k.public_armored).unwrap();
        let sig = String::from_utf8(testkit::sign(&k, b"data")).unwrap();
        // Flip a character inside the base64 body.
        let body_at = sig.find("\n\n").unwrap() + 20;
        let mut bytes = sig.into_bytes();
        bytes[body_at] = if bytes[body_at] == b'A' { b'B' } else { b'A' };
        assert!(v.verify_detached(b"data", &bytes).is_err());
    }
}
