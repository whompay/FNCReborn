//! Install / update: get the source, the game files, build the ReXGlue SDK,
//! recompile default.xex and build the game, then put everything in dist.
//! Every step is skipped when its inputs have not changed, so pressing
//! Install again after a failure (or an update) only redoes what is needed.

use crate::config;
use crate::display_tab;
use crate::fsx;
use crate::platform::{self, EXE};
use crate::report::{Reporter, capture, ninja_progress, run};
use crate::tools::{self, Toolchain};
use anyhow::{Context, Result, bail};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum GameSource {
    /// Game files already in dist/game (or the update case).
    None,
    Iso(PathBuf),
    /// A folder with default.xex and the databigs folder in it.
    Folder(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Options {
    pub dir: PathBuf,
    pub game: GameSource,
    pub update: bool,
    pub desktop_shortcut: bool,
    /// Start menu / application menu entries and remembering the folder.
    pub integrate: bool,
    pub accept_license: bool,
    /// Rebuild even when the source is unchanged.
    pub force: bool,
    /// Download / update the source from GitHub (off: build the source already
    /// in the folder, for testing local changes).
    pub fetch_source: bool,
}

pub fn is_install_folder(d: &Path) -> bool {
    d.join("scripts").join("setup.ps1").is_file()
        || d.join("setup.bat").is_file()
        || d.join("project").join("CMakeLists.txt").is_file()
}

fn has_other_files(d: &Path) -> bool {
    fs::read_dir(d)
        .map(|rd| {
            rd.flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                n != "setup-installer.log" && n != ".download"
            })
        })
        .unwrap_or(false)
}

fn sha1_file(p: &Path) -> Result<String> {
    let bytes = fs::read(p).with_context(|| format!("could not read {}", p.display()))?;
    Ok(crate::report::hex(&Sha1::digest(&bytes)).to_uppercase())
}

struct Stamps(PathBuf);
impl Stamps {
    fn done(&self, name: &str, key: &str) -> bool {
        fsx::read_text(&self.0.join(name)).as_deref() == Some(key)
    }
    fn mark(&self, name: &str, key: &str) -> Result<()> {
        fsx::write_text(&self.0.join(name), key)
    }
    fn clear(&self, name: &str) {
        let _ = fs::remove_file(self.0.join(name));
    }
}

/// Checks done before anything starts; returns a message for the user.
pub fn validate(o: &Options) -> Result<()> {
    let dir = &o.dir;
    if !dir.is_absolute() {
        bail!(
            "Choose a full install folder path, for example {}.",
            platform::default_install_dir().display()
        );
    }
    if o.update && !is_install_folder(dir) {
        bail!(
            "That folder is not an FNC Reborn folder. Choose the folder that contains the dist folder."
        );
    }
    if !o.fetch_source && !is_install_folder(dir) {
        bail!("--no-download needs a folder that already has the FNC Reborn source in it.");
    }
    if !o.update && dir.exists() && !is_install_folder(dir) && has_other_files(dir) {
        bail!("The install folder already contains other files. Choose an empty or new folder.");
    }
    let game_there = dir.join("dist").join("game").join("default.xex").is_file();
    match &o.game {
        GameSource::Iso(p) if !p.is_file() => {
            bail!("The disc image was not found: {}", p.display())
        }
        GameSource::Folder(p) if !p.join("default.xex").is_file() => bail!(
            "That game folder has no default.xex in it. Choose the folder that contains default.xex \
             and the databigs folder."
        ),
        GameSource::None if !game_there => {
            bail!(
                "Choose your Fight Night Champion disc image (.iso) or the folder with your game files first."
            )
        }
        _ => {}
    }
    Ok(())
}

