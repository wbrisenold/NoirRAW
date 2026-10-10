use std::{cell::RefCell, fs, path::PathBuf, rc::Rc, sync::mpsc, time::Duration};
use slint::{ComponentHandle, ModelRc, VecModel, Image, SharedString};
use noirraw::{image_io::{self, SceneImage}, lut::{self, Cube, LutEntry, InputSpace}, gpu::{GpuEngine, Controls}, library::{LibraryScanner, ScanOptions, ThumbnailGenerator, ThumbnailCache, LibraryImage, LibraryDb, ThumbnailData}};
use slint::wgpu_30::wgpu;
slint::include_modules!();

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Preferences { lut_folder: Option<PathBuf> }

fn prefs_path() -> Option<PathBuf> { dirs::config_dir().map(|x| x.join("NoirRAW").join("preferences.json")) }
fn load_prefs() -> Preferences { prefs_path().and_then(|p| fs::read_to_string(p).ok()).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default() }
fn save_prefs(p: &Preferences) { if let Some(path) = prefs_path() { if let Some(dir) = path.parent() { let _ = fs::create_dir_all(dir); } if let Ok(data) = serde_json::to_vec_pretty(p) { let _ = fs::write(path, data); } } }

#[derive(Default)]
struct State {
    source_name: String,
    photo: Option<SceneImage>,
    proxy: Option<SceneImage>,
    revision: u64,
    load_ticket: u64,
    entries: Vec<LutEntry>,
    lut_folder: Option<PathBuf>,
    selected: Option<usize>,
    loaded: Option<Cube>,
    controls: Controls,
    dirty: bool,
    library_images: Vec<LibraryImage>,
    thumbnails: Vec<ThumbnailData>,
    selected_index: i32,
    active_index: i32,
    library_folder: String,
}

fn chosen<'a>(s: &'a State) -> Option<(&'a PathBuf, &'a Cube, InputSpace)> {
    s.selected.and_then(|i| s.entries.get(i)).zip(s.loaded.as_ref()).map(|(e, c)| (&e.path, c, e.input))
}

fn labels(entries: &[LutEntry]) -> Vec<String> {
    let mut labels = vec!["No LUT — native color".to_string()];
    for (i, e) in entries.iter().enumerate() {
        labels.push(format!("{:04} · {} · {}", i + 1, e.label, e.input.name()));
    }
    labels
}

