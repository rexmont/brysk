#!/usr/bin/env python3
"""Sign a finished archive and produce the Sparkle feed; never publishes it."""
import argparse
import email.utils
import pathlib
import plistlib
import subprocess
import re
import urllib.parse
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parent.parent
SPARKLE = "http://www.andymatuschak.org/xml-namespaces/sparkle"
ET.register_namespace("sparkle", SPARKLE)
SIGN = ROOT / "target/sparkle/bin/sign_update"
ACCOUNT = "com.rexmont.brysk"


def build_feed(archive, info, version, notes, signature, previous=None):
    root = ET.fromstring(previous) if previous else ET.Element("rss", version="2.0")
    channel = root.find("channel")
    if channel is None:
        channel = ET.SubElement(root, "channel")
        ET.SubElement(channel, "title").text = "Brysk updates"
        ET.SubElement(channel, "link").text = "https://github.com/rexmont/brysk/releases"
        ET.SubElement(channel, "description").text = "Signed macOS updates for Brysk."
    build = info["CFBundleVersion"]
    url = f"https://github.com/rexmont/brysk/releases/download/v{urllib.parse.quote(version, safe='')}/{urllib.parse.quote(archive.name, safe='')}"
    for item in list(channel.findall("item")):
        enclosure = item.find("enclosure")
        # A replacement asset has a new signature; never retain its old signature at the same URL.
        if item.findtext(f"{{{SPARKLE}}}version") == build or (enclosure is not None and enclosure.get("url") == url):
            channel.remove(item)
    item = ET.Element("item")
    channel.insert(3, item)
    ET.SubElement(item, "title").text = f"Brysk {version}"
    ET.SubElement(item, "pubDate").text = email.utils.formatdate(usegmt=True)
    ET.SubElement(item, f"{{{SPARKLE}}}version").text = build
    ET.SubElement(item, f"{{{SPARKLE}}}shortVersionString").text = version
    ET.SubElement(item, f"{{{SPARKLE}}}minimumSystemVersion").text = info["LSMinimumSystemVersion"]
    ET.SubElement(item, "description", {f"{{{SPARKLE}}}descriptionFormat": "plain-text"}).text = notes
    ET.SubElement(item, f"{{{SPARKLE}}}fullReleaseNotesLink").text = f"https://github.com/rexmont/brysk/releases/tag/v{version}"
    ET.SubElement(item, "enclosure", {
        "url": url, "length": str(archive.stat().st_size), "type": "application/octet-stream",
        f"{{{SPARKLE}}}edSignature": signature,
    })
    ET.indent(root)
    return ET.tostring(root, encoding="utf-8", xml_declaration=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=pathlib.Path)
    parser.add_argument("app", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    args = parser.parse_args()
    info = plistlib.loads((args.app / "Contents/Info.plist").read_bytes())
    public = subprocess.check_output([ROOT / "target/sparkle/bin/generate_keys", "--account", ACCOUNT, "-p"], text=True).strip()
    if public != info.get("SUPublicEDKey"):
        parser.error("The signing key does not match this app's embedded update key.")
    signature = subprocess.check_output([SIGN, "--account", ACCOUNT, "-p", args.archive], text=True).strip()
    subprocess.run([SIGN, "--account", ACCOUNT, "--verify", args.archive, signature], check=True)
    version = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.MULTILINE).group(1)
    notes_path = ROOT / f"docs/releases/v{version}.md"
    notes = notes_path.read_text() if notes_path.exists() else f"Brysk {version} for macOS."
    notes = re.sub(r'\[([^\]]+)\]\([^)]+\)', r'\1', notes).replace("**", "").strip()
    previous = ROOT / "packaging/macos/appcast.xml"
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(build_feed(args.archive, info, version, notes, signature, previous.read_bytes() if previous.exists() else None))
    subprocess.run([SIGN, "--account", ACCOUNT, args.output], check=True)
    subprocess.run([SIGN, "--account", ACCOUNT, "--verify", args.output], check=True)
    print(args.output)


if __name__ == "__main__":
    main()
