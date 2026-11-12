//! Direct printing through the normal Windows print window (printer, copies, Preferences) - no PDF viewer.
//! Uses the .NET `System.Drawing.Printing` classes that ship with every Windows (via PowerShell),
//! so no extra DLL / driver code is needed. The sheet is printed at its real size (pixels / dpi).
use image::RgbImage;

#[cfg_attr(not(windows), allow(dead_code))]
const SCRIPT: &str = r#"param([string]$Files,[int]$Dpi,[int]$PaperW,[int]$PaperH)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$list = $Files -split '\|'
$doc = New-Object System.Drawing.Printing.PrintDocument
$doc.DocumentName = 'Ripon AI Photo Studio'
if (-not $doc.PrinterSettings.IsValid) { Write-Output 'ERR|No default printer is set in Windows. Choose a default printer and try again.'; exit 2 }
Add-Type -AssemblyName System.Windows.Forms
# the normal Windows print window: choose printer, copies, Preferences (paper, quality ...)
$owner = New-Object System.Windows.Forms.Form
$owner.TopMost = $true
$owner.ShowInTaskbar = $false
$owner.StartPosition = 'CenterScreen'
$owner.Size = New-Object System.Drawing.Size(1,1)
$owner.Opacity = 0
$owner.Show()
$dlg = New-Object System.Windows.Forms.PrintDialog
$dlg.Document = $doc
$dlg.UseEXDialog = $true
$dlg.AllowSomePages = $false
$dlg.AllowSelection = $false
$dlg.AllowPrintToFile = $false
$res = $dlg.ShowDialog($owner)
$owner.Close()
if ($res -ne [System.Windows.Forms.DialogResult]::OK) { Write-Output 'CANCEL|'; exit 0 }
$doc.PrintController = New-Object System.Drawing.Printing.StandardPrintController
$doc.DefaultPageSettings.Margins = New-Object System.Drawing.Printing.Margins(0,0,0,0)
$script:i = 0
$script:img = $null
$script:warn = ''
$doc.add_QueryPageSettings({ param($s,$e)
  $im = [System.Drawing.Image]::FromFile($list[$script:i])
  $land = $im.Width -gt $im.Height
  $im.Dispose()
  $e.PageSettings.Landscape = $false
  $pw = $PaperW; $ph = $PaperH
  if ($land) { $pw = $PaperH; $ph = $PaperW }
  $best = $null
  foreach ($p in $doc.PrinterSettings.PaperSizes) {
    if ([math]::Abs($p.Width - $PaperW) -le 12 -and [math]::Abs($p.Height - $PaperH) -le 12) { $best = $p; break }
    if ([math]::Abs($p.Width - $PaperH) -le 12 -and [math]::Abs($p.Height - $PaperW) -le 12) { $best = $p; $e.PageSettings.Landscape = $true; break }
  }
  if ($best -ne $null) { $e.PageSettings.PaperSize = $best } else { $script:warn = 'paper size not found in the printer - printed on its current paper' }
  if ($best -ne $null -and $land -and -not $e.PageSettings.Landscape) { $e.PageSettings.Landscape = $true }
})
$doc.add_PrintPage({ param($s,$e)
  $im = [System.Drawing.Image]::FromFile($list[$script:i])
  try {
    $g = $e.Graphics
    $g.TranslateTransform(-$e.PageSettings.HardMarginX, -$e.PageSettings.HardMarginY)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.DrawImage($im, 0, 0, [single]($im.Width * 100.0 / $Dpi), [single]($im.Height * 100.0 / $Dpi))
  } finally { $im.Dispose() }
  $script:i++
  $e.HasMorePages = ($script:i -lt $list.Count)
})
try { $doc.Print() } catch { Write-Output ('ERR|' + $_.Exception.Message); exit 3 }
Write-Output ('OK|' + $doc.PrinterSettings.PrinterName + '|' + $list.Count + '|' + $script:warn)
"#;

/// Blocking: send all pages to the default printer. Call from a worker thread.
/// Returns a message for the status bar.
pub fn print_pages(pages: &[RgbImage], dpi: u32, paper_mm: (f32, f32)) -> Result<String, String> {
    if pages.is_empty() {
        return Err("Nothing to print".into());
    }
    let dir = std::env::temp_dir().join(format!("ripon_print_{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create a temporary folder: {e}"))?;
    let mut files = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        let f = dir.join(format!("page{}.jpg", i + 1));
        std::fs::write(&f, crate::sheet::encode_jpeg(p, 97, dpi)).map_err(|e| format!("Could not write the print file: {e}"))?;
        files.push(f.to_string_lossy().into_owned());
    }
    let r = run(&dir, &files, dpi, paper_mm);
    let _ = std::fs::remove_dir_all(&dir);
    r
}

#[cfg(windows)]
fn run(dir: &std::path::Path, files: &[String], dpi: u32, paper_mm: (f32, f32)) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let script = dir.join("print.ps1");
    // UTF-8 with BOM so PowerShell 5 reads it correctly
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(SCRIPT.as_bytes());
    std::fs::write(&script, bytes).map_err(|e| e.to_string())?;
    let hundredths = |mm: f32| (mm / 25.4 * 100.0).round() as i32;
    let (w, h) = (hundredths(paper_mm.0.min(paper_mm.1)), hundredths(paper_mm.0.max(paper_mm.1)));
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-STA", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File"])
        .arg(&script)
        .args(vec!["-Files".to_string(), files.join("|"), "-Dpi".to_string(), dpi.to_string(), "-PaperW".to_string(), w.to_string(), "-PaperH".to_string(), h.to_string()])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("Could not start Windows printing (PowerShell): {e}"))?;
    parse(&String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr))
}

#[cfg(not(windows))]
fn run(_dir: &std::path::Path, _files: &[String], _dpi: u32, _paper_mm: (f32, f32)) -> Result<String, String> {
    Err("Direct printing works on Windows only. Use 'Save as PDF' on this system.".into())
}

#[cfg_attr(not(windows), allow(dead_code))]
fn parse(stdout: &str, stderr: &str) -> Result<String, String> {
    let line = stdout.lines().map(|l| l.trim()).find(|l| l.starts_with("OK|") || l.starts_with("ERR|") || l.starts_with("CANCEL|"));
    match line {
        Some(l) if l.starts_with("OK|") => {
            let p: Vec<&str> = l.split('|').collect();
            let mut s = format!("Sent {} sheet(s) to printer '{}'", p.get(2).unwrap_or(&"?"), p.get(1).unwrap_or(&"?"));
            if let Some(w) = p.get(3).filter(|w| !w.is_empty()) {
                s.push_str(&format!("  (note: {w})"));
            }
            Ok(s)
        }
        Some(l) if l.starts_with("CANCEL|") => Ok("Printing cancelled".into()),
        Some(l) => Err(format!("Printing failed: {}", &l[4..])),
        None => Err(format!("Printing failed. {}", stderr.trim().chars().take(300).collect::<String>())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_script_output() {
        assert_eq!(parse("OK|HP LaserJet|2|\r\n", "").unwrap(), "Sent 2 sheet(s) to printer 'HP LaserJet'");
        assert!(parse("OK|P|1|paper size not found\n", "").unwrap().contains("note: paper size not found"));
        assert!(parse("ERR|No default printer", "").unwrap_err().contains("No default printer"));
        assert_eq!(parse("CANCEL|", "").unwrap(), "Printing cancelled");
        assert!(parse("", "boom").unwrap_err().contains("boom"));
    }
}
