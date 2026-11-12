//! AI model registry: where every model lives, where it is downloaded from (links are
//! EDITABLE because release links expire), plus backup / restore of the whole model folder.
//!
//! Folder layout is the same as the Python "Ripon Computer" app:
//!   %LOCALAPPDATA%\RiponComputer\models\{rembg, gfpgan, realesrgan, yunet, document}\...
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub purpose: String,
    /// sub folder inside `models/`
    pub folder: String,
    pub file: String,
    /// tried in order - edit / add new links here when one expires
    pub urls: Vec<String>,
    pub min_bytes: u64,
    pub size_mb: u32,
}

const FE: &str = "https://github.com/facefusion/facefusion-assets/releases/download/models-3.0.0";
const RG: &str = "https://github.com/danielgatis/rembg/releases/download/v0.0.0";

#[allow(clippy::too_many_arguments)]
fn e(id: &str, label: &str, purpose: &str, folder: &str, file: &str, urls: Vec<String>, min_mb: u64, size_mb: u32) -> Entry {
    Entry { id: id.into(), label: label.into(), purpose: purpose.into(), folder: folder.into(), file: file.into(), urls, min_bytes: min_mb * 1_000_000, size_mb }
}

pub fn defaults() -> Vec<Entry> {
    vec![
        e("u2net_human_seg", "U2Net human (background, best for people)", "Background removal", "rembg", "u2net_human_seg.onnx", vec![format!("{RG}/u2net_human_seg.onnx")], 100, 168),
        e("isnet_general", "IS-Net general (background, sharp hair & edges)", "Background removal", "rembg", "isnet-general-use.onnx", vec![format!("{RG}/isnet-general-use.onnx")], 100, 170),
        e("span_x4", "SPAN x4 (fast upscale, recommended)", "Upscale x4", "realesrgan", "span_kendata_x4.onnx", vec![format!("{FE}/span_kendata_x4.onnx")], 1, 2),
        e("clear_reality_x4", "ClearReality x4 (fast upscale, photos)", "Upscale x4", "realesrgan", "clear_reality_x4.onnx", vec![format!("{FE}/clear_reality_x4.onnx")], 1, 2),
        e("ultra_sharp_x4", "UltraSharp x4 (sharp upscale)", "Upscale x4", "realesrgan", "ultra_sharp_x4.onnx", vec![format!("{FE}/ultra_sharp_x4.onnx")], 60, 67),
        e("real_esrgan_x4", "Real-ESRGAN x4 (best quality, slow)", "Upscale x4", "realesrgan", "real_esrgan_x4.onnx", vec![format!("{FE}/real_esrgan_x4.onnx")], 60, 70),
        e("real_esrgan_x2", "Real-ESRGAN x2 (quality, slow)", "Upscale x2", "realesrgan", "real_esrgan_x2.onnx", vec![format!("{FE}/real_esrgan_x2.onnx")], 60, 70),
        e("gfpgan_1.4", "GFPGAN 1.4 (face restore)", "Face restore", "gfpgan", "gfpgan_1.4.onnx", vec![format!("{FE}/gfpgan_1.4.onnx")], 300, 340),
        e("gfpgan_1.2", "GFPGAN 1.2 (face restore, softer)", "Face restore", "gfpgan", "gfpgan_1.2.onnx", vec![format!("{FE}/gfpgan_1.2.onnx")], 300, 340),
        e("codeformer", "CodeFormer (face restore, strong)", "Face restore", "gfpgan", "codeformer.onnx", vec![format!("{FE}/codeformer.onnx")], 360, 377),
    ]
}

/// %LOCALAPPDATA%\RiponComputer  (same root as the Python app)
pub fn app_dir() -> PathBuf {
    let home = std::env::var_os("RIPON_STUDIO_HOME").map(PathBuf::from);
    let p = match home {
        Some(h) => h,
        None => {
            let base = if cfg!(windows) {
                std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            } else {
                std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            };
            base.unwrap_or_else(|| PathBuf::from(".")).join("RiponComputer")
        }
    };
    ensure_dirs(&p);
    p
}

pub fn ensure_dirs(root: &Path) {
    for s in ["rembg", "gfpgan", "realesrgan", "realesrgan-ncnn", "yunet", "document"] {
        let _ = std::fs::create_dir_all(root.join("models").join(s));
    }
}

pub fn models_dir() -> PathBuf {
    app_dir().join("models")
}
fn registry_path() -> PathBuf {
    app_dir().join("models.json")
}