/// Returns a summary line for the end.
pub fn run_all(r: &Reporter, o: &Options) -> Result<String> {
    let dir = o.dir.clone();
    fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    r.open_log(&dir.join("setup-installer.log"));
    r.log(format!(
        "FNC Reborn Setup {} - install folder {}",
        config::APP_VERSION,
        dir.display()
    ));
    if let Some(free) = platform::free_gb(&dir) {
        r.log(format!("Free space on the install drive: {free} GB"));
        if free < config::MIN_FREE_GB {
            bail!(
                "Not enough free space on the drive of {}: {free} GB free. Setup needs about {} GB \
                 (build tools and the build). Free some space, or choose a folder on another drive.",
                dir.display(),
                config::NEEDED_FREE_GB
            );
        }
        if free < config::NEEDED_FREE_GB {
            r.log(format!(
                "Warning: only {free} GB free; a first build needs about {} GB.",
                config::NEEDED_FREE_GB
            ));
        }
    }
    r.progress(None);

    let tc = tools::ensure_all(r, o.accept_license, &dir)?;
    #[cfg(windows)]
    {
        let downloads = tools::root_for(&dir).join("downloads");
        fs::create_dir_all(&downloads)?;
        platform::ensure_vc_runtime(r, &downloads)?;
    }

    let updated = if o.fetch_source {
        get_source(r, &tc, &dir)?
    } else {
        r.log("Building the source already in the folder (--no-download).");
        true
    };
    let dist = dir.join("dist");
    let build = dir.join("build");
    let ready = dist.join("FNCReborn.exe").exists();
    let stamps = Stamps(build.join("stamps"));
    let toolchain_current = stamps.done("toolchain", config::TOOLCHAIN_ID);
    if !updated && ready && toolchain_current && !o.force && o.game == GameSource::None {
        finish(r, o, &dir);
        return Ok("FNC Reborn is already up to date.".into());
    }
    build_game(r, &tc, o, &dir)?;
    finish(r, o, &dir);
    Ok(if o.update {
        "FNC Reborn is updated."
    } else {
        "FNC Reborn is installed."
    }
    .into())
}

/// For CI: the toolchain, the SDK and the recompiler only (no game files
/// needed). `root` is an FNC Reborn source folder.
pub fn sdk_only(r: &Reporter, root: &Path, accept_license: bool) -> Result<String> {
    let tc = tools::ensure_all(r, accept_license, root)?;
    let build = root.join("build");
    let stamps = Stamps(build.join("stamps"));
    fs::create_dir_all(&stamps.0)?;
    let jobs = jobs().to_string();
    let (sdk_src, sdk_install) = (build.join("rexglue-sdk"), build.join("sdk"));
    build_sdk(r, &tc, root, &stamps, &sdk_src, &sdk_install, &jobs)?;
    let rexglue = host_rexglue(r, &tc, &sdk_src, &sdk_install, &build, &jobs)?;
    let mut c = tc.env.command(&rexglue);
    c.arg("--help");
    let _ = crate::report::run_code(r, &mut c, &mut |_: &str| {});
    for dll in ["rexruntime.dll", "rexgpu-xenos.dll"] {
        fsx::require(&sdk_install.join("bin").join(dll), dll)?;
    }
    Ok(format!("SDK and recompiler built: {}", rexglue.display()))
}

fn finish(r: &Reporter, o: &Options, dir: &Path) {
    if !o.integrate {
        r.progress(Some(1.0));
        return;
    }
    r.status("Creating shortcuts");
    let setup_copy = dir.join(format!("FNCReborn-Setup{EXE}"));
    let copied = match std::env::current_exe() {
        Ok(me) if me != setup_copy => fs::copy(&me, &setup_copy).is_ok(),
        Ok(_) => true,
        Err(_) => false,
    };
    platform::create_shortcuts(
        r,
        dir,
        o.desktop_shortcut,
        copied.then_some(setup_copy.as_path()),
    );
    platform::remember_install_dir(dir);
    r.progress(Some(1.0));
}

