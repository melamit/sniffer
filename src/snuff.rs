use eframe::egui;
use sniffer::*;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use walkdir::WalkDir;

#[allow(dead_code)]
enum ScanMsg {
    Progress { current: usize, total: usize },
    Done(Vec<ScannedMod>),
    Error(String),
}

enum PackMsg {
    Progress { current: usize, total: usize },
    Done,
    Error(String),
}

pub struct SnifferApp {
    folder: Option<PathBuf>,
    mods: Vec<ScannedMod>,
    checked: Vec<bool>,
    icons: Vec<Option<egui::TextureHandle>>,
    scanning: bool,
    scan_progress: f32,
    scan_status: String,
    output_format: String,
    error: Option<String>,
    packing: bool,
    pack_progress: f32,
    pack_status: String,
    scan_rx: Option<mpsc::Receiver<ScanMsg>>,
    pack_rx: Option<mpsc::Receiver<PackMsg>>,
    ready: bool,
    search_query: String,
    include_pattern: String,
    exclude_pattern: String,
}

impl Default for SnifferApp {
    fn default() -> Self {
        Self {
            folder: None,
            mods: Vec::new(),
            checked: Vec::new(),
            icons: Vec::new(),
            scanning: false,
            scan_progress: 0.0,
            scan_status: String::new(),
            output_format: "zip".to_string(),
            error: None,
            packing: false,
            pack_progress: 0.0,
            pack_status: String::new(),
            scan_rx: None,
            pack_rx: None,
            ready: false,
            search_query: String::new(),
            include_pattern: String::new(),
            exclude_pattern: String::new(),
        }
    }
}

impl SnifferApp {
    fn start_scan(&mut self) {
        let folder = self.folder.clone().unwrap();
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        self.scanning = true;
        self.scan_progress = 0.0;
        self.scan_status = "Scanning...".to_string();
        self.mods.clear();
        self.checked.clear();
        self.icons.clear();
        self.error = None;
        self.ready = false;
        self.search_query.clear();
        self.include_pattern.clear();
        self.exclude_pattern.clear();

        thread::spawn(move || {
            let jars: Vec<PathBuf> = WalkDir::new(&folder)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|e| e.to_str()) == Some("jar"))
                .map(|e| e.path().to_path_buf())
                .collect();

            let total = jars.len();
            if total == 0 {
                let _ = tx.send(ScanMsg::Done(Vec::new()));
                return;
            }

            let num_threads = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4);
            let chunk_size = total.div_ceil(num_threads);
            let mutex_results = std::sync::Mutex::new(Vec::with_capacity(total));
            let processed = std::sync::atomic::AtomicUsize::new(0);
            let result_tx = tx.clone();

            let work: Vec<(Vec<PathBuf>, mpsc::Sender<ScanMsg>)> = jars
                .chunks(chunk_size)
                .map(|c| (c.to_vec(), tx.clone()))
                .collect();

            std::thread::scope(|s| {
                let results = &mutex_results;
                let processed = &processed;
                for (chunk, sender) in work {
                    s.spawn(move || {
                        let mut local = Vec::new();
                        for path in &chunk {
                            let parsed = parse_jar(path);
                            for (info, icon_data) in parsed {
                                let icon_path = icon_data.as_ref().map(|d| d.path_in_jar.clone());
                                local.push(ScannedMod {
                                    info,
                                    file_path: path.clone(),
                                    icon_bytes: icon_data.map(|d| d.bytes),
                                    icon_path,
                                });
                            }
                            let p =
                                processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            let _ = sender.send(ScanMsg::Progress { current: p, total });
                        }
                        results.lock().unwrap().extend(local);
                    });
                }
            });

