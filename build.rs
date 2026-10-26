//! Puts the launcher icon and the version information into LimeCraft.exe
//! (what Explorer, the taskbar and the file properties show). On other systems,
//! or when no resource compiler (`windres` / `rc`) is found, the build goes on
//! without it: the program works the same, only the exe gets a plain icon.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let icon = manifest_dir.join("assets").join("icon.ico");
    if !icon.is_file() {
        println!("cargo:warning=assets/icon.ico not found: the exe gets no icon");
        return;
    }

    // "1.2.3" -> 1,2,3,0
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts: Vec<String> = version.split(['.', '-', '+']).take(3).map(|p| p.chars().filter(char::is_ascii_digit).collect::<String>()).collect();
    parts.resize(3, "0".into());
    let numeric = format!("{},{},{},0", parts[0], parts[1], parts[2]);

    // resource files use C-style strings: backslashes must be doubled
    let icon_path = icon.display().to_string().replace('\\', "\\\\");
    let rc = format!(
        r#"1 ICON "{icon_path}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
{{
    BLOCK "StringFileInfo"
    {{
        BLOCK "040904B0"
        {{
            VALUE "FileDescription", "LimeCraft - Minecraft launcher"
            VALUE "ProductName", "LimeCraft"
            VALUE "FileVersion", "{version}"
            VALUE "ProductVersion", "{version}"
            VALUE "InternalName", "LimeCraft"
            VALUE "OriginalFilename", "LimeCraft.exe"
            VALUE "LegalCopyright", "Apache-2.0"
        }}
    }}
    BLOCK "VarFileInfo"
    {{
        VALUE "Translation", 0x409, 1200
    }}
}}
"#
    );
    let rc_path = out_dir.join("limecraft.rc");
    std::fs::write(&rc_path, rc).expect("write the resource script");

    match embed_resource::compile(&rc_path, embed_resource::NONE) {
        embed_resource::CompilationResult::Ok | embed_resource::CompilationResult::NotWindows => {}
        embed_resource::CompilationResult::NotAttempted(why) => println!("cargo:warning=exe icon not embedded: {why}"),
        embed_resource::CompilationResult::Failed(why) => println!("cargo:warning=exe icon not embedded: {why}"),
    }
}
