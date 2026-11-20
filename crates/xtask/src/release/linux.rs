//! The Linux release: `dist/MarqueePlayer-<version>-x86_64.AppImage` (docs/releases.md, "Linux AppImage":
//! what it carries and what it takes from the system), its debug symbols split off into
//! `dist/symbols/<version>/linux/`. The glibc it needs is that of the system it is built on:
//! build on the oldest one it should run on (Ubuntu 22.04: glibc 2.35).
//!
//! Compiled everywhere (so it is linted and tested on Windows too); runs on Linux.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Release, copy, create_dir, notices, remove_dir, remove_file, version, which, write};
use crate::cmd;
use crate::report::{self, Fail};

pub(super) const APP_ID: &str = "app.marqueeplayer.MarqueePlayer";

/// Pinned tools: appimagetool 1.9.1 and the static type-2 runtime of 2025-11-08 (no libfuse
/// needed on the user's system; FUSE itself, or --appimage-extract-and-run). URL, SHA-256,
/// file name.
const APPIMAGETOOL: (&str, &str, &str) = (
    "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage",
    "ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0",
    "appimagetool-1.9.1-x86_64.AppImage",
);
const RUNTIME: (&str, &str, &str) = (
    "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64",
    "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d",
    "runtime-20251108-x86_64",
);

/// The libraries the AppImage carries (xkbcommon, for keyboard layouts on Wayland and X11, and
/// what its X11 half needs beside libxcb), with the packages they come from.
const BUNDLED_LIBS: [(&str, &str); 3] = [("libxkbcommon.so.0", "libxkbcommon0"), ("libxkbcommon-x11.so.0", "libxkbcommon-x11-0"), ("libxcb-xkb.so.1", "libxcb-xkb1")];

/// The fallback font it carries.
const FONT_DIR: &str = "/usr/share/fonts/truetype/dejavu";
const FONTS: [&str; 2] = ["DejaVuSans.ttf", "DejaVuSans-Bold.ttf"];

/// The libraries the binary may link: the system's (docs/releases.md, "Linux AppImage"). Anything else
/// stops the release until it is decided whether to bundle it.
const SYSTEM_LIBS: [&str; 12] = [
    "libc.so.6",
    "libm.so.6",
    "libdl.so.2",
    "libpthread.so.0",
    "librt.so.1",
    "ld-linux-x86-64.so.2",
    "libgcc_s.so.1",
    "libstdc++.so.6",
    "libpipewire-0.3.so.0",
    "libasound.so.2",
    "libfontconfig.so.1",
    "libfreetype.so.6",
];

