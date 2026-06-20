use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
struct Dependency {
    mod_id: String,
    version_range: Option<String>,
    mandatory: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ModInfo {
    filename: String,
    mod_id: Option<String>,
    name: Option<String>,
    icon: Option<String>,
    authors: Vec<String>,
    license: Option<String>,
    url: Option<String>,
    minecraft_version: Option<String>,
    dependencies: Vec<Dependency>,
    mod_loader: Option<String>,
}

struct IconData {
    bytes: Vec<u8>,
    path_in_jar: String,
}

fn extract_text(entry: &mut zip::read::ZipFile) -> String {
    let mut buf = String::new();
    entry.read_to_string(&mut buf).ok();
    buf
}

fn extract_bytes(entry: &mut zip::read::ZipFile) -> Vec<u8> {
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).ok();
    buf
}

fn parse_fabric_json(bytes: &str) -> Option<ModInfo> {
    let v: serde_json::Value = serde_json::from_str(bytes).ok()?;
    let obj = v.as_object()?;

    let mod_id = obj.get("id")?.as_str().map(|s| s.to_string());
    let name = obj.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());

    let icon = match obj.get("icon") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Object(m)) => {
            m.values().find_map(|v| v.as_str()).map(|s| s.to_string())
        }
        _ => None,
    };

    let authors = match obj.get("authors") {
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(m) => m.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()),
                _ => None,
            })
            .collect(),
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        _ => vec![],
    };

    let license = obj.get("license").and_then(|v| v.as_str()).map(|s| s.to_string());

    let url = obj
        .get("contact")
        .and_then(|c| c.as_object())
        .and_then(|c| {
            c.get("homepage")
                .or_else(|| c.get("sources"))
                .or_else(|| c.get("issues"))
        })
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut deps = Vec::new();
    let mut mc_version = None;

    for key in &["depends", "recommends", "breaks", "conflicts"] {
        if let Some(dep_map) = obj.get(*key).and_then(|d| d.as_object()) {
            for (dep_id, range_val) in dep_map {
                let range = range_val.as_str().map(|s| s.to_string());
                let mandatory = *key == "depends";
                if dep_id == "minecraft" {
                    mc_version = range;
                } else {
                    deps.push(Dependency {
                        mod_id: dep_id.clone(),
                        version_range: range,
                        mandatory,
                    });
                }
            }
        }
    }

    Some(ModInfo {
        filename: String::new(),
        mod_id,
        name,
        icon,
        authors,
        license,
        url,
        minecraft_version: mc_version,
        dependencies: deps,
        mod_loader: Some("fabric".to_string()),
    })
}

fn parse_forge_toml(bytes: &str) -> Vec<ModInfo> {
    let v: toml::Value = match toml::from_str(bytes) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let mods_table = match v.get("mods").and_then(|m| m.as_array()) {
        Some(arr) => arr,
        None => return vec![],
    };

    let mods: Vec<&toml::Value> = mods_table.iter().collect();
    let dep_map: HashMap<&str, &toml::Value> = v
        .get("dependencies")
        .and_then(|d| d.as_table())
        .map(|t| t.iter().map(|(k, v)| (k.as_str(), v)).collect())
        .unwrap_or_default();

    let mut results = Vec::new();
    for m in &mods {
        let table = match m.as_table() {
            Some(t) => t,
            None => continue,
        };

        let mod_id = table
            .get("modId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let name = table
            .get("displayName")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let icon = table
            .get("logoFile")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let license = table
            .get("license")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let url = table
            .get("displayURL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let authors = match table.get("authors") {
            Some(toml::Value::String(s)) => {
                s.split(',').map(|a| a.trim().to_string()).collect()
            }
            Some(toml::Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| match v {
                    toml::Value::String(s) => Some(s.clone()),
                    other => other.as_table().and_then(|t| t.get("name")).and_then(|n| n.as_str()).map(|s| s.to_string()),
                })
                .collect(),
            _ => vec![],
        };

        let mut deps = Vec::new();
        let mut mc_version = None;

        if let Some(mod_id_str) = &mod_id
            && let Some(dep_arr) = dep_map.get(mod_id_str.as_str()).and_then(|d| d.as_array()) {
                for dep in dep_arr {
                    if let Some(dep_table) = dep.as_table() {
                        let dep_mod_id = dep_table
                            .get("modId")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let range = dep_table
                            .get("versionRange")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let mandatory = dep_table
                            .get("mandatory")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);

                        if let Some(ref did) = dep_mod_id {
                            if did == "minecraft" {
                                mc_version = range;
                            } else if let Some(did_val) = dep_mod_id {
                                deps.push(Dependency {
                                    mod_id: did_val,
                                    version_range: range,
                                    mandatory,
                                });
                            }
                        }
                    }
                }
            }

        results.push(ModInfo {
            filename: String::new(),
            mod_id,
            name,
            icon,
            authors,
            license,
            url,
            minecraft_version: mc_version,
            dependencies: deps,
            mod_loader: Some("forge".to_string()),
        });
    }

    results
}

