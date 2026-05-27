//! `cargo xtask setup-native` — build MuPDF and the C wrapper for native dev.
//!
//! 1. Downloads MuPDF sources if the required version is not already present.
//! 2. Builds the `mupdf_wrapper` C library.
//! 3. Compiles MuPDF using system libraries.
//! 4. Creates symlinks in `target/mupdf_wrapper/<platform>/` so the Rust
//!    build script can find the static libraries.
//!
//! ## Required MuPDF version
//!
//! The version is pinned to [`thirdparty::MUPDF_VERSION`].  If the sources
//! already present on disk match this version the download is skipped.

use std::path::Path;

use anyhow::{Context, Result};
use clap::Args;

use super::util::thirdparty::MUPDF_VERSION;
use super::util::{cmd, mupdf_wrapper, thirdparty, workspace};

/// Marker file written after a successful native MuPDF build.
const NATIVE_BUILT_MARKER: &str = ".built-native";

/// Arguments for `cargo xtask setup-native`.
#[derive(Debug, Args)]
pub struct SetupNativeArgs {
    /// Force a re-download of MuPDF sources even if the correct version is
    /// already present.
    #[arg(long)]
    pub force: bool,
}

/// Builds MuPDF and the C wrapper for native (non-cross-compiled) development.
///
/// # Errors
///
/// Returns an error if any build step fails or if required tools (`make`,
/// `pkg-config`, `ar`) are not available.
pub fn run(args: SetupNativeArgs) -> Result<()> {
    let root = workspace::root()?;

    ensure_native_artifacts(&root, args.force)?;

    println!("\nNative setup complete!");
    println!("You can now run:");
    println!("  cargo test          - Run tests");
    println!("  cargo xtask build-kobo  - Build for Kobo (Linux & macOS)");

    Ok(())
}

pub fn ensure_native_artifacts(root: &Path, force: bool) -> Result<()> {
    thirdparty::download_libraries(&root.join("thirdparty"), &["libwebp"])?;
    build_libwebp_native(root)?;

    let mupdf_patched = ensure_mupdf_sources_with_webp_patches(root, force)?;
    if mupdf_patched {
        remove_native_wrapper_artifact(root)?;
    }

    let rebuild_mupdf = mupdf_patched || !native_mupdf_ready(root);
    if rebuild_mupdf {
        build_mupdf_native(root)?;
        write_native_build_marker(root)?;
    } else {
        println!("Native MuPDF build already present.");
    }

    build_mupdf_wrapper_native_if_needed(root)?;

    link_mupdf_artifacts(root)?;

    Ok(())
}

/// Builds libwebp from source for native development.
///
/// # Why we combine static archives manually
///
/// libwebp's build system creates separate `.a` files in sub-directories
/// (`dec/`, `dsp/`, `enc/`, `utils/`) but does **not** assemble them into a
/// single `src/.libs/libwebp.a` when building static-only.  We extract the
/// individual object files from each sub-archive and repack them into one
/// unified `libwebp.a` so that downstream `build.rs` scripts can simply link
/// with `-lwebp`.
pub fn build_libwebp_native(root: &Path) -> Result<()> {
    let libwebp_dir = root.join("thirdparty/libwebp");
    if libwebp_dir.join("src/.libs/libwebp.a").exists() {
        println!("libwebp already built for native.");
        return Ok(());
    }

    println!("Building libwebp for native development…");

    if !libwebp_dir.join("configure").exists() {
        cmd::run("sh", &["autogen.sh"], &libwebp_dir, &[("NOCONFIGURE", "1")])
            .context("failed to run autogen.sh for libwebp")?;
    }

    cmd::run(
        "./configure",
        &[
            "--disable-shared",
            "--enable-static",
            "--disable-libwebpmux",
            "--enable-libwebpdecoder",
            "--enable-libwebpdemux",
            "--disable-webp-tools",
            "--with-pic",
        ],
        &libwebp_dir,
        &[],
    )
    .context("failed to configure libwebp")?;

    cmd::run("make", &["-j4"], &libwebp_dir, &[]).context("failed to build libwebp")?;

    combine_libwebp_static_archives(&libwebp_dir)?;

    println!("✓ libwebp built successfully");
    Ok(())
}

