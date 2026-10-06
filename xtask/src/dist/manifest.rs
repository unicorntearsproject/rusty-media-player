//! The release file list, the stable `latest` aliases and the update manifest (`rusty-wave-latest.json`).
//!
//! The in-app updater reads the manifest (schema 1, see `docs/updates.md`). Its URLs name the immutable versioned files; only the `latest`
//! aliases are ever overwritten on the distribution bucket. The zsync file is the one exception: the manifest and the AppImage's embedded
//! update information both name its stable alias (`rusty-wave-latest-x86_64.AppImage.zsync`), whose content is the versioned
//! `.zsync` and whose internal URL points at the versioned AppImage, so the alias can never mismatch the file it describes.
use super::*;

/// Where the distribution bucket (`ut-software-dist`, us-east-1, publicly readable by its bucket policy) is served from. The AppImage embeds
/// `zsync|<base>/rusty-wave-latest-x86_64.AppImage.zsync` and the manifest's URLs start with it. `--base-url` replaces it for local tests
/// (any URL or `file://` path); `publish` always uses this one.
pub(super) const DIST_BASE_URL: &str = "https://ut-software-dist.s3.amazonaws.com";

pub(super) const MANIFEST_NAME: &str = "rusty-wave-latest.json";
pub(super) const ZSYNC_ALIAS: &str = "rusty-wave-latest-x86_64.AppImage.zsync";

/// One published installer: its manifest entry key, architecture, versioned name and `latest` alias name.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Asset {
    pub key: &'static str,
    pub arch: &'static str,
    pub name: String,
    pub alias: String,
    /// The `.zsync` of this file (versioned name), AppImage only.
    pub zsync: Option<String>,
}

/// The optional files of a release (the deb, rpm and AppImage are always there). Each goes along only when asked for, which says it was built
/// and verified in the same run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Extras {
    /// The Setup.exe and the portable zip (`--windows`).
    pub windows: bool,
    /// The macOS dmg (`--macos`).
    pub macos: bool,
    /// The Flatpak bundle (`--flatpak`).
    pub flatpak: bool,
    /// The web app zip (`--web`).
    pub web: bool,
    /// The Linux tarball (`--tarball`).
    pub tarball: bool,
}

impl Extras {
    /// Everything `--all` names.
    pub(super) const ALL: Extras =
        Extras { windows: true, macos: true, flatpak: true, web: true, tarball: true };
}

/// The files of a release, in manifest order.
pub(super) fn assets(ver: &str, x: Extras) -> Vec<Asset> {
    let mut v = vec![
        Asset {
            key: "linux-appimage",
            arch: "x86_64",
            name: format!("rusty-wave-{ver}-x86_64.AppImage"),
            alias: "rusty-wave-latest-x86_64.AppImage".into(),
            zsync: Some(format!("rusty-wave-{ver}-x86_64.AppImage.zsync")),
        },
        Asset {
            key: "linux-deb",
            arch: "amd64",
            name: format!("rusty-wave_{ver}_amd64.deb"),
            alias: "rusty-wave-latest_amd64.deb".into(),
            zsync: None,
        },
        Asset {
            key: "linux-rpm",
            arch: "x86_64",
            name: format!("rusty-wave-{ver}-1.x86_64.rpm"),
            alias: "rusty-wave-latest-1.x86_64.rpm".into(),
            zsync: None,
        },
    ];
    if x.tarball {
        v.push(Asset {
            key: "linux-tarball",
            arch: "x86_64",
            name: format!("rusty-wave-{ver}-linux-x86_64.tar.gz"),
            alias: "rusty-wave-latest-linux-x86_64.tar.gz".into(),
            zsync: None,
        });
    }
    if x.flatpak {
        v.push(Asset {
            key: "linux-flatpak",
            arch: "x86_64",
            name: format!("io.github.idometeor.RustyWave-{ver}.flatpak"),
            alias: "io.github.idometeor.RustyWave-latest.flatpak".into(),
            zsync: None,
        });
    }
    if x.windows {
        v.push(Asset {
            key: "windows-installer",
            arch: "x64",
            name: format!("rusty-wave-{ver}-x64-Setup.exe"),
            alias: "rusty-wave-latest-x64-Setup.exe".into(),
            zsync: None,
        });
        v.push(Asset {
            key: "windows-portable",
            arch: "x64",
            name: format!("rusty-wave-{ver}-windows-x64.zip"),
            alias: "rusty-wave-latest-windows-x64.zip".into(),
            zsync: None,
        });
    }
    if x.web {
        v.push(Asset {
            key: "web-pwa",
            arch: "any",
            name: format!("rusty-wave-web-{ver}.zip"),
            alias: "rusty-wave-web-latest.zip".into(),
            zsync: None,
        });
    }
    if x.macos {
        v.push(Asset {
            key: "macos-dmg",
            arch: "universal",
            name: format!("rusty-wave-{ver}-macos-universal.dmg"),
            alias: "rusty-wave-latest-macos-universal.dmg".into(),
            zsync: None,
        });
    }
    v
}

