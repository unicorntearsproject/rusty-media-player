#!/usr/bin/env bash
# Regenerates the key-and-signature fixtures of crates/rvp-update/tests/keymatrix.rs with throwaway gpg keys (no secret leaves the temporary
# keyring; nothing here is the release key). Times are faked so the files never change meaning: every key is made on 2026-10-01.
#   tools/gen-keymatrix.sh            (needs gpg and python3)
set -euo pipefail
out="$(cd "$(dirname "$0")/.." && pwd)/crates/rvp-update/tests/data/keymatrix"
export GNUPGHOME; GNUPGHOME="$(mktemp -d "${TMPDIR:-/tmp}/keymatrix-XXXXXX")"; chmod 700 "$GNUPGHOME"
# The throwaway keyring has its own gpg-agent: stop it on every way out (success, failure, Ctrl+C, kill). The directory itself stays
# (nothing here removes directories); it only holds throwaway keys.
cleanup() { gpgconf --homedir "$GNUPGHOME" --kill all >/dev/null 2>&1 || true; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$out"
G=(gpg --batch --no-tty --pinentry-mode loopback --passphrase '')
T0=20261001T100000   # the keys are made
T1=20261001T120000   # signatures are made (a day-key is still valid)
printf 'rusty-wave keymatrix payload\n' > "$out/payload.txt"

mk() {  # name primary-expiry subkey-expiry|none: make the key, print "primary-fpr subkey-fpr"
  local name="$1" pexp="$2" sexp="$3"
  "${G[@]}" --faked-system-time "$T0" --quick-generate-key "$name <$name@example.test>" ed25519 sign "$pexp" 2>/dev/null
  local p; p=$("${G[@]}" --list-keys --with-colons "$name@example.test" | awk -F: '/^fpr:/{print $10; exit}')
  local s=""
  if [ "$sexp" != none ]; then
    "${G[@]}" --faked-system-time "$T0" --quick-add-key "$p" ed25519 sign "$sexp" 2>/dev/null
    s=$("${G[@]}" --list-keys --with-colons "$p" | awk -F: '/^fpr:/{n++; if(n==2){print $10; exit}}')
  fi
  echo "$p $s"
}
pub() { "${G[@]}" --armor --export "$1" > "$out/$2.pub.asc"; }
sig() {  # signer-fpr(s)... -- outfile: detached armored signature over the payload at T1
  local args=() ; while [ "$1" != -- ]; do args+=(--local-user "$1!"); shift; done; shift
  "${G[@]}" --faked-system-time "$T1" --yes --armor --detach-sign "${args[@]}" --output "$out/$1.sig.asc" "$out/payload.txt"
}

read -r gp gs < <(mk good 2y 2y)
pub "$gp" good; sig "$gp" -- good-by-primary; sig "$gs" -- good-by-subkey; sig "$gp" "$gs" -- good-two-signatures
read -r fp fs < <(mk foreign 2y 2y)
pub "$fp" foreign; sig "$fp" -- foreign-by-primary; sig "$fs" -- foreign-by-subkey
read -r ep es < <(mk expsub 2y 1d)
pub "$ep" expired-subkey; sig "$es" -- expired-subkey
read -r xp _ < <(mk expprim 1d none)
pub "$xp" expired-primary; sig "$xp" -- expired-primary
read -r rp rs < <(mk revsub 2y 2y)
sig "$rs" -- revoked-subkey
printf 'key 1\nrevkey\ny\n0\n\ny\nsave\n' | "${G[@]}" --faked-system-time 20261002T100000 --command-fd 0 --status-fd 2 --edit-key "$rp" >/dev/null 2>&1 || true
pub "$rp" revoked-subkey
# A signing subkey without the primary-key binding (the subkey's cross-certification) in its binding signature: strip it from the unhashed area.
python3 - "$out/good.pub.asc" "$out/no-cross-certification.pub.asc" <<'P'
import base64, re, sys
src = open(sys.argv[1]).read()
body = re.sub(r'-----.*?-----|^[A-Za-z]+:.*$|^=.{4}$', '', src, flags=re.M | re.S).replace('\n', '')
data = base64.b64decode(body)
def packets(b):
    i = 0
    while i < len(b):
        h = b[i]; assert h & 0x80
        if h & 0x40:  # new format
            tag = h & 0x3f; l = b[i + 1]
            if l < 192: hl, n = 2, l
            elif l < 224: hl, n = 3, ((l - 192) << 8) + b[i + 2] + 192
            else: hl, n = 6, int.from_bytes(b[i + 2:i + 6], 'big')
        else:
            tag = (h >> 2) & 0xf; lt = h & 3
            hl, n = {0: (2, b[i + 1]), 1: (3, int.from_bytes(b[i + 1:i + 3], 'big')), 2: (5, int.from_bytes(b[i + 1:i + 5], 'big'))}[lt]
        yield tag, b[i + hl:i + hl + n]
        i += hl + n
def strip(sig):
    assert sig[0] == 4
    hashed = int.from_bytes(sig[4:6], 'big'); hs = sig[6:6 + hashed]
    uo = 6 + hashed; unh = int.from_bytes(sig[uo:uo + 2], 'big'); us = sig[uo + 2:uo + 2 + unh]
    keep = b''; i = 0
    while i < len(us):
        l = us[i]
        if l < 192: hl, n = 1, l
        elif l < 255: hl, n = 2, ((l - 192) << 8) + us[i + 1] + 192
        else: hl, n = 5, int.from_bytes(us[i + 1:i + 5], 'big')
        t = us[i + hl] & 0x7f
        if t != 32: keep += us[i:i + hl + n]
        i += hl + n
    return sig[:uo] + len(keep).to_bytes(2, 'big') + keep + sig[uo + 2 + unh:]
out = b''
for tag, p in packets(data):
    if tag == 2 and p[0] == 4 and p[1] == 0x18:
        p = strip(p)
    out += bytes([0xC0 | tag]) + (bytes([len(p)]) if len(p) < 192 else bytes([((len(p) - 192) >> 8) + 192, (len(p) - 192) & 255])) + p
b64 = base64.b64encode(out).decode()
open(sys.argv[2], 'w').write('-----BEGIN PGP PUBLIC KEY BLOCK-----\n\n' + '\n'.join(b64[i:i + 64] for i in range(0, len(b64), 64)) + '\n-----END PGP PUBLIC KEY BLOCK-----\n')
P
sig "$gs" -- no-cross-certification
echo "wrote $out"
