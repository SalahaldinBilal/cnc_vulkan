//! Lists and extracts files from the game's .big archives.
//! Usage: bigtool <folder or .big> <name filter> [output folder]
//! Without an output folder it only lists matching entries.

use std::fs;
use std::path::{Path, PathBuf};

struct Entry {
    name: String,
    offset: usize,
    size: usize,
}

fn be32(b: &[u8], at: usize) -> usize {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize
}

fn entries(data: &[u8]) -> Option<Vec<Entry>> {
    if data.len() < 16 || (&data[..4] != b"BIGF" && &data[..4] != b"BIG4") {
        return None;
    }
    let count = be32(data, 8);
    let mut at = 16;
    let mut list = Vec::with_capacity(count);
    for _ in 0..count {
        let offset = be32(data, at);
        let size = be32(data, at + 4);
        let start = at + 8;
        let end = start + data[start..].iter().position(|&c| c == 0)?;
        list.push(Entry { name: String::from_utf8_lossy(&data[start..end]).into_owned(), offset, size });
        at = end + 1;
    }
    Some(list)
}

fn archives(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    let mut list: Vec<PathBuf> = fs::read_dir(path)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    list.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("big")));
    list.sort();
    list
}

fn tga_to_png(bytes: &[u8], out: &Path) -> Option<()> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Tga).ok()?;
    image.save_with_format(out, image::ImageFormat::Png).ok()
}

/// `bigtool crop <image> <x> <y> <w> <h> <zoom> <out.png>`: a zoomed piece of an image, for looking at art.
fn crop(args: &[String]) {
    let n = |i: usize| args[i].parse::<u32>().expect("number");
    let image = image::open(&args[2]).expect("image");
    let piece = image.crop_imm(n(3), n(4), n(5), n(6));
    let zoom = n(7);
    piece
        .resize(piece.width() * zoom, piece.height() * zoom, image::imageops::FilterType::Nearest)
        .save(&args[8])
        .expect("save");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("crop") && args.len() == 9 {
        crop(&args);
        return;
    }
    if args.len() < 3 {
        eprintln!("usage: bigtool <folder or .big> <name filter> [output folder]");
        std::process::exit(1);
    }
    let filter = args[2].to_ascii_lowercase();
    let out = args.get(3).map(PathBuf::from);
    for archive in archives(Path::new(&args[1])) {
        let Ok(data) = fs::read(&archive) else { continue };
        let Some(list) = entries(&data) else { continue };
        let archive_name = archive.file_name().unwrap().to_string_lossy().into_owned();
        for e in list.iter().filter(|e| e.name.to_ascii_lowercase().contains(&filter)) {
            println!("{archive_name}: {} ({} bytes)", e.name, e.size);
            if let Some(out) = &out {
                let target = out.join(&archive_name).join(e.name.replace('\\', "/"));
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                let bytes = &data[e.offset..e.offset + e.size];
                fs::write(&target, bytes).unwrap();
                if e.name.to_ascii_lowercase().ends_with(".tga") {
                    match tga_to_png(bytes, &target.with_extension("png")) {
                        Some(()) => {}
                        None => eprintln!("  could not convert {} to png", e.name),
                    }
                }
            }
        }
    }
}