/// What the manifest says about one file.
#[derive(Debug)]
pub(super) struct Entry {
    pub key: &'static str,
    pub arch: &'static str,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    /// Whether the entry has a `zsync_url` (the stable alias).
    pub zsync: bool,
    /// A remark for people reading the manifest (clients ignore it).
    pub note: Option<&'static str>,
}

/// What a person should know about a file before running it.
pub(super) fn note(key: &str) -> Option<&'static str> {
    match key {
        "macos-dmg" => Some(
            "beta: ad-hoc signed only, not signed with a Developer ID and not notarized; Gatekeeper asks to allow it once (docs/release-testing.md, 5b)",
        ),
        _ => None,
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// The manifest text (schema 1). `base` has no trailing slash; the URLs name the versioned files.
pub(super) fn render(
    version: &str,
    released: &str,
    fingerprint: &str,
    base: &str,
    entries: &[Entry],
) -> String {
    let base = base.trim_end_matches('/');
    let mut s = String::new();
    s.push_str("{\n  \"schema\": 1,\n");
    s.push_str(&format!("  \"version\": \"{}\",\n", esc(version)));
    s.push_str(&format!("  \"released\": \"{}\",\n", esc(released)));
    s.push_str(&format!("  \"key_fingerprint\": \"{}\",\n", esc(fingerprint)));
    s.push_str("  \"files\": {\n");
    for (i, e) in entries.iter().enumerate() {
        let url = format!("{base}/{}", e.name);
        s.push_str(&format!("    \"{}\": {{\n", esc(e.key)));
        s.push_str(&format!("      \"arch\": \"{}\",\n", esc(e.arch)));
        s.push_str(&format!("      \"name\": \"{}\",\n", esc(&e.name)));
        s.push_str(&format!("      \"url\": \"{}\",\n", esc(&url)));
        s.push_str(&format!("      \"size\": {},\n", e.size));
        s.push_str(&format!("      \"sha256\": \"{}\",\n", esc(&e.sha256)));
        let more = e.zsync || e.note.is_some();
        s.push_str(&format!(
            "      \"signature_url\": \"{}.asc\"{}\n",
            esc(&url),
            if more { "," } else { "" }
        ));
        if e.zsync {
            let last = if e.note.is_some() { "," } else { "" };
            s.push_str(&format!(
                "      \"zsync_url\": \"{}\"{last}\n",
                esc(&format!("{base}/{ZSYNC_ALIAS}"))
            ));
        }
        if let Some(n) = e.note {
            s.push_str(&format!("      \"note\": \"{}\"\n", esc(n)));
        }
        s.push_str(if i + 1 < entries.len() { "    },\n" } else { "    }\n" });
    }
    s.push_str("  }\n}\n");
    s
}

/// Check the header of a `.zsync` file: it must download the versioned AppImage (`url`) and describe exactly its `size` and SHA-1.
pub(super) fn check_zsync(header: &str, url: &str, size: u64, sha1: &str) -> Result<(), String> {
    let mut seen = std::collections::HashMap::new();
    for l in header.lines() {
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            seen.insert(k.trim(), v.trim());
        }
    }
    let want = [("URL", url.to_string()), ("Length", size.to_string()), ("SHA-1", sha1.to_string())];
    for (k, w) in want {
        match seen.get(k) {
            Some(v) if *v == w => {}
            Some(v) => return Err(format!("zsync {k} is `{v}`, expected `{w}`")),
            None => return Err(format!("zsync header has no {k}")),
        }
    }
    Ok(())
}