/// Defaults merged with the user's saved links (user links win; user-added models are kept).
pub fn merge(saved: Vec<Entry>) -> Vec<Entry> {
    let mut out = defaults();
    for s in saved {
        if s.id == "silueta" {
            continue; // removed model
        }
        if let Some(d) = out.iter_mut().find(|d| d.id == s.id) {
            if !s.urls.is_empty() {
                d.urls = s.urls;
            }
        } else {
            out.push(s);
        }
    }
    out
}

static CACHE: std::sync::Mutex<Option<Vec<Entry>>> = std::sync::Mutex::new(None);

pub fn registry() -> Vec<Entry> {
    let mut g = CACHE.lock().unwrap();
    if let Some(c) = g.as_ref() {
        return c.clone();
    }
    let saved: Vec<Entry> = std::fs::read_to_string(registry_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let r = merge(saved);
    *g = Some(r.clone());
    r
}

pub fn reload_registry() {
    *CACHE.lock().unwrap() = None;
}

pub fn save_registry(r: &[Entry]) -> Result<(), String> {
    let j = serde_json::to_string_pretty(r).map_err(|e| e.to_string())?;
    std::fs::write(registry_path(), j).map_err(|e| e.to_string())?;
    reload_registry();
    Ok(())
}

pub fn entry(id: &str) -> Option<Entry> {
    registry().into_iter().find(|m| m.id == id)
}

pub fn dest_path(en: &Entry) -> PathBuf {
    models_dir().join(&en.folder).join(&en.file)
}

/// Existing, complete model file (also accepts the old flat `models/<file>` location).
pub fn find(id: &str) -> Option<PathBuf> {
    let en = entry(id)?;
    let ok = |p: &PathBuf| std::fs::metadata(p).map(|m| m.len() >= en.min_bytes).unwrap_or(false);
    let a = dest_path(&en);
    if ok(&a) {
        return Some(a);
    }
    let b = models_dir().join(&en.file);
    if ok(&b) {
        return Some(b);
    }
    None
}

pub fn is_installed(id: &str) -> bool {
    find(id).is_some()
}

pub fn client() -> ureq::Agent {
    ureq::Agent::config_builder().proxy(ureq::Proxy::try_from_env()).tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build()).timeout_connect(Some(std::time::Duration::from_secs(20))).build().into()
}

fn fetch(url: &str, dest: &Path, min_bytes: u64, size_hint: u64, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<(), String> {
    let mut resp = client().get(url).call().map_err(|er| er.to_string())?;
    let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(size_hint);
    let part = dest.with_extension("part");
    if let Some(d) = dest.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let mut rd = resp.body_mut().as_reader();
    let mut f = std::fs::File::create(&part).map_err(|er| er.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(f);
            let _ = std::fs::remove_file(&part);
            return Err("Cancelled".into());
        }
        let n = rd.read(&mut buf).map_err(|er| format!("Connection lost: {er}"))?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|er| er.to_string())?;
        done += n as u64;
        progress(done, total);
    }
    drop(f);
    if done < min_bytes {
        let _ = std::fs::remove_file(&part);
        return Err(format!("file too small ({} bytes) - the link probably returns an error page", done));
    }
    std::fs::rename(&part, dest).map_err(|er| er.to_string())
}

/// Try every link of the model in order. The error lists every failure and the manual folder.
pub fn download(id: &str, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<(), String> {
    let en = entry(id).ok_or("Unknown model")?;
    let dest = dest_path(&en);
    let mut errs = Vec::new();
    for u in en.urls.iter().filter(|u| !u.trim().is_empty()) {
        match fetch(u.trim(), &dest, en.min_bytes, en.size_mb as u64 * 1_000_000, progress, cancel) {
            Ok(()) => return Ok(()),
            Err(er) if er == "Cancelled" => return Err(er),
            Err(er) => errs.push(format!("- {u}\n   {er}")),
        }
    }
    Err(format!(
        "Could not download \"{}\".\n{}\n\nFix: open AI Models, edit the link (links expire), or download the file by hand and press Import.\nFolder: {}",
        en.label,
        if errs.is_empty() { "No link is set.".to_string() } else { errs.join("\n") },
        dest.parent().map(|p| p.display().to_string()).unwrap_or_default()
    ))
}

/// Copy a model file the user downloaded by hand into the right folder.
pub fn import(id: &str, src: &Path) -> Result<(), String> {
    let en = entry(id).ok_or("Unknown model")?;
    let len = std::fs::metadata(src).map_err(|er| er.to_string())?.len();
    if len < en.min_bytes {
        return Err(format!("That file is only {:.1} MB - too small for {}.", len as f32 / 1e6, en.label));
    }
    let dest = dest_path(&en);
    std::fs::copy(src, &dest).map(|_| ()).map_err(|er| er.to_string())
}

pub fn delete(id: &str) {
    if let Some(en) = entry(id) {
        let _ = std::fs::remove_file(dest_path(&en));
        let _ = std::fs::remove_file(models_dir().join(&en.file));
    }
}

// ---------------------------------------------------------------- backup / restore
/// Zip `root/models/**` and `root/models.json` (stored, models are already compressed).
pub fn pack(root: &Path, zip_path: &Path) -> Result<usize, String> {
    let f = std::fs::File::create(zip_path).map_err(|er| er.to_string())?;
    let mut z = zip::ZipWriter::new(f);
    let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    let mut n = 0;
    let mut stack = vec![root.join("models")];
    let mut files = Vec::new();
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().map(|x| x != "part").unwrap_or(true) {
                files.push(p);
            }
        }
    }
    let reg = root.join("models.json");
    if reg.exists() {
        files.push(reg);
    }
    for p in files {
        let rel = p.strip_prefix(root).map_err(|er| er.to_string())?.to_string_lossy().replace('\\', "/");
        z.start_file(rel, opt).map_err(|er| er.to_string())?;
        let mut src = std::fs::File::open(&p).map_err(|er| er.to_string())?;
        std::io::copy(&mut src, &mut z).map_err(|er| er.to_string())?;
        n += 1;
    }
    z.finish().map_err(|er| er.to_string())?;
    Ok(n)
}

