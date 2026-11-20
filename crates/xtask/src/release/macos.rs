//! The macOS release: `dist/MarqueePlayer-<version>-macos-<arch>.dmg` (docs/macos.md,
//! "The package"), a disk image holding `MarqueePlayer.app` and a link to /Applications,
//! named by this Mac's architecture (`arm64` on Apple Silicon, `x86_64` on Intel); the
//! binary before stripping and its `.dSYM` kept in `dist/symbols/<version>/macos/`.
//!
//! The bundle:
//!
//! ```text
//! MarqueePlayer.app/Contents/
//!     Info.plist                  written here ([`info_plist`])
//!     MacOS/MarqueePlayer         the dist build, stripped
//!     Resources/MarqueePlayer.icns  from the macos_icon example's PNGs ([`icns`])
//!     Resources/README.md, LICENSE.txt, THIRD-PARTY-NOTICES.txt
//! ```
//!
//! It is signed ad hoc (`codesign -s -`): Apple Silicon runs no unsigned code, and an ad-hoc
//! signature needs no Apple account. It is not notarised, so Gatekeeper asks the user once
//! (docs/macos.md, "First launch"). `MP_CODESIGN_IDENTITY` signs with a real identity
//! instead (see [`sign`]).
//!
//! Compiled everywhere (so it is linted and tested on Windows and Linux too); runs on macOS.
//! Apple's tools are taken from /usr/bin (the Xcode command line tools' shims), not from
//! PATH, where Homebrew's binutils or coreutils could stand in for them.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::linux::{copy_tree, run_ok, set_executable, symlink, text_of};
use super::{Release, copy, create_dir, remove_dir, remove_file, version, write};
use crate::cmd;
use crate::report::{self, Fail};

// `crate::platform::APP_ID` in crates/app, the bundle identifier (a test keeps them equal).
use super::linux::APP_ID;

/// The oldest macOS it runs on: LSMinimumSystemVersion, and MACOSX_DEPLOYMENT_TARGET for the
/// build (rustc and the C compilers of the build scripts), so the binary says the same.
const MIN_MACOS: &str = "11.0";

/// The bundle's folder and executable name.
const BUNDLE: &str = "MarqueePlayer.app";
const EXE: &str = "MarqueePlayer";
const ICON: &str = "MarqueePlayer";

/// `library::scan::AUDIO_EXTS` (xtask does not depend on the app's crates; a test reads
/// crates/library/src/scan.rs and keeps them equal): the files Finder offers to open with it.
const AUDIO_EXTS: [&str; 14] = ["opus", "ogg", "oga", "flac", "mp3", "mp2", "wav", "aif", "aiff", "aifc", "aac", "m4a", "m4b", "mp4"];

/// The icns entries: type code and the PNG's size in pixels (16 to 512 points, at 1x and
/// 2x). The macos_icon example writes `<size>.png` for each size here.
const ICNS_ENTRIES: [(&[u8; 4], u32); 10] =
    [(b"icp4", 16), (b"icp5", 32), (b"ic11", 32), (b"ic12", 64), (b"ic07", 128), (b"ic08", 256), (b"ic13", 256), (b"ic09", 512), (b"ic14", 512), (b"ic10", 1024)];

/// Apple's tools the release runs, in /usr/bin.
const TOOLS: [&str; 9] = ["codesign", "hdiutil", "dsymutil", "strip", "otool", "plutil", "xattr", "ditto", "xcrun"];

