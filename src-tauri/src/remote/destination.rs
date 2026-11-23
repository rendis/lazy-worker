//! A place an SSH connection may go to. The only way to build one is through the parsers here,
//! so a value of this type can never act as an SSH option or carry a shell word.

use std::{
    fmt,
    net::{Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

pub const DEFAULT_PORT: u16 = 22;

#[derive(Clone, Debug, PartialEq)]
pub struct Destination {
    user: String,
    host: String,
    port: u16,
}

impl Destination {
    /// Builds a destination from separate fields. The host may be an IP literal or a DNS name.
    pub fn new(user: &str, host: &str, port: Option<u16>) -> Result<Self, &'static str> {
        let port = port.unwrap_or(DEFAULT_PORT);
        if port == 0 || !valid_user(user) {
            return Err("destinationInvalid");
        }
        Ok(Self {
            user: user.to_owned(),
            host: normalize_host(host)?,
            port,
        })
    }

    /// Parses `user@host`, `user@host:port`, `user@[v6]` and `user@[v6]:port`. Nothing else.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let (user, rest) = text.split_once('@').ok_or("destinationInvalid")?;
        let (host, port) = split_port(rest)?;
        Self::new(user, host, port)
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    /// An IP literal without brackets, or a lowercase DNS name.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "{}@[{}]:{}", self.user, self.host, self.port)
        } else {
            write!(f, "{}@{}:{}", self.user, self.host, self.port)
        }
    }
}

fn valid_user(user: &str) -> bool {
    (1..=32).contains(&user.len())
        && user
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        && !user.starts_with(['-', '.'])
}

/// Splits the host from an optional port.
fn split_port(text: &str) -> Result<(&str, Option<u16>), &'static str> {
    if let Some(rest) = text.strip_prefix('[') {
        let (inner, after) = rest.split_once(']').ok_or("destinationInvalid")?;
        // Brackets are for IPv6 only.
        if !inner.contains(':') {
            return Err("destinationInvalid");
        }
        return match after {
            "" => Ok((inner, None)),
            _ => Ok((
                inner,
                Some(parse_port(
                    after.strip_prefix(':').ok_or("destinationInvalid")?,
                )?),
            )),
        };
    }
    match text.matches(':').count() {
        0 => Ok((text, None)),
        1 => {
            let (host, port) = text.split_once(':').ok_or("destinationInvalid")?;
            Ok((host, Some(parse_port(port)?)))
        }
        // More colons mean a bare IPv6 address, which has no port.
        _ => Ok((text, None)),
    }
}

fn parse_port(text: &str) -> Result<u16, &'static str> {
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("destinationInvalid");
    }
    match text.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err("destinationInvalid"),
    }
}

/// Accepts an IPv4 literal, an IPv6 literal (brackets optional), or a DNS name.
fn normalize_host(host: &str) -> Result<String, &'static str> {
    let bare = match host.strip_prefix('[') {
        Some(rest) => rest.strip_suffix(']').ok_or("destinationInvalid")?,
        None => host,
    };
    if bare.contains(':') {
        return ipv6_with_zone(bare);
    }
    // Brackets belong to IPv6 only.
    if bare.len() != host.len() {
        return Err("destinationInvalid");
    }
    if let Ok(address) = Ipv4Addr::from_str(bare) {
        return Ok(address.to_string());
    }
    if !valid_dns_name(bare) {
        return Err("destinationInvalid");
    }
    Ok(bare.to_ascii_lowercase())
}

/// A zone names a network interface and is for link-local addresses only.
fn ipv6_with_zone(text: &str) -> Result<String, &'static str> {
    let (address, zone) = match text.split_once('%') {
        Some((address, zone)) => (address, Some(zone)),
        None => (text, None),
    };
    let parsed = Ipv6Addr::from_str(address).map_err(|_| "destinationInvalid")?;
    match zone {
        None => Ok(parsed.to_string()),
        Some(zone) => {
            let valid = (1..=15).contains(&zone.len())
                && zone.bytes().all(|byte| byte.is_ascii_alphanumeric());
            if !valid || parsed.segments()[0] & 0xffc0 != 0xfe80 {
                return Err("destinationInvalid");
            }
            Ok(format!("{parsed}%{zone}"))
        }
    }
}

