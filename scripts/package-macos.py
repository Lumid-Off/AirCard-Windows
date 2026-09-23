#!/usr/bin/env python3
"""Build a macOS app bundle and drag-to-Applications DMG using system tools."""

import argparse
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parent.parent


def run(*args, capture=False):
    return subprocess.run(
        [str(arg) for arg in args], cwd=ROOT, check=True,
        stdout=subprocess.PIPE if capture else None,
    ).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--binary", type=Path, help="Package an existing Release binary instead of building")
    source.add_argument("--target", choices=["aarch64-apple-darwin", "x86_64-apple-darwin"],
                        help="Rust build target (default: native Mac hardware, including under Rosetta)")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("DMG packaging requires macOS and Xcode Command Line Tools")

    metadata = json.loads(run("cargo", "metadata", "--no-deps", "--format-version", "1", "--offline", capture=True))
    package = next(p for p in metadata["packages"] if Path(p["manifest_path"]) == ROOT / "Cargo.toml")
    version = package["version"]
    if args.binary is None:
        target = args.target
        if target is None:
            # uname and rustc report x86_64 when run through Rosetta on an ARM Mac.
            hardware = subprocess.run(["sysctl", "-n", "hw.optional.arm64"], capture_output=True, text=True)
            if hardware.returncode != 0 or hardware.stdout.strip() not in {"0", "1"}:
                parser.error("Cannot detect Mac hardware. Specify --target aarch64-apple-darwin or --target x86_64-apple-darwin")
            target = "aarch64-apple-darwin" if hardware.stdout.strip() == "1" else "x86_64-apple-darwin"
        print(f"Building for {target}", flush=True)
        run("cargo", "build", "--release", "--locked", "--target", target)
        binary = Path(metadata["target_directory"]) / target / "release" / "aircard"
    else:
        binary = args.binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f"Release executable not found or not executable: {binary}")

    architectures = set(run("lipo", "-archs", binary, capture=True).decode().split())
    suffixes = {
        frozenset({"x86_64"}): "x64",
        frozenset({"arm64"}): "arm64",
        frozenset({"x86_64", "arm64"}): "universal",
    }
    suffix = suffixes.get(frozenset(architectures))
    if suffix is None:
        parser.error(f"Unsupported binary architectures: {architectures}")
    if args.binary is None:
        expected_architecture = "arm64" if target == "aarch64-apple-darwin" else "x86_64"
        if architectures != {expected_architecture}:
            parser.error(f"Expected {expected_architecture} executable, found {architectures}")

    # Keep bundle compatibility metadata consistent with the executable's load commands.
    load_commands = run("xcrun", "vtool", "-show-build", binary, capture=True).decode()
    # LC_BUILD_VERSION also lists the linker's "version" (e.g. 27037.1),
    # which must never become the app's minimum macOS version.
    minimum_versions = re.findall(r"^\s*minos\s+(\d+\.\d+(?:\.\d+)?)\s*$", load_commands, re.M)
    minimum_versions += re.findall(
        r"cmd LC_VERSION_MIN_MACOSX\s+cmdsize \d+\s+version (\d+\.\d+(?:\.\d+)?)",
        load_commands,
    )
    if not minimum_versions or "IOS" in load_commands:
        parser.error("Expected a macOS Mach-O executable with a minimum OS version")
    minimum_os = max(minimum_versions, key=lambda value: tuple(map(int, value.split("."))))

    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    artifact = output / f"aircard-macos-{suffix}.dmg"
    # The previous DMG remains intact if packaging fails.
    with tempfile.TemporaryDirectory(prefix=".aircard-dmg-", dir=output) as temporary:
        staging = Path(temporary) / "volume"
        app = staging / "AirCard.app"
        contents = app / "Contents"
        (contents / "MacOS").mkdir(parents=True)
        (contents / "Resources").mkdir()
        shutil.copyfile(binary, contents / "MacOS" / "aircard")
        (contents / "MacOS" / "aircard").chmod(0o755)
        info = {
            "CFBundleDevelopmentRegion": "en",
            "CFBundleDisplayName": "AirCard",
            "CFBundleExecutable": "aircard",
            "CFBundleIdentifier": "io.github.lumid-off.aircard",
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleName": "AirCard",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": re.split(r"[-+]", version)[0],
            "CFBundleVersion": re.split(r"[-+]", version)[0],
            "LSApplicationCategoryType": "public.app-category.utilities",
            "LSMinimumSystemVersion": minimum_os,
            "NSHighResolutionCapable": True,
        }
        (contents / "Info.plist").write_bytes(plistlib.dumps(info))
        (contents / "PkgInfo").write_bytes(b"APPL????")
        shutil.copyfile(ROOT / "LICENSE", contents / "Resources" / "LICENSE")
        (staging / "Applications").symlink_to("/Applications", target_is_directory=True)
        (staging / "Read Me.txt").write_text(
            f"AirCard {version} for macOS ({suffix})\n\n"
            "Drag AirCard.app to Applications, then eject this disk image.\n"
            "Open AirCard from Applications. Connect and trust your iPhone to use device features.\n\n"
            "This build is ad-hoc signed, not Developer ID signed or notarized by Apple.\n",
            encoding="utf-8",
        )

        run("plutil", "-lint", contents / "Info.plist")
        # Ad-hoc signing also supplies the signature required by Apple Silicon.
        # This does not claim a verified developer identity or Apple notarization.
        run("codesign", "--force", "--sign", "-", "--timestamp=none", app)
        run("codesign", "--verify", "--deep", "--strict", "--verbose=2", app)
        temporary_dmg = Path(temporary) / artifact.name
        run("hdiutil", "create", "-volname", f"AirCard {version}", "-srcfolder", staging,
            "-fs", "HFS+", "-format", "UDZO", temporary_dmg)
        run("hdiutil", "verify", temporary_dmg)
        temporary_dmg.replace(artifact)
    print(f"Created {artifact} ({artifact.stat().st_size / 1024 / 1024:.1f} MiB)")


if __name__ == "__main__":
    try:
        main()
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        sys.exit(f"macOS packaging failed: {error}")
