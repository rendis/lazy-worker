//! The fixed SSH key pair that reaches `remote --pair`.
//!
//! This key is public on purpose. It ships in every copy of the app, so anyone can read it.
//! It is not a secret and it grants no access to anything. A device that opens a pairing window
//! adds one `authorized_keys` line for it, and that line runs only `remote --pair`, only from
//! the local networks, and only until the window closes. The secret that counts is the 6-digit
//! code on the screen of the device that is being paired. The SPAKE2 exchange checks it without
//! sending it, and it binds both device identities and the host key of the target.

/// The public half, as it goes into `authorized_keys`, with no comment.
pub const PUBLIC: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHKjGk2Wx4XBjjiB3D8MToO7hsZa2IGGUxDFhJHVv/Lo";

/// The private half. The controller writes it to `pairing_key` with mode 0600, because ssh
/// refuses a key file that others can read.
pub const PRIVATE: &str = "\
-----BEGIN OPENSSH PRIVATE KEY-----\n\
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n\
QyNTUxOQAAACByoxpNlseFwY44gdw/DE6Du4bGWtiBhlMQxYSR1b/y6AAAAJixiJensYiX\n\
pwAAAAtzc2gtZWQyNTUxOQAAACByoxpNlseFwY44gdw/DE6Du4bGWtiBhlMQxYSR1b/y6A\n\
AAAEAbBk1Fgz0VBneQr5w91NisoJQ92p5ROM4XflVTnuERV3KjGk2Wx4XBjjiB3D8MToO7\n\
hsZa2IGGUxDFhJHVv/LoAAAAE2xhenktd29ya2VyOnBhaXJpbmcBAg==\n\
-----END OPENSSH PRIVATE KEY-----\n\
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{ssh::tool, testing::scratch, write_atomic};
    use std::{fs, path::Path};

    #[test]
    fn the_private_half_belongs_to_the_public_half() {
        if !Path::new("/usr/bin/ssh-keygen").exists() {
            return;
        }
        let base = scratch("pairing-key");
        let file = base.join("pairing_key");
        write_atomic(&file, PRIVATE.as_bytes()).unwrap();
        let output = tool("/usr/bin/ssh-keygen")
            .args(["-y", "-f"])
            .arg(&file)
            .output()
            .unwrap();
        assert!(output.status.success());
        let derived = String::from_utf8(output.stdout).unwrap();
        assert_eq!(derived.split_whitespace().take(2).collect::<Vec<_>>().join(" "), PUBLIC);
        assert!(crate::remote::identity::parse_public_key(PUBLIC).is_ok());
        fs::remove_dir_all(base).unwrap();
    }
}
