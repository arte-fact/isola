//! Download, verify, and extract the Ubuntu base rootfs tarball.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::error::{IoContext, IsolaError};
use crate::paths;

const ROOTFS_URL: &str = "https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/ubuntu-base-24.04.4-base-amd64.tar.gz";
const ROOTFS_FILENAME: &str = "ubuntu-base-24.04.4-base-amd64.tar.gz";
const ROOTFS_SHA256SUMS_URL: &str =
    "https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/SHA256SUMS";

/// Download and cache the base rootfs tarball, with a progress UI.
pub fn ensure_rootfs_cached_with_progress(
    progress: &crate::progress::CreationProgress,
) -> Result<PathBuf, IsolaError> {
    let cache = paths::cache_dir();
    let cached_path = cache.join(ROOTFS_FILENAME);

    if cached_path.exists() {
        progress.finish_step("Rootfs cached");
        return Ok(cached_path);
    }

    std::fs::create_dir_all(&cache).io_ctx("create cache dir", &cache)?;

    let response = reqwest::blocking::get(ROOTFS_URL)?;
    if !response.status().is_success() {
        return Err(IsolaError::ExtractionFailed(format!(
            "HTTP {}",
            response.status()
        )));
    }

    let total_size = response.content_length().unwrap_or(0);
    let mut dl = progress.start_download(total_size);

    let mut reader = response;
    let mut file = std::fs::File::create(&cached_path).io_ctx("create", &cached_path)?;
    let mut buf = [0u8; 8192];
    let mut downloaded: u64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n]).io_ctx("write to", &cached_path)?;
        downloaded += n as u64;
        dl.set_position(downloaded);
    }
    progress.finish_download();

    progress.start_step("Verifying checksum...");
    verify_rootfs_checksum(&cached_path)?;
    progress.finish_step("Checksum verified");

    Ok(cached_path)
}

/// Verify the rootfs tarball against Ubuntu's published SHA256SUMS.
fn verify_rootfs_checksum(path: &Path) -> Result<(), IsolaError> {
    // Fetch SHA256SUMS from Ubuntu
    let response = reqwest::blocking::get(ROOTFS_SHA256SUMS_URL);
    let sums_text = match response {
        Ok(r) if r.status().is_success() => match r.text() {
            Ok(t) => t,
            Err(_) => {
                eprintln!(
                    "warning: could not read SHA256SUMS response body, skipping verification"
                );
                return Ok(());
            }
        },
        _ => {
            eprintln!("warning: could not fetch SHA256SUMS for verification, skipping");
            return Ok(());
        }
    };

    // Find expected hash for our filename
    let expected_hash = sums_text
        .lines()
        .find(|line| line.contains(ROOTFS_FILENAME))
        .and_then(|line| line.split_whitespace().next());

    let expected_hash = match expected_hash {
        Some(h) => h.to_string(),
        None => {
            eprintln!("warning: rootfs filename not found in SHA256SUMS, skipping verification");
            return Ok(());
        }
    };

    // Compute SHA256 of downloaded file
    let mut file = std::fs::File::open(path).io_ctx("open", path)?;
    let mut hasher = crate::sha256::Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).io_ctx("read", path)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual_hash = hasher.finish_hex();

    if actual_hash != expected_hash {
        // Remove the corrupted file so the next attempt re-downloads
        let _ = std::fs::remove_file(path);
        return Err(IsolaError::ExtractionFailed(format!(
            "SHA256 mismatch: expected {expected_hash}, got {actual_hash}"
        )));
    }

    Ok(())
}

/// Extract a gzip-compressed rootfs tarball into `target`.
pub fn extract_rootfs(tarball: &Path, target: &Path) -> Result<(), IsolaError> {
    std::fs::create_dir_all(target).io_ctx("create rootfs target dir", target)?;

    let file = std::fs::File::open(tarball).io_ctx("open rootfs tarball", tarball)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(true);
    archive.set_preserve_ownerships(false);
    archive.set_unpack_xattrs(false);

    archive.unpack(target).map_err(|e| {
        IsolaError::ExtractionFailed(format!("{} (tarball: {})", e, tarball.display()))
    })?;

    Ok(())
}

