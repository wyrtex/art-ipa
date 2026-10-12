#!/usr/bin/env python3
"""Assemble an unsigned iOS app bundle and wrap it in an .ipa.

Usage: make_bundle.py <app-id> <display-name> <version> <executable> <icon-png> <out-dir>

Writes <out-dir>/Payload/<Name>.app/{<app-id>, Info.plist, PkgInfo, AppIcon*.png} and zips it to
<out-dir>/<Name>-<version>-ios.ipa. Icons are resized with `sips` (macOS); elsewhere the source
PNG is copied unresized. The .ipa is unsigned: sideloading tools (AltStore, SideStore, Sideloadly,
TrollStore, `zsign`) sign it with the installer's own certificate.
"""
import os
import plistlib
import shutil
import subprocess
import sys
import zipfile

MIN_IOS = "15.0"

# (file stem, size in points, scales)
ICONS = [("AppIcon60x60", 60, (2, 3)), ("AppIcon76x76", 76, (1, 2)), ("AppIcon83.5x83.5", 83.5, (2,))]


def main() -> None:
    app_id, name, version, exe, icon, out = sys.argv[1:]
    payload = os.path.join(out, "Payload")
    bundle = os.path.join(payload, f"{name}.app")
    shutil.rmtree(payload, ignore_errors=True)
    os.makedirs(bundle)

    shutil.copy2(exe, os.path.join(bundle, app_id))
    os.chmod(os.path.join(bundle, app_id), 0o755)

    for stem, points, scales in ICONS:
        for scale in scales:
            px = int(points * scale)
            suffix = "" if scale == 1 else f"@{scale}x"
            dest = os.path.join(bundle, f"{stem}{suffix}.png")
            if shutil.which("sips"):
                subprocess.run(["sips", "-z", str(px), str(px), icon, "--out", dest], check=True, capture_output=True)
            else:
                shutil.copy2(icon, dest)

    with open(os.path.join(bundle, "Info.plist"), "wb") as f:
        plistlib.dump(info_plist(app_id, name, version), f, fmt=plistlib.FMT_BINARY)
    with open(os.path.join(bundle, "PkgInfo"), "w") as f:
        f.write("APPL????")

    ipa = os.path.join(out, f"{name}-{version}-ios.ipa")
    if os.path.exists(ipa):
        os.remove(ipa)
    with zipfile.ZipFile(ipa, "w", zipfile.ZIP_DEFLATED) as z:
        for root, _dirs, files in os.walk(payload):
            for file in files:
                path = os.path.join(root, file)
                info = zipfile.ZipInfo.from_file(path, os.path.relpath(path, out))
                info.compress_type = zipfile.ZIP_DEFLATED
                with open(path, "rb") as src:
                    z.writestr(info, src.read())
    print(ipa)


def info_plist(app_id: str, name: str, version: str) -> dict:
    icon_files = [stem for stem, _, _ in ICONS]
    return {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleExecutable": app_id,
        "CFBundleIdentifier": f"ai.storyteller.{app_id}",
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": name,
        "CFBundleDisplayName": name,
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "CFBundleSupportedPlatforms": ["iPhoneOS"],
        "CFBundleIcons": {"CFBundlePrimaryIcon": {"CFBundleIconFiles": icon_files[:1]}},
        "CFBundleIcons~ipad": {"CFBundlePrimaryIcon": {"CFBundleIconFiles": icon_files}},
        "DTPlatformName": "iphoneos",
        "MinimumOSVersion": MIN_IOS,
        "LSRequiresIPhoneOS": True,
        "UIDeviceFamily": [1, 2],
        "UIRequiredDeviceCapabilities": ["arm64", "metal"],
        # Full-screen, no launch storyboard needed (iOS 14+), no status bar over the app's menus.
        "UILaunchScreen": {},
        "UIRequiresFullScreen": True,
        "UIStatusBarHidden": True,
        "UIViewControllerBasedStatusBarAppearance": False,
        "UISupportedInterfaceOrientations": [
            "UIInterfaceOrientationLandscapeLeft",
            "UIInterfaceOrientationLandscapeRight",
            "UIInterfaceOrientationPortrait",
        ],
        "UISupportedInterfaceOrientations~ipad": [
            "UIInterfaceOrientationLandscapeLeft",
            "UIInterfaceOrientationLandscapeRight",
            "UIInterfaceOrientationPortrait",
            "UIInterfaceOrientationPortraitUpsideDown",
        ],
        # Trackpad / mouse pointer events on iPad.
        "UIApplicationSupportsIndirectInputEvents": True,
        # Saved files land in Documents; show that folder in the Files app.
        "UIFileSharingEnabled": True,
        "LSSupportsOpeningDocumentsInPlace": True,
        "NSMicrophoneUsageDescription": f"{name} records audio only when you ask it to.",
    }


if __name__ == "__main__":
    main()
