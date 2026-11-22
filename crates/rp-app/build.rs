//! Embeds the app icon and version details in rusty-popper.exe (Windows only).

fn main() {
    println!("cargo:rerun-if-changed=assets/rusty-popper.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/rusty-popper.ico")
            .set("ProductName", "Rusty Popper")
            .set("FileDescription", "Rusty Popper - alternate front end for PinUP Popper")
            .set("CompanyName", "Rusty Popper")
            .set("LegalCopyright", "MIT OR Apache-2.0");
        if let Err(e) = res.compile() {
            // A missing resource compiler shouldn't stop the build; the exe just has no icon.
            println!("cargo:warning=could not embed the icon: {e}");
        }
    }
}