impl Ctx {
    /// The base URL of the published files (`--base-url`, else the bucket).
    pub(super) fn base_url(&self) -> String {
        self.base_url.clone().unwrap_or_else(|| DIST_BASE_URL.to_string()).trim_end_matches('/').to_string()
    }

    /// The update information embedded in the AppImage.
    pub(super) fn update_info(&self) -> String {
        format!("zsync|{}/{ZSYNC_ALIAS}", self.base_url())
    }

    /// Write `rusty-wave-latest.json` in `dir` from the files named by `assets` that are in `dir` (with `strict`, a missing one is an
    /// error; otherwise it is left out). Signs it when `--sign` was given.
    pub(super) fn write_manifest(
        &self,
        dir: &Path,
        assets: &[Asset],
        strict: bool,
        base: &str,
    ) -> Result<PathBuf, String> {
        let mut entries = Vec::new();
        for a in assets {
            let p = dir.join(&a.name);
            if !p.is_file() {
                if strict {
                    return Err(format!("manifest: {} is missing in {}", a.name, dir.display()));
                }
                println!("(manifest: {} is not built, left out)", a.name);
                continue;
            }
            entries.push(Entry {
                key: a.key,
                arch: a.arch,
                name: a.name.clone(),
                size: fs::metadata(&p).map_err(|e| e.to_string())?.len(),
                sha256: sha256(&p)?,
                zsync: a.zsync.is_some(),
                note: note(a.key),
            });
        }
        if entries.is_empty() {
            return Err(format!("manifest: no release files in {} (build them first)", dir.display()));
        }
        let fpr = sign::resolve_public_fpr(&self.root)?;
        let text = render(&self.version, &self.date, &fpr, base, &entries);
        let path = dir.join(MANIFEST_NAME);
        write(&path, text.as_bytes())?;
        if self.sign.is_some() {
            self.gpg_detach(&path)?;
        }
        Ok(path)
    }