/// Returns true when the source changed (new install or new patches).
fn get_source(r: &Reporter, tc: &Toolchain, dir: &Path) -> Result<bool> {
    let git = |args: &[&str]| {
        let mut c = tc.env.command(&tc.git);
        c.arg("-C").arg(dir).args(args);
        c
    };
    if is_install_folder(dir) {
        r.status("Downloading the latest FNC Reborn update");
        let is_repo = dir.join(".git").is_dir();
        let before = if is_repo {
            capture(&mut git(&["rev-parse", "HEAD"])).unwrap_or_default()
        } else {
            String::new()
        };
        if !is_repo {
            run(
                r,
                git(&["init", "-q"]),
                "Preparing the folder for updates (git init)",
                |_| {},
            )?;
        }
        run(
            r,
            git(&["fetch", "--depth", "1", config::REPO_URL, config::BRANCH]),
            "Downloading the update",
            |_| {},
        )
        .context("Check your internet connection.")?;
        let _ = capture(&mut git(&["config", "core.autocrlf", "false"]));
        let after = capture(&mut git(&["rev-parse", "FETCH_HEAD"])).unwrap_or_default();
        if is_repo && !after.is_empty() && after == before {
            r.log(format!(
                "Source code is already the latest version ({}).",
                short(&after)
            ));
            return Ok(false);
        }
        run(
            r,
            git(&["checkout", "-f", "-B", config::BRANCH, "FETCH_HEAD"]),
            "Applying the update",
            |_| {},
        )?;
        let _ = capture(&mut git(&["remote", "remove", "origin"]));
        let _ = capture(&mut git(&["remote", "add", "origin", config::REPO_URL]));
        r.log(format!(
            "Updated {} -> {}",
            if before.is_empty() {
                "download".into()
            } else {
                short(&before)
            },
            short(&after)
        ));
        return Ok(true);
    }
    r.status("Downloading the FNC Reborn source code");
    let tmp = dir.join(".download");
    fsx::remove_dir(&tmp)?;
    let mut c = tc.env.command(&tc.git);
    c.args([
        "clone",
        "-c",
        "core.autocrlf=false",
        "--depth",
        "1",
        "--branch",
        config::BRANCH,
        config::REPO_URL,
    ])
    .arg(&tmp);
    run(r, c, "Downloading the source code", |_| {}).context("Check your internet connection.")?;
    for e in fs::read_dir(&tmp)? {
        let e = e?;
        fs::rename(e.path(), dir.join(e.file_name()))?;
    }
    fsx::remove_dir(&tmp)?;
    Ok(true)
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// The SDK commit the downloaded source wants (config/sdk_commit.txt, or the
/// line in scripts/setup.ps1).
pub fn sdk_commit(root: &Path) -> String {
    if let Some(c) = fsx::read_text(&root.join("config").join("sdk_commit.txt"))
        && c.len() == 40
    {
        return c;
    }
    if let Ok(s) = fs::read_to_string(root.join("scripts").join("setup.ps1")) {
        for l in s.lines() {
            if let Some(rest) = l.trim().strip_prefix("$SdkCommit = \"")
                && let Some(c) = rest.split('"').next()
                && c.len() == 40
            {
                return c.to_string();
            }
        }
    }
    config::SDK_COMMIT_FALLBACK.to_string()
}

fn jobs() -> u64 {
    if let Ok(value) = std::env::var("FNC_BUILD_JOBS")
        && let Ok(jobs) = value.parse::<u64>()
        && jobs > 0
    {
        return jobs;
    }
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get() as u64)
        .unwrap_or(4);
    // The recompiled files are huge: at most one compile per ~2 GB of RAM.
    cpus.min(platform::ram_gb() / 2).max(1)
}