fn valid_dns_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 253 || name.starts_with(['-', '.']) {
        return false;
    }
    let name = name.strip_suffix('.').unwrap_or(name);
    let labels_ok = name.split('.').all(|label| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    });
    // A last label of digits only could read as an odd IPv4 form such as `1.2.3` or `0x7f.1`.
    let last = name.rsplit('.').next().unwrap_or("");
    labels_ok && !last.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_name() -> String {
        [
            "a".repeat(63),
            "a".repeat(63),
            "a".repeat(63),
            "a".repeat(63),
        ]
        .join(".")
    }

    #[test]
    fn hostile_inputs_fail() {
        let long = long_name();
        assert!(long.len() > 253);
        let hostile = [
            "",
            "@",
            "u@",
            "@host",
            "-oProxyCommand=x",
            "u@-oProxyCommand=x",
            "-oProxyCommand=x@host",
            "u@-J",
            "u@-J host",
            "u@host;id",
            "u@$(id)",
            "u@`id`",
            "u@a b",
            "u@host\nHost *",
            "u\n@host",
            "u@host\r",
            "u@%h",
            "u@host%h",
            "h@x@y",
            "u@h@x",
            "ssh://h",
            "ssh://u@h",
            "u@ssh://h",
            "u@[::1",
            "u@::1]",
            "u@[host]",
            "u@[1.2.3.4]",
            "u@ex ample.com",
            "u@host/path",
            "u@host:",
            "u@host:0",
            "u@host:65536",
            "u@host:+22",
            "u@host:2 2",
            "u@host:22:22",
            "u@[::1]x",
            "u@[::1]:",
            "u@[::1]:99999",
            "u@[2001:db8::1%en0]",
            "u@fe80::1%",
            "u@fe80::1%en0%en1",
            "u@fe80::1%0123456789abcdef",
            "u@fe80::1%en-0",
            "u@2001:db8::1%en0",
            "u@.host",
            "u@host..com",
            "u@-host",
            "u@host-.com",
            "u@a.-b.com",
            "u@1.2.3",
            "u@1.2.3.4.5",
            "u@256.1.1.1",
            "u@0x7f.1",
            "u@im\u{0430}c.local",
            "u@\u{ff49}mac.local",
            "u@imac.local\u{200b}",
            "u@h\0ost",
            "u@host'",
            "u@\"host\"",
            "u@ho\\st",
            "u@host,other",
            "u@host*",
        ];
        for input in hostile {
            assert!(Destination::parse(input).is_err(), "accepted {input:?}");
        }
        assert!(Destination::parse(&format!("u@{long}")).is_err());
        for user in [
            "",
            "-u",
            ".u",
            "a b",
            "a@b",
            "a;b",
            "a\nb",
            "ü",
            &"a".repeat(33),
            "$(id)",
            "a/b",
        ] {
            assert!(
                Destination::new(user, "host", None).is_err(),
                "accepted user {user:?}"
            );
        }
        for host in ["", " ", "-o", "[::1", "a:b", "host\n", "%h", "[1.2.3.4]"] {
            assert!(
                Destination::new("u", host, None).is_err(),
                "accepted host {host:?}"
            );
        }
        assert!(Destination::new("u", "host", Some(0)).is_err());
    }

    #[test]
    fn good_inputs_pass() {
        let cases = [
            ("me@192.168.1.20", "me", "192.168.1.20", 22),
            ("me@100.64.1.2:2222", "me", "100.64.1.2", 2222),
            ("me@::1", "me", "::1", 22),
            ("me@[::1]", "me", "::1", 22),
            ("me@[::1]:2200", "me", "::1", 2200),
            ("me@[fe80::1%en0]:22", "me", "fe80::1%en0", 22),
            ("me@fe80::1%en0", "me", "fe80::1%en0", 22),
            ("me@[2001:DB8::1]", "me", "2001:db8::1", 22),
            ("me@imac.local", "me", "imac.local", 22),
            ("me@IMac.Local:22", "me", "imac.local", 22),
            ("me@imac.tail1234.ts.net", "me", "imac.tail1234.ts.net", 22),
            ("me@imac.local.", "me", "imac.local.", 22),
            ("a_b.c-d@host-1", "a_b.c-d", "host-1", 22),
            ("me@my-mac2", "me", "my-mac2", 22),
        ];
        for (input, user, host, port) in cases {
            let parsed = Destination::parse(input).unwrap_or_else(|code| panic!("{input}: {code}"));
            assert_eq!(
                (parsed.user(), parsed.host(), parsed.port()),
                (user, host, port),
                "{input}"
            );
        }
        let separate = Destination::new("me", "[fe80::1%en0]", Some(2222)).unwrap();
        assert_eq!(separate.host(), "fe80::1%en0");
        assert_eq!(separate.to_string(), "me@[fe80::1%en0]:2222");
        assert_eq!(
            Destination::new("me", &"a".repeat(63), None)
                .unwrap()
                .host()
                .len(),
            63
        );
    }

    #[test]
    fn display_parses_back_to_the_same_value() {
        for input in ["me@imac.local", "me@100.64.1.2:2222", "me@[fe80::1%en0]:22"] {
            let parsed = Destination::parse(input).unwrap();
            assert_eq!(Destination::parse(&parsed.to_string()).unwrap(), parsed);
        }
    }
}
