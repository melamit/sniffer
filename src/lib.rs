use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use regex::Regex;

#[derive(Debug, Clone, Serialize)]
pub struct Dependency {
    pub mod_id: String,
    pub version_range: Option<String>,
    pub mandatory: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModInfo {
    pub filename: String,
    pub mod_id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub authors: Vec<String>,
    pub license: Option<String>,
    pub url: Option<String>,
    pub minecraft_version: Option<String>,
    pub dependencies: Vec<Dependency>,
    pub mod_loader: Option<String>,
}

pub struct IconData {
    pub bytes: Vec<u8>,
    pub path_in_jar: String,
}

pub fn extract_text(entry: &mut zip::read::ZipFile) -> String {
    let mut buf = String::new();
    entry.read_to_string(&mut buf).ok();
    buf
}

pub fn extract_bytes(entry: &mut zip::read::ZipFile) -> Vec<u8> {
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).ok();
    buf
}

pub fn parse_fabric_json(bytes: &str) -> Option<ModInfo> {
    let v: serde_json::Value = serde_json::from_str(bytes).ok()?;
    let obj = v.as_object()?;

    let mod_id = obj.get("id")?.as_str().map(|s| s.to_string());
    let name = obj.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());
    let version = obj.get("version").and_then(|v| v.as_str()).map(|s| s.to_string());
    let description = obj.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());

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
        version,
        description,
        icon,
        authors,
        license,
        url,
        minecraft_version: mc_version,
        dependencies: deps,
        mod_loader: Some("fabric".to_string()),
    })
}

pub fn parse_forge_toml(bytes: &str) -> Vec<ModInfo> {
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
        let version = table
            .get("version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let description = table
            .get("description")
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
            version,
            description,
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

fn lenient_json_parse(bytes: &str) -> Option<serde_json::Value> {
    if let Ok(v) = serde_json::from_str(bytes) {
        return Some(v);
    }

    let re_single = Regex::new(r"'([^']*)'").ok()?;
    let fixed = re_single.replace_all(bytes, "\"$1\"").to_string();

    let mut out = String::with_capacity(fixed.len());
    let mut in_string = false;
    let mut escape_next = false;
    let mut i = 0;
    let chars: Vec<char> = fixed.chars().collect();

    while i < chars.len() {
        let ch = chars[i];

        if in_string {
            if escape_next {
                out.push(ch);
                escape_next = false;
            } else if ch == '\\' {
                escape_next = true;
                out.push(ch);
            } else if ch == '"' {
                in_string = false;
                out.push(ch);
            } else {
                out.push(ch);
            }
        } else {
            if ch == '"' {
                in_string = true;
                out.push(ch);
            } else if ch == '0' && i + 1 < chars.len() && chars[i + 1] == 'x' {
                let mut j = i + 2;
                while j < chars.len() && chars[j].is_ascii_hexdigit() {
                    j += 1;
                }
                if j > i + 2 {
                    out.push('"');
                    out.extend(&chars[i..j]);
                    out.push('"');
                    i = j - 1;
                } else {
                    out.push(ch);
                }
            } else {
                out.push(ch);
            }
        }
        i += 1;
    }

    let mut out2 = String::with_capacity(out.len());
    in_string = false;
    escape_next = false;
    for ch in out.chars() {
        if in_string {
            if escape_next {
                out2.push(ch);
                escape_next = false;
            } else if ch == '\\' {
                escape_next = true;
                out2.push(ch);
            } else if ch == '"' {
                in_string = false;
                out2.push(ch);
            } else if ch == '\n' {
                out2.push(' ');
            } else {
                out2.push(ch);
            }
        } else {
            if ch == '"' {
                in_string = true;
            } else if ch == '\r' {
                continue;
            }
            out2.push(ch);
        }
    }

    serde_json::from_str(&out2).ok()
}

pub fn parse_mcmod_info(bytes: &str) -> Vec<ModInfo> {
    let v: serde_json::Value = match lenient_json_parse(bytes) {
        Some(v) => v,
        None => return vec![],
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
        let version = obj.get("version").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
        let description = obj.get("description").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
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
            version,
            description,
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

pub fn parse_jar(path: &Path) -> Vec<(ModInfo, Option<IconData>)> {
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
                version: None,
                description: None,
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

pub fn filename_from_icon_path(icon_path: &str, mod_id: Option<&str>) -> String {
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

#[derive(Debug, Clone)]
pub struct ScannedMod {
    pub info: ModInfo,
    pub file_path: PathBuf,
    pub icon_bytes: Option<Vec<u8>>,
    pub icon_path: Option<String>,
    pub display_name: Option<String>,
}

pub fn scan_folder(dir: &Path) -> Vec<ScannedMod> {
    let mut results = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                results.extend(scan_folder(&child));
            } else if child.extension().and_then(|e| e.to_str()) == Some("jar") {
                let parsed = parse_jar(&child);
                for (info, icon_data) in parsed {
                    let icon_path = icon_data.as_ref().map(|d| d.path_in_jar.clone());
                    results.push(ScannedMod {
                        info,
                        file_path: child.clone(),
                        icon_bytes: icon_data.map(|d| d.bytes),
                        icon_path,
                        display_name: None,
                    });
                }
            }
        }
    }
    results
}