fn build_game(r: &Reporter, tc: &Toolchain, o: &Options, root: &Path) -> Result<()> {
    let build = root.join("build");
    let dist = root.join("dist");
    let game_dir = dist.join("game");
    let sdk_src = build.join("rexglue-sdk");
    let sdk_install = build.join("sdk");
    let game_build = build.join("game");
    let generated = build.join("generated");
    fs::create_dir_all(&build)?;
    fs::create_dir_all(&dist)?;
    let stamps = Stamps(build.join("stamps"));
    fs::create_dir_all(&stamps.0)?;
    let jobs = jobs().to_string();
    let cpu = platform::cpu_level();
    r.log(format!(
        "Build: {jobs} parallel jobs, CPU level {cpu}, {} GB RAM",
        platform::ram_gb()
    ));

    // A different compiler: start the build folders over.
    if !stamps.done("toolchain", config::TOOLCHAIN_ID) {
        r.status("Preparing the build folders for the new build tools");
        fsx::remove_dir(&game_build)?;
        fsx::remove_dir(&sdk_src.join("out"))?;
        fsx::remove_dir(&sdk_install)?;
        fsx::remove_dir(&build.join("host"))?;
        stamps.clear("sdk");
        stamps.clear("codegen");
        stamps.mark("toolchain", config::TOOLCHAIN_ID)?;
    }

    // ---- game files
    get_game_files(r, tc, o, root, &game_dir)?;
    let xex = sha1_file(&game_dir.join("default.xex"))?;
    if xex != config::KNOWN_XEX_SHA1 {
        r.log(format!(
            "WARNING: your default.xex (SHA-1 {xex}) is not the version this port was made for. The build may \
             fail or the game may not run correctly."
        ));
    }

    // ---- ReXGlue SDK
    let sdk_key = build_sdk(r, tc, root, &stamps, &sdk_src, &sdk_install, &jobs)?;

    // ---- recompile default.xex
    let manifest = root.join("config").join("fnc_manifest.toml");
    let codegen_key = format!("{sdk_key} {}", sha1_file(&manifest)?);
    if !stamps.done("codegen", &codegen_key) {
        let rexglue = host_rexglue(r, tc, &sdk_src, &sdk_install, &build, &jobs)?;
        r.status("Recompiling default.xex (PowerPC -> C++)");
        let mut c = tc.env.command(&rexglue);
        c.arg("codegen")
            .arg(&manifest)
            .arg("--ignore-stamp")
            .current_dir(root);
        run(r, c, "Recompiling default.xex", |_| {})?;
        stamps.mark("codegen", &codegen_key)?;
    } else {
        r.log("Recompiled code already generated - skipping.");
    }

    // ---- the game
    r.status("Building FNC Reborn (compiles ~70,000 functions; 15-60 minutes)");
    let mut c = tc.env.command(&tc.cmake);
    c.arg("-S")
        .arg(root.join("project"))
        .arg("-B")
        .arg(&game_build)
        .args(["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release"]);
    c.args(common_cmake_args(tc));
    c.arg(format!("-DFNC_GENERATED_DIR={}", cm(&generated)));
    c.arg(format!("-DFNC_CPU_LEVEL={cpu}"));
    c.env("REXSDK", &sdk_install);
    run(r, c, "Configuring the game build", |_| {})?;
    let mut c = tc.env.command(&tc.cmake);
    c.arg("--build").arg(&game_build).args(["-j", &jobs]);
    c.env("REXSDK", &sdk_install);
    run(r, c, "Building the game", |l| progress_line(r, l))?;

    // ---- dist
    r.status("Copying the game to dist");
    copy_to_dist(root, &game_build, &sdk_install, &dist).context(
        "Could not copy the game to dist. If FNC Reborn is running, close it and press Install again",
    )?;

    // ---- Settings > DISPLAY tab, in the player's own fe.big
    r.status("Adding the DISPLAY tab to the game's Settings screen");
    let fe_big = game_dir.join("databigs").join("xenon").join("fe.big");
    match display_tab::patch_fe_big(&fe_big) {
        Ok(true) => r.log("DISPLAY tab added (the original is kept as fe.big.orig)."),
        Ok(false) => r.log("DISPLAY tab already there."),
        Err(e) => r.log(format!(
            "Warning: the DISPLAY tab could not be added ({e:#}). The game works without it; the same \
             settings are in dist/FNCReborn.toml."
        )),
    }

    if !dist.join("FNCReborn.exe").exists() {
        bail!("The build finished but dist/FNCReborn.exe is missing.");
    }
    Ok(())
}

