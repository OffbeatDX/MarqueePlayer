//! The Windows release: `dist/MarqueePlayer-<version>-portable.zip` (the exe, the Visual C++
//! runtime DLLs beside it, portable.txt, the user README, LICENSE.txt,
//! THIRD-PARTY-NOTICES.txt) and `dist/MarqueePlayer-<version>-setup.exe` (Inno Setup 6, from
//! the same files without portable.txt), the exe copied beside Cargo.toml, and the exe and its
//! PDB kept in `dist/symbols/<version>/`.
//!
//! Compiled everywhere (so it is linted and tested on Linux too); runs on Windows.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Release, copy, create_dir, notices, remove_dir, remove_file, which};
use crate::cmd;
use crate::report::{self, Fail};
use crate::workspace::Workspace;

/// The Visual C++ runtime DLLs the exe imports, shipped beside it (Microsoft's "app-local"
/// deployment), so it runs on a PC without the redistributable installed.
const CRT: [&str; 3] = ["msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"];

/// What the zip holds, in this order.
const ZIP_FILES: [&str; 8] = ["MarqueePlayer.exe", "msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll", "portable.txt", "README.md", "LICENSE.txt", "THIRD-PARTY-NOTICES.txt"];

const QUIT_FIRST: &str = "Quit Marquee Player from its tray icon (right-click, Quit) and run this again.";

