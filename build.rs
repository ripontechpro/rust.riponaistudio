// Embeds the Ripon logo + product info into the Windows .exe (no effect on other platforms).
fn main() {
    println!("cargo:rerun-if-changed=assets/ripon_logo.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/ripon_logo.ico");
        res.set("ProductName", "Ripon AI Photo Studio");
        res.set("FileDescription", "Ripon AI Photo Studio - Edit, Enhance, Create");
        res.set("CompanyName", "Ripon AI Photo Studio");
        res.set("LegalCopyright", "Ripon AI Photo Studio");
        let _ = res.compile();
    }
}
