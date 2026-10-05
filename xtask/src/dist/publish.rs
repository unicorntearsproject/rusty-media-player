//! `cargo xtask dist publish`: copy the verified installers of this version, with a checksum file and signatures, to the local distribution
//! folder and to the S3 bucket. Never overwrites: it stops before copying anything if any file of this version exists in either place.
//!
//! Published (names carry the version, as the files come out of the build): `rusty-wave_<ver>_amd64.deb`, `rusty-wave-<ver>-1.x86_64.rpm`,
//! `rusty-wave-<ver>-x86_64.AppImage`, a detached `.asc` for each, `rusty-wave-<ver>-SHA256SUMS` and its `.asc`. The Windows installer and zip go
//! along only with `--windows` (say so only when they were built and verified in the same run). The sums file is named by version because the
//! destinations hold every version side by side.
use super::*;

const LOCAL_DIR: &str = "/home/jj/projects/_software-dist/rusty-wave";
const BUCKET: &str = "ut-software-dist";

impl Ctx {
    pub(super) fn publish(&self, windows: bool, dry_run: bool) -> Result<(), String> {
        if self.sign.is_none() {
            return Err("publish needs --sign (it signs the checksum file and checks every signature first)".into());
        }
        let ver = &self.version;
        if *ver != workspace_version(&self.root)? {
            return Err(format!("publish: --version {ver} differs from Cargo.toml; the deb and rpm carry Cargo's version"));
        }
        // 1. Everything in target/dist/release verifies against the committed public key (a throwaway keyring).
        self.verify()?;
        // 2. The files of this version.
        let mut names = vec![
            format!("rusty-wave_{ver}_amd64.deb"),
            format!("rusty-wave-{ver}-1.x86_64.rpm"),
            format!("rusty-wave-{ver}-x86_64.AppImage"),
        ];
        if windows {
            names.push(format!("rusty-wave-{ver}-x64-Setup.exe"));
            names.push(format!("rusty-wave-{ver}-windows-x64.zip"));
        }
        let out = self.out();
        for n in &names {
            for f in [n.clone(), format!("{n}.asc")] {
                if !out.join(&f).is_file() {
                    return Err(format!("publish: {f} is missing in {} (build with --sign)", out.display()));
                }
            }
        }
        // 3. A staging folder with exactly the published files, plus the checksum file for this version, signed and checked.
        let stage = self.dist().join("publish").join(format!("rusty-wave-{ver}"));
        fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
        if fs::read_dir(&stage).map_err(|e| e.to_string())?.next().is_some() {
            return Err(format!("publish: {} is not empty (a previous run); move it away first", stage.display()));
        }
        let mut sums = String::new();
        for n in &names {
            copy(&out.join(n), &stage.join(n))?;
            copy(&out.join(format!("{n}.asc")), &stage.join(format!("{n}.asc")))?;
            sums.push_str(&format!("{}  {n}\n", sha256(&stage.join(n))?));
        }
        let sums_name = format!("rusty-wave-{ver}-SHA256SUMS");
        write(&stage.join(&sums_name), sums.as_bytes())?;
        self.gpg_detach(&stage.join(&sums_name))?;
        sh(Command::new("sha256sum").current_dir(&stage).args(["-c", &sums_name]))?;
        let fpr = sign::resolve_public_fpr(&self.root)?;
        let o = Command::new("gpg")
            .env("GNUPGHOME", self.dist().join("verify-gnupg"))
            .args(["--batch", "--no-tty", "--status-fd", "1", "--verify"])
            .arg(stage.join(format!("{sums_name}.asc")))
            .arg(stage.join(&sums_name))
            .output()
            .map_err(|e| e.to_string())?;
        if !String::from_utf8_lossy(&o.stdout).contains(&format!("VALIDSIG {fpr}")) {
            return Err("publish: the checksum signature does not verify against the public key".into());
        }
        let mut files: Vec<String> = Vec::new();
        for n in &names {
            files.push(n.clone());
            files.push(format!("{n}.asc"));
        }
        files.push(sums_name.clone());
        files.push(format!("{sums_name}.asc"));
        // 4. Refuse to overwrite: check both destinations before touching either.
        let local = Path::new(LOCAL_DIR);
        let clash: Vec<&String> = files.iter().filter(|f| local.join(f).exists()).collect();
        if !clash.is_empty() {
            return Err(format!("publish: already in {LOCAL_DIR}: {clash:?}; bump the version"));
        }
        let mut clash = Vec::new();
        for f in &files {
            let o = Command::new("aws")
                .args(["s3api", "head-object", "--bucket", BUCKET, "--key", f])
                .output()
                .map_err(|e| format!("aws: {e}"))?;
            if o.status.success() {
                clash.push(f.clone());
            } else {
                let e = String::from_utf8_lossy(&o.stderr);
                if !(e.contains("404") || e.contains("Not Found")) {
                    return Err(format!("publish: could not check s3://{BUCKET}/{f}: {}", e.trim()));
                }
            }
        }
        if !clash.is_empty() {
            return Err(format!("publish: already in s3://{BUCKET}/: {clash:?}; bump the version"));
        }
        println!("publish: {} files, {} to {LOCAL_DIR} and s3://{BUCKET}/", files.len(), if dry_run { "would copy" } else { "copying" });
        for f in &files {
            println!("  {f}");
        }
        if dry_run {
            return Ok(());
        }
        for f in &files {
            copy(&stage.join(f), &local.join(f))?;
        }
        for f in &files {
            sh(Command::new("aws").args(["s3", "cp", "--only-show-errors"]).arg(stage.join(f)).arg(format!("s3://{BUCKET}/{f}")))?;
        }
        println!("published {} files for {ver}", files.len());
        Ok(())
    }
}