pub fn run(r: &mut Release) -> Result<(), Fail> {
    println!("Marquee Player release (Windows)");
    let exe = Workspace::exe("MarqueePlayer");
    let copy_here = r.root().join(&exe);
    let built = r.built();
    println!("target folder: {}", r.target.display());

    // ---- A running exe cannot be replaced: refuse now, not after minutes of building ----
    for f in [&copy_here, &built] {
        if crate::build::in_use(f) {
            return Err(Fail::msg(format!("{} is in use: Marquee Player is running from it. {QUIT_FIRST}", f.display())));
        }
    }

    // ---- What packaging needs besides Rust: found now, not after the build ----
    let iscc = find_iscc()?;
    let crt = find_crt()?;
    let tar = std::env::var_os("SystemRoot").map(|s| PathBuf::from(s).join("System32").join("tar.exe")).filter(|t| t.is_file()).or_else(|| which("tar"));
    let tar = tar.ok_or_else(|| Fail::msg("tar.exe (part of Windows 10 1803 and later, in System32) was not found: it makes the zip."))?;
    let powershell = which("powershell").ok_or_else(|| Fail::msg("Windows PowerShell (powershell.exe) is not on PATH: the import check and the version checks use it."))?;
    println!("Inno Setup:          {}", iscc.display());
    println!("Visual C++ runtime:  {}", crt.display());

    r.checks()?;
    let ver = r.build(&[], &["windows_icon"])?;

    // The version resource too: Explorer's Properties, and what the installer and updates read.
    let product = product_version(&powershell, &built)?;
    if product != ver {
        return Err(Fail::msg(format!("{} has the product version {product} in its version resource, not {ver}.", built.display())));
    }
    r.check_paths(&built)?;

    // ---- The exe beside Cargo.toml (the copy run from the checkout) ----
    copy(&built, &copy_here).map_err(|_| Fail::msg(format!("could not copy {} to {}. Is Marquee Player running from it? {QUIT_FIRST}", built.display(), copy_here.display())))?;
    println!("ok: exe ready: {}", copy_here.display());

    // ---- This release's debug symbols, outside the packages ----
    // The shipped exe carries only its PDB's file name; the PDB and the exact exe it matches are
    // kept so that a crash address from a user's log or dump can be resolved. Never shipped:
    // the PDB holds this machine's paths. Only this build's files are replaced: the Linux
    // release of the same version keeps its own in linux/.
    report::heading("debug symbols");
    let sym = r.dist().join("symbols").join(&ver);
    create_dir(&sym)?;
    for f in [built.clone(), built.with_extension("pdb")] {
        let to = sym.join(f.file_name().unwrap_or_default());
        remove_file(&to)?;
        copy(&f, &to)?;
    }
    println!("ok: symbols kept: {}", sym.display());

    // ---- The portable edition, from a staging folder ----
    report::heading("portable edition");
    let stage = r.dist().join("_stage");
    let port = stage.join("portable");
    let setup_src = stage.join("setup");
    remove_dir(&stage)?;
    create_dir(&port)?;
    create_dir(&setup_src)?;
    let packaging = r.root().join("packaging");
    copy(&built, &port.join(&exe))?;
    for d in CRT {
        copy(&crt.join(d), &port.join(d))?;
    }
    copy(&packaging.join("user-README.md"), &port.join("README.md"))?;
    copy(&packaging.join("LICENSE.txt"), &port.join("LICENSE.txt"))?;
    let vc = product_version(&powershell, &crt.join("msvcp140.dll"))?;
    let extras = [notices::Extra {
        title: format!("Microsoft Visual C++ runtime {vc} (msvcp140.dll, vcruntime140.dll, vcruntime140_1.dll)"),
        licence: "Microsoft Visual Studio licence terms (Distributable Code)".into(),
        source: "https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files".into(),
        body: notices::Body::Text(
            "\
Copyright (c) Microsoft Corporation. These files are not open source. They are
redistributed unmodified, beside MarqueePlayer.exe, as Microsoft allows for the
Visual C++ redistributable files (\"app-local\" deployment). They are covered by
Marquee Player's LICENSE.txt as part of it, and by no other licence here."
                .into(),
        ),
    }];
    super::write_notices(r, "x86_64-pc-windows-msvc", &ver, &extras, &port.join("THIRD-PARTY-NOTICES.txt"), true)?;
    // The installer gets the same files, without the portable marker.
    for f in std::fs::read_dir(&port).map_err(|e| Fail::msg(format!("could not read {}: {e}", port.display())))?.filter_map(Result::ok) {
        copy(&f.path(), &setup_src.join(f.file_name()))?;
    }
    copy(&packaging.join("windows").join("portable.txt"), &port.join("portable.txt"))?;
    // Every DLL the exe and those DLLs import is in the folder or part of Windows.
    let check_deps = packaging.join("windows").join("check-deps.ps1");
    let code = cmd::status(Command::new(&powershell).args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&check_deps).arg("-Folder").arg(&port))?;
    if code != 0 {
        return Err(Fail::msg("The portable folder would not start on a PC without the Visual C++ runtime (above). Nothing was packaged."));
    }
    let zip = r.dist().join(format!("MarqueePlayer-{ver}-portable.zip"));
    remove_file(&zip)?;
    // bsdtar: -a picks zip from the name.
    let code = cmd::status(Command::new(&tar).arg("-a").arg("-cf").arg(&zip).arg("-C").arg(&port).args(ZIP_FILES))?;
    if code != 0 {
        remove_file(&zip)?;
        return Err(Fail::msg("Packaging the portable edition failed (tar, above). The exe is ready; there is no zip or installer."));
    }
    println!("ok: portable edition: {}", zip.display());

    // ---- The installer ----
    // Its icon: the exe's artwork as an .ico, from the windows_icon example (built with the release
    // build, `Release::build`).
    let ico = stage.join("MarqueePlayer.ico");
    let out = cmd::output(Command::new(r.profile_dir().join("examples").join(Workspace::exe("windows_icon"))).arg(&ico))?;
    if !out.status.success() || !ico.is_file() {
        return Err(Fail::msg(format!("windows_icon did not write {}: {}", ico.display(), String::from_utf8_lossy(&out.stderr).trim())));
    }
    report::heading("installer (Inno Setup)");
    let setup = r.dist().join(format!("MarqueePlayer-{ver}-setup.exe"));
    remove_file(&setup)?;
    let mut c = Command::new(&iscc);
    c.arg("/Q").arg(format!("/DAppVersion={ver}")).arg(format!("/DSourceDir={}", setup_src.display())).arg(format!("/DIconFile={}", ico.display()));
    c.arg(format!("/O{}", r.dist().display())).arg(packaging.join("windows").join("MarqueePlayer.iss"));
    let code = cmd::status(&mut c)?;
    if code != 0 {
        return Err(Fail::msg(format!("Building the installer failed (Inno Setup, exit {code}, above). The portable zip is ready; there is no installer.")));
    }
    let setup_ver = product_version(&powershell, &setup)?;
    if setup_ver != ver {
        return Err(Fail::msg(format!("The installer says version {setup_ver}, not {ver}. The portable zip is ready; the installer is not to be used.")));
    }
    println!("ok: installer: {}", setup.display());
    remove_dir(&stage)?;
    r.recheck(&ver)?;

    r.summary(&ver, &[("Portable", zip), ("Installer", setup), ("Exe", copy_here), ("Symbols", sym)], &["Linux: the AppImage of this version is made by `cargo xtask release` in WSL or on Linux, from this checkout.".into()]);
    Ok(())
}

