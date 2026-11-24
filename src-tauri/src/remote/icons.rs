//! The icons of the applications of the devices that this one controls.
//!
//! The bytes come from another device, so nothing about them is trusted. A file is kept only when
//! it is a small PNG. Its name is a hash of the device, the application, and its version, so no
//! text from the other device is ever a part of a path. The webview reads the folder through the
//! asset protocol, which is scoped to this folder alone.

use super::{ensure_private_dir, write_atomic};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Cursor,
    path::PathBuf,
};

/// The largest icon file that is kept.
pub const MAX_BYTES: usize = 256 * 1024;
/// The largest width and height of an icon that is kept.
pub const MAX_SIDE: u32 = 256;

pub struct Icons {
    dir: PathBuf,
}

/// The file name of an icon, without its extension: 64 hexadecimal characters.
fn stem(device: &str, bundle_id: &str, version: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(device.as_bytes());
    hash.update([0]);
    hash.update(bundle_id.as_bytes());
    hash.update([0]);
    hash.update(version.as_bytes());
    hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Accepts a PNG of at most `MAX_BYTES` bytes and `MAX_SIDE` pixels on each side that decodes.
pub fn check_png(bytes: &[u8]) -> Result<(), &'static str> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err("appIconFailed");
    }
    let mut reader = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .map_err(|_| "appIconFailed")?;
    let info = reader.info();
    if info.width == 0 || info.height == 0 || info.width > MAX_SIDE || info.height > MAX_SIDE {
        return Err("appIconFailed");
    }
    // The size is known now, so the decoded image fits in a small buffer.
    let size = reader.output_buffer_size().ok_or("appIconFailed")?;
    let mut pixels = vec![0; size];
    reader.next_frame(&mut pixels).map_err(|_| "appIconFailed")?;
    Ok(())
}

impl Icons {
    /// Creates the folder with mode 0700.
    pub fn open(dir: PathBuf) -> Result<Self, &'static str> {
        ensure_private_dir(&dir)?;
        Ok(Self { dir })
    }

    pub fn path(&self, device: &str, bundle_id: &str, version: &str) -> PathBuf {
        self.dir.join(format!("{}.png", stem(device, bundle_id, version)))
    }

    /// The file of an icon that is already kept. A link or an oversized file is not one.
    pub fn cached(&self, device: &str, bundle_id: &str, version: &str) -> Option<PathBuf> {
        let path = self.path(device, bundle_id, version);
        let meta = fs::symlink_metadata(&path).ok()?;
        (meta.is_file() && meta.len() as usize <= MAX_BYTES).then_some(path)
    }

    /// Keeps the icon when it is a small PNG. Anything else is dropped.
    pub fn store(
        &self,
        device: &str,
        bundle_id: &str,
        version: &str,
        png: &[u8],
    ) -> Result<PathBuf, &'static str> {
        check_png(png)?;
        let path = self.path(device, bundle_id, version);
        write_atomic(&path, png)?;
        Ok(path)
    }

    #[cfg(test)]
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{our_uid, testing::scratch};
    use std::os::unix::fs::MetadataExt;

    const DEVICE: &str = "0123456789abcdef0123456789abcdef";

    /// A PNG of the given size, as the `png` crate writes it.
    pub fn picture(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![200; (width * height * 4) as usize])
            .unwrap();
        writer.finish().unwrap();
        bytes
    }

    #[test]
    fn a_small_png_is_kept_under_a_hash_name_and_read_back() {
        let base = scratch("icons");
        let icons = Icons::open(base.join("icons")).unwrap();
        assert_eq!(fs::metadata(icons.dir()).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(icons.dir()).unwrap().uid(), our_uid());
        assert!(icons.cached(DEVICE, "com.apple.Safari", "17.0").is_none());
        let png = picture(128, 128);
        let path = icons.store(DEVICE, "com.apple.Safari", "17.0", &png).unwrap();
        assert_eq!(icons.cached(DEVICE, "com.apple.Safari", "17.0"), Some(path.clone()));
        assert_eq!(fs::read(&path).unwrap(), png);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        // The file is directly in the folder and its name is 64 hexadecimal characters and `.png`.
        assert_eq!(path.parent().unwrap(), icons.dir());
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 64 + 4);
        assert!(name[..64].bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));
        assert!(name.ends_with(".png"));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_name_changes_with_the_device_the_application_and_the_version() {
        let names = [
            stem(DEVICE, "com.apple.Safari", "17.0"),
            stem("fedcba9876543210fedcba9876543210", "com.apple.Safari", "17.0"),
            stem(DEVICE, "com.apple.Notes", "17.0"),
            stem(DEVICE, "com.apple.Safari", "17.1"),
            // The separator keeps `a` + `bc` and `ab` + `c` apart.
            stem(DEVICE, "a", "bc"),
            stem(DEVICE, "ab", "c"),
        ];
        for (index, name) in names.iter().enumerate() {
            assert_eq!(names.iter().filter(|other| *other == name).count(), 1, "{index}");
        }
    }

    #[test]
    fn text_from_the_other_device_is_never_a_part_of_the_path() {
        let base = scratch("icons-path");
        let icons = Icons::open(base.join("icons")).unwrap();
        for hostile in ["../../x", "/etc/passwd", "a/b", "..", ".", "\0", "x\n.png"] {
            let path = icons.path(DEVICE, hostile, hostile);
            assert_eq!(path.parent().unwrap(), icons.dir(), "{hostile:?}");
            assert_eq!(path.file_name().unwrap().len(), 64 + 4, "{hostile:?}");
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn bytes_that_are_not_a_small_png_are_dropped() {
        let base = scratch("icons-bad");
        let icons = Icons::open(base.join("icons")).unwrap();
        let png = picture(64, 64);
        let mut cut = png.clone();
        cut.truncate(cut.len() - 20);
        let mut oversized_file = picture(32, 32);
        oversized_file.resize(MAX_BYTES + 1, 0);
        for (label, bytes) in [
            ("empty", Vec::new()),
            ("text", b"<svg onload=alert(1)>".to_vec()),
            ("html", b"<html><script>1</script></html>".to_vec()),
            ("signature only", b"\x89PNG\r\n\x1a\n".to_vec()),
            ("cut", cut),
            ("too wide", picture(257, 16)),
            ("too tall", picture(16, 257)),
            ("too large a file", oversized_file),
        ] {
            assert_eq!(icons.store(DEVICE, "com.example.x", "1", &bytes), Err("appIconFailed"), "{label}");
            assert!(icons.cached(DEVICE, "com.example.x", "1").is_none(), "{label}");
        }
        // The edge is allowed.
        assert!(icons.store(DEVICE, "com.example.x", "1", &picture(256, 256)).is_ok());
        assert_eq!(fs::read_dir(icons.dir()).unwrap().count(), 1, "no temp file is left");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_link_in_the_folder_is_not_a_cached_icon() {
        let base = scratch("icons-link");
        let icons = Icons::open(base.join("icons")).unwrap();
        fs::write(base.join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(base.join("secret"), icons.path(DEVICE, "com.example.x", "1")).unwrap();
        assert!(icons.cached(DEVICE, "com.example.x", "1").is_none());
        fs::remove_dir_all(base).unwrap();
    }
}
