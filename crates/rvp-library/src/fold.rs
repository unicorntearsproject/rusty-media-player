//! Text folding for sorting and searching: case-insensitive, accent-insensitive for the Latin scripts, numbers compare
//! by value ("Track 2" before "Track 10"). Other scripts are lower-cased and otherwise left alone.
use alloc::string::String;

/// The base letter(s) for an accented Latin letter (lower case in, lower case out), or `None` to keep the character.
fn base(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{e0}'..='\u{e5}' | '\u{101}' | '\u{103}' | '\u{105}' => "a",
        '\u{e6}' => "ae",
        '\u{e7}' | '\u{107}' | '\u{109}' | '\u{10b}' | '\u{10d}' => "c",
        '\u{10f}' | '\u{111}' | '\u{f0}' => "d",
        '\u{e8}'..='\u{eb}' | '\u{113}' | '\u{115}' | '\u{117}' | '\u{119}' | '\u{11b}' => "e",
        '\u{11d}' | '\u{11f}' | '\u{121}' | '\u{123}' => "g",
        '\u{125}' | '\u{127}' => "h",
        '\u{ec}'..='\u{ef}' | '\u{129}' | '\u{12b}' | '\u{12d}' | '\u{12f}' | '\u{131}' => "i",
        '\u{135}' => "j",
        '\u{137}' => "k",
        '\u{13a}' | '\u{13c}' | '\u{13e}' | '\u{142}' => "l",
        '\u{f1}' | '\u{144}' | '\u{146}' | '\u{148}' => "n",
        '\u{f2}'..='\u{f6}' | '\u{f8}' | '\u{14d}' | '\u{14f}' | '\u{151}' => "o",
        '\u{153}' => "oe",
        '\u{155}' | '\u{157}' | '\u{159}' => "r",
        '\u{15b}' | '\u{15d}' | '\u{15f}' | '\u{161}' => "s",
        '\u{df}' => "ss",
        '\u{163}' | '\u{165}' | '\u{167}' => "t",
        '\u{fe}' => "th",
        '\u{f9}'..='\u{fc}' | '\u{169}' | '\u{16b}' | '\u{16d}' | '\u{16f}' | '\u{171}' | '\u{173}' => "u",
        '\u{175}' => "w",
        '\u{fd}' | '\u{ff}' | '\u{177}' => "y",
        '\u{17a}' | '\u{17c}' | '\u{17e}' => "z",
        _ => return None,
    })
}

/// Lower-case `s` and strip accents from Latin letters; combining marks are dropped.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        for l in c.to_lowercase() {
            if ('\u{300}'..='\u{36f}').contains(&l) {
                continue;
            }
            match base(l) {
                Some(b) => out.push_str(b),
                None => out.push(l),
            }
        }
    }
    out
}

/// A sort key: folded, a leading "the " dropped, runs of digits padded so they compare by value.
pub fn sort_key(s: &str) -> String {
    let f = fold(s.trim());
    let f = f.strip_prefix("the ").unwrap_or(&f);
    natural(f)
}

/// Like [`sort_key`] without dropping "the" (titles, album names).
pub fn natural(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut digits = String::new();
    let flush = |digits: &mut String, out: &mut String| {
        if !digits.is_empty() {
            // Up to 12 digits keep their value; anything longer compares as text.
            let d = digits.trim_start_matches('0');
            for _ in d.len()..12 {
                out.push('0');
            }
            out.push_str(d);
            digits.clear();
        }
    };
    for c in s.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            flush(&mut digits, &mut out);
            out.push(c);
        }
    }
    flush(&mut digits, &mut out);
    out
}

/// A 32-bit FNV-1a hash (stable ids for albums and artists).
pub fn hash32(s: &str) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for b in s.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    h
}

/// A 64-bit FNV-1a hash (art ids: the hash of the encoded picture).
pub fn hash64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_accents_and_case() {
        assert_eq!(fold("Zo\u{eb} P\u{e9}rez"), "zoe perez");
        assert_eq!(fold("\u{dc}nal \u{c7}elik"), "unal celik");
        assert_eq!(fold("Sigur\u{f0}ur \u{de}\u{f3}rsson"), "sigurdur thorsson");
        assert_eq!(fold("Stra\u{df}e"), "strasse");
        assert_eq!(fold("\u{130}stanbul"), "istanbul");
        // Scripts without a Latin base are only lower-cased.
        assert_eq!(fold("\u{41a}\u{438}\u{43d}\u{43e}"), "\u{43a}\u{438}\u{43d}\u{43e}");
        assert_eq!(fold("\u{6771}\u{4eac}"), "\u{6771}\u{4eac}");
    }

    #[test]
    fn sorts_naturally_and_ignores_the() {
        let mut v = alloc::vec!["Track 10", "track 2", "Track 1"];
        v.sort_by_key(|s| natural(&fold(s)));
        assert_eq!(v, ["Track 1", "track 2", "Track 10"]);
        assert_eq!(sort_key("The Midnight Owls"), sort_key("Midnight Owls"));
        assert!(sort_key("The Zebras") > sort_key("Aardvark"));
    }

    #[test]
    fn hashes_are_stable() {
        assert_eq!(hash32("a"), 0xe40c_292c);
        assert_eq!(hash64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
