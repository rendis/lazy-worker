#!/usr/bin/env node
// Builds, signs, notarizes, packages, rehearses and drafts a macOS release.
//
//   npm run release -- <version> --summary "One line about the release"
//   npm run release -- --dry-run
//
// A dry run does every step for the current version and stops before it changes the repository
// or GitHub. Local names, such as the signing identity, come from `.release.local.json`.

import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import { createServer } from "node:http";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const identifier = "com.rendis.lazyworker";
const repository = "rendis/lazy-worker";
const targets = [
  { arch: "arm64", triple: "aarch64-apple-darwin", platform: "darwin-aarch64" },
  { arch: "x86_64", triple: "x86_64-apple-darwin", platform: "darwin-x86_64" },
];
const versionFiles = [
  ["package.json", version => `"version": "${version}"`, 1],
  ["package-lock.json", version => `"version": "${version}"`, 2],
  ["src-tauri/tauri.conf.json", version => `"version": "${version}"`, 1],
  ["src-tauri/Cargo.toml", version => `\nversion = "${version}"`, 1],
  ["src-tauri/Cargo.lock", version => `name = "lazy-worker"\nversion = "${version}"`, 1],
];

function fail(message) {
  console.error(`\nRelease stopped: ${message}`);
  process.exit(1);
}

function step(title) {
  console.log(`\n== ${title}`);
}

/** Runs a command and returns its output. `quiet` keeps the command's own output off the terminal. */
function run(command, args, { env, quiet = false, allowFailure = false } = {}) {
  try {
    return execFileSync(command, args, {
      cwd: root,
      env: { ...process.env, ...env },
      encoding: "utf8",
      stdio: quiet ? ["ignore", "pipe", "pipe"] : ["ignore", "pipe", "inherit"],
      maxBuffer: 64 * 1024 * 1024,
    }).trim();
  } catch (error) {
    if (allowFailure) return undefined;
    fail(`\`${command} ${args.join(" ")}\` failed.\n${error.stderr ?? error.message}`);
  }
}

/** Runs a command without blocking the process, so the local update server can answer it. */
function runAsync(command, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, stdio: "inherit" });
    child.on("error", reject);
    child.on("exit", code => (code === 0 ? resolve() : reject(new Error(`${command} exited with ${code}`))));
  });
}

function read(file) {
  return fs.readFileSync(path.join(root, file), "utf8");
}

function compareVersions(a, b) {
  const [left, right] = [a, b].map(version => version.split(".").map(Number));
  return left[0] - right[0] || left[1] - right[1] || left[2] - right[2];
}

function plist(app, key) {
  return run("plutil", ["-extract", key, "raw", path.join(app, "Contents/Info.plist")], { quiet: true });
}

/** Output of a tool that reports on standard error, such as `codesign -dv`. */
function report(command, ...args) {
  return run("sh", ["-c", 'exec "$@" 2>&1', "sh", command, ...args], { quiet: true, allowFailure: true }) ?? "";
}