pub fn run(r: &mut Release) -> Result<(), Fail> {
    println!("Marquee Player release (macOS)");
    let tool = |t: &str| PathBuf::from("/usr/bin").join(t);
    // ---- What it needs, found now, not after the build ----
    for t in TOOLS {
        if !tool(t).is_file() {
            return Err(Fail::msg(format!("/usr/bin/{t} is needed: install the Xcode command line tools (xcode-select --install).")));
        }
    }
    // The shims need the command line tools themselves, not only the shims.
    if !text_of(Command::new("/usr/bin/xcrun").args(["--find", "dsymutil"])).is_ok_and(|p| !p.trim().is_empty()) {
        return Err(Fail::msg("The Xcode command line tools are not installed (xcrun --find dsymutil failed): xcode-select --install."));
    }
    let arch = arch_name(std::env::consts::ARCH).ok_or_else(|| Fail::msg(format!("releases are made on arm64 and x86_64 Macs, not {}", std::env::consts::ARCH)))?;
    let triple = format!("{}-apple-darwin", std::env::consts::ARCH);
    let identity = std::env::var("MP_CODESIGN_IDENTITY").ok().filter(|v| !v.trim().is_empty());
    println!("architecture: {arch} ({triple})");
    println!("signature:    {}", identity.as_deref().map_or("ad hoc (MP_CODESIGN_IDENTITY is not set)".to_owned(), |i| format!("{i} (MP_CODESIGN_IDENTITY)")));
    println!("target folder: {}", r.target.display());

    // The binary is built for the oldest macOS the bundle says it runs on; the debug info goes
    // into a .dSYM that rustc makes (packed), which the symbols folder keeps.
    r.set_env("MACOSX_DEPLOYMENT_TARGET", MIN_MACOS);
    r.set_env("CARGO_PROFILE_DIST_SPLIT_DEBUGINFO", "packed");
    r.checks()?;
    let ver = r.build(&[], &["macos_icon"])?;
    let built = r.built();

    // What it needs from the system: macOS' own libraries and frameworks only (a Homebrew
    // library linked by accident would be missing on the user's Mac), and the minimum macOS.
    report::heading("what the binary needs");
    let libs = text_of(Command::new(tool("otool")).arg("-L").arg(&built))?;
    let foreign = foreign_libs(&libs);
    println!("    links: {}", linked_libs(&libs).join(" "));
    if !foreign.is_empty() {
        return Err(Fail::msg(format!("The binary links libraries that are not part of macOS: {} (Homebrew's?). Nothing was packaged.", foreign.join(", "))));
    }
    let load = text_of(Command::new(tool("otool")).arg("-l").arg(&built))?;
    match min_os(&load) {
        Some(m) if m == MIN_MACOS => println!("ok: needs macOS {m} or newer"),
        Some(m) => return Err(Fail::msg(format!("The binary needs macOS {m}, not {MIN_MACOS} (LC_BUILD_VERSION). Nothing was packaged."))),
        None => return Err(Fail::msg("otool -l shows no LC_BUILD_VERSION minos for the binary. Nothing was packaged.")),
    }

    // ---- Debug symbols, beside the other systems' of the same version, not shipped ----
    report::heading("debug symbols");
    let sym = r.dist().join("symbols").join(&ver).join("macos");
    let stage = r.dist().join("_stage-macos");
    for d in [&sym, &stage] {
        remove_dir(d)?;
        create_dir(d)?;
    }
    let dsym = sym.join(format!("{EXE}.dSYM"));
    let made = r.profile_dir().join(format!("{EXE}.dSYM"));
    if made.is_dir() {
        copy_tree(&made, &dsym)?;
    } else {
        // Not made by rustc (another split-debuginfo): from the object files the binary names.
        run_ok(Command::new(tool("dsymutil")).arg(&built).arg("-o").arg(&dsym), "dsymutil")?;
    }
    copy(&built, &sym.join(EXE))?;
    // The UUID that ties a crash report to this binary and its dSYM (informative).
    if let Ok(u) = text_of(Command::new("/usr/bin/xcrun").arg("dwarfdump").arg("--uuid").arg(&dsym)) {
        print!("{u}");
    }
    println!("ok: symbols kept: {}", sym.display());

    // ---- The bundle ----
    report::heading("MarqueePlayer.app");
    let app = stage.join(BUNDLE);
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let res = contents.join("Resources");
    for d in [&macos, &res] {
        create_dir(d)?;
    }
    let ship = macos.join(EXE);
    copy(&built, &ship)?;
    // The debug map (the object files' paths, which hold the build's folders) and the local
    // symbols go, as the release profile's `strip = "symbols"` does; the dSYM keeps them.
    run_ok(Command::new(tool("strip")).args(["-S", "-x"]).arg(&ship), "strip")?;
    set_executable(&ship)?;
    r.check_paths(&ship)?;

    let plist = contents.join("Info.plist");
    write(&plist, info_plist(&ver).as_bytes())?;
    run_ok(Command::new(tool("plutil")).arg("-lint").arg(&plist), "plutil refused Info.plist; plutil")?;
    write(&contents.join("PkgInfo"), b"APPL????")?;

    // The icon: the example's PNGs packed here (no iconutil).
    let pngs = stage.join("icon");
    let example = r.profile_dir().join("examples").join("macos_icon");
    let out = cmd::output(Command::new(&example).arg(&pngs))?;
    if !out.status.success() {
        return Err(Fail::msg(format!("macos_icon failed: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    let mut entries = Vec::new();
    for (code, px) in ICNS_ENTRIES {
        let f = pngs.join(format!("{px}.png"));
        let data = std::fs::read(&f).map_err(|e| Fail::msg(format!("could not read {}: {e}", f.display())))?;
        entries.push((*code, data));
    }
    let icns_bytes = icns(&entries).map_err(Fail::msg)?;
    write(&res.join(format!("{ICON}.icns")), &icns_bytes)?;

    let packaging = r.root().join("packaging");
    copy(&packaging.join("user-README.md"), &res.join("README.md"))?;
    copy(&packaging.join("LICENSE.txt"), &res.join("LICENSE.txt"))?;
    super::write_notices(r, &triple, &ver, &[], &res.join("THIRD-PARTY-NOTICES.txt"), false)?;

    // ---- The signature ----
    report::heading("signature");
    // codesign refuses files with extended attributes ("resource fork, Finder information,
    // or similar detritus"); what cannot be removed (com.apple.provenance) it accepts.
    let _ = cmd::status(Command::new(tool("xattr")).arg("-cr").arg(&app));
    sign(&tool("codesign"), &app, identity.as_deref())?;
    run_ok(Command::new(tool("codesign")).args(["--verify", "--deep", "--strict", "--verbose=2"]).arg(&app), "codesign --verify refused the bundle; codesign")?;
    if let Ok(d) = cmd::output(Command::new(tool("codesign")).args(["-d", "--verbose=2"]).arg(&app)) {
        // codesign -d writes to standard error.
        let said = String::from_utf8_lossy(&d.stderr);
        for l in said.lines().filter(|l| l.starts_with("Identifier=") || l.starts_with("Signature=") || l.starts_with("Authority=") || l.starts_with("Format=")) {
            println!("    {l}");
        }
    }
    // It starts, signed, from inside the bundle.
    let got = cmd::output(r.command(&ship).arg("--version"))?;
    let says = format!("{}{}", String::from_utf8_lossy(&got.stdout), String::from_utf8_lossy(&got.stderr)).trim().to_owned();
    if !got.status.success() || says != version::says(&ver) {
        return Err(Fail::msg(format!("The bundle's binary says \"{says}\", not \"{}\".", version::says(&ver))));
    }
    println!("ok: {BUNDLE} signed and starts: {says}");

    // ---- The disk image ----
    report::heading("disk image");
    let image = stage.join("image");
    create_dir(&image)?;
    // ditto keeps the bundle exactly as signed (permissions, links, attributes).
    run_ok(Command::new("/usr/bin/ditto").arg(&app).arg(image.join(BUNDLE)), "ditto")?;
    symlink("/Applications", &image.join("Applications"))?;
    let dmg = r.dist().join(format!("MarqueePlayer-{ver}-macos-{arch}.dmg"));
    remove_file(&dmg)?;
    // hdiutil fails now and then with "Resource busy" on a busy machine (CI runners): tried
    // three times.
    let mut ok = false;
    for attempt in 1..=3 {
        let mut c = Command::new(tool("hdiutil"));
        c.args(["create", "-volname", "Marquee Player", "-fs", "HFS+", "-format", "UDZO", "-ov", "-srcfolder"]).arg(&image).arg(&dmg);
        if cmd::status(&mut c)? == 0 {
            ok = true;
            break;
        }
        println!("hdiutil create failed (attempt {attempt} of 3)");
        remove_file(&dmg)?;
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    if !ok {
        return Err(Fail::msg("hdiutil could not make the disk image (above). The signed bundle is in dist/_stage-macos."));
    }
    run_ok(Command::new(tool("hdiutil")).arg("verify").arg(&dmg), "hdiutil verify")?;
    if let Some(id) = identity.as_deref() {
        // MP_CODESIGN_IDENTITY: the image is signed too, as notarisation wants.
        run_ok(Command::new(tool("codesign")).args(["--force", "--timestamp", "--sign", id]).arg(&dmg), "codesign (the disk image)")?;
    }
    println!("ok: disk image: {}", dmg.display());
    remove_dir(&stage)?;
    r.recheck(&ver)?;

    let mut notes = vec![format!("Needs      macOS {MIN_MACOS} or newer, {arch}")];
    if identity.is_none() {
        notes.push("Signed     ad hoc, not notarised: Gatekeeper asks once at first launch (docs/macos.md, \"First launch\")".to_owned());
    }
    r.summary(&ver, &[("Disk image", dmg), ("Symbols", sym)], &notes);
    Ok(())
}

/// Signs the bundle. Without an identity: ad hoc (`-s -`), which Apple Silicon requires of
/// any code and which needs no Apple account; it proves nothing about who made it.
///
/// ==== A REAL IDENTITY ====
/// With `MP_CODESIGN_IDENTITY` set to a "Developer ID Application: ..." identity in the
/// keychain, the bundle is signed with it, with the hardened runtime and a secure timestamp,
/// as notarisation requires. Notarising (`xcrun notarytool submit --wait`, then
/// `xcrun stapler staple` on the .dmg) is not done here: it needs an Apple developer account's
/// credentials, and would come after the disk image is signed.
fn sign(codesign: &Path, app: &Path, identity: Option<&str>) -> Result<(), Fail> {
    let mut c = Command::new(codesign);
    c.args(["--force", "--deep"]);
    match identity {
        Some(id) => c.args(["--options", "runtime", "--timestamp", "--sign", id]),
        None => c.args(["--sign", "-"]),
    };
    c.arg(app);
    run_ok(&mut c, "codesign")
}

/// What the package names the architecture: Apple's names (`uname -m`).
fn arch_name(rust_arch: &str) -> Option<&'static str> {
    match rust_arch {
        "aarch64" => Some("arm64"),
        "x86_64" => Some("x86_64"),
        _ => None,
    }
}

/// The bundle's Info.plist for release `ver`.
pub fn info_plist(ver: &str) -> String {
    let year = ver.split('.').next().unwrap_or_default();
    let exts: String = AUDIO_EXTS.iter().map(|e| format!("\t\t\t\t<string>{e}</string>\n")).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>Marquee Player</string>
	<key>CFBundleName</key>
	<string>Marquee Player</string>
	<key>CFBundleExecutable</key>
	<string>{EXE}</string>
	<key>CFBundleIconFile</key>
	<string>{ICON}</string>
	<key>CFBundleIdentifier</key>
	<string>{APP_ID}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleSignature</key>
	<string>????</string>
	<key>CFBundleShortVersionString</key>
	<string>{ver}</string>
	<key>CFBundleVersion</key>
	<string>{ver}</string>
	<key>LSMinimumSystemVersion</key>
	<string>{MIN_MACOS}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.music</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSSupportsAutomaticGraphicsSwitching</key>
	<true/>
	<key>NSPrincipalClass</key>
	<string>NSApplication</string>
	<key>NSHumanReadableCopyright</key>
	<string>Copyright © {year} Offbeat</string>
	<key>CFBundleDocumentTypes</key>
	<array>
		<dict>
			<key>CFBundleTypeName</key>
			<string>Audio file</string>
			<key>CFBundleTypeRole</key>
			<string>Viewer</string>
			<key>LSHandlerRank</key>
			<string>Alternate</string>
			<key>CFBundleTypeExtensions</key>
			<array>
{exts}			</array>
		</dict>
	</array>
</dict>
</plist>
"#
    )
}

/// The `.icns` container: the magic, the total length, then each entry as its type code,
/// its length (with the 8 header bytes) and its data, all big-endian. Every entry here is a
/// PNG, whose pixel size must be the one its type code stands for.
pub fn icns(entries: &[([u8; 4], Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    for (code, data) in entries {
        let want = ICNS_ENTRIES.iter().find(|(c, _)| *c == code).map(|(_, px)| *px).ok_or_else(|| format!("{}: not an icns type this writes", String::from_utf8_lossy(code)))?;
        match png_size(data) {
            Some((w, h)) if w == want && h == want => {}
            Some((w, h)) => return Err(format!("{}: the PNG is {w}x{h}, not {want}x{want}", String::from_utf8_lossy(code))),
            None => return Err(format!("{}: not a PNG", String::from_utf8_lossy(code))),
        }
        body.extend_from_slice(code);
        body.extend_from_slice(&u32::try_from(data.len() + 8).map_err(|_| "an icon entry over 4 GB")?.to_be_bytes());
        body.extend_from_slice(data);
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&u32::try_from(body.len() + 8).map_err(|_| "an icon file over 4 GB")?.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// A PNG's width and height, from its IHDR chunk.
fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" || &data[12..16] != b"IHDR" {
        return None;
    }
    let n = |i: usize| u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
    Some((n(16), n(20)))
}

/// The libraries in `otool -L`'s listing (the first line names the file itself).
fn linked_libs(listing: &str) -> Vec<String> {
    listing.lines().skip(1).filter_map(|l| l.split_whitespace().next()).map(str::to_owned).collect()
}

/// The linked libraries that are not part of macOS (/usr/lib, /System/Library).
fn foreign_libs(listing: &str) -> Vec<String> {
    linked_libs(listing).into_iter().filter(|l| !(l.starts_with("/usr/lib/") || l.starts_with("/System/Library/"))).collect()
}

/// The minimum macOS in `otool -l`'s load commands: LC_BUILD_VERSION's `minos`, or the older
/// LC_VERSION_MIN_MACOSX's `version`.
fn min_os(load: &str) -> Option<String> {
    let mut section = "";
    for l in load.lines().map(str::trim) {
        if let Some(c) = l.strip_prefix("cmd ") {
            section = if c == "LC_BUILD_VERSION" || c == "LC_VERSION_MIN_MACOSX" { c } else { "" };
            continue;
        }
        let key = if section == "LC_BUILD_VERSION" { "minos " } else if section == "LC_VERSION_MIN_MACOSX" { "version " } else { continue };
        if let Some(v) = l.strip_prefix(key) {
            return Some(v.trim().to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut p = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        p.extend_from_slice(&w.to_be_bytes());
        p.extend_from_slice(&h.to_be_bytes());
        p.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        p
    }

    #[test]
    fn icns_container() {
        let a = png(16, 16);
        let b = png(1024, 1024);
        let out = icns(&[(*b"icp4", a.clone()), (*b"ic10", b.clone())]).unwrap();
        assert_eq!(&out[..4], b"icns");
        assert_eq!(u32::from_be_bytes(out[4..8].try_into().unwrap()) as usize, out.len());
        assert_eq!(&out[8..12], b"icp4");
        assert_eq!(u32::from_be_bytes(out[12..16].try_into().unwrap()) as usize, a.len() + 8);
        assert_eq!(&out[16..16 + a.len()], &a[..]);
        let next = 16 + a.len();
        assert_eq!(&out[next..next + 4], b"ic10");
        assert_eq!(out.len(), 8 + 2 * 8 + a.len() + b.len());
        // A PNG of the wrong size, or not a PNG, or an unknown type, is refused.
        assert!(icns(&[(*b"ic07", png(64, 64))]).unwrap_err().contains("64x64"));
        assert!(icns(&[(*b"ic07", vec![0; 40])]).is_err());
        assert!(icns(&[(*b"ICN#", png(32, 32))]).is_err());
        // Every size the example writes is one an entry takes.
        for (_, px) in ICNS_ENTRIES {
            assert!([16, 32, 64, 128, 256, 512, 1024].contains(&px));
        }
    }

    /// The bundle names the app as the code does, and offers what the scanner takes.
    #[test]
    fn info_plist_matches_the_app() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
        assert!(read("crates/app/src/platform/mod.rs").contains(&format!("pub const APP_ID: &str = \"{APP_ID}\";")));
        let scan = read("crates/library/src/scan.rs");
        let line = scan.lines().find(|l| l.starts_with("pub const AUDIO_EXTS")).expect("AUDIO_EXTS in scan.rs");
        let list = &line[line.find("= &[").unwrap() + 4..line.rfind(']').unwrap()];
        let exts: Vec<&str> = list.split(',').map(|s| s.trim().trim_matches('"')).filter(|s| !s.is_empty()).collect();
        assert_eq!(exts, AUDIO_EXTS, "AUDIO_EXTS here and in crates/library/src/scan.rs differ");
        let ex = read("crates/app/examples/macos_icon.rs");
        assert!(ex.contains("[16, 32, 64, 128, 256, 512, 1024]"), "the example's sizes changed");

        let p = info_plist("2026.11.19.3");
        for want in [
            "<key>CFBundleIdentifier</key>\n\t<string>app.marqueeplayer.MarqueePlayer</string>",
            "<key>CFBundleName</key>\n\t<string>Marquee Player</string>",
            "<key>CFBundleExecutable</key>\n\t<string>MarqueePlayer</string>",
            "<key>CFBundleShortVersionString</key>\n\t<string>2026.11.19.3</string>",
            "<key>CFBundleVersion</key>\n\t<string>2026.11.19.3</string>",
            "<key>LSMinimumSystemVersion</key>\n\t<string>11.0</string>",
            "<key>NSHighResolutionCapable</key>\n\t<true/>",
            "<string>public.app-category.music</string>",
            "<string>Copyright © 2026 Offbeat</string>",
            "<key>CFBundleTypeRole</key>\n\t\t\t<string>Viewer</string>",
            "\t\t\t\t<string>flac</string>\n",
            "\t\t\t\t<string>mp4</string>\n\t\t\t</array>",
        ] {
            assert!(p.contains(want), "{want:?} not in\n{p}");
        }
        assert!(!p.contains("LSUIElement"), "it has a Dock icon");
        // Well formed as far as tags go: every <dict>, <array>, <key> and <string> closed.
        for tag in ["dict", "array", "key", "string"] {
            assert_eq!(p.matches(&format!("<{tag}>")).count(), p.matches(&format!("</{tag}>")).count(), "{tag}");
        }
        assert!(!p.contains('&') && p.ends_with("</plist>\n"));
    }

    #[test]
    fn reads_otool() {
        let l = "/x/MarqueePlayer:\n\t/System/Library/Frameworks/CoreAudio.framework/Versions/A/CoreAudio (compatibility version 1.0.0, current version 1.0.0)\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1345.0.0)\n\t/opt/homebrew/opt/opus/lib/libopus.0.dylib (compatibility version 10.0.0)\n";
        assert_eq!(linked_libs(l).len(), 3);
        assert_eq!(foreign_libs(l), ["/opt/homebrew/opt/opus/lib/libopus.0.dylib"]);
        let load = "Load command 9\n      cmd LC_BUILD_VERSION\n  cmdsize 32\n platform 1\n    minos 11.0\n      sdk 15.2\nLoad command 10\n      cmd LC_SOURCE_VERSION\n";
        assert_eq!(min_os(load).as_deref(), Some("11.0"));
        let old = "      cmd LC_VERSION_MIN_MACOSX\n  cmdsize 16\n  version 10.12\n      sdk 10.15\n";
        assert_eq!(min_os(old).as_deref(), Some("10.12"));
        assert_eq!(min_os("      cmd LC_SEGMENT_64\n  version 3\n"), None);
        assert_eq!(arch_name("aarch64"), Some("arm64"));
        assert_eq!(arch_name("x86_64"), Some("x86_64"));
        assert_eq!(arch_name("arm"), None);
    }
}
