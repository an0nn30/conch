fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("icons/conch.ico");
        res.set("ProductName", "Conch");
        res.set("FileDescription", "Conch Terminal Emulator");
        res.compile().expect("Failed to compile Windows resources");
    }
}