fn parse_mcmod_info(bytes: &str) -> Vec<ModInfo> {
    let v: serde_json::Value = match serde_json::from_str(bytes) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let arr = match v.as_array() {
        Some(a) => a,
        None => match v.get("modList").and_then(|m| m.as_array()) {
            Some(a) => a,
            None => return vec![],
        },
    };

    let mut results = Vec::new();
    for entry in arr {
        let obj = match entry.as_object() {
            Some(o) => o,
            None => continue,
        };

        let mod_id = obj.get("modid").and_then(|v| v.as_str()).map(|s| s.to_string());
        let name = obj.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());
        let icon = obj.get("logoFile").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
        let url = obj.get("url").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
        let mc_version = obj.get("mcversion").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());

        let authors = obj
            .get("authorList")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut deps = Vec::new();

        for key in &["requiredMods", "dependencies", "dependants"] {
            if let Some(arr) = obj.get(*key).and_then(|v| v.as_array()) {
                for dep in arr {
                    if let Some(raw) = dep.as_str() {
                        let parts: Vec<&str> = raw.splitn(2, '@').collect();
                        let dep_id = parts[0].trim();
                        let range = parts.get(1).map(|s| s.to_string());
                        if dep_id != "forge" && !deps.iter().any(|d: &Dependency| d.mod_id == dep_id) {
                            deps.push(Dependency {
                                mod_id: dep_id.to_string(),
                                version_range: range,
                                mandatory: *key == "requiredMods" || *key == "dependencies",
                            });
                        }
                    }
                }
            }
        }

        results.push(ModInfo {
            filename: String::new(),
            mod_id,
            name,
            icon,
            authors,
            license: None,
            url,
            minecraft_version: mc_version,
            dependencies: deps,
            mod_loader: Some("forge".to_string()),
        });
    }

    results
}

fn parse_jar(path: &Path) -> Vec<(ModInfo, Option<IconData>)> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };

    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(_) => return vec![],
    };

    let mut mods: Vec<(ModInfo, Option<String>)> = Vec::new();
    let mut has_class_files = false;

    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.name().replace('\\', "/");

        if name.ends_with(".class") {
            has_class_files = true;
        }

        if name == "fabric.mod.json" {
            let content = extract_text(&mut entry);
            if let Some(mut info) = parse_fabric_json(&content) {
                info.filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let icon = info.icon.clone();
                mods.push((info, icon));
            }
        } else if name == "mcmod.info" || name.ends_with("/mcmod.info") {
            let content = extract_text(&mut entry);
            let mut parsed = parse_mcmod_info(&content);
            for pm in &mut parsed {
                pm.filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
            }
            for pm in parsed {
                let icon = pm.icon.clone();
                mods.push((pm, icon));
            }
        } else if name == "META-INF/mods.toml" || name == "META-INF/neoforge.mods.toml" {
            let content = extract_text(&mut entry);
            let loader = if name == "META-INF/neoforge.mods.toml" {
                Some("neoforge".to_string())
            } else {
                Some("forge".to_string())
            };
            let parsed = parse_forge_toml(&content);
            for mut pm in parsed {
                pm.mod_loader = loader.clone();
                pm.filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let icon = pm.icon.clone();
                mods.push((pm, icon));
            }
        }
    }

    if mods.is_empty()
        && let Some(fname) = path.file_name().and_then(|n| n.to_str()).map(|s| s.to_string())
    {
        mods.push((
            ModInfo {
                filename: fname,
                mod_id: None,
                name: if has_class_files { Some("Unknown mod".to_string()) } else { None },
                icon: None,
                authors: vec![],
                license: None,
                url: None,
                minecraft_version: None,
                dependencies: vec![],
                mod_loader: if has_class_files { Some("forge".to_string()) } else { None },
            },
            None,
        ));
    }

    let result: Vec<(ModInfo, Option<IconData>)> = mods
        .into_iter()
        .map(|(info, icon_path_opt)| {
            let icon_data = icon_path_opt.as_ref().and_then(|icon_path| {
                let normalised = icon_path.replace('\\', "/");
                let candidates = if normalised.starts_with("META-INF/") {
                    vec![normalised.clone()]
                } else {
                    vec![normalised.clone(), format!("META-INF/{}", normalised)]
                };
                (0..archive.len()).find_map(|j| {
                    let mut e = archive.by_index(j).ok()?;
                    let ename = e.name().replace('\\', "/");
                    if candidates.iter().any(|c| c == &ename) {
                        Some(IconData {
                            bytes: extract_bytes(&mut e),
                            path_in_jar: normalised.clone(),
                        })
                    } else {
                        None
                    }
                })
            });
            (info, icon_data)
        })
        .collect();

    result
}