            drop(tx);
            let final_results = mutex_results.into_inner().unwrap();
            let _ = result_tx.send(ScanMsg::Done(final_results));
        });
    }

    fn poll_scan(&mut self) -> bool {
        let mut just_finished = false;
        if let Some(rx) = &self.scan_rx {
            let msgs: Vec<ScanMsg> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            for msg in msgs {
                match msg {
                    ScanMsg::Progress { current, total } => {
                        self.scan_progress = current as f32 / total as f32;
                        self.scan_status = format!("Scanning... {}/{}", current, total);
                    }
                    ScanMsg::Done(mods) => {
                        self.mods = mods;
                        self.scanning = false;
                        self.checked = vec![true; self.mods.len()];
                        self.ready = true;
                        self.scan_status = format!("Found {} mods", self.mods.len());
                        self.scan_rx = None;
                        just_finished = true;
                    }
                    ScanMsg::Error(e) => {
                        self.error = Some(e);
                        self.scanning = false;
                        self.scan_rx = None;
                    }
                }
            }
        }
        just_finished
    }

    fn load_icons(&mut self, ctx: &egui::Context) {
        self.icons = self
            .mods
            .iter()
            .map(|sm| {
                sm.icon_bytes.as_ref().and_then(|bytes| {
                    let img = image::load_from_memory(bytes).ok()?;
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(
                        [w as usize, h as usize],
                        &rgba,
                    );
                    Some(ctx.load_texture("icon", color_image, egui::TextureOptions::LINEAR))
                })
            })
            .collect();
    }

    fn start_pack(&mut self, save_path: PathBuf) {
        let selected: Vec<ScannedMod> = self
            .mods
            .iter()
            .enumerate()
            .filter(|&(i, _)| self.checked[i])
            .map(|(_, sm)| sm.clone())
            .collect();

        if selected.is_empty() {
            self.error = Some("No mods selected".to_string());
            return;
        }

        let format = self.output_format.clone();
        let (tx, rx) = mpsc::channel();
        self.pack_rx = Some(rx);
        self.packing = true;
        self.pack_progress = 0.0;
        self.pack_status = "Packing...".to_string();

        thread::spawn(move || {
            let result = match format.as_str() {
                "zip" => create_zip_archive(&save_path, &selected, &tx),
                "tar.gz" => create_tar_gz_archive(&save_path, &selected, &tx),
                "tar.xz" => create_tar_xz_archive(&save_path, &selected, &tx),
                _ => Err("Unknown format".into()),
            };
            match result {
                Ok(()) => {
                    let _ = tx.send(PackMsg::Done);
                }
                Err(e) => {
                    let _ = tx.send(PackMsg::Error(e.to_string()));
                }
            }
        });
    }

    fn poll_pack(&mut self) {
        if let Some(rx) = &self.pack_rx {
            let msgs: Vec<PackMsg> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            for msg in msgs {
                match msg {
                    PackMsg::Progress { current, total } => {
                        self.pack_progress = current as f32 / total as f32;
                        self.pack_status = format!("Packing... {}/{}", current, total);
                    }
                    PackMsg::Done => {
                        self.packing = false;
                        self.pack_status = "Done!".to_string();
                        self.pack_rx = None;
                    }
                    PackMsg::Error(e) => {
                        self.error = Some(e);
                        self.packing = false;
                        self.pack_rx = None;
                    }
                }
            }
        }
    }

    fn matches_filter(&self, sm: &ScannedMod) -> bool {
        if !self.search_query.is_empty() {
            let q = self.search_query.to_lowercase();
            let fields = [
                sm.info.filename.to_lowercase(),
                sm.info.name.clone().unwrap_or_default().to_lowercase(),
                sm.info.mod_id.clone().unwrap_or_default().to_lowercase(),
                sm.info.authors.join(", ").to_lowercase(),
            ];
            if !fields.iter().any(|f| f.contains(&q)) {
                return false;
            }
        }
        if !self.include_pattern.is_empty()
            && let Ok(re) = regex::Regex::new(&self.include_pattern)
        {
            let fields = [
                &sm.info.filename,
                sm.info.name.as_deref().unwrap_or(""),
                sm.info.mod_id.as_deref().unwrap_or(""),
                &sm.info.authors.join(", "),
            ];
            if !fields.iter().any(|f| re.is_match(f)) {
                return false;
            }
        }
        if !self.exclude_pattern.is_empty()
            && let Ok(re) = regex::Regex::new(&self.exclude_pattern)
        {
            let fields = [
                &sm.info.filename,
                sm.info.name.as_deref().unwrap_or(""),
                sm.info.mod_id.as_deref().unwrap_or(""),
                &sm.info.authors.join(", "),
            ];
            if fields.iter().any(|f| re.is_match(f)) {
                return false;
            }
        }
        true
    }

    fn show_save_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_file_name(format!("mods.{}", self.output_format));
        match self.output_format.as_str() {
            "zip" => {
                dialog = dialog.add_filter("ZIP", &["zip"]);
            }
            "tar.gz" => {
                dialog = dialog.add_filter("TAR.GZ", &["tar.gz"]);
            }
            "tar.xz" => {
                dialog = dialog.add_filter("TAR.XZ", &["tar.xz"]);
            }
            _ => {}
        }
        if let Some(path) = dialog.save_file() {
            self.start_pack(path);
        }
    }
}

