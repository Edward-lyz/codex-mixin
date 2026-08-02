#!/usr/bin/env python3
"""Download the latest available NASA ISS desktop wallpapers.

The script intentionally uses only Python's standard library so release jobs
can refresh the bundled card artwork without adding a package dependency.

NASA publishes a month's wallpapers partway through that month, so the sync
falls back to recent months instead of failing a release that runs on the 1st.
"""

from __future__ import annotations

import argparse
import calendar
import datetime as dt
import hashlib
import html.parser
import json
import os
from pathlib import Path
import re
import struct
import sys
import tempfile
import unicodedata
import urllib.parse
import urllib.request


DEFAULT_PAGE_URL = (
    "https://www.nasa.gov/international-space-station/"
    "desktop-and-mobile-wallpapers/"
)
USER_AGENT = "codex-mixin-wallpaper-sync/1.0 (+https://github.com/Edward-lyz/codex-mixin)"

# How many earlier months to accept when the requested month is not published.
MONTH_FALLBACK = 3


class WallpaperPageParser(html.parser.HTMLParser):
    """Extract Desktop / Image Only links from one month section."""

    def __init__(self, month_name: str) -> None:
        super().__init__(convert_charrefs=True)
        self.month_name = month_name
        self.in_target_month = False
        self.in_heading = False
        self.heading_parts: list[str] = []
        self.current_title: str | None = None
        self.images: list[dict[str, str]] = []

    def handle_starttag(
        self,
        tag: str,
        attrs: list[tuple[str, str | None]],
    ) -> None:
        attributes = dict(attrs)
        if tag == "h2":
            heading_id = attributes.get("id")
            if heading_id in calendar.month_name:
                self.in_target_month = heading_id == self.month_name
            self.in_heading = True
            self.heading_parts = []
            return

        if tag != "a" or not self.in_target_month or self.current_title is None:
            return

        label = attributes.get("aria-label", "")
        href = attributes.get("href")
        normalized_label = re.sub(r"\s+", " ", label.lower())
        if (
            href
            and "download" in normalized_label
            and "desktop" in normalized_label
            and "image only" in normalized_label
        ):
            self.images.append({"title": self.current_title, "url": href})

    def handle_data(self, data: str) -> None:
        if self.in_heading:
            self.heading_parts.append(data)

    def handle_endtag(self, tag: str) -> None:
        if tag != "h2" or not self.in_heading:
            return
        heading = re.sub(r"\s+", " ", "".join(self.heading_parts)).strip()
        marker = ": Desktop and Mobile Wallpapers"
        if self.in_target_month and marker in heading:
            self.current_title = heading.split(marker, 1)[0].strip()
        self.in_heading = False
        self.heading_parts = []


def parse_args() -> argparse.Namespace:
    today = dt.date.today()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--month",
        type=int,
        default=today.month,
        choices=range(1, 13),
        metavar="1-12",
        help="month to sync (default: current month)",
    )
    parser.add_argument("--year", type=int, default=today.year)
    parser.add_argument("--page-url", default=DEFAULT_PAGE_URL)
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "macos"
        / "assets"
        / "nasa-wallpapers",
    )
    return parser.parse_args()


def fetch(url: str) -> bytes:
    encoded_url = urllib.parse.quote(url, safe=":/?=&%")
    request = urllib.request.Request(encoded_url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=45) as response:
        return response.read()


def safe_slug(title: str) -> str:
    normalized = unicodedata.normalize("NFKD", title)
    ascii_title = normalized.encode("ascii", "ignore").decode("ascii")
    slug = re.sub(r"[^a-z0-9]+", "-", ascii_title.lower()).strip("-")
    if not slug:
        raise ValueError(f"could not create filename for title: {title!r}")
    return slug


def png_dimensions(data: bytes) -> tuple[int, int]:
    if len(data) < 24 or data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("download is not a PNG")
    return struct.unpack(">II", data[16:24])


def atomic_write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.",
        dir=path.parent,
    )
    try:
        with os.fdopen(descriptor, "wb") as file:
            file.write(data)
        os.replace(temporary_name, path)
    except BaseException:
        try:
            os.unlink(temporary_name)
        except FileNotFoundError:
            pass
        raise


def shift_month(year: int, month: int, delta: int) -> tuple[int, int]:
    index = year * 12 + (month - 1) + delta
    return index // 12, index % 12 + 1


def collect_month_images(page_html: str, month_name: str) -> list[dict[str, str]]:
    """Deduplicated Desktop / Image Only links for one month section."""
    parser = WallpaperPageParser(month_name)
    parser.feed(page_html)
    unique_images: list[dict[str, str]] = []
    seen_urls: set[str] = set()
    for image in parser.images:
        if image["url"] not in seen_urls:
            unique_images.append(image)
            seen_urls.add(image["url"])
    return unique_images


def main() -> None:
    args = parse_args()
    page_html = fetch(args.page_url).decode("utf-8")
    requested_name = calendar.month_name[args.month]
    unique_images: list[dict[str, str]] = []
    issue_year, issue_month = args.year, args.month
    for offset in range(MONTH_FALLBACK + 1):
        year, month = shift_month(args.year, args.month, -offset)
        unique_images = collect_month_images(page_html, calendar.month_name[month])
        if unique_images:
            issue_year, issue_month = year, month
            break
    if not unique_images:
        raise SystemExit(
            "no Desktop / Image Only wallpapers found for "
            f"{requested_name} or the {MONTH_FALLBACK} months before it"
        )
    if (issue_year, issue_month) != (args.year, args.month):
        print(
            f"warning: {requested_name} {args.year} is not published yet; "
            f"using {calendar.month_name[issue_month]} {issue_year}",
            file=sys.stderr,
        )
    month_name = calendar.month_name[issue_month]

    args.output.mkdir(parents=True, exist_ok=True)
    manifest_images: list[dict[str, object]] = []
    expected_files: set[str] = set()
    for image in unique_images:
        data = fetch(image["url"])
        width, height = png_dimensions(data)
        if width < 1_920 or height < 1_080:
            raise SystemExit(
                f"{image['title']} is only {width}x{height}; expected at least 1920x1080"
            )
        filename = f"{safe_slug(image['title'])}.png"
        expected_files.add(filename)
        atomic_write(args.output / filename, data)
        manifest_images.append(
            {
                "fileName": filename,
                "title": image["title"],
                "credit": "NASA",
                "sourceURL": image["url"],
                "width": width,
                "height": height,
                "sha256": hashlib.sha256(data).hexdigest(),
            }
        )

    for stale_file in args.output.glob("*.png"):
        if stale_file.name not in expected_files:
            stale_file.unlink()

    manifest = {
        "schemaVersion": 1,
        "issue": f"{issue_year:04d}-{issue_month:02d}",
        "sourcePage": args.page_url,
        "images": manifest_images,
    }
    atomic_write(
        args.output / "manifest.json",
        (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode("utf-8"),
    )
    print(
        f"Synced {len(manifest_images)} NASA wallpapers for "
        f"{month_name} {issue_year} to {args.output}"
    )
    for image in manifest_images:
        print(f"- {image['title']}: {image['width']}x{image['height']}")


if __name__ == "__main__":
    main()
