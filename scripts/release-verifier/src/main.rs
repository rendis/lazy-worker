//! Rehearses a release: installs the new version over a copy of the previous one with the real Tauri updater.
//!
//! `scripts/release.mjs` runs it against a local server before anything is published.
//! Keep `tauri` and `tauri-plugin-updater` at the versions the app ships.

use serde_json::{json, Value};
use std::{env, fs, path::PathBuf, time::Duration};
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri_plugin_updater::UpdaterExt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let [config, baseline, executable, platform, endpoint, expected]: [String; 6] =
        env::args().skip(1).collect::<Vec<_>>().try_into().expect("six arguments");
    assert!(endpoint.starts_with("http://127.0.0.1:"), "loopback server required");
    assert!(platform == "darwin-aarch64" || platform == "darwin-x86_64");
    let conf: Value = serde_json::from_slice(&fs::read(config)?)?;
    let mut updater_config = conf["plugins"]["updater"].clone();
    assert_eq!(updater_config["requireSignedVersion"], true);
    updater_config["endpoints"] = json!([endpoint]);
    updater_config["dangerousInsecureTransportProtocol"] = json!(true);

    let mut context = mock_context(noop_assets());
    context.package_info_mut().version = baseline.parse()?;
    context.config_mut().plugins.0.insert("updater".into(), updater_config);
    let app = mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)?;
    let updater = app
        .updater_builder()
        .executable_path(PathBuf::from(executable))
        .target(&platform)
        .no_proxy()
        .timeout(Duration::from_secs(45))
        .build()?;
    let mut downloaded = 0;
    tauri::async_runtime::block_on(async {
        let update = updater.check().await?.expect("a newer release");
        assert_eq!(update.current_version, baseline);
        assert_eq!(update.version, expected);
        update.download_and_install(|chunk, _| downloaded += chunk, || {}).await
    })?;
    println!("{platform}: Tauri downloaded, verified and installed {expected} over {baseline} ({downloaded} bytes)");
    Ok(())
}
