use super::{run_limited, valid_bundle_id, App, ICON_TIMEOUT, MAX_APPS, MAX_ICON};
use crate::remote::peers::clean_name;
use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};
use plist::{Dictionary, Value};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

/// The folders that hold applications. A plain folder inside the first one is read too.
const SYSTEM_FOLDERS: [&str; 4] = [
    "/Applications",
    "/Applications/Utilities",
    "/System/Applications",
    "/System/Applications/Utilities",
];
/// A folder with more entries than this is read only up to it.
const MAX_ENTRIES: usize = 4000;
/// An `Info.plist` larger than this is not an application's.
const MAX_PLIST: u64 = 1024 * 1024;
const MAX_VERSION: usize = 32;

pub fn supported() -> Result<(), &'static str> {
    Ok(())
}

pub fn scan() -> Vec<App> {
    let mut roots: Vec<(PathBuf, bool)> = SYSTEM_FOLDERS
        .iter()
        .enumerate()
        .map(|(index, folder)| (PathBuf::from(folder), index == 0))
        .collect();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.push((home.join("Applications"), false));
    }
    scan_in(&roots)
}

/// The names in a folder, in a fixed order so that the same identifier always comes from the same copy.
fn entries(folder: &Path) -> Vec<(PathBuf, bool)> {
    let Ok(read) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<(PathBuf, bool)> = read
        .take(MAX_ENTRIES)
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        // A link is not a plain folder, so only a real directory is entered.
        .map(|entry| {
            let plain = entry.file_type().is_ok_and(|kind| kind.is_dir());
            (entry.path(), plain)
        })
        .collect();
    found.sort();
    found
}

fn is_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "app")
}

/// Reads the folders in order. The first copy of an identifier wins.
pub(super) fn scan_in(roots: &[(PathBuf, bool)]) -> Vec<App> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    let mut add = |path: &Path| {
        if apps.len() >= MAX_APPS {
            return;
        }
        // Safari is a link on recent systems, so the real bundle is read.
        let Ok(real) = fs::canonicalize(path) else {
            return;
        };
        if let Some(app) = read_app(&real).filter(|app| seen.insert(app.bundle_id.clone())) {
            apps.push(app);
        }
    };
    for (root, nested) in roots {
        for (path, plain) in entries(root) {
            if is_bundle(&path) {
                add(&path);
            } else if *nested && plain {
                for (inner, _) in entries(&path) {
                    if is_bundle(&inner) {
                        add(&inner);
                    }
                }
            }
        }
    }
    apps.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.bundle_id.cmp(&b.bundle_id))
    });
    apps
}

fn read_app(bundle: &Path) -> Option<App> {
    if !is_bundle(bundle) {
        return None;
    }
    let info = bundle.join("Contents/Info.plist");
    if fs::metadata(&info).ok()?.len() > MAX_PLIST {
        return None;
    }
    let value = Value::from_file(&info).ok()?;
    let stem = bundle.file_stem()?.to_str()?;
    let (bundle_id, name, version) = parse_info(value.as_dictionary()?, stem)?;
    Some(App {
        bundle_id,
        name,
        version,
        path: bundle.to_owned(),
    })
}

/// A flag in an `Info.plist` is a boolean, and in some files a string or a number.
fn flag(info: &Dictionary, key: &str) -> bool {
    match info.get(key) {
        Some(Value::Boolean(value)) => *value,
        Some(Value::String(text)) => {
            matches!(text.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")
        }
        Some(Value::Integer(number)) => number.as_signed().is_some_and(|number| number != 0),
        _ => false,
    }
}

fn text<'a>(info: &'a Dictionary, key: &str) -> Option<&'a str> {
    info.get(key).and_then(Value::as_string)
}

/// The identifier, the name, and the version of an application, or nothing for one that has no
/// valid identifier or that runs in the background only. Text goes to people, so it is cleaned
/// as a device name is.
pub(super) fn parse_info(info: &Dictionary, stem: &str) -> Option<(String, String, String)> {
    let bundle_id = text(info, "CFBundleIdentifier")?.trim();
    if !valid_bundle_id(bundle_id) || flag(info, "LSUIElement") || flag(info, "LSBackgroundOnly") {
        return None;
    }
    let name = ["CFBundleDisplayName", "CFBundleName"]
        .into_iter()
        .filter_map(|key| text(info, key))
        .map(clean_name)
        .chain(std::iter::once(clean_name(stem)))
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| bundle_id.to_owned());
    let version = text(info, "CFBundleShortVersionString")
        .map(|version| {
            clean_name(version)
                .chars()
                .take(MAX_VERSION)
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .unwrap_or_default();
    Some((bundle_id.to_owned(), name, version))
}

/// The identifiers of the applications that are open and that have a Dock icon.
pub fn running() -> HashSet<String> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| app.activationPolicy() == NSApplicationActivationPolicy::Regular)
        .filter_map(|app| app.bundleIdentifier())
        .map(|id| id.to_string())
        .filter(|id| valid_bundle_id(id))
        .collect()
}

