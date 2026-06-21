use regex::Regex;
use sniffer::*;
use std::fs;
use std::path::PathBuf;

fn print_help() {
    eprintln!(
        "Usage: sniffer [options] <modpack-folder>

Parse mod JARs from a modpack folder and extract metadata.

Arguments:
  <modpack-folder>          Path to the modpack directory (searched recursively)

Options:
  -o, --output <file>       Write output to file (required; no stdout output)
  -F, --format <fmt>        Output format: json (default) or yaml
  -i, --icons-dir <dir>     Extract mod icons into the specified directory
  -e, --exclude <regex>     Exclude mods whose mod_id matches the regex
  -n, --exclude-filename <regex>
                            Exclude mods whose filename matches the regex
      --exclude-nometa      Exclude mods without parseable metadata
      --exclude-nologo      Exclude mods without an icon/logo
  -h, --help                Show this help message"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut dir: Option<PathBuf> = None;
    let mut output_path: Option<PathBuf> = None;
    let mut icons_dir: Option<PathBuf> = None;
    let mut exclude_pattern: Option<String> = None;
    let mut exclude_filename_pattern: Option<String> = None;
    let mut exclude_nometa = false;
    let mut exclude_nologo = false;
    let mut format: String = "json".to_string();
    let mut i = 1;

    while i < args.len() {
        let eat_arg = |i: &mut usize| -> String {
            *i += 1;
            if *i < args.len() {
                args[*i].clone()
            } else {
                eprintln!("Error: {} requires a value", args[*i - 1]);
                std::process::exit(1);
            }
        };

        match args[i].as_str() {
            "-h" | "--help" => {
                print_help();
                return;
            }
            "-e" | "--exclude" => {
                exclude_pattern = Some(eat_arg(&mut i));
            }
            "-n" | "--exclude-filename" => {
                exclude_filename_pattern = Some(eat_arg(&mut i));
            }
            "--exclude-nometa" => {
                exclude_nometa = true;
            }
            "--exclude-nologo" => {
                exclude_nologo = true;
            }
            "-i" | "--icons-dir" => {
                icons_dir = Some(PathBuf::from(eat_arg(&mut i)));
            }
            "-o" | "--output" => {
                output_path = Some(PathBuf::from(eat_arg(&mut i)));
            }
            "-F" | "--format" => {
                let val = eat_arg(&mut i).to_lowercase();
                if val != "json" && val != "yaml" {
                    eprintln!("Error: --format must be 'json' or 'yaml'");
                    std::process::exit(1);
                }
                format = val;
            }
            arg if arg.starts_with('-') => {
                eprintln!("Error: unknown flag {} (use -h for help)", arg);
                std::process::exit(1);
            }
            _ => {
                if dir.is_none() {
                    dir = Some(PathBuf::from(&args[i]));
                } else {
                    eprintln!("Error: unexpected argument {} (use -h for help)", args[i]);
                    std::process::exit(1);
                }
            }
        }
        i += 1;
    }

    let dir = match dir {
        Some(d) => d,
        None => {
            print_help();
            std::process::exit(1);
        }
    };

    if !dir.is_dir() {
        eprintln!("Error: {} is not a directory", dir.display());
        std::process::exit(1);
    }

    let exclude_re = exclude_pattern
        .as_ref()
        .map(|p| Regex::new(p).unwrap_or_else(|e| {
            eprintln!("Error: invalid regex '{}': {}", p, e);
            std::process::exit(1);
        }));

    let exclude_filename_re = exclude_filename_pattern
        .as_ref()
        .map(|p| Regex::new(p).unwrap_or_else(|e| {
            eprintln!("Error: invalid regex '{}': {}", p, e);
            std::process::exit(1);
        }));

    if let Some(ref icons) = icons_dir {
        fs::create_dir_all(icons).unwrap_or_else(|e| {
            eprintln!("Error: cannot create icons dir '{}': {}", icons.display(), e);
            std::process::exit(1);
        });
    }

    let scanned = scan_folder(&dir);

    let mut all_mods: Vec<ModInfo> = Vec::new();

    for sm in scanned {
        if sm.info.mod_loader.is_none() {
            eprintln!("Warning: {} is not a mod (no metadata or class files)", sm.info.filename);
        }

        let excluded_by_id = sm
            .info
            .mod_id
            .as_ref()
            .and_then(|id| exclude_re.as_ref().map(|re| re.is_match(id)))
            .unwrap_or(false);

        let excluded_by_filename = exclude_filename_re
            .as_ref()
            .map(|re| re.is_match(&sm.info.filename))
            .unwrap_or(false);

        let excluded_nometa = exclude_nometa && sm.info.mod_id.is_none();
        let excluded_nologo = exclude_nologo && sm.info.icon.is_none();

        if excluded_by_id || excluded_by_filename || excluded_nometa || excluded_nologo {
            continue;
        }

        let mut info = sm.info;

        if let (Some(icons), Some(ref icon_bytes), Some(ref icon_path)) = (&icons_dir, sm.icon_bytes, sm.icon_path) {
            let fname = filename_from_icon_path(icon_path, info.mod_id.as_deref());
            let icon_path_out = icons.join(&fname);
            fs::write(&icon_path_out, icon_bytes).unwrap_or_else(|e| {
                eprintln!("Warning: failed to write icon '{}': {}", icon_path_out.display(), e);
            });
            info.icon = Some(format!("{}/{}", icons.file_name().unwrap_or(icons.as_os_str()).to_string_lossy(), fname));
        }

        all_mods.push(info);
    }

    let parsed_count = all_mods.iter().filter(|m| m.mod_id.is_some()).count();
    let total_count = all_mods.len();
    eprintln!("Discovered {} mods ({} JARs)", parsed_count, total_count);

    let output = match format.as_str() {
        "yaml" => serde_yaml::to_string(&all_mods).unwrap_or_else(|e| {
            eprintln!("Error serializing to YAML: {}", e);
            std::process::exit(1);
        }),
        _ => serde_json::to_string_pretty(&all_mods).unwrap_or_default(),
    };

    if let Some(mut path) = output_path {
        if path.is_dir() {
            let fname = format!("manifest.{}", format);
            path = path.join(&fname);
        }
        fs::write(&path, &output).unwrap_or_else(|e| {
            eprintln!("Error writing output: {}", e);
            std::process::exit(1);
        });
    } else {
        println!("{}", output);
    }
}
