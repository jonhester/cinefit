"""Where a tag's guide goes: prints `main` (cinefit.org), `next`
(next.cinefit.org), both, or nothing, one per line.

    python3 deploy-targets.py TAG [ALL_TAGS...]

By SemVer precedence, so the order tags are pushed in does not matter: a
patch to an older line (v0.1.5 after v0.2.0) is not the newest release and
leaves cinefit.org alone.

- A release goes to cinefit.org if it is the newest release. It also goes
  to next.cinefit.org unless a newer prerelease is there, so next never
  shows something older than the release.
- A prerelease goes to next.cinefit.org if it is newer than every other
  tag, release or prerelease.
"""
import re
import sys

TAG = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?$")


def parse(tag):
    m = TAG.match(tag)
    if not m:
        return None
    core = tuple(int(x) for x in m.group(1, 2, 3))
    pre = m.group(4)
    return core, (tuple(pre.split(".")) if pre else None)


def key(v):
    """SemVer 2.0 precedence as a sort key."""
    core, pre = v
    if pre is None:
        return core, (1,)
    ids = tuple((0, int(i), "") if i.isdigit() else (1, 0, i) for i in pre)
    return core, (0, ids)


def targets(tag, all_tags):
    v = parse(tag)
    if v is None:
        return []
    others = [p for p in map(parse, all_tags) if p is not None and p != v]
    newer = [o for o in others if key(o) > key(v)]
    if v[1] is None:
        out = []
        if not any(o[1] is None for o in newer):
            out.append("main")
            if not newer:
                out.append("next")
        return out
    return [] if newer else ["next"]


if __name__ == "__main__":
    for t in targets(sys.argv[1], sys.argv[2:]):
        print(t)