/// Extracts sub-archives from a static libwebp build and repacks them into
/// a single `src/.libs/libwebp.a`.
///
/// See [`build_libwebp_native`] for why this is necessary.
fn combine_libwebp_static_archives(libwebp_dir: &Path) -> Result<()> {
    let libs_dir = libwebp_dir.join("src/.libs");
    std::fs::create_dir_all(&libs_dir).context("failed to create src/.libs for libwebp")?;

    let sublibs = [
        "dec/.libs/libwebpdecode.a",
        "dsp/.libs/libwebpdsp.a",
        "enc/.libs/libwebpencode.a",
        "utils/.libs/libwebputils.a",
    ];

    let mut objects = vec![];
    for sublib in &sublibs {
        let path = libwebp_dir.join("src").join(sublib);
        let extract_dir = libs_dir.join(format!("extract_{}", sublib.replace('/', "_")));
        std::fs::create_dir_all(&extract_dir)?;
        cmd::run("ar", &["x", path.to_str().unwrap()], &extract_dir, &[])
            .with_context(|| format!("failed to extract objects from {}", path.display()))?;
        for entry in std::fs::read_dir(&extract_dir)? {
            objects.push(entry?.path());
        }
    }

    let libwebp_a = libs_dir.join("libwebp.a");
    let mut ar_args: Vec<&str> = vec!["rcs", libwebp_a.to_str().unwrap()];
    for obj in &objects {
        ar_args.push(obj.to_str().unwrap());
    }
    cmd::run("ar", &ar_args, &libwebp_dir, &[]).context("failed to create combined libwebp.a")?;

    Ok(())
}

/// Ensures MuPDF sources at the required version are present in
/// `thirdparty/mupdf/` and that Cadmus' WebP patch series is applied.
///
/// If the version header is missing or reports a different version the
/// existing directory is removed and the sources are re-downloaded.
pub fn ensure_mupdf_sources(root: &Path, force: bool) -> Result<()> {
    ensure_mupdf_sources_with_webp_patches(root, force).map(|_| ())
}

fn ensure_mupdf_sources_with_webp_patches(root: &Path, force: bool) -> Result<bool> {
    let mupdf_dir = root.join("thirdparty/mupdf");
    let version_header = mupdf_dir.join("include/mupdf/fitz/version.h");
    let current_version = read_mupdf_version(&version_header);

    if force || current_version.as_deref() != Some(MUPDF_VERSION) {
        if let Some(v) = &current_version {
            println!("MuPDF version mismatch: have '{v}', need '{MUPDF_VERSION}'");
        }

        println!("Downloading MuPDF {MUPDF_VERSION} sources…");

        if mupdf_dir.exists() {
            thirdparty::clean_untracked(&mupdf_dir)
                .context("failed to clean untracked files from thirdparty/mupdf")?;
        }

        thirdparty::download_libraries(&root.join("thirdparty"), &["mupdf"])?;
    } else {
        println!("MuPDF {MUPDF_VERSION} already present.");
    }

    apply_mupdf_webp_patches_if_needed(&mupdf_dir)
}

fn apply_mupdf_webp_patches_if_needed(mupdf_dir: &Path) -> Result<bool> {
    let patched = thirdparty::apply_mupdf_webp_patches_if_needed(mupdf_dir)?;
    if patched {
        remove_native_build_marker(mupdf_dir);
    }

    Ok(patched)
}

fn remove_native_build_marker(mupdf_dir: &Path) {
    let marker = mupdf_dir.join(NATIVE_BUILT_MARKER);
    if marker.exists() {
        std::fs::remove_file(&marker).ok();
    }
}

