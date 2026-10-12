#[cfg(target_os = "ios")]
pub use ios::window_safe_area_insets;

#[cfg(target_os = "ios")]
mod ios {
    use egui::{SafeAreaInsets, epaint::MarginF32};
    use objc2_foundation::MainThreadMarker;
    use objc2_ui_kit::UIView;
    use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use winit::window::Window;

    /// The safe area insets of the window's view, in UIKit points (art-ipa).
    ///
    /// The area of the view that isn't covered by the notch / Dynamic Island, the rounded
    /// corners, the home indicator or the status bar. Read from winit's own view rather than
    /// from the first foreground `UIWindowScene` (upstream egui-winit), which gives zero insets
    /// for apps without a scene manifest.
    ///
    /// Once winit v0.31 has been released this can be removed in favor of
    /// `winit::Window::safe_area`.
    pub fn window_safe_area_insets(window: &Window) -> Option<SafeAreaInsets> {
        MainThreadMarker::new()?;
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::UiKit(handle) = handle.as_raw() else {
            return None;
        };
        #[expect(unsafe_code)]
        // SAFETY: winit's handle points at its live `UIView`; we are on the main thread.
        let insets = unsafe { handle.ui_view.cast::<UIView>().as_ref().safeAreaInsets() };
        Some(SafeAreaInsets(MarginF32 {
            top: insets.top as f32,
            left: insets.left as f32,
            right: insets.right as f32,
            bottom: insets.bottom as f32,
        }))
    }
}