fn create_zip_archive(
    path: &Path,
    mods: &[ScannedMod],
    tx: &mpsc::Sender<PackMsg>,
) -> Result<(), Box<dyn std::error::Error>> {
    use zip::write::SimpleFileOptions;
    let file = fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let total = mods.len();
    for (i, sm) in mods.iter().enumerate() {
        let data = fs::read(&sm.file_path)?;
        let name = sm
            .file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown.jar");
        zip.start_file(name, SimpleFileOptions::default())?;
        zip.write_all(&data)?;
        let _ = tx.send(PackMsg::Progress {
            current: i + 1,
            total,
        });
    }
    zip.finish()?;
    Ok(())
}

fn create_tar_gz_archive(
    path: &Path,
    mods: &[ScannedMod],
    tx: &mpsc::Sender<PackMsg>,
) -> Result<(), Box<dyn std::error::Error>> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let file = fs::File::create(path)?;
    let enc = GzEncoder::new(file, Compression::default());
    let mut tar = tar::Builder::new(enc);
    let total = mods.len();
    for (i, sm) in mods.iter().enumerate() {
        let data = fs::read(&sm.file_path)?;
        let name = sm
            .file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown.jar");
        let mut header = tar::Header::new_old();
        header.set_size(data.len() as u64);
        header.set_cksum();
        tar.append_data(&mut header, name, data.as_slice())?;
        let _ = tx.send(PackMsg::Progress {
            current: i + 1,
            total,
        });
    }
    let enc = tar.into_inner()?;
    enc.finish()?;
    Ok(())
}

fn create_tar_xz_archive(
    path: &Path,
    mods: &[ScannedMod],
    tx: &mpsc::Sender<PackMsg>,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = fs::File::create(path)?;
    let enc = xz2::write::XzEncoder::new(file, 6);
    let mut tar = tar::Builder::new(enc);
    let total = mods.len();
    for (i, sm) in mods.iter().enumerate() {
        let data = fs::read(&sm.file_path)?;
        let name = sm
            .file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown.jar");
        let mut header = tar::Header::new_old();
        header.set_size(data.len() as u64);
        header.set_cksum();
        tar.append_data(&mut header, name, data.as_slice())?;
        let _ = tx.send(PackMsg::Progress {
            current: i + 1,
            total,
        });
    }
    let enc = tar.into_inner()?;
    enc.finish()?;
    Ok(())
}

