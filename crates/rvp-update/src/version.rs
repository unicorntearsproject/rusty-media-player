//! Semantic versions, enough to order release numbers (`0.0.3-ci1` is older than `0.0.3`).
use std::cmp::Ordering;
use std::fmt;

/// One dot-separated part of a pre-release tag.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Num(u64),
    Text(String),
}

impl Ord for Part {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Part::Num(a), Part::Num(b)) => a.cmp(b),
            (Part::Text(a), Part::Text(b)) => a.cmp(b),
            // Numbers sort before text.
            (Part::Num(_), Part::Text(_)) => Ordering::Less,
            (Part::Text(_), Part::Num(_)) => Ordering::Greater,
        }
    }
}

impl PartialOrd for Part {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `major.minor.patch` with an optional `-pre.release` tag (a `+build` suffix and a leading `v` are ignored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<Part>,
}

impl Version {
    /// Parse a version; `None` if it is not one.
    pub fn parse(text: &str) -> Option<Version> {
        let t = text.trim();
        let t = t.strip_prefix('v').unwrap_or(t);
        let t = t.split('+').next()?;
        let (core, pre) = match t.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (t, None),
        };
        let mut nums = core.split('.');
        let mut next = || -> Option<u64> {
            let s = nums.next()?;
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            s.parse().ok()
        };
        let (major, minor, patch) = (next()?, next()?, next()?);
        if nums.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(p) => {
                let parts: Vec<Part> = p
                    .split('.')
                    .map(|s| match s.parse::<u64>() {
                        Ok(n) if s.bytes().all(|b| b.is_ascii_digit()) => Part::Num(n),
                        _ => Part::Text(s.to_string()),
                    })
                    .collect();
                if parts.is_empty() || p.split('.').any(|s| s.is_empty()) {
                    return None;
                }
                parts
            }
        };
        Some(Version { major, minor, patch, pre })
    }

    /// True for `1.2.3-rc1` style versions.
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A release is newer than its pre-releases.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            let parts: Vec<String> = self
                .pre
                .iter()
                .map(|p| match p {
                    Part::Num(n) => n.to_string(),
                    Part::Text(t) => t.clone(),
                })
                .collect();
            write!(f, "-{}", parts.join("."))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn orders_numbers_not_text() {
        assert!(v("0.0.3") > v("0.0.2"));
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("v0.0.3"), v("0.0.3+build5"));
    }

    #[test]
    fn prereleases_sort_before_the_release() {
        assert!(v("0.0.3-ci1") < v("0.0.3"));
        assert!(v("0.0.3-ci1") > v("0.0.2"));
        assert!(v("0.0.3-rc.2") > v("0.0.3-rc.1"));
        assert!(v("0.0.3-rc.10") > v("0.0.3-rc.9"));
        assert!(v("0.0.3-alpha") < v("0.0.3-beta"));
        assert!(v("0.0.3-1") < v("0.0.3-alpha"));
        assert!(v("0.0.3-rc") < v("0.0.3-rc.1"));
    }

    #[test]
    fn release_candidates_of_one_oh_are_before_the_release_and_after_the_betas() {
        // 1.0.0-rc1 is what the first release candidate is called (SemVer: `rc1` is one identifier, compared as text, so rc1 < rc2 < rc9).
        assert!(v("1.0.0-rc1") < v("1.0.0-rc2") && v("1.0.0-rc2") < v("1.0.0"));
        assert!(v("1.0.0-rc9") < v("1.0.0"));
        assert!(v("1.0.0-beta1") < v("1.0.0-rc1"));
        assert!(
            v("0.99.0") < v("1.0.0-rc1"),
            "an installed 0.x is offered the candidate when pre-releases are wanted"
        );
        assert!(v("1.0.0-rc1").is_prerelease() && !v("1.0.0").is_prerelease());
        assert_eq!(v("1.0.0-rc1").to_string(), "1.0.0-rc1");
    }

    #[test]
    fn rejects_what_is_not_a_version() {
        for bad in ["", "1", "1.2", "1.2.3.4", "a.b.c", "1.2.x", "1.2.3-", "1..3", "1.2.3-a..b", "-1.2.3"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(v("0.0.3-ci1").to_string(), "0.0.3-ci1");
    }
}