/// Asks the helper for one 128 x 128 PNG of the icon of the bundle at `path`.
pub fn render_icon(path: &Path) -> Result<Vec<u8>, &'static str> {
    let mut command = crate::activity::helper_command().map_err(|_| "appIconFailed")?;
    command.arg("--app-icon").arg(path);
    run_limited(command, ICON_TIMEOUT, MAX_ICON)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::testing::scratch;

    fn info(entries: &[(&str, Value)]) -> Dictionary {
        let mut info = Dictionary::new();
        for (key, value) in entries {
            info.insert((*key).to_owned(), value.clone());
        }
        info
    }

    fn string(text: &str) -> Value {
        Value::String(text.to_owned())
    }

    #[test]
    fn the_name_comes_from_the_display_name_then_the_bundle_name_then_the_file() {
        let id = ("CFBundleIdentifier", string("com.example.app"));
        let both = info(&[
            id.clone(),
            ("CFBundleDisplayName", string("Display")),
            ("CFBundleName", string("Name")),
        ]);
        assert_eq!(parse_info(&both, "File").unwrap().1, "Display");
        let bundle = info(&[id.clone(), ("CFBundleName", string("Name"))]);
        assert_eq!(parse_info(&bundle, "File").unwrap().1, "Name");
        assert_eq!(parse_info(&info(&[id.clone()]), "File").unwrap().1, "File");
        // A name that is empty after cleaning falls through to the next one.
        let blank = info(&[id, ("CFBundleDisplayName", string(" \u{200B}\u{202E} "))]);
        assert_eq!(parse_info(&blank, "File").unwrap().1, "File");
    }

    #[test]
    fn text_from_the_file_is_cleaned_like_a_device_name() {
        let hostile = info(&[
            ("CFBundleIdentifier", string(" com.example.app ")),
            ("CFBundleDisplayName", string("\u{202E}Evil\u{0007} App\n")),
            ("CFBundleShortVersionString", string(&format!("\u{200B}1.2.3{}", "9".repeat(60)))),
        ]);
        let (id, name, version) = parse_info(&hostile, "File").unwrap();
        assert_eq!(id, "com.example.app");
        assert_eq!(name, "Evil App");
        assert_eq!(version.chars().count(), 32);
        assert!(version.starts_with("1.2.3"));
        let long = info(&[
            ("CFBundleIdentifier", string("com.example.app")),
            ("CFBundleName", string(&"n".repeat(200))),
        ]);
        assert_eq!(parse_info(&long, "File").unwrap().1.chars().count(), 60);
    }

    #[test]
    fn an_application_needs_a_valid_identifier() {
        for bad in ["", "bad id", "-x", "com.example/app", "a;b"] {
            let invalid = info(&[("CFBundleIdentifier", string(bad))]);
            assert!(parse_info(&invalid, "File").is_none(), "{bad:?}");
        }
        assert!(parse_info(&Dictionary::new(), "File").is_none());
        let number = info(&[("CFBundleIdentifier", Value::Integer(5.into()))]);
        assert!(parse_info(&number, "File").is_none());
    }

    #[test]
    fn background_and_agent_applications_are_skipped() {
        let id = ("CFBundleIdentifier", string("com.example.app"));
        for flag in ["LSUIElement", "LSBackgroundOnly"] {
            for on in [Value::Boolean(true), string("1"), string("YES"), Value::Integer(1.into())] {
                let hidden = info(&[id.clone(), (flag, on)]);
                assert!(parse_info(&hidden, "File").is_none(), "{flag}");
            }
            for off in [Value::Boolean(false), string("0"), Value::Integer(0.into())] {
                let shown = info(&[id.clone(), (flag, off)]);
                assert!(parse_info(&shown, "File").is_some(), "{flag}");
            }
        }
    }

    fn write_app(folder: &Path, file: &str, id: &str, name: &str) {
        let contents = folder.join(file).join("Contents");
        fs::create_dir_all(&contents).unwrap();
        let mut dictionary = Dictionary::new();
        dictionary.insert("CFBundleIdentifier".into(), string(id));
        dictionary.insert("CFBundleName".into(), string(name));
        Value::Dictionary(dictionary)
            .to_file_xml(contents.join("Info.plist"))
            .unwrap();
    }

    #[test]
    fn the_scan_follows_links_skips_duplicates_and_reads_one_plain_folder_level() {
        let base = scratch("apps-scan");
        let applications = base.join("Applications");
        let elsewhere = base.join("elsewhere");
        fs::create_dir_all(&applications).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        write_app(&applications, "Zed.app", "com.example.zed", "Zed");
        write_app(&applications, "Alpha.app", "com.example.alpha", "alpha");
        // The same identifier twice: the first folder wins.
        write_app(&applications, "Alpha2.app", "com.example.alpha", "Copy");
        write_app(&elsewhere, "Linked.app", "com.example.linked", "Linked");
        std::os::unix::fs::symlink(elsewhere.join("Linked.app"), applications.join("Link.app")).unwrap();
        // A plain folder is read one level. A bundle folder or a deeper one is not.
        write_app(&applications.join("Suite"), "Tool.app", "com.example.tool", "Tool");
        write_app(&applications.join("Suite/Deep"), "Hidden.app", "com.example.deep", "Deep");
        write_app(&applications.join("Pack.app/Contents/Inner"), "Nested.app", "com.example.nested", "Nested");
        // A link to a folder is not entered.
        std::os::unix::fs::symlink(&elsewhere, applications.join("Shortcut")).unwrap();
        write_app(&applications.join("Not Apps"), "Plain.txt", "com.example.plain", "Plain");
        // A background app, and a bundle with no valid Info.plist.
        let agent = applications.join("Agent.app/Contents");
        fs::create_dir_all(&agent).unwrap();
        fs::write(
            agent.join("Info.plist"),
            r#"<?xml version="1.0"?><plist version="1.0"><dict>
            <key>CFBundleIdentifier</key><string>com.example.agent</string>
            <key>LSUIElement</key><true/></dict></plist>"#,
        )
        .unwrap();
        fs::create_dir_all(applications.join("Broken.app/Contents")).unwrap();
        fs::write(applications.join("Broken.app/Contents/Info.plist"), "not a plist").unwrap();
        // The second folder has the same identifier as the first, and a new one.
        let second = base.join("second");
        write_app(&second, "Alpha Again.app", "com.example.alpha", "Again");
        write_app(&second, "Beta.app", "com.example.beta", "Beta");

        let found = scan_in(&[(applications.clone(), true), (second, false)]);
        let ids: Vec<_> = found.iter().map(|app| app.bundle_id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "com.example.alpha",
                "com.example.beta",
                "com.example.linked",
                "com.example.tool",
                "com.example.zed",
            ]
        );
        // The first copy of an identifier is the one that is kept, and a link gives the real bundle.
        assert_eq!(found[0].name, "alpha");
        assert!(found[0].path.ends_with("Alpha.app"));
        let linked = found.iter().find(|app| app.bundle_id == "com.example.linked").unwrap();
        assert_eq!(linked.path, fs::canonicalize(elsewhere.join("Linked.app")).unwrap());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_list_is_capped() {
        let base = scratch("apps-cap");
        for number in 0..MAX_APPS + 5 {
            write_app(&base, &format!("A{number}.app"), &format!("com.example.a{number}"), "A");
        }
        assert_eq!(scan_in(&[(base.clone(), false)]).len(), MAX_APPS);
        fs::remove_dir_all(base).unwrap();
    }

    /// Reads this Mac only. It prints counts and sizes, and opens nothing.
    #[test]
    fn this_mac_lists_its_applications_reads_the_open_state_and_draws_an_icon() {
        let apps = scan();
        let running = running();
        let listed = super::super::list().expect("the list");
        println!("installed applications: {}", apps.len());
        println!("listed applications: {}", listed.len());
        println!("open applications with a Dock icon: {}", running.len());
        println!("listed as open: {}", listed.iter().filter(|app| app.open).count());
        assert!(!apps.is_empty());
        assert_eq!(listed.len(), apps.len());
        let calculator = Path::new("/System/Applications/Calculator.app");
        if !calculator.exists() {
            return;
        }
        let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "binaries/activity-helper-{}-apple-darwin",
            if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" }
        ));
        let mut command = std::process::Command::new(helper);
        command.arg("--app-icon").arg(calculator);
        let png = run_limited(command, ICON_TIMEOUT, MAX_ICON).expect("the helper draws an icon");
        println!("Calculator icon: {} bytes", png.len());
        assert!(png.starts_with(super::super::PNG_SIGNATURE));
        let decoder = png::Decoder::new(std::io::Cursor::new(&png)).read_info().unwrap();
        let (width, height) = (decoder.info().width, decoder.info().height);
        println!("Calculator icon size: {width} x {height}");
        assert_eq!((width, height), (128, 128));
    }
}