/// Restore a backup into `root`. Only `models/...` and `models.json` are accepted, paths that try
/// to leave the folder are ignored.
pub fn unpack(zip_path: &Path, root: &Path) -> Result<usize, String> {
    let f = std::fs::File::open(zip_path).map_err(|er| er.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|er| format!("Not a valid backup zip: {er}"))?;
    let mut n = 0;
    for i in 0..z.len() {
        let mut ent = z.by_index(i).map_err(|er| er.to_string())?;
        if ent.is_dir() {
            continue;
        }
        let Some(rel) = ent.enclosed_name() else { continue };
        let ok = rel == Path::new("models.json") || rel.starts_with("models");
        if !ok {
            continue;
        }
        let out = root.join(&rel);
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(|er| er.to_string())?;
        }
        let mut w = std::fs::File::create(&out).map_err(|er| er.to_string())?;
        std::io::copy(&mut ent, &mut w).map_err(|er| er.to_string())?;
        n += 1;
    }
    Ok(n)
}

pub fn backup(zip_path: &Path) -> Result<usize, String> {
    pack(&app_dir(), zip_path)
}
pub fn restore(zip_path: &Path) -> Result<usize, String> {
    let n = unpack(zip_path, &app_dir());
    reload_registry();
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ripon_test_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn backup_restore_roundtrip() {
        let a = tmp("a");
        let b = tmp("b");
        ensure_dirs(&a);
        std::fs::write(a.join("models/gfpgan/x.onnx"), b"hello model").unwrap();
        std::fs::write(a.join("models/rembg/y.onnx"), vec![7u8; 5000]).unwrap();
        std::fs::write(a.join("models.json"), b"[]").unwrap();
        std::fs::write(a.join("settings.json"), b"{}").unwrap();
        let z = a.join("bk.zip");
        let n = pack(&a, &z).unwrap();
        assert_eq!(n, 3); // settings.json is not part of the backup
        let m = unpack(&z, &b).unwrap();
        assert_eq!(m, 3);
        assert_eq!(std::fs::read(b.join("models/gfpgan/x.onnx")).unwrap(), b"hello model");
        assert_eq!(std::fs::read(b.join("models/rembg/y.onnx")).unwrap().len(), 5000);
        assert!(!b.join("settings.json").exists());
    }

    #[test]
    fn restore_ignores_unsafe_paths() {
        let a = tmp("c");
        let z = a.join("evil.zip");
        {
            let f = std::fs::File::create(&z).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("../escape.txt", o).unwrap();
            w.write_all(b"x").unwrap();
            w.start_file("other/file.txt", o).unwrap();
            w.write_all(b"x").unwrap();
            w.start_file("models/ok.onnx", o).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let out = a.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let n = unpack(&z, &out).unwrap();
        assert_eq!(n, 1);
        assert!(out.join("models/ok.onnx").exists());
        assert!(!a.join("escape.txt").exists());
    }

    #[test]
    fn user_links_override_defaults() {
        let mut d = defaults();
        d[0].urls = vec!["https://example.com/new.onnx".into()];
        let m = merge(vec![d[0].clone()]);
        assert_eq!(m[0].urls[0], "https://example.com/new.onnx");
        assert_eq!(m.len(), defaults().len());
    }
}
