use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let part = |name: &str| env::var(name).unwrap().parse::<u16>().unwrap();
    let numbers = format!(
        "{},{},{},0",
        part("CARGO_PKG_VERSION_MAJOR"),
        part("CARGO_PKG_VERSION_MINOR"),
        part("CARGO_PKG_VERSION_PATCH")
    );
    let rc = format!(
        r#"1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEFLAGSMASK 0x3f
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x2
FILESUBTYPE 0x0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "FileDescription", "cnc-vulkan for C&C Generals and Zero Hour\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "dinput8\0"
      VALUE "OriginalFilename", "dinput8.dll\0"
      VALUE "ProductName", "cnc-vulkan\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("version.rc");
    fs::write(&path, rc).unwrap();
    embed_resource::compile_for_cdylib(&path, embed_resource::NONE).manifest_required().unwrap();
}
