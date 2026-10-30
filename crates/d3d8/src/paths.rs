use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::OnceLock;

use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;

pub struct Paths {
    pub exe: PathBuf,
    pub exe_dir: PathBuf,
    pub game_id: &'static str,
    /// `None` only when LOCALAPPDATA is unset, in which case nothing is written to disk.
    pub data_dir: Option<PathBuf>,
}

static PATHS: OnceLock<Paths> = OnceLock::new();

pub fn get() -> &'static Paths {
    PATHS.get_or_init(resolve)
}

fn resolve() -> Paths {
    let exe = main_module_path();
    let exe_dir = exe.parent().map(PathBuf::from).unwrap_or_default();
    let dir_name = exe_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let game_id = if dir_name.contains("zero hour") { "zh" } else { "generals" };
    let data_dir = std::env::var_os("LOCALAPPDATA").map(|base| {
        let dir = PathBuf::from(base).join("cnc-vulkan").join(game_id);
        let _ = std::fs::create_dir_all(&dir);
        dir
    });
    Paths { exe, exe_dir, game_id, data_dir }
}

fn main_module_path() -> PathBuf {
    let mut buf = vec![0u16; 1024];
    let len = unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) };
    buf.truncate(len as usize);
    PathBuf::from(OsString::from_wide(&buf))
}

pub fn data_subdir(name: &str) -> Option<PathBuf> {
    let dir = get().data_dir.as_ref()?.join(name);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}