fn build_sdk(
    r: &Reporter,
    tc: &Toolchain,
    root: &Path,
    stamps: &Stamps,
    sdk_src: &Path,
    sdk_install: &Path,
    jobs: &str,
) -> Result<String> {
    let commit = sdk_commit(root);
    let patch = root.join("patches").join("rexglue-sdk.patch");
    let sdk_key = format!("{commit} {} {}", sha1_file(&patch)?, config::TOOLCHAIN_ID);
    if stamps.done("sdk", &sdk_key) {
        r.log("ReXGlue SDK already built - skipping.");
        return Ok(sdk_key);
    }
    prepare_sdk_source(r, tc, sdk_src, &commit, &patch)?;
    r.status("Building the ReXGlue SDK (about 5-20 minutes)");
    let bdir = sdk_src.join("out").join("build").join("fncreborn");
    let mut c = tc.env.command(&tc.cmake);
    c.arg("-S")
        .arg(sdk_src)
        .arg("-B")
        .arg(&bdir)
        .args(["-G", "Ninja Multi-Config"]);
    c.args(common_cmake_args(tc));
    c.args([
        "-DCMAKE_C_FLAGS=-march=x86-64-v2",
        "-DCMAKE_CXX_FLAGS=-march=x86-64-v2",
        "-DCMAKE_CXX_STANDARD=23",
        "-DCMAKE_CONFIGURATION_TYPES=Debug;Release;RelWithDebInfo",
        "-DCMAKE_DEFAULT_BUILD_TYPE=Release",
    ]);
    c.arg(format!("-DCMAKE_INSTALL_PREFIX={}", cm(sdk_install)));
    run(r, c, "Configuring the ReXGlue SDK", |_| {})?;
    let mut c = tc.env.command(&tc.cmake);
    c.arg("--build")
        .arg(&bdir)
        .args(["--target", "install", "--config", "Release", "-j", jobs]);
    run(r, c, "Building the ReXGlue SDK", |l| progress_line(r, l))?;
    stamps.mark("sdk", &sdk_key)?;
    stamps.clear("codegen");
    Ok(sdk_key)
}

fn progress_line(r: &Reporter, line: &str) {
    if let Some((a, b)) = ninja_progress(line)
        && b > 0
    {
        r.progress(Some(a as f32 / b as f32));
    }
}

