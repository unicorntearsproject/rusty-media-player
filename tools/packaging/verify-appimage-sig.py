#!/usr/bin/env python3
"""Verify the signature appimagetool --sign embeds in an AppImage (ELF sections .sha256_sig and .sig_key).

    tools/packaging/verify-appimage-sig.py <file.AppImage> <fingerprint> [public-key.asc]

The signature is over the hex SHA-256 of the file with those two sections zeroed. It is checked with `gpg` in a throwaway keyring that holds
only the given public key (default: packaging/keys/rusty-wave-release.asc), so it proves the file was signed by that key, not merely by the key
embedded in the file. Exit status 0 only when gpg reports a valid signature by the fingerprint and the embedded key is the same key.
"""
import hashlib, os, re, subprocess, sys, tempfile

def main():
    path, fpr = sys.argv[1], sys.argv[2].replace(" ", "").upper()
    here = os.path.dirname(os.path.abspath(__file__))
    pub = sys.argv[3] if len(sys.argv) > 3 else os.path.join(here, "../../packaging/keys/rusty-wave-release.asc")
    data = open(path, "rb").read()
    out = subprocess.check_output(["readelf", "-S", "-W", path]).decode()
    def section(name):
        m = re.search(r"\]\s+" + re.escape(name) + r"\s+\S+\s+\w+\s+(\w+)\s+(\w+)", out)
        if not m:
            sys.exit(f"{path}: no {name} section (not signed)")
        return int(m.group(1), 16), int(m.group(2), 16)
    (so, sl), (ko, kl) = section(".sha256_sig"), section(".sig_key")
    sig, key = data[so:so + sl].rstrip(b"\0"), data[ko:ko + kl].rstrip(b"\0")
    if not sig:
        sys.exit(f"{path}: empty signature section (not signed)")
    blank = bytearray(data)
    blank[so:so + sl] = bytes(sl)
    blank[ko:ko + kl] = bytes(kl)
    digest = hashlib.sha256(blank).hexdigest().encode()
    with tempfile.TemporaryDirectory(prefix="rvp-aisig-") as d:
        os.chmod(d, 0o700)
        env = dict(os.environ, GNUPGHOME=d)
        def gpg(*a, **kw):
            return subprocess.run(["gpg", "--batch", "--no-tty", *a], env=env, capture_output=True, text=True, **kw)
        r = gpg("--import", pub)
        if r.returncode:
            sys.exit(r.stderr)
        # The key embedded in the file must be the release key, too.
        emb = subprocess.run(["gpg", "--batch", "--show-keys", "--with-colons"], input=key, env=env, capture_output=True)
        emb_fprs = [l.split(":")[9] for l in emb.stdout.decode().splitlines() if l.startswith("fpr")]
        if fpr not in emb_fprs:
            sys.exit(f"embedded key is {emb_fprs}, not {fpr}")
        open(os.path.join(d, "sig.asc"), "wb").write(sig)
        open(os.path.join(d, "digest"), "wb").write(digest)
        r = gpg("--status-fd", "1", "--verify", os.path.join(d, "sig.asc"), os.path.join(d, "digest"))
        subprocess.run(["gpgconf", "--kill", "all"], env=env)
        # Made by the release key itself or by one of its signing subkeys (then the primary's fingerprint is the last field).
        words = [l.split()[1] for l in r.stdout.splitlines() if l.startswith("[GNUPG:] ") and len(l.split()) > 1]
        refused = {"BADSIG", "ERRSIG", "EXPSIG", "EXPKEYSIG", "REVKEYSIG", "NO_PUBKEY", "FAILURE"}
        valid = [l.split()[2:] for l in r.stdout.splitlines() if l.startswith("[GNUPG:] VALIDSIG ")]
        if refused & set(words) or words.count("GOODSIG") != 1 or len(valid) != 1 or valid[0][-1] != fpr:
            sys.exit(f"bad signature:\n{r.stdout}{r.stderr}")
    print(f"{os.path.basename(path)}: embedded signature OK (key {fpr})")

main()