fn filename_from_icon_path(icon_path: &str, mod_id: Option<&str>) -> String {
    let no_query = icon_path.split('?').next().unwrap_or(icon_path);
    if let Some(id) = mod_id {
        let ext = no_query.rsplit('.').next_back().filter(|s| s.len() <= 4).unwrap_or("png");
        format!("{}.{}", id, ext)
    } else {
        let basename = no_query.split('/').next_back().unwrap_or(no_query);
        if basename.is_empty() {
            "icon".to_string()
        } else {
            basename.to_string()
        }
    }
}

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

    let mut all_mods: Vec<ModInfo> = Vec::new();
    let icons_dir_ref = icons_dir.as_ref();

    find_jars(&dir, &mut all_mods, &icons_dir_ref, &exclude_re, &exclude_filename_re, exclude_nometa, exclude_nologo);

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
    }
}

fn find_jars(path: &Path, all_mods: &mut Vec<ModInfo>, icons_dir_ref: &Option<&PathBuf>, exclude_re: &Option<Regex>, exclude_filename_re: &Option<Regex>, exclude_nometa: bool, exclude_nologo: bool) {
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                find_jars(&child, all_mods, icons_dir_ref, exclude_re, exclude_filename_re, exclude_nometa, exclude_nologo);
            } else if child.extension().and_then(|e| e.to_str()) == Some("jar") {
                let parsed = parse_jar(&child);

                if parsed.iter().all(|(info, _)| info.mod_loader.is_none()) {
                    let fname = child.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    eprintln!("Warning: {} is not a mod (no metadata or class files)", fname);
                }

                for (mut info, icon_data) in parsed {
                    let excluded_by_id = info
                        .mod_id
                        .as_ref()
                        .and_then(|id| exclude_re.as_ref().map(|re| re.is_match(id)))
                        .unwrap_or(false);

                    let excluded_by_filename = exclude_filename_re
                        .as_ref()
                        .map(|re| re.is_match(&info.filename))
                        .unwrap_or(false);

                    let excluded_nometa = exclude_nometa && info.mod_id.is_none();
                    let excluded_nologo = exclude_nologo && info.icon.is_none();

                    if excluded_by_id || excluded_by_filename || excluded_nometa || excluded_nologo {
                        continue;
                    }

                    if let (Some(icons), Some(icon_data)) = (icons_dir_ref, &icon_data) {
                        let fname = filename_from_icon_path(&icon_data.path_in_jar, info.mod_id.as_deref());
                        let icon_path = icons.join(&fname);
                        fs::write(&icon_path, &icon_data.bytes).unwrap_or_else(|e| {
                            eprintln!("Warning: failed to write icon '{}': {}", icon_path.display(), e);
                        });
                        info.icon = Some(format!("{}/{}", icons.file_name().unwrap_or(icons.as_os_str()).to_string_lossy(), fname));
                    }

                    all_mods.push(info);
                }
            }
        }
    }
}