/// CMake path: forward slashes.
fn cm(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Compiler settings every Windows-target configure gets.
fn common_cmake_args(tc: &Toolchain) -> Vec<String> {
    let mut v = Vec::new();
    match &tc.cross_file {
        Some(f) => v.push(format!("-DCMAKE_TOOLCHAIN_FILE={}", cm(f))),
        None => {
            v.push(format!("-DCMAKE_C_COMPILER={}", cm(&tc.clang())));
            v.push(format!("-DCMAKE_CXX_COMPILER={}", cm(&tc.clangxx())));
            v.push(format!(
                "-DCMAKE_RC_COMPILER={}",
                cm(&tc.llvm_bin.join(format!("llvm-rc{EXE}")))
            ));
            v.push("-DCMAKE_LINKER_TYPE=LLD".into());
        }
    }
    v.push(format!("-DCMAKE_MAKE_PROGRAM={}", cm(&ninja_path(tc))));
    // Only the release C runtime is downloaded: CMake's compiler checks must not
    // link a Debug program (msvcrtd.lib). Without Visual Studio on the PC the
    // first configure failed with "could not open 'msvcrtd.lib'".
    v.push("-DCMAKE_TRY_COMPILE_CONFIGURATION=Release".into());
    v
}

fn ninja_path(tc: &Toolchain) -> PathBuf {
    tc.env.path_prefix[2].join(format!("ninja{EXE}"))
}

fn prepare_sdk_source(
    r: &Reporter,
    tc: &Toolchain,
    sdk_src: &Path,
    commit: &str,
    patch: &Path,
) -> Result<()> {
    let git = |args: &[&str]| {
        let mut c = tc.env.command(&tc.git);
        c.arg("-C").arg(sdk_src).args(args);
        c
    };
    if !sdk_src.join(".git").is_dir() {
        r.status("Downloading the ReXGlue SDK");
        fsx::remove_dir(sdk_src)?;
        let mut c = tc.env.command(&tc.git);
        c.args(["clone", "-c", "core.autocrlf=false", config::SDK_REPO])
            .arg(sdk_src);
        run(r, c, "Downloading the ReXGlue SDK", |_| {})
            .context("Check your internet connection.")?;
    }
    r.status(format!(
        "Checking out ReXGlue SDK {} and its dependencies",
        short(commit)
    ));
    if run(
        r,
        git(&[
            "-c",
            "advice.detachedHead=false",
            "checkout",
            "--force",
            commit,
        ]),
        "git checkout",
        |_| {},
    )
    .is_err()
    {
        run(
            r,
            git(&["fetch", "origin"]),
            "Downloading the ReXGlue SDK update",
            |_| {},
        )?;
        run(
            r,
            git(&[
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "--force",
                commit,
            ]),
            "git checkout",
            |_| {},
        )?;
    }
    // Files an older patch added (checkout --force leaves them).
    let _ = capture(&mut git(&[
        "clean",
        "-fdq",
        "--",
        "include",
        "src",
        "resources",
        "cmake",
    ]));
    if run(
        r,
        git(&[
            "submodule",
            "update",
            "--init",
            "--recursive",
            "--force",
            "--depth",
            "1",
        ]),
        "git submodule",
        |_| {},
    )
    .is_err()
    {
        run(
            r,
            git(&["submodule", "update", "--init", "--recursive", "--force"]),
            "Downloading the SDK dependencies",
            |_| {},
        )?;
    }
    #[cfg(windows)]
    fix_mspack_links(sdk_src);

    r.status("Applying the Fight Night Champion patch to the SDK");
    let patch_s = patch.to_string_lossy().into_owned();
    if capture(&mut git(&["apply", "--check", &patch_s])).is_some() {
        run(
            r,
            git(&["apply", "--whitespace=nowarn", &patch_s]),
            "Applying the SDK patch",
            |_| {},
        )?;
    } else if capture(&mut git(&["apply", "--reverse", "--check", &patch_s])).is_some() {
        r.log("Patch already applied.");
    } else {
        bail!("The SDK patch does not apply. Delete the build folder and press Install again.");
    }
    Ok(())
}

/// Git on Windows checks libmspack's symlinked sources out as small text
/// files containing the link target; replace them with the real files.
#[cfg(windows)]
fn fix_mspack_links(sdk_src: &Path) {
    let dir = sdk_src
        .join("thirdparty")
        .join("libmspack")
        .join("cabextract")
        .join("mspack");
    let Ok(rd) = fs::read_dir(&dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if e.metadata()
            .map(|m| m.is_file() && m.len() < 300)
            .unwrap_or(false)
            && let Ok(t) = fs::read_to_string(&p)
        {
            let t = t.trim();
            if t.starts_with("../") && !t.contains('\n') {
                let target = dir.join(t);
                if target.is_file() {
                    let _ = fs::copy(&target, &p);
                }
            }
        }
    }
}

/// The recompiler has to run on this PC: on Windows the SDK build installs
/// it; on Linux a native copy is built once.
fn host_rexglue(
    r: &Reporter,
    tc: &Toolchain,
    sdk_src: &Path,
    sdk_install: &Path,
    build: &Path,
    jobs: &str,
) -> Result<PathBuf> {
    if cfg!(windows) {
        let p = sdk_install.join("bin").join("rexglue.exe");
        fsx::require(&p, "The recompiler (rexglue.exe)")?;
        return Ok(p);
    }
    let host = build.join("host").join("sdk");
    r.status("Building the recompiler for this PC");
    let mut c = tc.env.command(&tc.cmake);
    c.arg("-S").arg(sdk_src).arg("-B").arg(&host).args([
        "-G",
        "Ninja",
        "-DCMAKE_BUILD_TYPE=Release",
    ]);
    c.arg(format!("-DCMAKE_C_COMPILER={}", cm(&tc.clang())));
    c.arg(format!("-DCMAKE_CXX_COMPILER={}", cm(&tc.clangxx())));
    c.args([
        "-DCMAKE_C_FLAGS=-march=x86-64-v2",
        "-DCMAKE_CXX_FLAGS=-march=x86-64-v2",
        "-DCMAKE_CXX_STANDARD=23",
    ]);
    c.arg("-DCMAKE_LINKER_TYPE=LLD");
    c.arg(format!("-DCMAKE_MAKE_PROGRAM={}", cm(&ninja_path(tc))));
    // Only the recompiler is built here: SDL needs no window or sound
    // system, so no X11 / Wayland / ALSA development packages are needed.
    c.args([
        "-DSDL_UNIX_CONSOLE_BUILD=ON",
        "-DSDL_X11=OFF",
        "-DSDL_WAYLAND=OFF",
        "-DSDL_KMSDRM=OFF",
        "-DSDL_OPENGL=OFF",
        "-DSDL_OPENGLES=OFF",
        "-DSDL_ALSA=OFF",
        "-DSDL_JACK=OFF",
        "-DSDL_PIPEWIRE=OFF",
        "-DSDL_PULSEAUDIO=OFF",
        "-DSDL_SNDIO=OFF",
        "-DSDL_DBUS=OFF",
        "-DSDL_IBUS=OFF",
        "-DSDL_HIDAPI_LIBUSB=OFF",
    ]);
    run(r, c, "Configuring the recompiler", |_| {})?;
    let mut c = tc.env.command(&tc.cmake);
    c.arg("--build")
        .arg(&host)
        .args(["--target", "rexglue", "-j", jobs]);
    run(r, c, "Building the recompiler", |l| progress_line(r, l)).context(LINUX_DEPS)?;
    // The SDK writes executables under its source tree, not its CMake build tree.
    find_file(&sdk_src.join("out"), "rexglue")
        .or_else(|| find_file(&host, "rexglue"))
        .context("The recompiler was built but not found")
}

const LINUX_DEPS: &str = "Building programs for this PC needs the C++ standard library headers. Install them \
    (Debian/Ubuntu: sudo apt install build-essential, Fedora: sudo dnf install gcc-c++, Arch: sudo pacman -S \
    base-devel) and press Install again";

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if p.file_name().map(|n| n == name).unwrap_or(false) {
            return Some(p);
        }
    }
    None
}

