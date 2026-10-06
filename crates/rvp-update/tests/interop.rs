//! A signature made by the real `gpg` with the real release key (over `tests/data/interop.txt`) verifies with the key compiled in.
//! The fixture is public: it signs a one-line text file, nothing else.
use rvp_update::Verifier;

#[test]
fn gpg_signatures_by_the_release_key_verify() {
    let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/interop.txt")).unwrap();
    let sig = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/interop.txt.asc")).unwrap();
    let v = Verifier::release();
    assert_eq!(v.verify_detached(&data, &sig), Ok(()));
    let mut other = data.clone();
    other.push(b'!');
    assert!(v.verify_detached(&other, &sig).is_err());
}