/// Reads the MuPDF version string from the version header file.
///
/// Returns `None` if the file does not exist or the version cannot be parsed.
fn read_mupdf_version(header: &Path) -> Option<String> {
    let content = std::fs::read_to_string(header).ok()?;

    // The header contains a line like: #define FZ_VERSION "1.27.0"
    for line in content.lines() {
        if line.contains("FZ_VERSION") && line.contains('"') {
            let start = line.find('"')? + 1;
            let end = line.rfind('"')?;
            if start < end {
                return Some(line[start..end].to_owned());
            }
        }
    }

    None
}

/// Returns `true` when the full native setup is complete.
///
/// Checks that the build marker, the compiled `libmupdf.a`, the C wrapper
/// library `libmupdf_wrapper.a`, and both symlinks in
/// `target/mupdf_wrapper/<platform>/` are all present.
pub fn native_setup_done(root: &Path) -> bool {
    let platform_dir = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };

    let wrapper_dir = root.join(format!("target/mupdf_wrapper/{platform_dir}"));

    native_mupdf_ready(root)
        && wrapper_dir.join("libmupdf.a").exists()
        && wrapper_dir.join("libmupdf_wrapper.a").exists()
}

/// Returns `true` when native MuPDF libraries are present and marked as built.
fn native_mupdf_ready(root: &Path) -> bool {
    let marker = root.join("thirdparty/mupdf").join(NATIVE_BUILT_MARKER);
    if !marker.exists() {
        return false;
    }

    let libmupdf = root.join("thirdparty/mupdf/build/release/libmupdf.a");

    libmupdf.exists()
}

/// Writes the native build marker in the MuPDF source directory.
fn write_native_build_marker(root: &Path) -> Result<()> {
    let marker = root.join("thirdparty/mupdf").join(NATIVE_BUILT_MARKER);
    std::fs::write(&marker, "").with_context(|| {
        format!(
            "failed to write native build marker at {}",
            marker.display()
        )
    })
}

/// Builds the `mupdf_wrapper` C static library for the native platform.
fn build_mupdf_wrapper_native_if_needed(root: &Path) -> Result<()> {
    println!("Ensuring mupdf_wrapper is available…");
    mupdf_wrapper::build_native_if_needed(root)
}

fn remove_native_wrapper_artifact(root: &Path) -> Result<()> {
    let platform_dir = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };
    let lib = root.join(format!(
        "target/mupdf_wrapper/{platform_dir}/libmupdf_wrapper.a"
    ));

    if lib.exists() {
        std::fs::remove_file(&lib)
            .with_context(|| format!("failed to remove stale {}", lib.display()))?;
    }

    Ok(())
}

/// Compiles MuPDF using system libraries for the native platform.
fn build_mupdf_native(root: &Path) -> Result<()> {
    println!("Building MuPDF for native development…");

    let mupdf_dir = root.join("thirdparty/mupdf");

    // Remove git metadata that interferes with the MuPDF build system.
    for entry in ["gitattributes", ".gitattributes"] {
        let path = mupdf_dir.join(entry);
        if path.exists() {
            std::fs::remove_file(&path).ok();
        }
    }

    cmd::run("make", &["clean"], &mupdf_dir, &[]).ok();
    cmd::run("make", &["verbose=yes", "generate"], &mupdf_dir, &[])?;

    let sys_cflags = collect_system_cflags()?;
    let xcflags = format!(
        "-DFZ_ENABLE_ICC=0 -DFZ_ENABLE_SPOT_RENDERING=0 \
         -DFZ_ENABLE_ODT_OUTPUT=0 -DFZ_ENABLE_OCR_OUTPUT=0 \
         -DHAVE_WEBP=1 -I{root}/thirdparty/libwebp/src {sys_cflags}",
        root = root.display()
    );

    // Linker flags for libwebp static libraries
    let xlibs = format!(
        "-L{root}/thirdparty/libwebp/src/.libs -lwebp \
         -L{root}/thirdparty/libwebp/src/demux/.libs -lwebpdemux",
        root = root.display()
    );

    cmd::run(
        "make",
        &[
            "verbose=yes",
            "mujs=no",
            "tesseract=no",
            "extract=no",
            "archive=no",
            "brotli=no",
            "barcode=no",
            "commercial=no",
            "USE_SYSTEM_LIBS=yes",
            &format!("XCFLAGS={xcflags}"),
            &format!("XLIBS={xlibs}"),
            "libs",
        ],
        &mupdf_dir,
        &[],
    )
}

