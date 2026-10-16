"""Puts the version the guide describes on every page, before a build.

    python3 stamp-version.py TAG     # v0.2.0, v0.3.0-beta.1
    python3 stamp-version.py         # a build from a branch: unreleased

Edits docmd.config.json and the landing page in place, in the CI checkout
only (guide.yml runs it before `docmd build`); the committed files say
unreleased, which is what a local or PR build is.

- Every page: the version in the sidebar, and in the footer.
- A prerelease (next.cinefit.org) also gets a banner across the top saying
  it is not released yet, with a link to the released guide.
"""
import json
import re
import sys

tag = sys.argv[1] if len(sys.argv) > 1 else ""
m = re.match(r"^v(\d+\.\d+\.\d+)(-[0-9A-Za-z.-]+)?$", tag)
if m:
    version, pre = m.group(1) + (m.group(2) or ""), bool(m.group(2))
else:
    version, pre = "", False
label = f"cinefit {version}" if version else "Unreleased: built from main"

c = json.load(open("docmd.config.json"))
banners = {
    "sidebar-top": {"content": f"Guide for **{label}**" if version else f"**{label}**",
                    "type": "info", "dismissible": False},
}
if pre or not version:
    banners["top"] = {
        "content": (f"This is the guide for **cinefit {version}**, a prerelease."
                    if pre else "This guide is built from unreleased code.")
                   + " The current release's guide is at cinefit.org.",
        "type": "warning", "dismissible": False,
        "link": {"url": "https://cinefit.org/", "text": "Go to the released guide"},
    }
c["layout"]["banners"] = banners
c["layout"].pop("banner", None)
foot = c["layout"]["footer"]["content"]
foot = re.sub(r"^Guide for [^.]*\. ", "", foot)
c["layout"]["footer"]["content"] = f"Guide for {label}. " + foot
json.dump(c, open("docmd.config.json", "w"), indent=2)
open("docmd.config.json", "a").write("\n")

p = "docs/index.md"
s = open(p).read()
s = re.sub(r'<span class="version">[^<]*</span>', f'<span class="version">{label}</span>', s)
open(p, "w").write(s)
print(label + (" (prerelease)" if pre else ""))
