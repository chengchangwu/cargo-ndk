use std::{env, ffi::OsStr, path::PathBuf};

use cargo_metadata::MetadataCommand;
use gumdrop::Options;

use crate::meta::Target;

#[derive(Debug, Options)]
struct Args {
    #[options(help = "show help information")]
    help: bool,

    #[options(free, help = "args to be passed to cargo")]
    cargo_args: Vec<String>,

    #[options(help = "triple for the target(s)")]
    target: Vec<Target>,

    #[options(help = "platform (also known as API level)")]
    platform: Option<u8>,

    #[options(help = "output to a jniLibs directory in the correct sub-directories")]
    output_dir: Option<PathBuf>,
}

fn derive_ndk_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("ANDROID_NDK_HOME") {
        return Some(PathBuf::from(path));
    };

    if let Some(path) = env::var_os("NDK_HOME") {
        return Some(PathBuf::from(path));
    };

    if let Some(sdk_path) = env::var_os("ANDROID_SDK_HOME") {
        let path = PathBuf::from(sdk_path).join("ndk-bundle");

        if path.exists() {
            return Some(path);
        }
    };

    // Check Android Studio installed directories
    #[cfg(windows)]
    let base_dir = pathos::user::local_dir();
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let base_dir = pathos::user::data_dir();

    let ndk_dir = base_dir.join("Android").join("sdk").join("ndk");
    if ndk_dir.exists() {
        let mut paths = std::fs::read_dir(&ndk_dir)
            .ok()?
            .flat_map(Result::ok)
            .map(|x| x.path())
            .collect::<Vec<_>>();
        paths.sort();
        paths.reverse();
        return paths.first().cloned();
    }

    None
}

fn print_usage() {
    println!("cargo-ndk -- Brendan Molloy <https://github.com/bbqsrc/cargo-ndk>\n\nUsage: cargo ndk [OPTIONS] <CARGO_ARGS>\n");
    println!("{}", Args::usage());
}

pub(crate) fn run(args: Vec<String>) {
    log::trace!("Args: {:?}", args);

    if args.is_empty() || args.contains(&"-h".into()) || args.contains(&"--help".into()) {
        print_usage();

        std::process::exit(0);
    }

    let is_release = args.contains(&"--release".into());
    log::trace!("is_release: {}", is_release);

    let args = match Args::parse_args(&args, gumdrop::ParsingStyle::StopAtFirstFree) {
        Ok(args) if args.help => {
            print_usage();
            std::process::exit(0);
        }
        Ok(args) => args,
        Err(e) => {
            log::error!("{}", e);
            std::process::exit(2);
        }
    };

    let metadata = MetadataCommand::new()
        .manifest_path("./Cargo.toml")
        .exec()
        .unwrap();

    // We used to check for NDK_HOME, so we'll keep doing that. But we'll also try ANDROID_NDK_HOME
    // and $ANDROID_SDK_HOME/ndk-bundle as this is how Android Studio configures the world
    let ndk_home = match derive_ndk_path() {
        Some(v) => {
            log::info!("Using NDK at path: {}", v.display());
            v
        }
        None => {
            log::error!("Could not find any NDK.");
            log::error!(
                "Set the environment ANDROID_NDK_HOME to your NDK installation's root directory,\nor install the NDK using Android Studio."
            );
            return;
        }
    };

    let current_dir = std::env::current_dir().expect("current directory could not be resolved");
    let config = match crate::meta::config(&current_dir.join("Cargo.toml"), is_release) {
        Ok(v) => v,
        Err(e) => {
            log::error!("{}", e);
            std::process::exit(1);
        }
    };

    // Try command line, then config. Config falls back to defaults in any case.
    let targets = if !args.target.is_empty() {
        args.target
    } else {
        config.targets
    };

    let platform = config.platform;
    let platform = args.platform.unwrap_or_else(|| platform);

    if let Some(output_dir) = args.output_dir.as_ref() {
        std::fs::create_dir_all(output_dir).expect("failed to create output directory");
    }

    log::info!("NDK API level: {}", platform);
    log::info!(
        "Building targets: {}",
        targets
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    for target in targets.iter() {
        let triple = target.triple();
        log::info!("Building {} ({})", &target, &triple);

        let status = crate::cargo::run(&current_dir, &ndk_home, triple, platform, &args.cargo_args);
        let code = status.code().unwrap_or(-1);

        if code != 0 {
            log::info!("If the build failed due to a missing target, you can run this command:");
            log::info!("");
            log::info!("    rustup target install {}", triple);
            std::process::exit(code);
        }
    }

    let out_dir = metadata.target_directory;

    if let Some(output_dir) = args.output_dir.as_ref() {
        log::info!("Copying libraries to {}...", &output_dir.display());

        for target in targets {
            let arch_output_dir = output_dir.join(target.to_string());
            std::fs::create_dir_all(&arch_output_dir).unwrap();

            let dir =
                out_dir
                    .join(target.triple())
                    .join(if is_release { "release" } else { "debug" });

            log::trace!("Target path: {}", dir.display());

            let so_files = std::fs::read_dir(&dir)
                .ok()
                .unwrap()
                .flat_map(Result::ok)
                .map(|x| x.path())
                .filter(|x| x.extension() == Some(OsStr::new("so")))
                .collect::<Vec<_>>();

            for so_file in so_files {
                let dest = arch_output_dir.join(so_file.file_name().unwrap());
                log::info!("{} -> {}", &so_file.display(), dest.display());
                std::fs::copy(so_file, &dest).unwrap();

                let _ = crate::cargo::strip(&ndk_home, &target.triple(), &dest);
            }
        }
    }
}