/// Compiles a small helper program for this PC (not for the game).
fn host_tool(
    r: &Reporter,
    tc: &Toolchain,
    out: &Path,
    sources: &[PathBuf],
    c_sources: &[PathBuf],
    includes: &[PathBuf],
) -> Result<()> {
    fs::create_dir_all(out.parent().unwrap())?;
    let objdir = out.with_extension("obj.d");
    fs::create_dir_all(&objdir)?;
    let mut objs = Vec::new();
    for (i, s) in c_sources.iter().enumerate() {
        let o = objdir.join(format!("c{i}.o"));
        let mut c = tc.env.command(tc.clang());
        c.args([
            "-O2",
            "-c",
            "-D_CRT_SECURE_NO_WARNINGS",
            "-D_CRT_NONSTDC_NO_DEPRECATE",
        ]);
        for inc in includes {
            c.arg("-I").arg(inc);
        }
        c.arg(s).arg("-o").arg(&o);
        run(r, c, "Compiling a helper", |_| {})?;
        objs.push(o);
    }
    let mut c = tc.env.command(tc.clangxx());
    c.args([
        "-std=c++20",
        "-O2",
        "-fuse-ld=lld",
        "-D_CRT_SECURE_NO_WARNINGS",
    ]);
    for inc in includes {
        c.arg("-I").arg(inc);
    }
    c.args(sources).args(&objs).arg("-o").arg(out);
    let res = run(r, c, "Compiling a helper", |_| {});
    if cfg!(windows) {
        res
    } else {
        res.context(LINUX_DEPS)
    }
}

