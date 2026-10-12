#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android", target_os = "emscripten"))))]
mod linux;
#[cfg(all(
	unix,
	not(any(target_os = "macos", target_os = "ios", target_os = "android", target_os = "emscripten"))
))]
pub use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "macos")]
mod osx;
#[cfg(target_os = "macos")]
pub use osx::*;

#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "ios")]
#[allow(unused_imports, unreachable_pub)]
pub use ios::*;