fn update_list(app: &MainWindow, s: &State) {
    let strings = labels(&s.entries);
    app.set_lut_labels(ModelRc::new(VecModel::from(strings.into_iter().map(Into::into).collect::<Vec<slint::SharedString>>())));
    let index = s.selected.map(|i| i + 1).unwrap_or(0);
    app.set_selected_lut(labels(&s.entries).get(index).cloned().unwrap_or_default().into());
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|a| a == "--smoke-gpu") { return noirraw::gpu::smoke_gpu().map_err(Into::into); }

    let mut wgpu_settings = slint::wgpu_30::WGPUSettings::default();
    wgpu_settings.device_required_limits = slint::wgpu_30::wgpu::Limits::default();
    slint::BackendSelector::new().require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Automatic(wgpu_settings)).select()?;

    let app = MainWindow::new()?;
    let state = Rc::new(RefCell::new(State::default()));

    let db_path = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("NoirRAW").join("library.db");
    let _ = fs::create_dir_all(db_path.parent().unwrap());
    let db = LibraryDb::open(&db_path).expect("Failed to open library DB");
    let scanner = LibraryScanner::new(db, ScanOptions::default());
    let thumb_gen = ThumbnailGenerator::default();
    let thumb_cache = ThumbnailCache::new(thumb_gen);

    {
        let mut s = state.borrow_mut();
        let prefs = load_prefs();
        s.lut_folder = prefs.lut_folder;
        if let Some(dir) = &s.lut_folder {
            s.entries = lut::index_folder(dir).unwrap_or_default();
        }
        update_list(&app, &s);
    }

    let gpu = Rc::new(RefCell::new(None::<GpuEngine>));

    let (decoded_tx, decoded_rx) = mpsc::channel::<(u64, PathBuf, Result<SceneImage, String>)>();
    let decode_poller = slint::Timer::default();

    {
        let state = state.clone();
        let weak = app.as_weak();
        decode_poller.start(slint::TimerMode::Repeated, Duration::from_millis(70), move || {
            while let Ok((ticket, path, result)) = decoded_rx.try_recv() {
                let Some(app) = weak.upgrade() else { continue };
                let mut s = state.borrow_mut();
                if ticket != s.load_ticket { continue }
                match result {
                    Ok(photo) => {
                        let proxy = photo.proxy(1080);
                        let (w, h) = (photo.width, photo.height);
                        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                        s.revision += 1;
                        s.photo = Some(photo);
                        s.proxy = Some(proxy);
                        s.source_name = name.clone();
                        s.dirty = true;
                        app.set_loupe_name(name.into());
                        app.set_working_status(format!("{} × {} source loaded · 1080px GPU working image", w, h).into());
                        app.window().request_redraw();
                    }
                    Err(e) => app.set_working_status(format!("Decode failed: {e}").into())
                }
            }
        });
    }

    {
        let weak = app.as_weak();
        let state = state.clone();
        let engine = gpu.clone();
        app.window().set_rendering_notifier(move |phase, api| {
            let Some(app) = weak.upgrade() else { return };
            let slint::GraphicsAPI::WGPU30 { device, queue, .. } = api else { return };
            match phase {
                slint::RenderingState::RenderingSetup => {
                    *engine.borrow_mut() = Some(GpuEngine::new(device.clone(), queue.clone()));
                    app.set_gpu_status("Metal/wgpu ready".into());
                }
                slint::RenderingState::BeforeRendering => {
                    let mut s = state.borrow_mut();
                    if !s.dirty { return; }
                    let Some(img) = s.proxy.as_ref() else { return };
                    let mut binding = engine.borrow_mut();
                    let Some(gpu) = binding.as_mut() else { return };
                    let controls = s.controls;
                    let rev = s.revision;
                    let lut = chosen(&s);
                    let (texture, changed) = gpu.preview(img, rev, lut, controls);
                    if changed {
                        match slint::Image::try_from(texture) {
                            Ok(img) => app.set_loupe_image(img),
                            Err(e) => app.set_working_status(format!("GPU display: {e:?}").into())
                        }
                    }
                    app.set_gpu_status(format!("GPU frames {} · scene uploads {} · LUT uploads {} · selected {}",
                        gpu.render_count, gpu.scene_upload_count, gpu.lut_upload_count,
                        s.selected.and_then(|i| s.entries.get(i)).map(|x| x.label.as_str()).unwrap_or("none")).into());
                    s.dirty = false;
                }
                slint::RenderingState::RenderingTeardown => { *engine.borrow_mut() = None; }
                _ => {}
            }
        })?;
    }

    // Open folder callback
    {
        let state = state.clone();
        let weak = app.as_weak();
        let scanner = scanner.clone();
        let thumb_cache = thumb_cache.clone();
        app.on_open_folder(move || {
            let Some(folder) = rfd::FileDialog::new().pick_folder() else { return };
            if let Some(app) = weak.upgrade() {
                app.set_working_status("Scanning folder…".into());
                let state = state.clone();
                let weak = app.as_weak();
                let scanner = scanner.clone();
                let thumb_cache = thumb_cache.clone();
                std::thread::spawn(move || {
                    let result = scanner.scan_folder(&folder);
                    let state = state.clone();
                    let weak = weak.clone();
                    match result {
                        Ok(_stats) => {
                            let mut s = state.borrow_mut();
                            let images = scanner.db.list_images(1000, 0).unwrap_or_default();
                            let mut thumbs = Vec::new();
                            for img in images.iter() {
                                let thumb_img = thumb_cache.get_or_generate(&img.path)
                                    .unwrap_or_else(|| {
                                        // Create a minimal placeholder image (1x1 transparent)
                                        let rgba = [0u8, 0, 0, 0];
                                        Image::from_rgb8(&rgba, 1, 1).unwrap()
                                    });
                                thumbs.push(ThumbnailData {
                                    path: img.path.to_string_lossy().to_string(),
                                    name: img.filename.clone(),
                                    rating: img.rating as i32,
                                    color_label: img.color_label as i32,
                                    pick_reject: img.pick_reject as i32,
                                    thumbnail: thumb_img,
                                    width: img.width as i32,
                                    height: img.height as i32,
                                    has_raw: img.has_raw,
                                    is_selected: false,
                                    is_active: false,
                                });
                            }
                            s.thumbnails = thumbs;
                            s.library_images = images.iter().cloned().collect();
                            s.library_folder = folder.to_string_lossy().to_string();
                            s.selected_index = -1;
                            s.active_index = -1;

                            if let Some(app) = weak.upgrade() {
                                app.set_thumbnails(ModelRc::new(VecModel::from(s.thumbnails.clone().into_iter().map(Into::into).collect::<Vec<_>>())));
                                app.set_library_folder(s.library_folder.clone().into());
                                app.set_working_status(format!("Loaded {} images", s.thumbnails.len()).into());
                                app.window().request_redraw();
                            }
                        }
                        Err(e) => {
                            if let Some(app) = weak.upgrade() {
                                app.set_working_status(format!("Scan failed: {e}").into());
                            }
                        }
                    }
                });
            }
        });
    }

    // Select thumbnail
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_select_thumbnail(move |index| {
            if let Some(app) = weak.upgrade() {
                let mut s = state.borrow_mut();
                s.selected_index = index;
                if index >= 0 && index < s.thumbnails.len() as i32 {
                    s.thumbnails[index as usize].is_selected = true;
                    s.active_index = index;
                    let thumb = &s.thumbnails[index as usize];
                    app.set_loupe_name(thumb.name.clone().into());
                    app.set_loupe_info(format!("{} × {}", thumb.width, thumb.height).into());
                }
                app.set_thumbnails(ModelRc::new(VecModel::from(s.thumbnails.clone().into_iter().map(Into::into).collect::<Vec<_>>())));
                app.window().request_redraw();
            }
        });
    }

    // Activate thumbnail (double-click)
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_activate_thumbnail(move |index| {
            if let Some(app) = weak.upgrade() {
                let mut s = state.borrow_mut();
                if index >= 0 && index < s.library_images.len() as i32 {
                    let img = &s.library_images[index as usize];
                    s.active_index = index;
                    s.selected_index = index;
                    app.set_loupe_name(img.filename.clone().into());
                    app.set_loupe_info(format!("{} × {}", img.width, img.height).into());
                    if let Ok(photo) = image_io::load(&img.path) {
                        s.photo = Some(photo.clone());
                        s.proxy = Some(photo.proxy(1080));
                        s.revision += 1;
                        s.dirty = true;
                        app.set_working_status(format!("Loaded {} × {}", photo.width, photo.height).into());
                    }
                }
                app.window().request_redraw();
            }
        });
    }

    // Open library settings
    {
        let weak = app.as_weak();
        app.on_open_library_settings(move || {
            if let Some(app) = weak.upgrade() {
                app.set_working_status("Settings not implemented yet".into());
            }
        });
    }

    // Refresh library
    {
        let state = state.clone();
        let weak = app.as_weak();
        let scanner = scanner.clone();
        let thumb_cache = thumb_cache.clone();
        app.on_refresh_library(move || {
            if let Some(app) = weak.upgrade() {
                let folder = state.borrow().library_folder.clone();
                if folder.is_empty() { return; }
                app.set_working_status("Refreshing library…".into());
                let state = state.clone();
                let weak = app.as_weak();
                let scanner = scanner.clone();
                let thumb_cache = thumb_cache.clone();
                std::thread::spawn(move || {
                    let result = scanner.scan_folder(&PathBuf::from(&folder));
                    let state = state.clone();
                    let weak = weak.clone();
                    match result {
                        Ok(_) => {
                            let mut s = state.borrow_mut();
                            let images = scanner.db.list_images(1000, 0).unwrap_or_default();
                            let mut thumbs = Vec::new();
                            for img in images.iter() {
                                let thumb_img = thumb_cache.get_or_generate(&img.path)
                                    .unwrap_or_else(|| {
                                        // Create a minimal placeholder image (1x1 transparent)
                                        let rgba = [0u8, 0, 0, 0];
                                        Image::from_rgb8(&rgba, 1, 1).unwrap()
                                    });
                                thumbs.push(ThumbnailData {
                                    path: img.path.to_string_lossy().to_string(),
                                    name: img.filename.clone(),
                                    rating: img.rating as i32,
                                    color_label: img.color_label as i32,
                                    pick_reject: img.pick_reject as i32,
                                    thumbnail: thumb_img,
                                    width: img.width as i32,
                                    height: img.height as i32,
                                    has_raw: img.has_raw,
                                    is_selected: false,
                                    is_active: false,
                                });
                            }
                            s.thumbnails = thumbs;
                            s.library_images = images.iter().cloned().collect();
                            s.selected_index = -1;
                            s.active_index = -1;

                            if let Some(app) = weak.upgrade() {
                                app.set_thumbnails(ModelRc::new(VecModel::from(s.thumbnails.clone().into_iter().map(Into::into).collect::<Vec<_>>())));
                                app.set_working_status(format!("Refreshed: {} images", s.thumbnails.len()).into());
                                app.window().request_redraw();
                            }
                        }
                        Err(e) => {
                            if let Some(app) = weak.upgrade() {
                                app.set_working_status(format!("Refresh failed: {e}").into());
                            }
                        }
                    }
                });
            }
        });
    }

    // Toggle grid/loupe
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_toggle_grid_loupe(move || {
            if let Some(app) = weak.upgrade() {
                let mut s = state.borrow_mut();
                s.dirty = true;
                app.window().request_redraw();
            }
        });
    }

    // Exposure
    {
        let s = state.clone();
        let weak = app.as_weak();
        app.on_change_exposure(move |value| {
            s.borrow_mut().controls.exposure = value;
            s.borrow_mut().dirty = true;
            if let Some(app) = weak.upgrade() { app.window().request_redraw(); }
        });
    }

    // Contrast
    {
        let s = state.clone();
        let weak = app.as_weak();
        app.on_change_contrast(move |value| {
            s.borrow_mut().controls.contrast = value;
            s.borrow_mut().dirty = true;
            if let Some(app) = weak.upgrade() { app.window().request_redraw(); }
        });
    }

    // LUT opacity
    {
        let s = state.clone();
        let weak = app.as_weak();
        app.on_change_opacity(move |value| {
            s.borrow_mut().controls.strength = value;
            s.borrow_mut().dirty = true;
            if let Some(app) = weak.upgrade() { app.window().request_redraw(); }
        });
    }

    // Select LUT - inline logic using a simpler closure approach
    {
        let state = state.clone();
        let app_weak = app.as_weak();
        app.on_select_lut(move |value| {
            let value_str = value.to_string();
            let app_weak = app_weak.clone();
            let state = state.clone();
            if let Some(app) = app_weak.upgrade() {
                let mut s = state.borrow_mut();
                let options = labels(&s.entries);
                let idx = options.iter().position(|x| x == &value_str);
                let selected = idx.and_then(|i| i.checked_sub(1));
                if selected == s.selected { return }
                match selected {
                    Some(i) => {
                        match lut::load_entry(&s.entries[i]) {
                            Ok(cube) => {
                                s.selected = Some(i);
                                s.loaded = Some(cube);
                                app.set_working_status(format!("Ready: {} — film+print replaced by imported LUT", s.entries[i].label).into());
                            }
                            Err(e) => {
                                s.selected = None;
                                s.loaded = None;
                                app.set_working_status(format!("Rejected LUT: {e}").into());
                            }
                        }
                    }
                    None => {
                        s.selected = None;
                        s.loaded = None;
                        app.set_working_status("Native color mode — no LUT".into());
                    }
                }
                s.dirty = true;
                app.window().request_redraw();
            }
        });
    }

    // Export full
    {
        let state = state.clone();
        let weak = app.as_weak();
        let engine = gpu.clone();
        app.on_export_full(move || {
            let Some(app) = weak.upgrade() else { return };
            let s = state.borrow();
            let Some(image) = s.photo.as_ref() else { app.set_working_status("Open a photo first".into()); return };
            let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name("NoirRAW-export.png").save_file() else { return };
            app.set_working_status("Exporting full original resolution through the GPU…".into());
            let controls = s.controls;
            let selected = chosen(&s);
            let mut binding = engine.borrow_mut();
            let Some(gpu) = binding.as_mut() else { app.set_working_status("GPU not available".into()); return };
            match gpu.export(image, selected, controls).and_then(|bytes| image_io::save_encoded_srgb(&path, image.width, image.height, &bytes)) {
                Ok(()) => app.set_working_status(format!("Exported {} × {} PNG · {}", image.width, image.height, path.display()).into()),
                Err(e) => app.set_working_status(format!("Export failed: {e}").into())
            }
        });
    }

    // Key pressed
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_key_pressed(move |key| {
            let mut s = state.borrow_mut();
            match key.as_str() {
                "Right" => {
                    if s.active_index + 1 < s.thumbnails.len() as i32 {
                        s.active_index += 1;
                        s.selected_index = s.active_index;
                        app.set_thumbnails(ModelRc::new(VecModel::from(s.thumbnails.clone().into_iter().map(Into::into).collect::<Vec<_>>())));
                        app.window().request_redraw();
                    }
                }
                "Left" => {
                    if s.active_index > 0 {
                        s.active_index -= 1;
                        s.selected_index = s.active_index;
                        app.set_thumbnails(ModelRc::new(VecModel::from(s.thumbnails.clone().into_iter().map(Into::into).collect::<Vec<_>>())));
                        app.window().request_redraw();
                    }
                }
                key if key >= "1" && key <= "5" => {
                    if s.active_index >= 0 && s.active_index < s.thumbnails.len() as i32 {
                        let rating = key.parse::<i32>().unwrap_or(0);
                        s.thumbnails[s.active_index as usize].rating = rating;
                        app.set_thumbnails(ModelRc::new(VecModel::from(s.thumbnails.clone().into_iter().map(Into::into).collect::<Vec<_>>())));
                        app.window().request_redraw();
                    }
                }
                "P" | "p" => {
                    if s.active_index >= 0 && s.active_index < s.thumbnails.len() as i32 {
                        s.thumbnails[s.active_index as usize].pick_reject = 1;
                        app.set_thumbnails(ModelRc::new(VecModel::from(s.thumbnails.clone().into_iter().map(Into::into).collect::<Vec<_>>())));
                        app.window().request_redraw();
                    }
                }
                "X" | "x" => {
                    if s.active_index >= 0 && s.active_index < s.thumbnails.len() as i32 {
                        s.thumbnails[s.active_index as usize].pick_reject = -1;
                        app.set_thumbnails(ModelRc::new(VecModel::from(s.thumbnails.clone().into_iter().map(Into::into).collect::<Vec<_>>())));
                        app.window().request_redraw();
                    }
                }
                _ => {}
            }
        });
    }

    app.run()?;
    Ok(())
}