pub fn run(r: &mut Release) -> Result<(), Fail> {
    println!("Marquee Player release (Linux)");

    // ---- What it needs, found now, not after the build ----
    for t in ["pkg-config", "desktop-file-validate", "appstreamcli", "patchelf", "objcopy", "objdump", "curl", "sha256sum", "date"] {
        if which(t).is_none() {
            return Err(Fail::msg(format!("{t} is needed (docs/releases.md, \"Linux AppImage\").")));
        }
    }
    let home = super::home().ok_or_else(|| Fail::msg("HOME is not set"))?;
    let env_dir = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mp_build = home.join(".local").join("share").join("mp-build");
    let tools = env_dir("MP_APPIMAGE_TOOLS").unwrap_or_else(|| mp_build.join("appimage-tools"));
    let pw_prefix = env_dir("MP_PIPEWIRE_PREFIX").unwrap_or_else(|| mp_build.join("pipewire-0.3.65"));
    let pw_lib = pw_prefix.join("lib").join("x86_64-linux-gnu");
    let built_pw = pw_prefix.is_dir();
    if built_pw {
        let pc = [pw_lib.join("pkgconfig"), pw_prefix.join("lib").join("pkgconfig")];
        r.set_env("PKG_CONFIG_PATH", join_paths(&pc, "PKG_CONFIG_PATH")?);
    }
    if !cmd::status(r.command("pkg-config").args(["--atleast-version=0.3.65", "libpipewire-0.3"]))?.eq(&0) {
        let found = cmd::output(r.command("pkg-config").args(["--modversion", "libpipewire-0.3"])).ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
        return Err(Fail::msg(format!(
            "libpipewire-0.3 0.3.65 or newer is needed to build against (found: {}). On Ubuntu 22.04: packaging/linux/pipewire-sdk.sh",
            found.as_deref().unwrap_or("none")
        )));
    }
    // Tests and the checks below run the binary: with a PipeWire built for building only, its
    // library stands in for the system's old one (never in the AppImage).
    if built_pw {
        r.set_env("LD_LIBRARY_PATH", join_paths(&[pw_lib], "LD_LIBRARY_PATH")?);
    }
    for f in FONTS {
        if !Path::new(FONT_DIR).join(f).is_file() {
            return Err(Fail::msg(format!("{FONT_DIR}/{f} is needed (fonts-dejavu-core): the AppImage's fallback font.")));
        }
    }
    let ldconfig = which("ldconfig").unwrap_or_else(|| PathBuf::from("/sbin/ldconfig"));
    let listing = cmd::output(Command::new(&ldconfig).arg("-p")).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let mut libs = Vec::new();
    for (l, package) in BUNDLED_LIBS {
        let path = find_lib(&listing, l).ok_or_else(|| Fail::msg(format!("{l} is needed ({package}): the AppImage carries it.")))?;
        libs.push((l, package, path));
    }
    let appimagetool = fetch(&tools, APPIMAGETOOL)?;
    let runtime = fetch(&tools, RUNTIME)?;
    let pkg = |args: &[&str]| cmd::output(r.command("pkg-config").args(args)).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
    println!("appimagetool: {}", appimagetool.display());
    println!("PipeWire to build against: {} ({})", pkg(&["--modversion", "libpipewire-0.3"]), pkg(&["--variable=libdir", "libpipewire-0.3"]));
    println!("target folder: {}", r.target.display());

    // The tag editor's tests write fixtures only inside the temporary folder: it must hold the
    // target folder (docs/linux.md, "Building").
    let tmp = r.target.join("tmp");
    create_dir(&tmp)?;
    r.set_env("TMPDIR", &tmp);
    r.checks()?;

    // ---- The release binary. It finds the libraries the AppImage carries through its RUNPATH
    // ($ORIGIN/../lib), not through LD_LIBRARY_PATH, which the programs it starts would inherit.
    let ver = r.build(&["-C", "link-arg=-Wl,-rpath,$ORIGIN/../lib"], &[])?;
    let built = r.built();

    // What it needs from the system: the newest glibc and libstdc++ symbol versions it uses,
    // and the libraries it links (dlopen'ed ones, Wayland, X11, GL, are found at run time).
    let dynsyms = text_of(Command::new("objdump").arg("-T").arg(&built))?;
    let headers = text_of(Command::new("objdump").arg("-p").arg(&built))?;
    let glibc = newest_version(&dynsyms, "GLIBC_").unwrap_or_default();
    let glibcxx = newest_version(&dynsyms, "GLIBCXX_").unwrap_or_default();
    let needed = needed_libs(&headers);
    println!("    needs glibc {glibc}, libstdc++ (GLIBCXX) {glibcxx}; links: {}", needed.join(" "));
    // The PipeWire functions it calls: all of them in libpipewire 0.3.50 (docs/releases.md, "Linux AppImage").
    println!("    from libpipewire: {}", pipewire_functions(&dynsyms).join(" "));
    if let Some(n) = needed.iter().find(|n| !SYSTEM_LIBS.contains(&n.as_str())) {
        return Err(Fail::msg(format!("The binary links {n}, which the release does not know whether to bundle (docs/releases.md, \"Linux AppImage\").")));
    }

    // ---- Debug symbols, kept beside the Windows release's of the same version, not shipped ----
    report::heading("debug symbols");
    let sym = r.dist().join("symbols").join(&ver).join("linux");
    let stage = r.dist().join("_stage-linux");
    for d in [&sym, &stage] {
        remove_dir(d)?;
        create_dir(d)?;
    }
    let debug = sym.join("MarqueePlayer.debug");
    let ship = stage.join("MarqueePlayer");
    run_ok(Command::new("objcopy").arg("--only-keep-debug").arg(&built).arg(&debug), "objcopy")?;
    run_ok(Command::new("objcopy").args(["--strip-debug", "--strip-unneeded"]).arg(format!("--add-gnu-debuglink={}", debug.display())).arg(&built).arg(&ship), "objcopy")?;
    copy(&ship, &sym.join("MarqueePlayer"))?;
    println!("ok: symbols kept: {}", sym.display());
    // What ships: the debug file, never shipped, keeps the build's folders.
    r.check_paths(&ship)?;

    // ---- The AppDir ----
    report::heading("AppImage");
    let app = stage.join("AppDir");
    let share = app.join("usr").join("share");
    let doc = share.join("doc").join("marquee-player");
    let lib = app.join("usr").join("lib");
    for d in [app.join("usr").join("bin"), lib.clone(), share.join("applications"), share.join("metainfo"), share.join("marquee-player"), share.join("fonts").join("marquee-player"), doc.clone()] {
        create_dir(&d)?;
    }
    let linux = r.root().join("packaging").join("linux");
    install(&ship, &app.join("usr").join("bin").join("MarqueePlayer"))?;
    install(&linux.join("AppRun"), &app.join("AppRun"))?;
    let desktop = format!("{APP_ID}.desktop");
    copy(&linux.join(&desktop), &share.join("applications").join(&desktop))?;
    copy(&linux.join(&desktop), &app.join(&desktop))?;
    copy_tree(&linux.join("icons").join("hicolor"), &share.join("icons").join("hicolor"))?;
    let icon = format!("{APP_ID}.png");
    copy(&linux.join("icons").join("hicolor").join("256x256").join("apps").join(&icon), &app.join(&icon))?;
    symlink(&icon, &app.join(".DirIcon"))?;
    // The release in the metainfo is this one.
    let metainfo = format!("{APP_ID}.metainfo.xml");
    let xml = std::fs::read_to_string(linux.join(&metainfo)).map_err(|e| Fail::msg(format!("could not read {metainfo}: {e}")))?;
    let today = text_of(Command::new("date").arg("+%F"))?.trim().to_owned();
    let xml = set_release(&xml, &ver, &today).ok_or_else(|| Fail::msg("Could not set the release in the metainfo."))?;
    let metainfo_out = share.join("metainfo").join(&metainfo);
    write(&metainfo_out, xml.as_bytes())?;
    for (l, _, path) in &libs {
        copy(path, &lib.join(l))?;
        run_ok(Command::new("patchelf").args(["--set-rpath", "$ORIGIN"]).arg(lib.join(l)), "patchelf")?;
    }
    // The fallback font and fontconfig's file for it.
    copy(&linux.join("fonts.conf"), &share.join("marquee-player").join("fonts.conf"))?;
    for f in FONTS {
        copy(&Path::new(FONT_DIR).join(f), &share.join("fonts").join("marquee-player").join(f))?;
    }
    copy(&r.root().join("packaging").join("user-README.md"), &doc.join("README.md"))?;
    copy(&r.root().join("packaging").join("LICENSE.txt"), &doc.join("LICENSE.txt"))?;
    let copyright = |package: &str| notices::Body::File(PathBuf::from(format!("/usr/share/doc/{package}/copyright")));
    let extra = |title: &str, licence: &str, source: &str, package: &str| notices::Extra { title: title.into(), licence: licence.into(), source: source.into(), body: copyright(package) };
    let extras = [
        extra("libxkbcommon (usr/lib in the AppImage)", "MIT", "https://xkbcommon.org/", "libxkbcommon0"),
        extra("libxkbcommon-x11 (usr/lib in the AppImage)", "MIT", "https://xkbcommon.org/", "libxkbcommon-x11-0"),
        extra("libxcb-xkb (usr/lib in the AppImage)", "MIT/X11", "https://xcb.freedesktop.org/", "libxcb-xkb1"),
        extra("DejaVu fonts (usr/share/fonts in the AppImage)", "Bitstream Vera Fonts licence, public domain changes", "https://dejavu-fonts.github.io/", "fonts-dejavu-core"),
    ];
    super::write_notices(r, "x86_64-unknown-linux-gnu", &ver, &extras, &doc.join("THIRD-PARTY-NOTICES.txt"), false)?;

    // An older validator (desktop-file-utils 0.26, Ubuntu 22.04) does not know the keys of the
    // Desktop Entry specification 1.5 (SingleMainWindow, PrefersNonDefaultGPU): those are let
    // through; anything else it calls an error stops the release.
    let dfv = cmd::output(Command::new("desktop-file-validate").arg(app.join(&desktop)))?;
    let said = format!("{}{}", String::from_utf8_lossy(&dfv.stdout), String::from_utf8_lossy(&dfv.stderr));
    if !said.trim().is_empty() {
        println!("{}", said.trim_end());
    }
    if desktop_errors(&said) {
        return Err(Fail::msg("desktop-file-validate refused the desktop file."));
    }
    run_ok(Command::new("appstreamcli").args(["validate", "--no-net", "--explain"]).arg(&metainfo_out), "appstreamcli refused the metainfo; appstreamcli")?;

    let out = r.dist().join(format!("MarqueePlayer-{ver}-x86_64.AppImage"));
    remove_file(&out)?;
    // appimagetool is itself an AppImage: extracted and run, so it needs no FUSE here (WSL).
    let log_path = stage.join("appimagetool.log");
    let log = std::fs::File::create(&log_path).map_err(|e| Fail::msg(format!("could not create {}: {e}", log_path.display())))?;
    let log2 = log.try_clone().map_err(|e| Fail::msg(format!("{}: {e}", log_path.display())))?;
    let mut c = r.command(&appimagetool);
    c.env("ARCH", "x86_64").env("APPIMAGE_EXTRACT_AND_RUN", "1").arg("--no-appstream").arg("--runtime-file").arg(&runtime).arg(&app).arg(&out).stdout(log).stderr(log2);
    if cmd::status(&mut c)? != 0 {
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        println!("{}", lines[lines.len().saturating_sub(20)..].join("\n"));
        return Err(Fail::msg("appimagetool failed (above)."));
    }
    set_executable(&out)?;
    // It opens: its own launcher, without FUSE.
    let got = cmd::output(r.command(&out).args(["--appimage-extract-and-run", "--version"]))?;
    let says = format!("{}{}", String::from_utf8_lossy(&got.stdout), String::from_utf8_lossy(&got.stderr)).trim().to_owned();
    if !got.status.success() {
        return Err(Fail::msg(format!("The AppImage does not start: {says}")));
    }
    if says != version::says(&ver) {
        return Err(Fail::msg(format!("The AppImage says \"{says}\", not \"{}\".", version::says(&ver))));
    }
    println!("ok: AppImage: {}", out.display());
    remove_dir(&stage)?;
    r.recheck(&ver)?;

    r.summary(
        &ver,
        &[("AppImage", out), ("Symbols", sym)],
        &[format!("Needs      glibc {glibc} or newer, libstdc++ with GLIBCXX_{glibcxx}, libpipewire-0.3 0.3.50 or newer")],
    );
    Ok(())
}

