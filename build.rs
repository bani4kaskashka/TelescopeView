// Embeds the icon and version info in the Windows exe, so Explorer, the taskbar and
// "Open with" show the telescope instead of a blank program icon.
fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.set("FileDescription", "TelescopeView");
        res.set("ProductName", "TelescopeView");
        if let Err(e) = res.compile() {
            println!("cargo:warning=no exe icon: {e}");
        }
    }
}
