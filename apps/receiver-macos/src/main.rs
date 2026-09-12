#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    macos::run()
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("receiver-macos requires macOS; receiver-core tests are portable.");
}
