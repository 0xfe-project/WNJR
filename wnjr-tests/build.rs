use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Suite {
    dir: &'static str,
    version_file: &'static str,
    version: &'static str,
    url: &'static str,
    env_name: &'static str,
}

const TESTSUITE_VERSION: &str = "51279a9d02cbba193cb25142d115388d7b83299c";
const WASI_TESTSUITE_VERSION: &str = "caf3b66fa3457cc17156864d971387a7e9f5933b";

const SUITES: &[Suite] = &[
    Suite {
        dir: "webassembly-testsuite",
        version_file: "testsuite_version.txt",
        version: TESTSUITE_VERSION,
        url: "https://github.com/WebAssembly/testsuite/archive/51279a9d02cbba193cb25142d115388d7b83299c.tar.gz",
        env_name: "TESTSUITE_DIR",
    },
    // `prod/testsuite-base`, NOT `main`. The suite's default branch ships only
    // sources and a `build.py` that needs a wasi-sdk clang to produce the test
    // binaries; this branch ships them prebuilt. Pinning `main` here leaves the
    // runner with zero `.wasm` files to discover, which is not a loud failure --
    // it is an empty run that reports success. See `find_test_suite_directories`,
    // which refuses an empty discovery for the same reason.
    Suite {
        dir: "wasi-testsuite",
        version_file: "wasi_testsuite_version.txt",
        version: WASI_TESTSUITE_VERSION,
        url: "https://github.com/WebAssembly/wasi-testsuite/archive/caf3b66fa3457cc17156864d971387a7e9f5933b.tar.gz",
        env_name: "WASI_TESTSUITE_DIR",
    },
];

fn main() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let cargo_target_dir: PathBuf = Path::new(&out_dir)
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "target"))
        .unwrap_or_else(|| Path::new("target"))
        .to_path_buf();

    for suite in SUITES {
        let suite_dir = cargo_target_dir.join(suite.dir);
        let version_file = suite_dir.join(suite.version_file);
        if !up_to_date(&suite_dir, &version_file, suite.version) {
            eprintln!("Downloading {}...", suite.dir);
            download_and_extract(&suite_dir, suite).expect("Failed to download testsuite");
            fs::write(&version_file, suite.version).expect("Failed to write version file");
            eprintln!("{} downloaded and extracted", suite.dir);
        }
        if version_file.exists() {
            println!("cargo:rerun-if-changed={}", version_file.display());
        }
        println!("cargo:rustc-env={}={}", suite.env_name, suite_dir.display());
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
}

fn up_to_date(suite_dir: &Path, version_file: &Path, version: &str) -> bool {
    if !(suite_dir.exists() && version_file.exists()) {
        return false;
    }
    match fs::read_to_string(version_file) {
        Ok(existing) => existing.trim() == version,
        Err(_) => false,
    }
}

fn download_and_extract(suite_dir: &Path, suite: &Suite) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = suite_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    if suite_dir.exists() {
        fs::remove_dir_all(suite_dir)?;
    }

    let temp_dir = suite_dir.with_extension("tmp");
    fs::create_dir_all(&temp_dir)?;
    let tar_path = temp_dir.join("testsuite.tar.gz");

    download_file(suite.url, &tar_path)?;
    extract_tar_gz(&tar_path, &temp_dir)?;

    let extracted = fs::read_dir(&temp_dir)?
        .filter_map(|entry| entry.ok())
        .find(|entry| entry.file_type().ok().is_some_and(|ft| ft.is_dir()))
        .ok_or("No directory found in extracted archive")?;
    fs::rename(extracted.path(), suite_dir)?;
    fs::remove_dir_all(temp_dir)?;
    Ok(())
}

fn download_file(url: &str, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for tool in ["curl", "wget"] {
        let mut command = Command::new(tool);
        if tool == "curl" {
            command.args(["-L", "-o"]).arg(dest).arg(url);
        } else {
            command.args(["-O"]).arg(dest).arg(url);
        }
        if command.status().is_ok_and(|status| status.success()) {
            return Ok(());
        }
    }
    Err("Neither curl nor wget found. Please install one to download the testsuite.".into())
}

fn extract_tar_gz(archive_path: &Path, dest_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let ok = Command::new("tar")
        .args(["-xzf"])
        .arg(archive_path)
        .arg("-C")
        .arg(dest_dir)
        .status()
        .is_ok_and(|status| status.success());
    if ok {
        return Ok(());
    }
    Err("tar command not found. Please install tar to extract the testsuite.".into())
}