fn get_game_files(
    r: &Reporter,
    tc: &Toolchain,
    o: &Options,
    root: &Path,
    game_dir: &Path,
) -> Result<()> {
    let have = game_dir.join("default.xex").is_file();
    match &o.game {
        GameSource::None => {
            if !have {
                bail!("The game files are missing. Choose your disc image (.iso) or game folder.");
            }
            r.log("Game files already in dist/game - skipping.");
        }
        GameSource::Folder(src) => {
            if have && same_dir(src, game_dir) {
                return Ok(());
            }
            r.status(format!("Copying the game files from {}", src.display()));
            fsx::remove_dir(game_dir)?;
            let mut n = 0u64;
            fsx::copy_tree(
                r,
                src,
                game_dir,
                &|name, _| name.eq_ignore_ascii_case("$SystemUpdate"),
                &mut n,
            )?;
            r.log(format!("Copied {n} files."));
        }
        GameSource::Iso(iso) => {
            if have {
                r.log("Game files already in dist/game - the disc image is not needed.");
                return Ok(());
            }
            let extractor = root
                .join("build")
                .join("host")
                .join(format!("xiso_extract{EXE}"));
            r.status("Building the disc image extractor");
            host_tool(
                r,
                tc,
                &extractor,
                &[root
                    .join("tools")
                    .join("xiso_extract")
                    .join("xiso_extract.cpp")],
                &[],
                &[],
            )?;
            r.status(format!(
                "Extracting the game from {} (about 6 GB)",
                iso.display()
            ));
            fsx::remove_dir(game_dir)?;
            let mut c = tc.env.command(&extractor);
            c.arg(iso).arg(game_dir);
            for s in ["$SystemUpdate", "layer1filler.bin", "layer2filler.bin"] {
                c.arg("--skip-name").arg(s);
            }
            run(r, c, "Extracting the disc image", |_| {})?;
        }
    }
    if !game_dir.join("default.xex").is_file() {
        bail!(
            "The game files have no default.xex - is this an Xbox 360 Fight Night Champion disc?"
        );
    }
    Ok(())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn copy_to_dist(root: &Path, game_build: &Path, sdk_install: &Path, dist: &Path) -> Result<()> {
    fs::copy(game_build.join("FNCReborn.exe"), dist.join("FNCReborn.exe"))?;
    for e in fs::read_dir(sdk_install.join("bin"))? {
        let p = e?.path();
        if p.extension()
            .map(|x| x.eq_ignore_ascii_case("dll"))
            .unwrap_or(false)
        {
            fs::copy(&p, dist.join(p.file_name().unwrap()))?;
        }
    }
    // Default settings, once: the player's changes are kept on updates.
    let config = dist.join("FNCReborn.toml");
    if !config.exists() {
        fs::copy(root.join("config").join("FNCReborn.toml"), &config)?;
    }
    Ok(())
}
