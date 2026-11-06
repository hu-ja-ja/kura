fn main() {
    winresource::WindowsResource::new()
        .set_icon("assets/kura.ico")
        .set("ProductName", "Kura")
        .set("FileDescription", "Kura")
        .set("ProductVersion", env!("CARGO_PKG_VERSION"))
        .set("FileVersion", env!("CARGO_PKG_VERSION"))
        .set("OriginalFilename", "kura.exe")
        .set("LegalCopyright", "GPL-3.0-only")
        .compile()
        .expect("the Kura icon resource should compile");
}
