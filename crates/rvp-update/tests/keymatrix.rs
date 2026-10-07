//! Which detached signatures the updater accepts for the release key, as a table: the primary's own, a valid signing subkey's; and what it
//! refuses: an expired or revoked subkey, a subkey without the primary's binding and its own cross-certification, a foreign key or a foreign key's
//! subkey, a signature file holding two signatures, a tampered file or signature, an expired primary. The keys and signatures are fixtures
//! made with throwaway gpg keys by `tools/gen-keymatrix.sh` (every key made on 2026-10-01 10:00, every signature on 2026-10-01 12:00), checked at
//! two clocks: shortly after (everything of one day is still valid) and a week later (the day-long keys have expired).
use rvp_update::{Verifier, VerifyError};
use std::fs;
use std::path::PathBuf;

const SAME_DAY: u64 = 1_790_848_800 + 3 * 3600; // 2026-10-01 13:00 UTC
const A_WEEK_LATER: u64 = SAME_DAY + 6 * 86_400;

fn data(name: &str) -> Vec<u8> {
    let p: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests/data/keymatrix", name].iter().collect();
    fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn verifier(key: &str) -> Verifier {
    Verifier::from_armored(&String::from_utf8(data(&format!("{key}.pub.asc"))).unwrap()).unwrap()
}

fn check(key: &str, sig: &str, now: u64) -> Result<(), VerifyError> {
    verifier(key).verify_detached_at(&data("payload.txt"), &data(&format!("{sig}.sig.asc")), now)
}

#[test]
fn a_signature_by_the_primary_or_by_a_valid_signing_subkey_is_accepted() {
    for now in [SAME_DAY, A_WEEK_LATER] {
        assert_eq!(check("good", "good-by-primary", now), Ok(()), "primary at {now}");
        assert_eq!(check("good", "good-by-subkey", now), Ok(()), "subkey at {now}");
    }
}

#[test]
fn an_expired_subkey_is_refused_once_it_has_expired_but_not_before() {
    assert_eq!(check("expired-subkey", "expired-subkey", SAME_DAY), Ok(()));
    assert_eq!(check("expired-subkey", "expired-subkey", A_WEEK_LATER), Err(VerifyError::OutOfValidity));
}

#[test]
fn an_expired_primary_refuses_what_it_signed() {
    assert_eq!(check("expired-primary", "expired-primary", SAME_DAY), Ok(()));
    assert_eq!(check("expired-primary", "expired-primary", A_WEEK_LATER), Err(VerifyError::OutOfValidity));
}

#[test]
fn a_revoked_subkey_is_refused() {
    for now in [SAME_DAY, A_WEEK_LATER] {
        assert_eq!(
            check("revoked-subkey", "revoked-subkey", now),
            Err(VerifyError::SubkeyRevoked),
            "at {now}"
        );
    }
}

#[test]
fn a_subkey_without_the_primary_binding_cross_certification_is_refused() {
    assert_eq!(
        check("no-cross-certification", "no-cross-certification", SAME_DAY),
        Err(VerifyError::UnboundSubkey)
    );
}

#[test]
fn a_foreign_key_or_a_foreign_keys_subkey_is_refused() {
    for sig in ["foreign-by-primary", "foreign-by-subkey"] {
        assert_eq!(check("good", sig, SAME_DAY), Err(VerifyError::WrongKey), "{sig}");
    }
}

#[test]
fn a_signature_file_with_two_signatures_is_refused_even_though_both_are_good() {
    let e = check("good", "good-two-signatures", SAME_DAY).unwrap_err();
    assert!(matches!(&e, VerifyError::BadSignature(m) if m.contains("2 signatures")), "{e:?}");
}

#[test]
fn a_tampered_file_or_signature_is_refused() {
    let v = verifier("good");
    let sig = data("good-by-subkey.sig.asc");
    let mut file = data("payload.txt");
    file[0] ^= 1;
    assert_eq!(v.verify_detached_at(&file, &sig, SAME_DAY), Err(VerifyError::Mismatch));
    // One flipped character inside the armored body.
    let text = String::from_utf8(sig).unwrap();
    let at = text.find("\n\n").unwrap() + 20;
    let mut bytes = text.into_bytes();
    bytes[at] = if bytes[at] == b'A' { b'B' } else { b'A' };
    assert!(v.verify_detached_at(&data("payload.txt"), &bytes, SAME_DAY).is_err());
    // Nothing, and not a signature at all.
    assert!(matches!(
        v.verify_detached_at(&data("payload.txt"), b"", SAME_DAY),
        Err(VerifyError::BadSignature(_))
    ));
}

#[test]
fn the_committed_release_key_refuses_all_of_it() {
    // None of the throwaway keys' signatures is accepted by the real release key.
    let release = Verifier::release();
    for sig in ["good-by-primary", "good-by-subkey", "foreign-by-primary", "expired-subkey"] {
        let r = release.verify_detached_at(&data("payload.txt"), &data(&format!("{sig}.sig.asc")), SAME_DAY);
        assert_eq!(r, Err(VerifyError::WrongKey), "{sig}");
    }
}
