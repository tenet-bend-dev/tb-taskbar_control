// build.rs
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winres::WindowsResource::new();
        
        // 1. Icon Logic (points to assets/icon.ico)
        res.set_icon("assets/icon.ico"); 

        // 2. Tenet Bend Metadata
        res.set("ProductName", "TENET BEND ⧉ Taskbar Control");
        res.set("FileDescription", "Lightweight Win32 Taskbar State Manager");
        res.set("LegalCopyright", "© 2026 TENET BEND ⧉. All rights reserved.");
        
        // 3. Versioning
        res.set("ProductVersion", "1.0.0-stable");
        res.set("FileVersion", "1.0.0.0");

        // 4. Manifest (Long Path Support)
        res.set_manifest(r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#);

        // 5. Finalize
        res.compile().unwrap();
    }
}