function sha256(file) {
  return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

// ---------------------------------------------------------------------------------------------- input

/** Sends a file to Apple, waits for the answer, and staples the ticket to `target`. */
function notarize(upload, target, label) {
  const result = JSON.parse(run("xcrun", ["notarytool", "submit", upload, "--keychain-profile", local.notaryProfile,
    "--wait", "--output-format", "json"], { quiet: true }));
  if (result.status !== "Accepted") {
    fail(`Apple answered "${result.status}" for ${label}. See \`xcrun notarytool log ${result.id}\`.`);
  }
  run("xcrun", ["stapler", "staple", "-q", target], { quiet: true });
  // The ticket check reaches Apple and can fail once without a real fault.
  const stapled = [1, 2, 3].some(() => run("xcrun", ["stapler", "validate", "-q", target], { quiet: true, allowFailure: true }) !== undefined);
  if (!stapled) fail(`the notarization ticket on ${label} does not validate.`);
}

const args = process.argv.slice(2);
const dryRun = args.includes("--dry-run");
const summaryIndex = args.indexOf("--summary");
const summary = summaryIndex >= 0 ? args[summaryIndex + 1] : undefined;
const current = JSON.parse(read("package.json")).version;
const version = args.find((arg, index) => !arg.startsWith("--") && (summaryIndex < 0 || index !== summaryIndex + 1))
  ?? (dryRun ? current : undefined);

if (!version || !/^\d+\.\d+\.\d+$/.test(version)) {
  fail("give a version such as 0.5.0, or use --dry-run to rehearse the current version.");
}
if (!dryRun && !summary) fail('give one line for the update notice: --summary "What changed".');
if (!dryRun && compareVersions(version, current) <= 0) fail(`${version} is not newer than ${current}.`);
if (dryRun && version !== current) fail("a dry run rehearses the current version, so give no other version.");

const localFile = path.join(root, ".release.local.json");
if (!fs.existsSync(localFile)) {
  fail("`.release.local.json` is missing. Copy `scripts/release.local.example.json` and fill in your local names.");
}
const local = JSON.parse(fs.readFileSync(localFile, "utf8"));
for (const key of ["signingIdentity", "notaryProfile", "updaterKeyService"]) {
  if (!local[key]) fail(`\`.release.local.json\` has no \`${key}\`.`);
}

// A release uses its own toolchain when the everyday one lacks a target.
const rustHome = local.rustHome?.replace(/^~(?=\/)/, process.env.HOME);
const buildEnvironment = {
  APPLE_SIGNING_IDENTITY: local.signingIdentity,
  ...(rustHome && {
    CARGO_HOME: path.join(rustHome, "cargo"),
    RUSTUP_HOME: path.join(rustHome, "rustup"),
    PATH: `${path.join(rustHome, "cargo/bin")}:${process.env.PATH}`,
  }),
};

const tag = `v${version}`;
const stage = path.join(root, "src-tauri/target/release-candidate", version);
const packages = path.join(stage, "packages");
const artifact = (arch, extension) => path.join(packages, `Lazy-Worker-${version}-macos-${arch}.${extension}`);
// The tool that builds the disk image. `uvx` downloads this exact version the first time and keeps it.
const diskImageBuilder = "dmgbuild==1.6.7";
const diskImageSource = path.join(root, "scripts/disk-image");
const diskImage = arch => path.join(packages, `Lazy-Worker-${arch === "arm64" ? "apple-silicon" : "intel"}.dmg`);

// ---------------------------------------------------------------------------------------------- checks

step(dryRun ? `Dry run of ${version}` : `Release ${version}`);

const branch = run("git", ["rev-parse", "--abbrev-ref", "HEAD"], { quiet: true });
const dirty = run("git", ["status", "--porcelain"], { quiet: true });
if (dryRun) {
  if (dirty) console.log("Note: the working tree has changes. A release needs a clean one.");
} else {
  if (branch !== "main") fail(`releases come from main, and this is ${branch}.`);
  if (dirty) fail("the working tree has changes. Commit or stash them, so the release matches a commit.");
  run("git", ["fetch", "--quiet", "origin", "main"], { quiet: true });
  if (run("git", ["rev-list", "--count", "HEAD..origin/main"], { quiet: true }) !== "0") {
    fail("main is behind origin/main. Pull first.");
  }
  if (run("git", ["tag", "--list", tag], { quiet: true })) fail(`the tag ${tag} already exists.`);
}

// The panel shows the changes of each release from this file.
if (!read("src/changelog.ts").includes(`version: "${version}"`)) {
  const missing = `src/changelog.ts has no entry for ${version}`;
  if (dryRun) console.log(`Note: ${missing}. A release needs one.`);
  else fail(`${missing}. Add its changes in English and Spanish first.`);
}

run("gh", ["auth", "status"], { quiet: true });
if (run("uvx", ["--version"], { quiet: true, allowFailure: true }) === undefined) {
  fail("uvx is missing. Install uv (https://docs.astral.sh/uv/), which runs the disk image builder.");
}
const published = JSON.parse(run("gh", ["release", "list", "--repo", repository, "--limit", "50",
  "--json", "tagName,isDraft,isPrerelease"], { quiet: true }))
  .filter(release => !release.isDraft && !release.isPrerelease && /^v\d+\.\d+\.\d+$/.test(release.tagName))
  .map(release => release.tagName.slice(1))
  .filter(candidate => compareVersions(candidate, version) < 0)
  .sort(compareVersions);
const previous = published.at(-1);
if (!previous) fail("found no earlier published release to rehearse the update from.");

if (!run("security", ["find-identity", "-v", "-p", "codesigning"], { quiet: true }).includes(local.signingIdentity)) {
  fail("the signing identity in `.release.local.json` is not in the keychain.");
}
const sysroot = run("rustc", ["--print", "sysroot"], { env: buildEnvironment, quiet: true });
for (const { triple } of targets) {
  if (!fs.existsSync(path.join(sysroot, "lib/rustlib", triple))) {
    fail(`the Rust toolchain has no ${triple} target. Set \`rustHome\` in \`.release.local.json\` or add the target.`);
  }
}

// The key must be the one earlier releases used, or installed copies reject the update.
const keyOutput = run("security", ["find-generic-password", "-s", local.updaterKeyService, "-w"],
  { quiet: true, allowFailure: true });
if (!keyOutput) fail("could not read the updater key from the keychain.");
const updaterKey = JSON.parse(keyOutput);
const configuredKey = JSON.parse(read("src-tauri/tauri.macos.conf.json")).plugins.updater.pubkey;
const previousKey = JSON.parse(run("git", ["show", `v${previous}:src-tauri/tauri.macos.conf.json`], { quiet: true }))
  .plugins.updater.pubkey;
if (updaterKey.publicKey !== configuredKey || configuredKey !== previousKey) {
  fail(`the updater key must match the one release ${previous} used.`);
}
console.log(`Checks passed. The update will be rehearsed from ${previous}.`);

step("Build and test");
run("npm", ["run", "--silent", "build"]);
run("cargo", ["test", "--quiet", "--manifest-path", "src-tauri/Cargo.toml"], { env: buildEnvironment });

// ---------------------------------------------------------------------------------------------- version

if (!dryRun) {
  step(`Set the version to ${version}`);
  for (const [file, text, count] of versionFiles) {
    const content = read(file);
    const [from, to] = [text(current), text(version)];
    // The lock file repeats other packages' versions, so only its first entries belong to this app.
    const scope = file === "package-lock.json" ? content.slice(0, 400) : content;
    if (scope.split(from).length - 1 !== count) fail(`${file} does not hold version ${current} as expected.`);
    fs.writeFileSync(path.join(root, file), scope.split(from).join(to) + content.slice(scope.length));
  }
  run("git", ["add", ...versionFiles.map(([file]) => file)]);
  run("git", ["commit", "--quiet", "-m", `chore: release ${version}`]);
}

// ---------------------------------------------------------------------------------------------- build

fs.rmSync(stage, { recursive: true, force: true });
fs.mkdirSync(packages, { recursive: true });

for (const { arch, triple } of targets) {
  step(`Build ${arch}`);
  run("npm", ["run", "--silent", "tauri", "--", "build", "--target", triple, "--bundles", "app"],
    { env: buildEnvironment, quiet: true });
  const built = path.join(root, "src-tauri/target", triple, "release/bundle/macos/Lazy Worker.app");
  const app = path.join(stage, arch, "Lazy Worker.app");
  fs.mkdirSync(path.dirname(app), { recursive: true });
  run("ditto", [built, app]);

  if (plist(app, "CFBundleShortVersionString") !== version) fail(`the ${arch} app is not version ${version}.`);
  if (plist(app, "CFBundleIdentifier") !== identifier) fail(`the ${arch} app has another identifier.`);
  run("codesign", ["--verify", "--deep", "--strict", app], { quiet: true });
  for (const name of ["lazy-worker", "activity-helper"]) {
    const binary = path.join(app, "Contents/MacOS", name);
    const details = report("codesign", "-dv", "--verbose=4", binary);
    if (run("lipo", ["-archs", binary], { quiet: true }) !== arch) fail(`${name} for ${arch} has another architecture.`);
    if (!details.includes(`Authority=${local.signingIdentity}`)) fail(`${name} for ${arch} lacks the Developer ID signature.`);
    if (!/CodeDirectory.*runtime/.test(details)) fail(`${name} for ${arch} lacks the hardened runtime.`);
    if (!/minos 12\.0/.test(run("xcrun", ["vtool", "-show-build", binary], { quiet: true }))) {
      fail(`${name} for ${arch} does not target macOS 12.0.`);
    }
  }
  console.log(`${arch}: version, identifier, architecture, signature, hardened runtime and macOS 12.0 verified.`);

  step(`Notarize ${arch}`);
  const upload = path.join(stage, `notarize-${arch}.zip`);
  run("ditto", ["-c", "-k", "--keepParent", app, upload]);
  notarize(upload, app, `the ${arch} app`);
  fs.rmSync(upload);
  if (!report("spctl", "-a", "-vv", app).includes("Notarized Developer ID")) {
    fail(`Gatekeeper does not accept the ${arch} app as notarized.`);
  }
  console.log(`${arch}: accepted by Apple, stapled, and accepted by Gatekeeper.`);
}

// ---------------------------------------------------------------------------------------------- package

step("Package");
const manifest = {
  version,
  notes: summary ?? `Dry run of ${version}.`,
  pub_date: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
  platforms: {},
};
for (const { arch, platform } of targets) {
  const app = path.join(stage, arch, "Lazy Worker.app");
  // The ZIP is for people. The tarball holds the stapled app for the updater.
  run("ditto", ["-c", "-k", "--keepParent", app, artifact(arch, "zip")]);
  run("/usr/bin/tar", ["--disable-copyfile", "-czf", artifact(arch, "app.tar.gz"), "-C", path.dirname(app), path.basename(app)]);
  run(path.join(root, "node_modules/.bin/tauri"), ["signer", "sign", "--app-version", version, artifact(arch, "app.tar.gz")], {
    env: { TAURI_SIGNING_PRIVATE_KEY: updaterKey.privateKey, TAURI_SIGNING_PRIVATE_KEY_PASSWORD: updaterKey.password ?? "" },
    quiet: true,
  });
  // The disk image is for people who install by dragging the app to Applications.
  // Its name has no version, so a link to the latest release always finds it.
  // dmgbuild lays out the window: the background with Miga, and each icon on its pad.
  fs.rmSync(diskImage(arch), { force: true });
  run("uvx", ["--from", diskImageBuilder, "dmgbuild", "-s", path.join(diskImageSource, "settings.py"),
    "-D", `app=${app}`, "-D", `background=${path.join(diskImageSource, "background.png")}`, "Lazy Worker", diskImage(arch)],
  { quiet: true });
  // The copy inside the image must still be the signed and stapled app.
  const mounted = run("hdiutil", ["attach", "-nobrowse", "-readonly", diskImage(arch)], { quiet: true })
    .split("\n").at(-1).split("\t").at(-1).trim();
  const copy = path.join(mounted, path.basename(app));
  const intact = run("codesign", ["--verify", "--deep", "--strict", copy], { quiet: true, allowFailure: true }) !== undefined
    && run("xcrun", ["stapler", "validate", "-q", copy], { quiet: true, allowFailure: true }) !== undefined
    && fs.existsSync(path.join(mounted, ".background.tiff"));
  run("hdiutil", ["detach", "-quiet", mounted], { quiet: true });
  if (!intact) fail(`the ${arch} disk image lost the app's signature, its notarization ticket, or its background.`);
  run("codesign", ["--sign", local.signingIdentity, "--timestamp", diskImage(arch)], { quiet: true });
  notarize(diskImage(arch), diskImage(arch), `the ${arch} disk image`);
  if (!report("spctl", "-a", "-t", "open", "--context", "context:primary-signature", "-vv", diskImage(arch))
    .includes("Notarized Developer ID")) {
    fail(`Gatekeeper does not accept the ${arch} disk image as notarized.`);
  }
  manifest.platforms[platform] = {
    signature: fs.readFileSync(artifact(arch, "app.tar.gz.sig"), "utf8").trim(),
    url: `https://github.com/${repository}/releases/download/${tag}/${path.basename(artifact(arch, "app.tar.gz"))}`,
  };
}
fs.writeFileSync(path.join(packages, "latest.json"), JSON.stringify(manifest, null, 2) + "\n");
const checksums = targets.flatMap(({ arch }) => [diskImage(arch), artifact(arch, "zip"), artifact(arch, "app.tar.gz")])
  .map(file => `${sha256(file)}  ${path.basename(file)}`).join("\n") + "\n";
fs.writeFileSync(path.join(packages, "SHA256SUMS.txt"), checksums);
console.log(fs.readdirSync(packages).sort().join("\n"));

// ---------------------------------------------------------------------------------------------- rehearsal

step(`Rehearse the update from ${previous}`);
const rehearsal = path.join(stage, "rehearsal");
fs.mkdirSync(rehearsal, { recursive: true });
run("gh", ["release", "download", `v${previous}`, "--repo", repository, "--pattern", "*.zip", "--dir", rehearsal], { quiet: true });
const verifierTarget = path.join(root, "src-tauri/target/release-verifier");
run("cargo", ["build", "--quiet", "--release", "--manifest-path", "scripts/release-verifier/Cargo.toml"],
  { env: { ...buildEnvironment, CARGO_TARGET_DIR: verifierTarget } });
const verifier = path.join(verifierTarget, "release/lazy-worker-release-verification");

const server = createServer((request, response) => {
  const name = path.basename(request.url ?? "");
  if (name === "latest.json") {
    // The published manifest points at GitHub. The rehearsal serves the same signatures from here.
    const { port } = server.address();
    const platforms = Object.fromEntries(Object.entries(manifest.platforms).map(([platform, entry]) =>
      [platform, { signature: entry.signature, url: `http://127.0.0.1:${port}/${path.basename(entry.url)}` }]));
    response.end(JSON.stringify({ version, platforms }));
  } else if (fs.existsSync(path.join(packages, name))) {
    fs.createReadStream(path.join(packages, name)).pipe(response);
  } else {
    response.writeHead(404).end();
  }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
try {
  const endpoint = `http://127.0.0.1:${server.address().port}/latest.json`;
  for (const { arch, platform } of targets) {
    const install = path.join(rehearsal, arch);
    fs.mkdirSync(install);
    run("ditto", ["-x", "-k", path.join(rehearsal, `Lazy-Worker-${previous}-macos-${arch}.zip`), install]);
    const app = path.join(install, "Lazy Worker.app");
    const binary = path.join(app, "Contents/MacOS/lazy-worker");
    if (plist(app, "CFBundleShortVersionString") !== previous) fail(`the ${arch} baseline is not version ${previous}.`);
    await runAsync(verifier, [path.join(root, "src-tauri/tauri.macos.conf.json"), previous, binary, platform, endpoint, version])
      .catch(error => fail(`the ${arch} update rehearsal failed: ${error.message}`));
    if (plist(app, "CFBundleShortVersionString") !== version) fail(`the ${arch} rehearsal did not install ${version}.`);
    for (const name of ["lazy-worker", "activity-helper"]) {
      const installed = fs.readFileSync(path.join(app, "Contents/MacOS", name));
      if (!installed.equals(fs.readFileSync(path.join(stage, arch, "Lazy Worker.app/Contents/MacOS", name)))) {
        fail(`${name} installed by the ${arch} rehearsal differs from the built one.`);
      }
    }
    run("codesign", ["--verify", "--deep", "--strict", app], { quiet: true });
    run("xcrun", ["stapler", "validate", "-q", app], { quiet: true });
    console.log(`${arch}: the updater installed ${version} over ${previous}, signed, stapled and byte for byte.`);
  }
} finally {
  server.close();
}

// ---------------------------------------------------------------------------------------------- publish

if (dryRun) {
  step("Dry run complete");
  console.log(`Packages are in ${path.relative(root, packages)}. Nothing was committed, tagged or published.`);
  process.exit(0);
}

step("Publish a draft");
const notes = path.join(stage, "release-notes.md");
fs.writeFileSync(notes, `${summary}

If you use an earlier Lazy Worker, choose **Restart to update** when the update appears. To install it for the first time, download **Lazy-Worker-apple-silicon.dmg** or **Lazy-Worker-intel.dmg**, open it, and drag **Lazy Worker** to Applications. The [README](https://github.com/${repository}#install) shows how to tell which Mac you have. Every package is signed with Developer ID and notarized.
`);
run("git", ["tag", "-a", tag, "-m", `Lazy Worker ${version}`]);
run("git", ["push", "--quiet", "origin", "main", tag]);
const url = run("gh", ["release", "create", tag, "--repo", repository, "--verify-tag", "--draft",
  "--title", `Lazy Worker ${version}`, "--notes-file", notes, "--generate-notes",
  ...fs.readdirSync(packages).map(file => path.join(packages, file))], { quiet: true });
console.log(`Draft ready: ${url}\nReview the notes, then publish it. Installed copies see the update once it is published.`);
