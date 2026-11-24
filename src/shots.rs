//! Screenshot mode. Run as `<exe> --shots <folder> [--theme dark]` and the
//! app shows each made-up scene in turn, writes it to the folder as
//! `<n>.png` (to look at) and `<n>_<w>x<h>.rgba` (for app-factory's
//! `tools/compose-screenshot.ps1`), then closes. Nothing real happens in this
//! mode and no setting is saved.
//!
//! Factory-owned driver: the scenes themselves are set up by
//! `App::show_scene` in `app.rs`.

use std::path::{Path, PathBuf};

use eframe::egui;

pub struct Shots {
    dir: PathBuf,
    /// 0-based scene being shown.
    scene: usize,
    /// Frames drawn of this scene, so fonts and layout settle before capture.
    frames: u32,
    requested: bool,
}

impl Shots {
    /// `Some` when the arguments ask for screenshot mode.
    pub fn from_args(args: &[String]) -> Option<Self> {
        let at = args.iter().position(|a| a == "--shots")?;
        let dir = args.get(at + 1).filter(|d| !d.is_empty())?;
        Some(Self {
            dir: PathBuf::from(dir),
            scene: 0,
            frames: 0,
            requested: false,
        })
    }

    /// Call once per frame. Returns `Some(scene)` on the frame the app must
    /// set that scene up; captures it a few frames later and closes the
    /// window after the last of `scenes`.
    pub fn drive(&mut self, ctx: &egui::Context, scenes: usize) -> Option<usize> {
        let image = ctx.input(|input| {
            input.raw.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let (w, h) = (image.size[0], image.size[1]);
            let _ = std::fs::create_dir_all(&self.dir);
            let n = self.scene + 1;
            let _ = std::fs::write(self.dir.join(format!("{n}_{w}x{h}.rgba")), image.as_raw());
            let _ = write_png(&self.dir.join(format!("{n}.png")), w, h, image.as_raw());
            self.scene += 1;
            self.frames = 0;
            self.requested = false;
        }
        if self.scene >= scenes {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return None;
        }
        let set_up = (self.frames == 0).then_some(self.scene);
        if self.frames >= 5 && !self.requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.requested = true;
        }
        self.frames += 1;
        ctx.request_repaint();
        set_up
    }
}

/// The theme asked for on the command line with `--theme light|dark`.
pub fn theme_from_args(args: &[String]) -> Option<egui::Theme> {
    let at = args.iter().position(|a| a == "--theme")?;
    match args.get(at + 1).map(String::as_str) {
        Some("dark") => Some(egui::Theme::Dark),
        Some("light") => Some(egui::Theme::Light),
        _ => None,
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// An RGBA image as PNG bytes. Uncompressed ("stored") deflate, so it needs
/// no compression crate; the files are large but only made in this mode.
fn png_bytes(width: usize, height: usize, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((width * 4 + 1) * height);
    for row in rgba.chunks_exact(width * 4).take(height) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    if blocks.peek().is_none() {
        zlib.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none()));
        zlib.extend_from_slice(&(block.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn write_png(path: &Path, width: usize, height: usize, rgba: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, png_bytes(width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn png_has_signature_size_and_end() {
        let png = png_bytes(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn arguments_select_the_mode_and_theme() {
        let args: Vec<String> = ["--shots", "out", "--theme", "dark"].iter().map(|s| s.to_string()).collect();
        assert!(Shots::from_args(&args).is_some());
        assert_eq!(theme_from_args(&args), Some(egui::Theme::Dark));
        assert!(Shots::from_args(&["--shots".to_owned()]).is_none());
        assert!(Shots::from_args(&[]).is_none());
    }
}
