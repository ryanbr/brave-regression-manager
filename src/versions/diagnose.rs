//! Pure-Rust analysis of a Windows installer .exe to figure out *what format
//! it actually is*. We've burned several iterations guessing; this lets us
//! see the real layout in one shot.

use anyhow::Result;
use std::path::Path;

pub fn diagnose_installer(archive: &Path) -> Result<String> {
    let bytes = std::fs::read(archive)?;
    let mut r = Vec::<String>::new();

    r.push(format!("== {} ==", archive.display()));
    r.push(format!("size: {} bytes ({})", bytes.len(), human(bytes.len() as u64)));
    r.push(format!("first 64 bytes hex:    {}", hex_window(&bytes, 0, 64)));
    r.push(format!("first 64 bytes ascii:  {}", ascii_window(&bytes, 0, 64)));

    // -- PE walk --------------------------------------------------------
    if bytes.len() < 0x40 || &bytes[..2] != b"MZ" {
        r.push("not a PE/MZ file".into());
        return finish(r, &bytes);
    }
    let e_lfanew = u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap()) as usize;
    if e_lfanew + 24 > bytes.len() || &bytes[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
        r.push(format!("invalid PE header at 0x{e_lfanew:x}"));
        return finish(r, &bytes);
    }
    let machine    = u16::from_le_bytes(bytes[e_lfanew + 4..e_lfanew + 6].try_into().unwrap());
    let n_sections = u16::from_le_bytes(bytes[e_lfanew + 6..e_lfanew + 8].try_into().unwrap()) as usize;
    let opt_size   = u16::from_le_bytes(bytes[e_lfanew + 0x14..e_lfanew + 0x16].try_into().unwrap()) as usize;
    let sec_off    = e_lfanew + 24 + opt_size;
    r.push(format!("PE header @ 0x{e_lfanew:x}; machine=0x{machine:04x} ({}); {n_sections} sections; opt_header={opt_size} bytes",
                   match machine { 0x8664 => "x64", 0x014c => "x86", 0xaa64 => "arm64", _ => "?" }));

    let mut overlay_start = 0usize;
    for i in 0..n_sections {
        let off = sec_off + i * 40;
        if off + 40 > bytes.len() { break; }
        let name     = std::str::from_utf8(&bytes[off..off + 8])
            .unwrap_or("?").trim_end_matches('\0').to_string();
        let raw_size = u32::from_le_bytes(bytes[off + 16..off + 20].try_into().unwrap()) as usize;
        let raw_ptr  = u32::from_le_bytes(bytes[off + 20..off + 24].try_into().unwrap()) as usize;
        let end      = raw_ptr.saturating_add(raw_size);
        overlay_start = overlay_start.max(end);
        r.push(format!("  section {i}: {:>8}  raw_off=0x{raw_ptr:08x}  raw_size=0x{raw_size:08x}",
                       name));
    }

    // -- Overlay --------------------------------------------------------
    if overlay_start < bytes.len() {
        let overlay_size = bytes.len() - overlay_start;
        r.push(format!("overlay @ 0x{overlay_start:x}, size {} bytes ({})",
                       overlay_size, human(overlay_size as u64)));
        r.push(format!("overlay first 64 hex:   {}",   hex_window(&bytes, overlay_start, 64)));
        r.push(format!("overlay first 64 ascii: {}", ascii_window(&bytes, overlay_start, 64)));
    } else {
        r.push("no overlay (file ends at last section)".into());
    }

    // -- Magic scans ----------------------------------------------------
    const SIG_7Z:    &[u8] = &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];
    const SIG_NSIS:  &[u8] = b"NullsoftInst";
    const SIG_MSI:   &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]; // CFB
    const SIG_INNO:  &[u8] = b"Inno Setup Setup";
    const SIG_WIX:   &[u8] = b"Wix Toolset";
    const SIG_CAB:   &[u8] = b"MSCF";
    const SIG_LZMA1: &[u8] = &[0x5D, 0x00, 0x00];

    for (label, sig) in [
        ("7z magic   ", SIG_7Z),
        ("NSIS magic ", SIG_NSIS),
        ("MSI/CFB    ", SIG_MSI),
        ("Inno Setup ", SIG_INNO),
        ("WiX        ", SIG_WIX),
        ("MSCF (CAB) ", SIG_CAB),
        ("LZMA1      ", SIG_LZMA1),
    ] {
        let hits: Vec<usize> = scan_all(&bytes, sig);
        let preview: Vec<String> = hits.iter().take(10).map(|p| format!("0x{p:x}")).collect();
        r.push(format!("{label}: {} matches{}",
                       hits.len(),
                       if preview.is_empty() { String::new() } else { format!(" — {}", preview.join(", ")) }));
    }

    finish(r, &bytes)
}

fn finish(mut r: Vec<String>, _bytes: &[u8]) -> Result<String> {
    r.push("---".into());
    Ok(r.join("\n"))
}

fn scan_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || hay.len() < needle.len() { return Vec::new(); }
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            out.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

fn hex_window(b: &[u8], off: usize, n: usize) -> String {
    let end = (off + n).min(b.len());
    b[off..end].iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

fn ascii_window(b: &[u8], off: usize, n: usize) -> String {
    let end = (off + n).min(b.len());
    b[off..end].iter()
        .map(|x| if x.is_ascii_graphic() || *x == b' ' { *x as char } else { '.' })
        .collect()
}

fn human(b: u64) -> String {
    const KB: u64 = 1024; const MB: u64 = KB * 1024; const GB: u64 = MB * 1024;
    if b >= GB      { format!("{:.2} GB", b as f64 / GB as f64) }
    else if b >= MB { format!("{:.1} MB", b as f64 / MB as f64) }
    else if b >= KB { format!("{} KB", b / KB) }
    else            { format!("{b} B") }
}