    /// `dist manifest [--base-url U] [--windows] [--macos] [--flatpak] [--web] [--tarball] [--all]`: the manifest of what is built in
    /// `target/dist/release`.
    pub(super) fn manifest(&self, x: Extras) -> Result<(), String> {
        let list = assets(&self.version, x);
        let path = self.write_manifest(&self.out(), &list, false, &self.base_url())?;
        print!("{}", fs::read_to_string(&path).map_err(|e| e.to_string())?);
        println!("{}", path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &'static str, arch: &'static str, name: &str, zsync: bool) -> Entry {
        Entry { key, arch, name: name.into(), size: 123, sha256: "ab".repeat(32), zsync, note: None }
    }

    #[test]
    fn manifest_has_the_agreed_shape() {
        let entries = [
            entry("linux-appimage", "x86_64", "rusty-wave-0.0.3-x86_64.AppImage", true),
            entry("linux-deb", "amd64", "rusty-wave_0.0.3_amd64.deb", false),
        ];
        let got = render(
            "0.0.3",
            "2026-10-06",
            "E13FF843723D54068E45A3FF54BF2FA407093CEE",
            "https://h.example/d/",
            &entries,
        );
        let sha = "ab".repeat(32);
        let want = format!(
            r#"{{
  "schema": 1,
  "version": "0.0.3",
  "released": "2026-10-06",
  "key_fingerprint": "E13FF843723D54068E45A3FF54BF2FA407093CEE",
  "files": {{
    "linux-appimage": {{
      "arch": "x86_64",
      "name": "rusty-wave-0.0.3-x86_64.AppImage",
      "url": "https://h.example/d/rusty-wave-0.0.3-x86_64.AppImage",
      "size": 123,
      "sha256": "{sha}",
      "signature_url": "https://h.example/d/rusty-wave-0.0.3-x86_64.AppImage.asc",
      "zsync_url": "https://h.example/d/rusty-wave-latest-x86_64.AppImage.zsync"
    }},
    "linux-deb": {{
      "arch": "amd64",
      "name": "rusty-wave_0.0.3_amd64.deb",
      "url": "https://h.example/d/rusty-wave_0.0.3_amd64.deb",
      "size": 123,
      "sha256": "{sha}",
      "signature_url": "https://h.example/d/rusty-wave_0.0.3_amd64.deb.asc"
    }}
  }}
}}
"#
        );
        assert_eq!(got, want);
    }

    #[test]
    fn the_dmg_is_marked_as_an_unsigned_beta() {
        let mut e = entry("macos-dmg", "universal", "a.dmg", false);
        e.note = note(e.key);
        let got = render("0.0.3", "2026-10-06", "F", "https://h", &[e]);
        assert!(got.contains("\"signature_url\": \"https://h/a.dmg.asc\",\n"), "{got}");
        assert!(got.contains("\"note\": \"beta: ad-hoc signed only"), "{got}");
        assert!(serde_json_ok(&got), "{got}");
    }

    /// Cheap well-formedness check without a JSON dependency: balanced braces and no `,` before a closing brace.
    fn serde_json_ok(s: &str) -> bool {
        s.matches('{').count() == s.matches('}').count() && !s.replace(char::is_whitespace, "").contains(",}")
    }

    #[test]
    fn manifest_escapes_and_accepts_file_urls() {
        let e = [entry("macos-dmg", "universal", "a\"b\\c.dmg", false)];
        let got = render("0.0.3", "2026-10-06", "F", "file:///tmp/dist", &e);
        assert!(got.contains(r#""name": "a\"b\\c.dmg""#), "{got}");
        assert!(got.contains(r#""url": "file:///tmp/dist/a\"b\\c.dmg""#), "{got}");
    }

    #[test]
    fn assets_cover_the_aliases_and_optional_platforms() {
        let base = assets("0.0.3", Extras::default());
        let keys: Vec<_> = base.iter().map(|a| a.key).collect();
        assert_eq!(keys, ["linux-appimage", "linux-deb", "linux-rpm"]);
        assert_eq!(base[0].alias, "rusty-wave-latest-x86_64.AppImage");
        assert_eq!(base[0].zsync.as_deref(), Some("rusty-wave-0.0.3-x86_64.AppImage.zsync"));
        assert_eq!(base[1].alias, "rusty-wave-latest_amd64.deb");
        assert_eq!(base[2].alias, "rusty-wave-latest-1.x86_64.rpm");
        let all = assets("0.0.3", Extras::ALL);
        let keys: Vec<_> = all.iter().map(|a| a.key).collect();
        assert_eq!(
            keys[3..],
            [
                "linux-tarball",
                "linux-flatpak",
                "windows-installer",
                "windows-portable",
                "web-pwa",
                "macos-dmg"
            ]
        );
        assert_eq!(all[4].name, "io.github.idometeor.RustyWave-0.0.3.flatpak");
        assert_eq!(all[7].name, "rusty-wave-web-0.0.3.zip");
        // An alias never carries a version, a versioned name always does.
        assert!(all.iter().all(|a| a.alias.contains("latest") && a.name.contains("0.0.3")));
        // Aliases are distinct, so no two files fight for one alias.
        let mut aliases: Vec<_> = all.iter().map(|a| &a.alias).collect();
        aliases.sort();
        aliases.dedup();
        assert_eq!(aliases.len(), all.len());
    }

    #[test]
    fn zsync_header_must_name_the_versioned_file() {
        let h = "zsync: 0.6.3\nFilename: a\nMTime: x\nBlocksize: 2048\nLength: 99\nHash-Lengths: 2,2,4\nURL: https://h/a-0.0.3\nSHA-1: deadbeef\n\nbinary";
        assert!(check_zsync(h, "https://h/a-0.0.3", 99, "deadbeef").is_ok());
        assert!(check_zsync(h, "https://h/a-latest", 99, "deadbeef").unwrap_err().contains("URL"));
        assert!(check_zsync(h, "https://h/a-0.0.3", 98, "deadbeef").unwrap_err().contains("Length"));
        assert!(check_zsync(h, "https://h/a-0.0.3", 99, "00").unwrap_err().contains("SHA-1"));
        // Only the header counts: a `URL:` after the blank line is binary data.
        assert!(check_zsync("zsync: 0.6.3\n\nURL: x\n", "x", 1, "s").unwrap_err().contains("no URL"));
    }
}