/// Collects system library CFLAGS via `pkg-config` on macOS.
///
/// On Linux, MuPDF's build system detects system libraries automatically.
/// On macOS it needs explicit CFLAGS gathered from pkg-config.
fn collect_system_cflags() -> Result<String> {
    if !cfg!(target_os = "macos") {
        return Ok(String::new());
    }

    let libs = [
        "freetype2",
        "harfbuzz",
        "libopenjp2",
        "libjpeg",
        "libwebp",
        "zlib",
        "jbig2dec",
        "gumbo",
    ];

    let mut flags = String::new();
    for lib in libs {
        if let Ok(f) = cmd::output("pkg-config", &["--cflags", lib], Path::new("."), &[]) {
            if !f.is_empty() {
                flags.push(' ');
                flags.push_str(&f);
            }
        }
    }

    Ok(flags.trim().to_owned())
}

/// Creates symlinks in `target/mupdf_wrapper/<platform>/` pointing to the
/// compiled MuPDF static libraries.
fn link_mupdf_artifacts(root: &Path) -> Result<()> {
    let platform_dir = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };

    let target_dir = root.join(format!("target/mupdf_wrapper/{platform_dir}"));
    std::fs::create_dir_all(&target_dir)
        .context("failed to create target/mupdf_wrapper directory")?;

    let release_dir = root.join("thirdparty/mupdf/build/release");

    let libmupdf = release_dir.join("libmupdf.a");
    if !libmupdf.exists() {
        anyhow::bail!("libmupdf.a not found after build -- check MuPDF build output");
    }

    symlink_force(&libmupdf, &target_dir.join("libmupdf.a"))?;
    println!("✓ Created libmupdf.a in target/mupdf_wrapper/{platform_dir}");

    let libmupdf_third = release_dir.join("libmupdf-third.a");
    if !libmupdf_third.exists() {
        println!("Creating empty libmupdf-third.a (system libs used instead)…");
        cmd::run("ar", &["cr", "libmupdf-third.a"], &release_dir, &[])?;
    }

    symlink_force(&libmupdf_third, &target_dir.join("libmupdf-third.a"))?;
    println!("✓ Created libmupdf-third.a");

    Ok(())
}

/// Creates a symlink at `link` pointing to `target`, removing any existing
/// file or symlink at `link` first.
fn symlink_force(target: &Path, link: &Path) -> Result<()> {
    if link.exists() || link.symlink_metadata().is_ok() {
        std::fs::remove_file(link)
            .with_context(|| format!("failed to remove existing {}", link.display()))?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("failed to create symlink {}", link.display()))?;

    #[cfg(not(unix))]
    std::fs::copy(target, link)
        .with_context(|| format!("failed to copy {} to {}", target.display(), link.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn read_mupdf_version_parses_define() {
        let tmp = tempfile::tempdir().unwrap();
        let header = tmp.path().join("version.h");
        fs::write(
            &header,
            r#"/* MuPDF version */
#define FZ_VERSION "1.27.0"
#define FZ_VERSION_MAJOR 1
"#,
        )
        .unwrap();

        let version = read_mupdf_version(&header);
        assert_eq!(version.as_deref(), Some("1.27.0"));
    }

    #[test]
    fn read_mupdf_version_returns_none_for_malformed_header() {
        let tmp = tempfile::tempdir().unwrap();
        let header = tmp.path().join("version.h");
        fs::write(&header, "/* no version define here */\n").unwrap();

        let version = read_mupdf_version(&header);
        assert!(version.is_none());
    }
}