/// A file's product version, from its version resource (trimmed: Inno Setup pads it).
fn product_version(powershell: &Path, file: &Path) -> Result<String, Fail> {
    let out = cmd::output(
        Command::new(powershell).args(["-NoProfile", "-NonInteractive", "-Command", "(Get-Item -LiteralPath $env:MP_XTASK_FILE).VersionInfo.ProductVersion"]).env("MP_XTASK_FILE", file),
    )?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || v.is_empty() {
        return Err(Fail::msg(format!("could not read the version of {}: {}", file.display(), String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(v)
}

/// Inno Setup 6's compiler, ISCC.exe (6.3 or newer: an older one stops at the script's
/// `x64compatible` with its own message): `MP_INNO_SETUP` (its folder, or ISCC.exe itself),
/// else where its installer puts it, for the user or for everyone, else where its uninstall
/// entry says it went.
fn find_iscc() -> Result<PathBuf, Fail> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut candidates = Vec::new();
    if let Some(d) = env("MP_INNO_SETUP") {
        candidates.push(d.join("ISCC.exe"));
        candidates.push(d);
    }
    for root in [env("LOCALAPPDATA").map(|l| l.join("Programs")), env("ProgramFiles(x86)"), env("ProgramFiles")].into_iter().flatten() {
        candidates.push(root.join("Inno Setup 6").join("ISCC.exe"));
    }
    for key in [
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
        r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
        r"HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
    ] {
        if let Some(loc) = registry_string(key, "InstallLocation") {
            candidates.push(PathBuf::from(loc).join("ISCC.exe"));
        }
    }
    candidates
        .into_iter()
        .find(|c| c.is_file() && c.file_name().is_some_and(|n| n.eq_ignore_ascii_case("ISCC.exe")))
        .map(|c| std::path::absolute(&c).unwrap_or(c))
        .ok_or_else(|| {
            Fail::msg(
                "Inno Setup 6 was not found: the installer is built with it (free: https://jrsoftware.org/isdl.php).\n       \
                 Install it (for you only is enough), or unpack it anywhere and set MP_INNO_SETUP to its folder.",
            )
        })
}

/// A string value from the registry (`reg query`), if it is there.
fn registry_string(key: &str, name: &str) -> Option<String> {
    let out = cmd::output(Command::new("reg").args(["query", key, "/v", name])).ok()?;
    if !out.status.success() {
        return None;
    }
    reg_value(&String::from_utf8_lossy(&out.stdout), name)
}

/// The value in `reg query`'s output: `    <name>    REG_SZ    <value>`.
fn reg_value(out: &str, name: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let l = l.trim_start();
        let rest = l.strip_prefix(name)?.trim_start();
        let v = rest.strip_prefix("REG_EXPAND_SZ").or_else(|| rest.strip_prefix("REG_SZ"))?.trim();
        (!v.is_empty()).then(|| v.to_owned())
    })
}

/// The folder of the Visual C++ runtime DLLs to ship: `MP_VC_REDIST`, else
/// `VC\Redist\MSVC\<version>\x64\Microsoft.VC14x.CRT` of the newest Visual Studio with the C++
/// tools (vswhere), the newest version there.
fn find_crt() -> Result<PathBuf, Fail> {
    let has_all = |d: &Path| CRT.iter().all(|n| d.join(n).is_file());
    if let Some(d) = std::env::var_os("MP_VC_REDIST").filter(|v| !v.is_empty()).map(PathBuf::from) {
        if has_all(&d) {
            return Ok(std::path::absolute(&d).unwrap_or(d));
        }
        return Err(Fail::msg(format!("MP_VC_REDIST is {}, which does not hold {}.", d.display(), CRT.join(", "))));
    }
    let mut roots: Vec<String> = Vec::new();
    if let Some(vswhere) = std::env::var_os("ProgramFiles(x86)").map(|p| PathBuf::from(p).join(r"Microsoft Visual Studio\Installer\vswhere.exe")).filter(|p| p.is_file()) {
        for args in [&["-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"][..], &["-all", "-products", "*", "-property", "installationPath"][..]] {
            if let Ok(out) = cmd::output(Command::new(&vswhere).args(args)) {
                roots.extend(String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned));
            }
        }
    }
    let mut seen = Vec::new();
    for vs in roots {
        if seen.contains(&vs) {
            continue;
        }
        seen.push(vs.clone());
        let redist = Path::new(&vs).join(r"VC\Redist\MSVC");
        let mut versions: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir(&redist)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir())
            .filter_map(|e| Some((numeric_version(&e.file_name().to_string_lossy())?, e.path())))
            .collect();
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, v) in versions {
            let crt = std::fs::read_dir(v.join("x64")).into_iter().flatten().filter_map(Result::ok).map(|e| e.path()).find(|p| {
                let n = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                p.is_dir() && n.starts_with("microsoft.vc") && n.ends_with(".crt")
            });
            if let Some(crt) = crt.filter(|c| has_all(c)) {
                return Ok(crt);
            }
        }
    }
    Err(Fail::msg(format!(
        "The Visual C++ runtime DLLs to ship beside the exe were not found ({}).\n       \
         They come with Visual Studio's C++ tools (VC\\Redist\\MSVC\\<version>\\x64\\Microsoft.VC14x.CRT);\n       \
         or set MP_VC_REDIST to a folder that holds them.",
        CRT.join(", ")
    )))
}

/// `14.51.36231` as numbers; None for a name that is not a version (`v145`, `onecore`).
fn numeric_version(name: &str) -> Option<Vec<u64>> {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    parts.iter().map(|p| if p.bytes().all(|b| b.is_ascii_digit()) { p.parse().ok() } else { None }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redist_versions_sort_as_numbers() {
        assert_eq!(numeric_version("14.51.36231"), Some(vec![14, 51, 36231]));
        assert!(numeric_version("14.9.1") < numeric_version("14.10.0"));
        for no in ["v145", "14", "onecore", "14.x", "14..1", ""] {
            assert_eq!(numeric_version(no), None, "{no}");
        }
    }

    #[test]
    fn reads_reg_query_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\...\\Inno Setup 6_is1\r\n    InstallLocation    REG_SZ    C:\\Users\\A\\AppData\\Local\\Programs\\Inno Setup 6\\\r\n\r\n";
        assert_eq!(reg_value(out, "InstallLocation").as_deref(), Some(r"C:\Users\A\AppData\Local\Programs\Inno Setup 6\"));
        assert_eq!(reg_value("    InstallLocation    REG_SZ    \r\n", "InstallLocation"), None);
        assert_eq!(reg_value("ERROR: not found", "InstallLocation"), None);
    }
}
