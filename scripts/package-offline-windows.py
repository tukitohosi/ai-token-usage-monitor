"""Package an already-built Tauri release with a verified local WebView2 offline installer.

This bypasses the bundler's network metadata lookup and reuses its generated NSIS
script unchanged except for the two offline-installer definitions.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--webview-installer", required=True, type=Path)
    parser.add_argument("--makensis", required=True, type=Path)
    args = parser.parse_args()
    runtime = args.webview_installer.resolve(strict=True)
    compiler = args.makensis.resolve(strict=True)
    if runtime.stat().st_size < 100_000_000:
        raise SystemExit("Use the complete WebView2 standalone installer, not its bootstrapper.")
    env = os.environ.copy()
    env["AI_TOKEN_WEBVIEW2_INSTALLER"] = str(runtime)
    powershell = Path(env.get("SystemRoot", r"C:\Windows")) / "System32/WindowsPowerShell/v1.0/powershell.exe"
    verified = subprocess.run([str(powershell), "-NoProfile", "-Command",
        "$s=Get-AuthenticodeSignature -LiteralPath $env:AI_TOKEN_WEBVIEW2_INSTALLER; "
        "if ($s.Status -ne 'Valid' -or $s.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') { exit 1 }"],
        env=env, creationflags=subprocess.CREATE_NO_WINDOW)
    if verified.returncode:
        raise SystemExit("The offline installer must have a valid Microsoft digital signature.")
    config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
    version = config["version"]
    generated = ROOT / "src-tauri/target/release/nsis/x64"
    original = (generated / "installer.nsi").read_text(encoding="utf-8-sig")
    if f'!define VERSION "{version}"' not in original:
        raise SystemExit("Generated installer version does not match tauri.conf.json; rebuild first.")
    if not re.search(r'^!define WEBVIEW2INSTALLERARGS "/silent"$', original, re.M):
        raise SystemExit("Expected the standard silent WebView2 installation arguments.")
    runtime_nsis = str(runtime).replace("$", "$$").replace(chr(34), "$" + chr(92) + chr(34))
    script = original
    for name, value in [("INSTALLWEBVIEW2MODE", "offlineInstaller"), ("WEBVIEW2INSTALLERPATH", runtime_nsis)]:
        script, count = re.subn(r'^!define ' + name + r' ".*"$', lambda _: f'!define {name} "{value}"', script, flags=re.M)
        if count != 1:
            raise SystemExit(f"Expected exactly one {name} definition.")
    tools = ROOT / ".tools"
    tools.mkdir(exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix="offline-nsis-", dir=tools))
    shutil.copytree(generated, staging, dirs_exist_ok=True)
    (staging / "installer.nsi").write_text(script, encoding="utf-8-sig")
    env.pop("NSISDIR", None)
    env.pop("NSISCONFDIR", None)
    subprocess.run([str(compiler), "-INPUTCHARSET", "UTF8", "-OUTPUTCHARSET", "UTF8", "-V3", str(staging / "installer.nsi")],
        cwd=staging, env=env, check=True, creationflags=subprocess.CREATE_NO_WINDOW)
    built = staging / "nsis-output.exe"
    if built.stat().st_size < runtime.stat().st_size * 0.8:
        raise SystemExit("Installer is unexpectedly small; offline runtime embedding was not verified.")
    destination = ROOT / "release" / f"AI-Token-Usage-Monitor-{version}-Setup.exe"
    destination.parent.mkdir(exist_ok=True)
    shutil.copyfile(built, destination)
    digest = hashlib.sha256(destination.read_bytes()).hexdigest()
    (destination.parent / f"SHA256-v{version}.txt").write_text(f"{digest}  {destination.name}\n", encoding="utf-8")
    print(json.dumps({"installer": str(destination), "bytes": destination.stat().st_size,
        "sha256": digest, "webviewInstallMode": "offlineInstaller",
        "runtimeSha256": hashlib.sha256(runtime.read_bytes()).hexdigest(), "staging": str(staging)}, ensure_ascii=True))


if __name__ == "__main__":
    main()