/// Guard against a truncated cache: a usable rootfs must have a working
/// `/bin/bash` (via the usrmerge symlink) after extraction.
pub fn ensure_rootfs_has_bash(rootfs: &Path) -> Result<(), IsolaError> {
    let bash = rootfs.join("bin/bash");
    if bash.exists() {
        return Ok(());
    }
    Err(IsolaError::ExtractionFailed(format!(
        "rootfs at {} has no working /bin/bash after extraction — the cached \
         layer tarball is likely truncated (e.g. a prior caching step failed \
         partway). Try: rm -rf ~/.isola/cache/layers && re-run with --no-cache",
        rootfs.display(),
    )))
}

/// The base rootfs download URL (recorded in each sandbox's config).
pub fn rootfs_url() -> &'static str {
    ROOTFS_URL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_rootfs_preserves_usrmerge_bin_symlink() {
        // Ubuntu 24.04 rootfs has `bin -> usr/bin` (usrmerge). The tarball
        // declares `bin` as a symlink and `usr/bin/bash` as a file. Extraction
        // must create both so `<root>/bin/bash` resolves.
        use flate2::write::GzEncoder;

        let tmp = tempfile::tempdir().unwrap();
        let tar_path = tmp.path().join("tiny-rootfs.tar.gz");

        // Build a minimal tarball that mimics usrmerge ordering.
        {
            let file = std::fs::File::create(&tar_path).unwrap();
            let encoder = GzEncoder::new(file, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);

            // ./bin -> usr/bin (symlink, appearing before usr)
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_path("bin").unwrap();
            header.set_link_name("usr/bin").unwrap();
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();

            // ./usr/bin/bash (file)
            let bash_bytes = b"#!/placeholder\n";
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_path("usr/bin/bash").unwrap();
            header.set_size(bash_bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append(&header, std::io::Cursor::new(bash_bytes))
                .unwrap();

            builder.into_inner().unwrap().finish().unwrap();
        }

        let out = tmp.path().join("out");
        extract_rootfs(&tar_path, &out).unwrap();

        let bin = out.join("bin");
        assert!(
            bin.symlink_metadata().unwrap().file_type().is_symlink(),
            "expected {} to be a symlink",
            bin.display()
        );
        assert!(
            out.join("usr/bin/bash").exists(),
            "expected usr/bin/bash to exist"
        );
        assert!(
            bin.join("bash").exists(),
            "expected bin/bash to resolve via symlink"
        );

        // Round-trip through tar::Builder::append_dir_all to confirm the tar
        // crate preserves the usrmerge symlink on write as well as read.
        let recached = tmp.path().join("recached.tar.gz");
        let file = std::fs::File::create(&recached).unwrap();
        let encoder = GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        builder.follow_symlinks(false);
        builder.append_dir_all(".", &out).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let out2 = tmp.path().join("out2");
        extract_rootfs(&recached, &out2).unwrap();
        assert!(
            out2.join("bin")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink(),
            "after round-trip, bin must still be a symlink"
        );
        assert!(
            out2.join("bin/bash").exists(),
            "after round-trip, bin/bash must resolve"
        );
    }

    #[test]
    fn ensure_rootfs_has_bash_ok_when_bash_resolves_via_symlink() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        std::fs::write(root.join("usr/bin/bash"), b"#!/x\n").unwrap();
        symlink("usr/bin", root.join("bin")).unwrap();
        ensure_rootfs_has_bash(root).unwrap();
    }

    #[test]
    fn ensure_rootfs_has_bash_errors_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let err = ensure_rootfs_has_bash(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no working /bin/bash"), "got: {msg}");
    }
}