impl eframe::App for SnifferApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let just_scanned = self.poll_scan();
        if just_scanned {
            self.load_icons(ctx);
        }
        self.poll_pack();

        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open folder").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.folder = Some(path);
                            self.start_scan();
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Selection", |ui| {
                    ui.add_enabled_ui(self.ready, |ui| {
                        if ui.button("Select All").clicked() {
                            for c in &mut self.checked {
                                *c = true;
                            }
                            ui.close_menu();
                        }
                        if ui.button("Deselect All").clicked() {
                            for c in &mut self.checked {
                                *c = false;
                            }
                            ui.close_menu();
                        }
                        if ui.button("Invert Selection").clicked() {
                            for c in &mut self.checked {
                                *c = !*c;
                            }
                            ui.close_menu();
                        }
                    });
                });
                ui.menu_button("Export", |ui| {
                    ui.add_enabled_ui(self.ready && !self.packing, |ui| {
                        ui.label("Format:");
                        egui::ComboBox::from_id_source("format")
                            .selected_text(&self.output_format)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.output_format,
                                    "zip".to_string(),
                                    "ZIP",
                                );
                                ui.selectable_value(
                                    &mut self.output_format,
                                    "tar.gz".to_string(),
                                    "TAR.GZ",
                                );
                                ui.selectable_value(
                                    &mut self.output_format,
                                    "tar.xz".to_string(),
                                    "TAR.XZ",
                                );
                            });
                        ui.separator();
                        if ui.button("Save As...").clicked() {
                            self.show_save_dialog();
                            ui.close_menu();
                        }
                    });
                });

            });
        });

        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame {
                inner_margin: egui::Margin::symmetric(6.0, 2.0),
                ..Default::default()
            })
            .show(ctx, |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if let Some(ref err) = self.error {
                    ui.colored_label(egui::Color32::RED, err);
                } else if self.scanning {
                    ui.label(&self.scan_status);
                    let (pb_rect, _) = ui.allocate_exact_size(
                        egui::vec2(120.0, 3.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(pb_rect, 1.0, egui::Color32::DARK_GRAY);
                    let fill_w = pb_rect.width() * self.scan_progress;
                    let fill_rect = egui::Rect::from_min_size(
                        pb_rect.min,
                        egui::vec2(fill_w, pb_rect.height()),
                    );
                    ui.painter().rect_filled(fill_rect, 1.0, egui::Color32::GREEN);
                } else if self.packing {
                    ui.label(&self.pack_status);
                    let (pb_rect, _) = ui.allocate_exact_size(
                        egui::vec2(120.0, 3.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(pb_rect, 1.0, egui::Color32::DARK_GRAY);
                    let fill_w = pb_rect.width() * self.pack_progress;
                    let fill_rect = egui::Rect::from_min_size(
                        pb_rect.min,
                        egui::vec2(fill_w, pb_rect.height()),
                    );
                    ui.painter().rect_filled(fill_rect, 1.0, egui::Color32::GREEN);
                } else if self.ready {
                    let shown: Vec<usize> = (0..self.mods.len())
                        .filter(|&i| self.matches_filter(&self.mods[i]))
                        .collect();
                    let sel = shown.iter().filter(|&&i| self.checked[i]).count();
                    ui.label(format!("Selected: {}/{}", sel, shown.len()));
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.folder.is_none() {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.4);
                    ui.heading("Sniffer");
                    ui.add_space(8.0);
                    ui.label("Use File > Open folder to get started");
                });
            } else if self.scanning {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.4);
                    ui.label("Scanning mods...");
                });
            } else if self.packing {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.4);
                    ui.label("Packing mods...");
                });
            } else if self.ready {
                let filtered_indices: Vec<usize> = (0..self.mods.len())
                    .filter(|&i| self.matches_filter(&self.mods[i]))
                    .collect();

                let _shown_count = filtered_indices.len();
                let _sel_shown = filtered_indices.iter().filter(|&&i| self.checked[i]).count();

                ui.horizontal(|ui| {
                    ui.label("\u{1F50D}");
                    ui.add(egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text("Search...")
                        .desired_width(140.0));
                    ui.label("Inc:");
                    ui.add(egui::TextEdit::singleline(&mut self.include_pattern)
                        .hint_text("include regex")
                        .desired_width(100.0));
                    ui.label("Exc:");
                    ui.add(egui::TextEdit::singleline(&mut self.exclude_pattern)
                        .hint_text("exclude regex")
                        .desired_width(100.0));
                });

                ui.add_space(2.0);
                ui.separator();

                let row_height = 64.0;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.style_mut().wrap = Some(false);
                        for &i in &filtered_indices {
                            let checked = &mut self.checked[i];
                            let icon = &self.icons[i];
                            let sm = &self.mods[i];
                            ui.add_sized(
                                egui::vec2(ui.available_width(), row_height),
                                |ui: &mut egui::Ui| {
                                    ui.horizontal(|ui| {
                                        ui.checkbox(checked, "");
                                        if let Some(handle) = icon {
                                            ui.add(
                                                egui::Image::new(handle)
                                                    .max_width(64.0)
                                                    .max_height(64.0),
                                            );
                                        } else {
                                            ui.add_space(64.0);
                                        }
                                        ui.vertical(|ui| {
                                            if let Some(ref name) = sm.info.name {
                                                ui.strong(name);
                                            } else {
                                                ui.strong(&sm.info.filename);
                                            }
                                            ui.label(&sm.info.filename);
                                            if let Some(ref id) = sm.info.mod_id {
                                                ui.label(format!("ID: {}", id));
                                            }
                                            if !sm.info.authors.is_empty() {
                                                ui.label(format!(
                                                    "Authors: {}",
                                                    sm.info.authors.join(", ")
                                                ));
                                            }
                                        });
                                    }).response
                                },
                            );
                            ui.separator();
                        }
                    });
            }
        });

        if self.scanning || self.packing {
            ctx.request_repaint();
        }
    }
}

fn main() {
    let options = eframe::NativeOptions::default();
    if let Err(e) = eframe::run_native(
        "Sniffer",
        options,
        Box::new(|_cc| Box::new(SnifferApp::default())),
    ) {
        eprintln!("Error: {}", e);
    }
}