/// The user name, from `id -un`.
pub fn user_name() -> Option<String> {
    let out = cmd::output(Command::new("id").arg("-un")).ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// `dirs`, then what the variable `existing` has, as a search path.
fn join_paths(dirs: &[PathBuf], existing: &str) -> Result<std::ffi::OsString, Fail> {
    let mut all: Vec<PathBuf> = dirs.to_vec();
    if let Some(e) = std::env::var_os(existing).filter(|v| !v.is_empty()) {
        all.extend(std::env::split_paths(&e));
    }
    std::env::join_paths(all).map_err(|e| Fail::msg(format!("{existing}: {e}")))
}

/// What a program printed, when it succeeded.
pub(super) fn text_of(c: &mut Command) -> Result<String, Fail> {
    let out = cmd::output(c)?;
    if !out.status.success() {
        return Err(Fail::msg(format!("{} failed: {}", cmd::describe(c), String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub(super) fn run_ok(c: &mut Command, what: &str) -> Result<(), Fail> {
    match cmd::status(c)? {
        0 => Ok(()),
        code => Err(Fail::msg(format!("{what} failed (exit {code}): {}", cmd::describe(c)))),
    }
}

/// A pinned download in `tools`, fetched (curl) unless it is there with the right SHA-256.
fn fetch(tools: &Path, (url, sha, name): (&str, &str, &str)) -> Result<PathBuf, Fail> {
    let file = tools.join(name);
    if file.is_file() && sha256(&file)? == sha {
        return Ok(file);
    }
    create_dir(tools)?;
    let part = tools.join(format!("{name}.part"));
    if cmd::status(Command::new("curl").args(["-sSfL", "-o"]).arg(&part).arg(url))? != 0 {
        return Err(Fail::msg(format!("Could not download {url}")));
    }
    if sha256(&part)? != sha {
        return Err(Fail::msg(format!("{url} is not the expected file (SHA-256).")));
    }
    std::fs::rename(&part, &file).map_err(|e| Fail::msg(format!("could not move {} to {}: {e}", part.display(), file.display())))?;
    set_executable(&file)?;
    Ok(file)
}

fn sha256(file: &Path) -> Result<String, Fail> {
    let out = text_of(Command::new("sha256sum").arg(file))?;
    Ok(out.split_whitespace().next().unwrap_or_default().to_ascii_lowercase())
}

/// Copies a file and makes it executable (`install -m 0755`).
fn install(from: &Path, to: &Path) -> Result<(), Fail> {
    copy(from, to)?;
    set_executable(to)
}

pub(super) fn set_executable(file: &Path) -> Result<(), Fail> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).map_err(|e| Fail::msg(format!("could not make {} executable: {e}", file.display())))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

pub(super) fn symlink(target: &str, link: &Path) -> Result<(), Fail> {
    remove_file(link)?;
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(target, link);
    #[cfg(not(unix))]
    let made: std::io::Result<()> = Err(std::io::Error::new(std::io::ErrorKind::Unsupported, format!("no symbolic links here ({target})")));
    made.map_err(|e| Fail::msg(format!("could not link {}: {e}", link.display())))
}

pub(super) fn copy_tree(from: &Path, to: &Path) -> Result<(), Fail> {
    create_dir(to)?;
    for e in std::fs::read_dir(from).map_err(|e| Fail::msg(format!("could not read {}: {e}", from.display())))?.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            copy_tree(&p, &to.join(e.file_name()))?;
        } else {
            copy(&p, &to.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// The x86-64 library `name` in `ldconfig -p`'s listing
/// (`\tlibx.so.0 (libc6,x86-64) => /lib/x86_64-linux-gnu/libx.so.0`).
fn find_lib(listing: &str, name: &str) -> Option<PathBuf> {
    listing.lines().find_map(|l| {
        let mut words = l.split_whitespace();
        (words.next() == Some(name) && l.contains("x86-64")).then(|| l.split_whitespace().last().map(PathBuf::from)).flatten()
    })
}

/// The newest `GLIBC_2.35`-style symbol version in `objdump -T`'s listing.
fn newest_version(dynsyms: &str, prefix: &str) -> Option<String> {
    let mut found: Vec<(Vec<u32>, String)> = Vec::new();
    for word in dynsyms.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        if let Some(v) = word.strip_prefix(prefix) {
            let v: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            if let Some(n) = v.split('.').map(|p| p.parse().ok()).collect::<Option<Vec<u32>>>() {
                found.push((n, v));
            }
        }
    }
    found.into_iter().max().map(|(_, v)| v)
}

/// The NEEDED entries of `objdump -p`.
fn needed_libs(headers: &str) -> Vec<String> {
    headers.lines().filter_map(|l| {
        let mut w = l.split_whitespace();
        (w.next() == Some("NEEDED")).then(|| w.next().map(str::to_owned)).flatten()
    }).collect()
}

/// The undefined `pw_*` functions in `objdump -T`'s listing, sorted, once each.
fn pipewire_functions(dynsyms: &str) -> Vec<String> {
    let mut f: Vec<String> = dynsyms.lines().filter(|l| l.contains("*UND*")).filter_map(|l| l.split_whitespace().last()).filter(|n| n.starts_with("pw_")).map(str::to_owned).collect();
    f.sort();
    f.dedup();
    f
}

/// The metainfo with its `<release .../>` made this one.
fn set_release(xml: &str, ver: &str, date: &str) -> Option<String> {
    let start = xml.find("<release version=\"")?;
    let end = start + xml[start..].find("/>")? + 2;
    Some(format!("{}<release version=\"{ver}\" date=\"{date}\" type=\"stable\"/>{}", &xml[..start], &xml[end..]))
}

/// Whether desktop-file-validate's output has an error other than the two keys an older
/// validator does not know.
fn desktop_errors(output: &str) -> bool {
    output.lines().filter(|l| !(l.contains("key \"SingleMainWindow\" in group \"Desktop Entry\", but keys extending") || l.contains("key \"PrefersNonDefaultGPU\" in group \"Desktop Entry\", but keys extending"))).any(|l| l.contains("error:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ldconfig_and_objdump() {
        let listing = "\tlibxkbcommon.so.0 (libc6) => /lib/i386-linux-gnu/libxkbcommon.so.0\n\tlibxkbcommon.so.0 (libc6,x86-64) => /lib/x86_64-linux-gnu/libxkbcommon.so.0\n\tlibxkbcommon-x11.so.0 (libc6,x86-64) => /lib/x86_64-linux-gnu/libxkbcommon-x11.so.0\n";
        assert_eq!(find_lib(listing, "libxkbcommon.so.0"), Some(PathBuf::from("/lib/x86_64-linux-gnu/libxkbcommon.so.0")));
        assert_eq!(find_lib(listing, "libxcb-xkb.so.1"), None);

        let dynsyms = "0000 DF *UND*\t0000 (GLIBC_2.2.5) memcpy\n0000 DF *UND*\t0000 (GLIBC_2.34) pthread_create\n0000 DF *UND*\t0000 (GLIBC_2.9) pipe2\n0000 DF *UND*\t0000 (GLIBCXX_3.4.29) _ZSt\n0000 DF *UND*\t0000 Base pw_init\n0000 DF *UND*\t0000 Base pw_init\n0000 DF *UND*\t0000 Base pw_stream_new\n0000 DF .text\t0000 Base pw_not_imported\n";
        assert_eq!(newest_version(dynsyms, "GLIBC_").as_deref(), Some("2.34"));
        assert_eq!(newest_version(dynsyms, "GLIBCXX_").as_deref(), Some("3.4.29"));
        assert_eq!(pipewire_functions(dynsyms), ["pw_init", "pw_stream_new"]);
        assert_eq!(needed_libs("Dynamic Section:\n  NEEDED               libc.so.6\n  NEEDED               libpipewire-0.3.so.0\n  RUNPATH              $ORIGIN/../lib\n"), ["libc.so.6", "libpipewire-0.3.so.0"]);
    }

    #[test]
    fn metainfo_release() {
        let xml = "<releases>\n    <release version=\"1.0\" date=\"2026-11-16\" type=\"development\"/>\n  </releases>";
        assert_eq!(set_release(xml, "2026.11.19.1", "2026-11-19").as_deref(), Some("<releases>\n    <release version=\"2026.11.19.1\" date=\"2026-11-19\" type=\"stable\"/>\n  </releases>"));
        assert_eq!(set_release("<releases/>", "1", "d"), None);
    }

    #[test]
    fn desktop_validator_output() {
        assert!(!desktop_errors("x.desktop: error: key \"SingleMainWindow\" in group \"Desktop Entry\", but keys extending the format should start with \"X-\"\n"));
        assert!(desktop_errors("x.desktop: error: value \"Foo\" for key \"Categories\" is not registered\n"));
        assert!(!desktop_errors("x.desktop: hint: value item \"Audio\" in key \"Categories\"\n"));
    }
}
